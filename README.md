# lfm-pc-agent — local AI PC control with Liquid AI LFM

An **openclaw-style** demo: a tiny [Liquid AI **LFM2-VL**](https://www.liquid.ai/blog/lfm2-vl-efficient-vision-language-models)
model, running **entirely on your Mac**, drives the computer in a *perceive → reason →
act* loop. It looks at your screen, decides one action at a time, and clicks / types /
launches apps to finish a task.

Nothing leaves the machine — the **model**, the **screen**, and the **control** are all
local. That's the "host-by-default" philosophy: you own the data and the blast radius.

```
        ┌─────────────────────────── your Mac ───────────────────────────┐
        │                                                                 │
  task ─┼─▶  PERCEIVE                 REASON                  ACT          │
        │   screencapture ─┐     ┌─ llama.cpp ────┐     ┌─ osascript ──┐   │
        │   osascript (AX) ─┼──▶  │  LFM2-24B-A2B  │ ──▶ │  cliclick    │ ──┼─▶ clicks
        │   numbered list  ─┘     │  picks an id   │     │  open -a     │   │    keys
        │        ▲                └────────────────┘     └──────────────┘   │    text
        │        └───────────────── re-observe ◀───────────────────────────┘
        └─────────────────────────────────────────────────────────────────┘
```

## Why it works with a 1.6B model

A ~1.6B vision model **cannot** reliably regress raw click coordinates on a Retina
screenshot — and Liquid AI say as much (fine-tune for narrow use; not for safety-critical
use). So we don't ask it to.

Instead the orchestrator reads the macOS **accessibility tree** and hands the model a
**numbered list** of real UI elements:

```
  1: button "Save"
  2: textfield "Search" (value: "")
  3: TextArea (value: "")
```

The model's only grounding job is to **pick a number** (the classic *Set-of-Marks* /
OmniParser technique). Rust owns the id → on-screen-rectangle map and does the actual,
pixel-accurate click. The model still *sees* the screenshot for context — pass `--marks`
to draw the numbered boxes right onto the image.

## Setup (one time)

```bash
./setup.sh          # brew install llama.cpp + cliclick, then starts the model server
```

`setup.sh` launches:

```bash
llama-server -hf LiquidAI/LFM2-24B-A2B-GGUF:Q4_K_M -c 8192 --port 8080 -ngl 99 --jinja
```

(first run downloads ~14 GB; `-ngl 99` runs it on the Apple Silicon GPU / a CUDA GPU).
Leave it running. See **[BENCHMARKS.md](BENCHMARKS.md)** for why this model.

Then grant two macOS permissions to your terminal (they **cannot** be set from the CLI):

- **System Settings ▸ Privacy & Security ▸ Accessibility** — read the UI tree & click.
- **System Settings ▸ Privacy & Security ▸ Screen Recording** — screenshot the window.

Quit and reopen the terminal so the grants apply.

## Run

```bash
cargo run -- doctor            # verify model server + permissions + tools
cargo run -- observe          # print what the model would see for the front window (safe, no actions)
cargo run -- observe --marks  # also write /tmp/lfm-pc-agent-marks.png with numbered boxes

# the demo task (drives the real UI — keep a throwaway window in front):
cargo run -- run --verify-file ~/Desktop/zeroclaw-demo.txt
```

Useful flags for `run`:

| flag | meaning |
|------|---------|
| `"<task>"` | natural-language task (positional; defaults to the TextEdit demo) |
| `--marks` | draw numbered Set-of-Marks boxes on the screenshot |
| `--vision` | also send a screenshot (vision models only; default is AX-only) |
| `--step-delay <s>` | pause before each action (slow it down to narrate) |
| `--max-iters <n>` | hard cap on loop steps (default 12) |
| `--yes` | skip the one-time "proceed?" prompt |
| `--verify-file <path>` | after `done`, assert this file exists (ground-truth check) |
| `--url` / `--model` | point at a different server / model (env `LFM_URL`) |

## The demo task

> *Open TextEdit, type a note, and save it to the Desktop as `zeroclaw-demo`.*

Every step succeeds **without** pixel-accurate clicking: `open_app` launches TextEdit,
`type` fills the document, `key cmd+s` opens the Save sheet, the model picks the filename
field and Save button by id, and `--verify-file` confirms the file really landed on disk.
TextEdit is a native AppKit app with a rich accessibility tree — the happy path.

> **Note on the saved filename:** TextEdit saves **RTF by default**, so a document named
> `zeroclaw-demo` may land as `zeroclaw-demo.rtf`. For the `--verify-file
> ~/Desktop/zeroclaw-demo.txt` check to pass exactly, set TextEdit to plain text first
> (**Format ▸ Make Plain Text**, or `defaults write com.apple.TextEdit RichText -bool
> false`), or point `--verify-file` at the extension your TextEdit actually writes.

## Safety

This is **host-by-default**: it sends real keystrokes and clicks to the frontmost app.

- A one-time **"Proceed?"** prompt before any action (skip with `--yes`).
- A hard **iteration cap** (`--max-iters`).
- **Ctrl-C** aborts instantly.
- The model only chooses **existing elements / keystrokes** — it can't fabricate coordinates.
- Demo against a **throwaway window** and a scratch file; don't point it at anything you can't lose.

## Action schema

One JSON object per turn, grammar-constrained by llama.cpp:

```json
{"thought":"the Save button is element 6","action":"click","id":6}
```

`open_app · click · type · key · wait · done` — see `SYSTEM_PROMPT` in
`src/main.rs`.

## Layout

| file | role |
|------|------|
| `src/perceive.rs` | accessibility tree + screenshot → numbered elements |
| `src/reason.rs` | call llama-server, JSON-schema-constrained action |
| `src/act.rs` | execute an action via cliclick / osascript / open |
| `src/marks.rs` | optional Set-of-Marks overlay (numbered boxes) |
| `src/main.rs` | CLI + the perceive→reason→act loop + doctor/observe |

## Model notes

- **Default: `LiquidAI/LFM2-24B-A2B-GGUF:Q4_K_M`** — the benchmarked pick (79% exact-match,
  ties SOTA; see **[BENCHMARKS.md](BENCHMARKS.md)**). It's a text model, so it's driven
  AX-only (the default `run` mode); ~14 GB Q4 fits a 24 GB GPU or a 32 GB+ Mac.
- Tiny/edge tier (worse, ~30–36%): `LiquidAI/LFM2.5-VL-1.6B-GGUF:Q8_0` — vision-capable,
  run it with `--vision`.
- Any OpenAI-compatible server works via `--url` + `--model` — e.g. a remote llama.cpp or
  Ollama on a GPU box (`--url http://<host>:<port> --model <name>`).

Licensing: LFM models use the LFM Open License (free for research and for companies under
$10M revenue) — check the model card before any commercial deployment.
