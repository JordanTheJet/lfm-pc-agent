//! lfm-pc-agent — an openclaw-style local "AI PC" agent.
//!
//! A tiny Liquid AI LFM2-VL model, served locally by llama.cpp, drives the Mac in a
//! perceive → reason → act loop. The model only ever *picks a numbered UI element* (or
//! a keystroke / app); Rust resolves that to a real click via the accessibility tree.
//! Nothing leaves the machine: model, screen, and control are all local.

mod act;
mod bench;
mod marks;
mod perceive;
mod reason;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use perceive::ObservedElement;
use std::io::Write;
use std::time::Duration;

const SYSTEM_PROMPT: &str = r#"You are an on-device agent that controls a macOS computer to accomplish a user's task. You work one step at a time in a perceive→act loop.

Each turn you receive: the frontmost app, a NUMBERED list of on-screen UI elements, your recent actions, and usually a screenshot of the active window.

Reply with EXACTLY ONE JSON object for the single next action. No prose, no markdown.

Schema: {"thought":"<one short sentence>","action":"<name>", ...fields}

Actions:
- {"action":"open_app","app":"TextEdit"}          launch or focus an application
- {"action":"click","id":N}                        click on-screen UI element number N
- {"action":"type","id":N,"text":"..."}            focus element N then type (omit id to type into the focused field)
- {"action":"key","keys":"cmd+s"}                  press a key combo (return, cmd+s, cmd+space, cmd+l, ...)
- {"action":"wait"}                                wait briefly for the UI to settle
- {"action":"done","text":"<result>"}              the task is fully complete

Rules:
- Reference elements ONLY by their id from the list. NEVER invent x/y coordinates.
- Prefer keyboard shortcuts: cmd+s to save, open_app to launch, return to confirm a dialog.
- Exactly one action per turn; you will see the result next turn.
- Emit "done" as soon as the goal is achieved."#;

#[derive(Parser)]
#[command(
    name = "lfm-pc-agent",
    version,
    about = "openclaw-style local AI PC control, driven by a local Liquid AI LFM2-VL model"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the perceive→reason→act loop on a natural-language task.
    Run(RunArgs),
    /// Perceive the front window and print exactly what the model would see (no actions).
    Observe(ObserveArgs),
    /// Check prerequisites: model server, macOS permissions, helper tools.
    Doctor(ConnArgs),
    /// Benchmark a model against the offline AX-selection fixture suite.
    Bench(BenchArgs),
}

#[derive(Args, Clone)]
struct ConnArgs {
    /// Base URL of the local llama-server (OpenAI-compatible).
    #[arg(long, default_value = "http://127.0.0.1:8080", env = "LFM_URL")]
    url: String,
    /// Model name to send (llama-server uses whatever model is loaded).
    #[arg(long, default_value = "lfm2-24b-a2b")]
    model: String,
}

#[derive(Args)]
struct RunArgs {
    /// The task, in natural language.
    #[arg(
        default_value = "Open TextEdit, type 'Hello from a local LFM2-VL agent.', then save the document to the Desktop with the name zeroclaw-demo.txt"
    )]
    task: String,
    #[command(flatten)]
    conn: ConnArgs,
    /// Maximum loop iterations.
    #[arg(long, default_value_t = 12)]
    max_iters: usize,
    /// Seconds to pause before each action (slow it down to narrate the demo).
    #[arg(long, default_value_t = 0.8)]
    step_delay: f64,
    /// Skip the one-time "proceed?" confirmation and start immediately.
    #[arg(long)]
    yes: bool,
    /// Also send a screenshot (for vision models). Default is AX-only, which the
    /// benchmarked LFM2-24B-A2B (a text model) needs and which scores just as well.
    #[arg(long)]
    vision: bool,
    /// Draw numbered Set-of-Marks boxes on the screenshot before sending it.
    #[arg(long)]
    marks: bool,
    /// After 'done', assert this file exists (ground-truth success check).
    #[arg(long)]
    verify_file: Option<String>,
}

#[derive(Args)]
struct ObserveArgs {
    /// Also write a Set-of-Marks annotated screenshot to /tmp.
    #[arg(long)]
    marks: bool,
}

#[derive(Args)]
struct BenchArgs {
    #[command(flatten)]
    conn: ConnArgs,
    /// Path to the fixture suite (JSON array of cases).
    #[arg(long, default_value = "bench/fixtures.json")]
    fixtures: String,
    /// Attach fixture screenshots to the prompt (vision models only).
    #[arg(long)]
    vision: bool,
    /// Skip the JSON-schema/grammar constraint — let a thinking model reason freely
    /// (the action is parsed out of the raw output). For completeness comparisons.
    #[arg(long)]
    no_grammar: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run(a) => run(a),
        Cmd::Observe(a) => observe(a),
        Cmd::Doctor(a) => doctor(&a),
        Cmd::Bench(a) => bench::run(&a.conn.url, &a.conn.model, &a.fixtures, a.vision, !a.no_grammar),
    }
}

/// Scratch PNG path under the OS temp dir (`/tmp` on Unix, `%TEMP%` on Windows) so the
/// tool behaves identically on every platform instead of hardcoding `/tmp`.
fn tmp_png(name: &str) -> String {
    std::env::temp_dir()
        .join(format!("lfm-pc-agent-{name}.png"))
        .to_string_lossy()
        .into_owned()
}

fn run(a: RunArgs) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()?;

    println!("\n┌─ lfm-pc-agent ───────────────────────────────────────────");
    println!("│ task : {}", a.task);
    println!("│ model: {}  @ {}", a.conn.model, a.conn.url);
    println!(
        "│ mode : {}{}, max {} steps",
        if a.vision { "vision" } else { "ax-only" },
        if a.marks { "+marks" } else { "" },
        a.max_iters
    );
    println!("└──────────────────────────────────────────────────────────");
    println!(
        "⚠  This drives your real keyboard & mouse on the frontmost app.\n   Keep a throwaway window in front; Ctrl-C aborts at any time.\n"
    );

    if !a.yes {
        print!("Proceed? [y/N] ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
        println!();
    }

    let mut history: Vec<String> = Vec::new();

    for iter in 1..=a.max_iters {
        let proc = perceive::frontmost_process().unwrap_or_default();
        let bounds = perceive::front_window_bounds(&proc).ok();
        let els = perceive::ax_tree(&proc).unwrap_or_default();

        let image_b64 = if a.vision {
            capture(&bounds, a.marks, &els)
        } else {
            None
        };

        println!("── step {iter}/{} ─ {proc} ─ {} elements", a.max_iters, els.len());

        let user_text = build_user_text(&a.task, &proc, &els, &history, image_b64.is_some());
        let action = reason::decide(
            &client,
            &a.conn.url,
            &a.conn.model,
            SYSTEM_PROMPT,
            &user_text,
            image_b64.as_deref(),
            true,
        )?;

        if !action.thought.is_empty() {
            println!("  🧠 {}", action.thought);
        }
        println!("  ▶  {}", describe(&action));

        if action.action == "done" {
            let msg = action.text.clone().unwrap_or_default();
            println!("\n✅ done: {msg}");
            verify(&a.verify_file);
            return Ok(());
        }

        if a.step_delay > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(a.step_delay));
        }

        match act::execute(&action, &els) {
            Ok(act::Outcome::Continue(s)) => history.push(format!("{}: {s}", action.action)),
            Ok(act::Outcome::Done(s)) => {
                println!("\n✅ done: {s}");
                verify(&a.verify_file);
                return Ok(());
            }
            Err(e) => {
                println!("  ⚠  action failed: {e}");
                history.push(format!("{} FAILED: {e}", action.action));
            }
        }
    }

    println!("\n⏹  reached max iterations ({}).", a.max_iters);
    Ok(())
}

/// Capture the active window (or full screen), optionally with marks, return base64 PNG.
fn capture(bounds: &Option<(f64, f64, f64, f64)>, marks: bool, els: &[ObservedElement]) -> Option<String> {
    // Only treat bounds as a usable region when they describe a real rectangle; this
    // keeps the screenshot guard and the marks guard in lockstep.
    let region = match bounds {
        Some((x, y, w, h)) if *w > 1.0 && *h > 1.0 => Some((*x, *y, *w, *h)),
        _ => None,
    };
    let obs = tmp_png("obs");
    let marks_png = tmp_png("marks");
    let captured = match region {
        Some((x, y, w, h)) => perceive::screenshot_region(x, y, w, h, &obs),
        None => perceive::screenshot_full(&obs),
    };
    if captured.is_err() {
        return None;
    }
    // Marks need the window origin/scale, so only overlay them on a region capture.
    let send_path: &str = if marks && region.is_some() {
        match marks::annotate(&obs, els, region, &marks_png) {
            Ok(()) => &marks_png,
            Err(_) => &obs,
        }
    } else {
        &obs
    };
    perceive::png_base64(send_path).ok()
}

fn build_user_text(
    task: &str,
    proc: &str,
    els: &[ObservedElement],
    history: &[String],
    has_image: bool,
) -> String {
    let listing = if els.is_empty() {
        "(no readable elements — use open_app / key / type)".to_string()
    } else {
        els.iter()
            .map(|e| format!("  {}: {}", e.id, e.label()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let recent = if history.is_empty() {
        "(none yet)".to_string()
    } else {
        history
            .iter()
            .rev()
            .take(6)
            .rev()
            .map(|h| format!("  - {h}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "TASK: {task}\n\n\
         FRONTMOST APP: {proc}\n\
         {screenshot}\n\
         UI ELEMENTS (choose by id):\n{listing}\n\n\
         RECENT ACTIONS:\n{recent}\n\n\
         Reply with ONE JSON action for the next step.",
        screenshot = if has_image {
            "A screenshot of the active window is attached below."
        } else {
            "(no screenshot this turn)"
        }
    )
}

fn describe(a: &reason::Action) -> String {
    match a.action.as_str() {
        "open_app" => format!("open_app {}", a.app.clone().unwrap_or_default()),
        "click" => format!("click #{}", a.id.unwrap_or(-1)),
        "type" => format!(
            "type{} {:?}",
            a.id.map(|i| format!(" #{i}")).unwrap_or_default(),
            a.text.clone().unwrap_or_default()
        ),
        "key" => format!("key {}", a.keys.clone().or(a.text.clone()).unwrap_or_default()),
        other => other.to_string(),
    }
}

fn verify(path: &Option<String>) {
    if let Some(p) = path {
        let expanded = expand_tilde(p);
        if std::path::Path::new(&expanded).exists() {
            println!("🔎 ground-truth check: {p} exists ✓");
        } else {
            println!("🔎 ground-truth check: {p} NOT found ✗");
        }
    }
}

fn observe(a: ObserveArgs) -> Result<()> {
    let proc = perceive::frontmost_process()?;
    let bounds = perceive::front_window_bounds(&proc).ok();
    let els = perceive::ax_tree(&proc)?;
    println!("frontmost app : {proc}");
    match bounds {
        Some((x, y, w, h)) => println!("window bounds : x={x} y={y} w={w} h={h} (points)"),
        None => println!("window bounds : (none)"),
    }
    println!("elements      : {}", els.len());
    for e in &els {
        let (cx, cy) = e.center();
        println!("  {:>2}: {:<48} @ ({:.0},{:.0})", e.id, e.label(), cx, cy);
    }
    let obs = tmp_png("obs");
    let marks_png = tmp_png("marks");
    let cap = match bounds {
        Some((x, y, w, h)) if w > 1.0 && h > 1.0 => {
            perceive::screenshot_region(x, y, w, h, &obs)
        }
        _ => perceive::screenshot_full(&obs),
    };
    match cap {
        Ok(()) => {
            println!("\nscreenshot    : {obs}");
            if a.marks {
                match marks::annotate(&obs, &els, bounds, &marks_png) {
                    Ok(()) => println!("set-of-marks  : {marks_png}"),
                    Err(e) => println!("set-of-marks  : skipped ({e})"),
                }
            }
        }
        Err(e) => println!("\nscreenshot    : failed ({e}) — grant Screen Recording"),
    }
    Ok(())
}

fn doctor(c: &ConnArgs) -> Result<()> {
    println!("lfm-pc-agent doctor\n");

    // 1. Accessibility read
    match perceive::frontmost_process() {
        Ok(p) => ok(&format!("Accessibility read works (frontmost app: {p})")),
        Err(e) => bad(&format!(
            "Accessibility read FAILED ({e}).\n     → System Settings ▸ Privacy & Security ▸ Accessibility ▸ add your terminal"
        )),
    }

    // 2. Screen capture
    let doctor_png = tmp_png("doctor");
    match perceive::screenshot_full(&doctor_png) {
        Ok(()) => {
            let sz = std::fs::metadata(&doctor_png)
                .map(|m| m.len())
                .unwrap_or(0);
            if sz > 0 {
                ok(&format!("Screen capture works ({sz} bytes)"));
                note("If captures look like the desktop only, grant Screen Recording too.");
            } else {
                bad("Screen capture produced an empty file — grant Screen Recording.");
            }
        }
        Err(e) => bad(&format!("Screen capture FAILED ({e})")),
    }

    // 3. cliclick
    if act::has_cliclick() {
        ok("cliclick found (point-accurate clicks & typing)");
    } else {
        note("cliclick NOT found — falling back to AppleScript AX clicks. `brew install cliclick` for best results.");
    }

    // 4. Model server
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let models_url = format!("{}/v1/models", c.url.trim_end_matches('/'));
    match client.get(&models_url).send() {
        Ok(r) if r.status().is_success() => {
            let body = r.text().unwrap_or_default();
            let name = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["data"][0]["id"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "(unknown)".into());
            ok(&format!("Model server reachable at {} (loaded: {name})", c.url));
        }
        Ok(r) => bad(&format!("Model server responded {} at {}", r.status(), c.url)),
        Err(e) => bad(&format!(
            "Model server unreachable at {} ({e}).\n     → start it: llama-server -hf LiquidAI/LFM2.5-VL-1.6B-GGUF:Q8_0 -c 8192 --port 8080 -ngl 99",
            c.url
        )),
    }

    println!("\nWhen all four are green, run:  lfm-pc-agent run --verify-file ~/Desktop/zeroclaw-demo.txt");
    Ok(())
}

fn ok(s: &str) {
    println!("  ✅ {s}");
}
fn bad(s: &str) {
    println!("  ❌ {s}");
}
fn note(s: &str) {
    println!("     ℹ  {s}");
}

fn expand_tilde(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    p.to_string()
}

/// The user's home directory: `$HOME` on Unix, `%USERPROFILE%` (or the
/// `HOMEDRIVE`+`HOMEPATH` pair) on Windows.
fn home_dir() -> Option<std::path::PathBuf> {
    if let Some(h) = std::env::var_os("HOME") {
        return Some(std::path::PathBuf::from(h));
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return Some(std::path::PathBuf::from(h));
    }
    match (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) => {
            let mut s = std::ffi::OsString::from(drive);
            s.push(path);
            Some(std::path::PathBuf::from(s))
        }
        _ => None,
    }
}
