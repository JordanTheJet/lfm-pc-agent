//! Action: map a model-chosen Action onto deterministic macOS host primitives.
//!
//! All grounding (which pixel to click) is resolved here from the accessibility
//! rectangle the model referenced by id — the model never supplies coordinates.
//! Clicks/typing go through `cliclick` when available (clean, point-accurate), with
//! an AppleScript accessibility-press fallback. Key combos and app launches use the
//! built-in `osascript` / `open`, so the only optional dependency is cliclick.

use crate::perceive::{run_osa, ObservedElement};
use crate::reason::Action;
use anyhow::{anyhow, Result};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

/// What happened after executing an action.
pub enum Outcome {
    /// Keep looping; the string is a one-line summary for the action history.
    Continue(String),
    /// The model declared the task finished.
    Done(String),
}

pub fn execute(act: &Action, els: &[ObservedElement]) -> Result<Outcome> {
    match act.action.as_str() {
        "open_app" => {
            let app = act
                .app
                .as_deref()
                .or(act.text.as_deref())
                .ok_or_else(|| anyhow!("open_app needs an \"app\""))?;
            let ok = Command::new("open").args(["-a", app]).status()?.success();
            sleep(1.3);
            if ok {
                Ok(Outcome::Continue(format!("launched {app}")))
            } else {
                Ok(Outcome::Continue(format!("could not launch {app}")))
            }
        }
        "click" => {
            let el = find(els, act.id)?;
            click(el)?;
            sleep(0.6);
            Ok(Outcome::Continue(format!("clicked #{} ({})", el.id, el.label())))
        }
        "type" => {
            let text = act.text.clone().unwrap_or_default();
            let mut focused = None;
            if let Some(id) = act.id {
                if let Ok(el) = find(els, Some(id)) {
                    click(el)?; // focus the field first
                    sleep(0.3);
                    focused = Some(id);
                }
            }
            if text.is_empty() {
                // An empty 'type' is just a focus click; report it honestly rather
                // than logging a misleading `typed ""`.
                return Ok(Outcome::Continue(match focused {
                    Some(id) => format!("focused #{id} (no text to type)"),
                    None => "type called with no text and no id (no-op)".into(),
                }));
            }
            type_text(&text)?;
            sleep(0.4);
            Ok(Outcome::Continue(format!("typed {:?}", truncate(&text, 40))))
        }
        "key" => {
            let keys = act
                .keys
                .as_deref()
                .or(act.text.as_deref())
                .ok_or_else(|| anyhow!("key needs \"keys\""))?;
            press_keys(keys)?;
            sleep(0.5);
            Ok(Outcome::Continue(format!("pressed {keys}")))
        }
        "wait" => {
            sleep(1.0);
            Ok(Outcome::Continue("waited".into()))
        }
        "done" => Ok(Outcome::Done(act.text.clone().unwrap_or_default())),
        other => Ok(Outcome::Continue(format!("ignored unknown action {other:?}"))),
    }
}

fn find(els: &[ObservedElement], id: Option<i64>) -> Result<&ObservedElement> {
    let id = id.ok_or_else(|| anyhow!("action needs an element id"))?;
    els.iter()
        .find(|e| e.id as i64 == id)
        .ok_or_else(|| anyhow!("no element with id {id} in the current view"))
}

/// Click an element by its accessibility-reported center point.
fn click(el: &ObservedElement) -> Result<()> {
    let (cx, cy) = el.center();
    // Guard against AX elements with no real position — clicking (0,0) would hit the
    // top-left of the main display (Apple menu / window controls).
    if cx < 1.0 && cy < 1.0 {
        return Err(anyhow!("element #{} has no on-screen position to click", el.id));
    }
    if has_cliclick() {
        run_cliclick(&[format!("c:{},{}", cx as i64, cy as i64)])?;
        Ok(())
    } else {
        // Fallback: accessibility press by role + name (works for most buttons).
        ax_press(el)
    }
}

fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    if has_cliclick() {
        run_cliclick(&[format!("t:{text}")])
    } else {
        let esc = text.replace('\\', "\\\\").replace('"', "\\\"");
        run_osa(&format!(
            "tell application \"System Events\" to keystroke \"{esc}\""
        ))
        .map(|_| ())
    }
}

/// Parse a combo like "cmd+s", "cmd+space", "return" and synthesize it via System Events.
fn press_keys(spec: &str) -> Result<()> {
    // Split on '+' only — '-' is a real key ("cmd+-" = zoom out) and must not be a separator.
    let parts: Vec<String> = spec
        .split('+')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(anyhow!("empty key spec"));
    }
    let (mods, key) = parts.split_at(parts.len() - 1);
    let key = &key[0];

    let mut using = Vec::new();
    for m in mods {
        match m.as_str() {
            "cmd" | "command" | "⌘" => using.push("command down"),
            "ctrl" | "control" => using.push("control down"),
            "alt" | "opt" | "option" => using.push("option down"),
            "shift" => using.push("shift down"),
            _ => {}
        }
    }
    let using_clause = if using.is_empty() {
        String::new()
    } else {
        format!(" using {{{}}}", using.join(", "))
    };

    let named: Option<u16> = match key.as_str() {
        "return" | "enter" => Some(36),
        "tab" => Some(48),
        "space" => Some(49),
        "esc" | "escape" => Some(53),
        "delete" | "backspace" => Some(51),
        "left" => Some(123),
        "right" => Some(124),
        "down" => Some(125),
        "up" => Some(126),
        _ => None,
    };

    let script = if let Some(code) = named {
        format!("tell application \"System Events\" to key code {code}{using_clause}")
    } else {
        let ch = key.replace('\\', "\\\\").replace('"', "\\\"");
        format!("tell application \"System Events\" to keystroke \"{ch}\"{using_clause}")
    };
    run_osa(&script).map(|_| ())
}

/// Press an element via the accessibility API by re-finding it by role + name.
fn ax_press(el: &ObservedElement) -> Result<()> {
    let class = match el.role.as_str() {
        "AXButton" => "button",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio button",
        "AXMenuButton" => "menu button",
        "AXPopUpButton" => "pop up button",
        _ => "UI element",
    };
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    // Re-find by name, or by accessibility description for nameless (icon-only) buttons;
    // never match `whose name is ""`, which would grab the wrong control.
    let matcher = if !el.name.is_empty() {
        format!("name is \"{}\"", esc(&el.name))
    } else if !el.desc.is_empty() {
        format!("description is \"{}\"", esc(&el.desc))
    } else {
        return Err(anyhow!(
            "element #{} has no name/description; install cliclick for coordinate clicks",
            el.id
        ));
    };
    let proc = esc(&crate::perceive::frontmost_process().unwrap_or_default());
    let script = format!(
        "tell application \"System Events\" to tell process \"{proc}\"\n\
         click (first {class} of (entire contents of front window) whose {matcher})\n\
         end tell"
    );
    run_osa(&script).map(|_| ())
}

fn run_cliclick(args: &[String]) -> Result<()> {
    let ok = Command::new("cliclick").args(args).status()?.success();
    if ok {
        Ok(())
    } else {
        Err(anyhow!("cliclick failed for args {args:?}"))
    }
}

/// Whether the `cliclick` binary is on PATH (cached).
pub fn has_cliclick() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        Command::new("cliclick")
            .arg("p")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn sleep(secs: f64) {
    if secs > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(secs));
    }
}
