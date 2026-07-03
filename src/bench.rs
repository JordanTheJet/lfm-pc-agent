//! Offline model shootout for the AX-grounded selection task.
//!
//! Reuses the *exact* live code path — `crate::SYSTEM_PROMPT`, `crate::build_user_text`,
//! and `crate::reason::decide` (the json_schema → json_object → unconstrained ladder) —
//! so every model is judged apples-to-apples on the thing that actually matters here:
//! picking the right action/element from a numbered accessibility list, with reliable
//! JSON and low per-step latency. Pixel grounding is out of scope by design.

use crate::perceive::ObservedElement;
use crate::reason;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Fixture {
    name: String,
    task: String,
    #[serde(default)]
    frontmost_app: String,
    #[serde(default)]
    history: Vec<String>,
    #[serde(default)]
    elements: Vec<FixtureEl>,
    #[serde(default)]
    screenshot: Option<String>,
    expected: Vec<Expected>,
}

#[derive(Deserialize)]
struct FixtureEl {
    id: usize,
    role: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
struct Expected {
    action: String,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    keys: Option<String>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    text_contains: Option<String>,
}

struct Score {
    name: String,
    valid: bool,
    action_match: bool,
    exact: bool,
    fair: bool,
    safe: bool,
    ms: u128,
    pred: String,
}

/// Run the fixture suite against one model endpoint and print a scorecard.
pub fn run(url: &str, model: &str, fixtures_path: &str, vision: bool, constrain: bool) -> Result<()> {
    let raw = std::fs::read_to_string(fixtures_path)
        .with_context(|| format!("read fixtures {fixtures_path}"))?;
    let fixtures: Vec<Fixture> =
        serde_json::from_str(&raw).context("parse fixtures JSON (expected an array)")?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;

    println!("\n┌─ bench ─ {} fixtures ─ {model} @ {url} ─ {} ─ {}", fixtures.len(),
        if vision { "vision" } else { "text/--no-vision" },
        if constrain { "grammar" } else { "unconstrained/thinking" });
    println!("│ {:<30} {:>4} {:>5} {:>4} {:>6} {:>4} {:>7}  pred", "case", "json", "exact", "fair", "action", "safe", "ms");

    let mut scores = Vec::new();
    for f in &fixtures {
        let els: Vec<ObservedElement> = f
            .elements
            .iter()
            .map(|e| ObservedElement {
                id: e.id,
                role: e.role.clone(),
                name: e.label.clone(),
                desc: String::new(),
                value: e.value.clone(),
                // Dummy but valid geometry — the model only ever sees label(), never coords.
                x: 100.0,
                y: 100.0,
                w: 80.0,
                h: 24.0,
            })
            .collect();

        let image_b64 = if vision {
            f.screenshot
                .as_deref()
                .and_then(|p| crate::perceive::png_base64(p).ok())
        } else {
            None
        };
        let has_image = image_b64.is_some();

        let user_text =
            crate::build_user_text(&f.task, &f.frontmost_app, &els, &f.history, has_image);

        let t0 = Instant::now();
        let result = reason::decide(
            &client,
            url,
            model,
            crate::SYSTEM_PROMPT,
            &user_text,
            image_b64.as_deref(),
            constrain,
        );
        let ms = t0.elapsed().as_millis();

        let score = match result {
            Ok(pred) => {
                let action_match = f.expected.iter().any(|e| e.action == pred.action);
                let exact = f.expected.iter().any(|e| matches(&pred, e));
                // Fair: an exact match, OR clicking the very element that an expected `type`
                // action targets — a valid focus-then-type strategy the single-turn score
                // otherwise penalizes even though the model selected the right target.
                let fair = exact
                    || f.expected.iter().any(|e| {
                        e.action == "type"
                            && e.id.is_some()
                            && pred.action == "click"
                            && pred.id == e.id
                    });
                let safe = is_safe(&pred, &els);
                Score {
                    name: f.name.clone(),
                    valid: true,
                    action_match,
                    exact,
                    fair,
                    safe,
                    ms,
                    pred: describe(&pred),
                }
            }
            Err(e) => Score {
                name: f.name.clone(),
                valid: false,
                action_match: false,
                exact: false,
                fair: false,
                safe: true,
                ms,
                pred: format!("<error: {}>", short(&e.to_string())),
            },
        };

        println!(
            "│ {:<30} {:>4} {:>5} {:>4} {:>6} {:>4} {:>7}  {}",
            trunc(&score.name, 30),
            mark(score.valid),
            mark(score.exact),
            mark(score.fair),
            mark(score.action_match),
            mark(score.safe),
            score.ms,
            score.pred
        );
        scores.push(score);
    }

    summarize(model, &scores);
    Ok(())
}

fn matches(pred: &reason::Action, exp: &Expected, ) -> bool {
    if pred.action != exp.action {
        return false;
    }
    if let Some(eid) = exp.id {
        if pred.id != Some(eid) {
            return false;
        }
    }
    if let Some(ek) = &exp.keys {
        let pk = pred.keys.clone().or_else(|| pred.text.clone()).unwrap_or_default();
        if norm_keys(&pk) != norm_keys(ek) {
            return false;
        }
    }
    if let Some(ea) = &exp.app {
        let pa = pred.app.clone().or_else(|| pred.text.clone()).unwrap_or_default();
        if pa.trim().to_lowercase() != ea.trim().to_lowercase() {
            return false;
        }
    }
    if let Some(tc) = &exp.text_contains {
        let pt = pred.text.clone().unwrap_or_default().to_lowercase();
        if !pt.contains(&tc.to_lowercase()) {
            return false;
        }
    }
    true
}

/// A click/type must reference an element id that actually exists in the list.
fn is_safe(pred: &reason::Action, els: &[ObservedElement]) -> bool {
    let id_present = |id: Option<i64>| id.is_some_and(|i| els.iter().any(|e| e.id as i64 == i));
    match pred.action.as_str() {
        "click" => id_present(pred.id),
        "type" => pred.id.is_none() || id_present(pred.id),
        _ => true,
    }
}

fn norm_keys(s: &str) -> String {
    s.to_lowercase()
        .replace(' ', "")
        .replace("command", "cmd")
        .replace("control", "ctrl")
        .replace("option", "alt")
        .replace("escape", "esc")
        .replace("enter", "return")
}

fn describe(a: &reason::Action) -> String {
    match a.action.as_str() {
        "open_app" => format!("open_app {}", a.app.clone().unwrap_or_default()),
        "click" => format!("click #{}", a.id.unwrap_or(-1)),
        "type" => format!(
            "type{} {:?}",
            a.id.map(|i| format!(" #{i}")).unwrap_or_default(),
            trunc(&a.text.clone().unwrap_or_default(), 24)
        ),
        "key" => format!("key {}", a.keys.clone().or_else(|| a.text.clone()).unwrap_or_default()),
        other => other.to_string(),
    }
}

fn summarize(model: &str, scores: &[Score]) {
    let n = scores.len().max(1);
    let pct = |c: usize| format!("{:.0}%", 100.0 * c as f64 / n as f64);
    let valid = scores.iter().filter(|s| s.valid).count();
    let exact = scores.iter().filter(|s| s.exact).count();
    let fair = scores.iter().filter(|s| s.fair).count();
    let action = scores.iter().filter(|s| s.action_match).count();
    let safe = scores.iter().filter(|s| s.safe).count();

    let mut lat: Vec<u128> = scores.iter().map(|s| s.ms).collect();
    lat.sort_unstable();
    let p = |q: f64| lat.get(((lat.len() as f64 * q) as usize).min(lat.len().saturating_sub(1))).copied().unwrap_or(0);

    println!("└─────────────────────────────────────────────────────────────");
    println!(
        "  {model}\n   EXACT {} ({}/{})   FAIR {} ({}/{})   action {}   valid-JSON {}   safe {}   p50 {}ms  p95 {}ms",
        pct(exact), exact, scores.len(), pct(fair), fair, scores.len(), pct(action), pct(valid), pct(safe), p(0.5), p(0.95)
    );
    // One machine-greppable line for cross-model comparison.
    println!(
        "  RESULT\t{model}\texact={}\tfair={}\taction={}\tvalid={}\tsafe={}\tp50={}\tp95={}",
        exact, fair, action, valid, safe, p(0.5), p(0.95)
    );
}

fn mark(b: bool) -> &'static str {
    if b {
        "✓"
    } else {
        "·"
    }
}
fn trunc(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    t
}
fn short(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(60).collect()
}
