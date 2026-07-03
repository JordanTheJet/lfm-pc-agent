//! Perception: turn the live macOS screen into something a tiny VLM can act on.
//!
//! Two grounding sources, both python-free:
//!   * the **accessibility tree** (`osascript` / System Events) → a numbered list of
//!     actionable UI elements with their on-screen rectangles (in points).
//!   * a **screenshot** of the active window (`screencapture`) → base64 PNG for the
//!     vision model to "look at".
//!
//! The model never sees raw coordinates; it only ever picks an element *id*. The
//! id→rectangle mapping lives here in Rust, which is what makes a ~1.6B model usable
//! as a controller.

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use std::process::Command;

/// One actionable element discovered in the frontmost window's accessibility tree.
#[derive(Debug, Clone)]
pub struct ObservedElement {
    pub id: usize,
    pub role: String,
    pub name: String,
    pub desc: String,
    pub value: String,
    /// Global, top-left-origin screen rectangle, in **points** (not pixels).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl ObservedElement {
    /// Center of the element in screen points — what we click.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Human/model-readable one-liner, e.g. `button "Save"` or `textfield (value: "")`.
    pub fn label(&self) -> String {
        let role = self.role.strip_prefix("AX").unwrap_or(&self.role);
        let mut s = role.to_string();
        let title = if !self.name.is_empty() {
            &self.name
        } else {
            &self.desc
        };
        if !title.is_empty() {
            s.push_str(&format!(" \"{}\"", title));
        }
        if !self.value.is_empty() && self.value != self.name {
            let v: String = self.value.chars().take(60).collect();
            s.push_str(&format!(" (value: \"{}\")", v));
        }
        s
    }
}

/// Run a (possibly multi-line) AppleScript via `osascript -e` and return stdout.
pub fn run_osa(script: &str) -> Result<String> {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("failed to spawn osascript")?;
    if !out.status.success() {
        return Err(anyhow!(
            "osascript failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Name of the frontmost application process (e.g. "TextEdit").
pub fn frontmost_process() -> Result<String> {
    let s = run_osa(
        "tell application \"System Events\" to get name of first process whose frontmost is true",
    )?;
    Ok(s.trim().to_string())
}

/// Bounds of the frontmost window of `proc`, in points: (x, y, w, h).
pub fn front_window_bounds(proc: &str) -> Result<(f64, f64, f64, f64)> {
    let script = format!(
        "tell application \"System Events\" to tell process \"{}\"\n\
         set p to position of front window\n\
         set s to size of front window\n\
         return ((item 1 of p) as text) & \",\" & ((item 2 of p) as text) & \",\" & ((item 1 of s) as text) & \",\" & ((item 2 of s) as text)\n\
         end tell",
        esc(proc)
    );
    let out = run_osa(&script)?;
    let parts: Vec<&str> = out.trim().split(',').collect();
    if parts.len() != 4 {
        return Err(anyhow!("could not read window bounds: {:?}", out));
    }
    let mut nums = [0.0f64; 4];
    for (i, p) in parts.iter().enumerate() {
        // Surface a parse failure (e.g. a localized number or `missing value`) rather
        // than silently coercing to 0.0 and returning a bogus rectangle.
        nums[i] = p
            .trim()
            .parse::<f64>()
            .map_err(|_| anyhow!("non-numeric window bound in {:?}", out))?;
    }
    Ok((nums[0], nums[1], nums[2], nums[3]))
}

/// Walk the accessibility tree of `proc`'s front window and return a numbered list of
/// actionable elements. Best-effort: returns an empty Vec for AX-blind apps.
pub fn ax_tree(proc: &str) -> Result<Vec<ObservedElement>> {
    let script = format!("{}\n{}", AX_HANDLER, ax_body(proc));
    let raw = run_osa(&script)?;
    let mut out: Vec<ObservedElement> = Vec::new();
    for line in raw.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 9 {
            continue;
        }
        let w: f64 = f[7].trim().parse().unwrap_or(0.0);
        let h: f64 = f[8].trim().parse().unwrap_or(0.0);
        // Skip zero-area / off-screen nodes — they have no clickable target and a
        // (0,0) element would otherwise click the corner of the main display.
        if w < 1.0 || h < 1.0 {
            continue;
        }
        // Renumber 1..N in Rust so ids are contiguous and always map to a kept element.
        let id = out.len() + 1;
        out.push(ObservedElement {
            id,
            role: f[1].trim().to_string(),
            name: clean_field(f[2]),
            desc: clean_field(f[3]),
            value: clean_field(f[4]),
            x: f[5].trim().parse().unwrap_or(0.0),
            y: f[6].trim().parse().unwrap_or(0.0),
            w,
            h,
        });
        if out.len() >= 60 {
            break;
        }
    }
    Ok(out)
}

/// Capture a screen region (points) to a PNG file. `-x` silent, `-o` no shadow.
pub fn screenshot_region(x: f64, y: f64, w: f64, h: f64, path: &str) -> Result<()> {
    let r = format!("{},{},{},{}", x as i64, y as i64, w.max(1.0) as i64, h.max(1.0) as i64);
    let status = Command::new("screencapture")
        .args(["-x", "-o", "-R", &r, path])
        .status()
        .context("failed to spawn screencapture")?;
    if !status.success() {
        return Err(anyhow!("screencapture exited with {:?}", status.code()));
    }
    Ok(())
}

/// Capture the whole main display to a PNG file.
pub fn screenshot_full(path: &str) -> Result<()> {
    let status = Command::new("screencapture")
        .args(["-x", "-o", path])
        .status()
        .context("failed to spawn screencapture")?;
    if !status.success() {
        return Err(anyhow!("screencapture exited with {:?}", status.code()));
    }
    Ok(())
}

/// Read a PNG file and base64-encode it for an OpenAI-style image_url data URL.
pub fn png_base64(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Escape a string for safe embedding inside an AppleScript double-quoted literal.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Trim a field and treat AppleScript's literal "missing value" as empty.
fn clean_field(s: &str) -> String {
    let t = s.trim();
    if t == "missing value" {
        String::new()
    } else {
        t.to_string()
    }
}

/// Top-level AppleScript handler that strips tabs/newlines and truncates a field,
/// so each element renders as exactly one tab-delimited record.
const AX_HANDLER: &str = r#"on cleanText(t)
    if t is missing value then return ""
    try
        set t to t as text
    on error
        return ""
    end try
    set AppleScript's text item delimiters to {tab, return, linefeed}
    set parts to every text item of t
    set AppleScript's text item delimiters to " "
    set t to (parts as text)
    set AppleScript's text item delimiters to ""
    if (count of t) > 80 then set t to text 1 thru 80 of t
    return t
end cleanText"#;

fn ax_body(proc: &str) -> String {
    format!(
        r#"tell application "System Events"
    tell process "{proc}"
        set out to ""
        try
            set theWindow to front window
        on error
            return ""
        end try
        try
            with timeout of 8 seconds
                set elementList to entire contents of theWindow
            end timeout
        on error
            return ""
        end try
        set idx to 0
        repeat with el in elementList
            set elRole to ""
            try
                set elRole to (role of el) as text
            end try
            if elRole is in {{"AXButton", "AXTextField", "AXTextArea", "AXCheckBox", "AXRadioButton", "AXPopUpButton", "AXMenuButton", "AXComboBox", "AXSlider", "AXLink"}} then
                set idx to idx + 1
                set elName to ""
                try
                    set elName to my cleanText(name of el)
                end try
                set elDesc to ""
                try
                    set elDesc to my cleanText(description of el)
                end try
                set elValue to ""
                try
                    set elValue to my cleanText(value of el)
                end try
                set px to 0
                set py to 0
                set sw to 0
                set sh to 0
                try
                    set elPos to position of el
                    set px to (item 1 of elPos)
                    set py to (item 2 of elPos)
                    set elSize to size of el
                    set sw to (item 1 of elSize)
                    set sh to (item 2 of elSize)
                end try
                set out to out & idx & tab & elRole & tab & elName & tab & elDesc & tab & elValue & tab & px & tab & py & tab & sw & tab & sh & linefeed
            end if
        end repeat
        return out
    end tell
end tell"#,
        proc = esc(proc)
    )
}
