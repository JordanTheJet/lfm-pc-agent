#!/usr/bin/env bash
# One-time setup for the lfm-pc-agent demo on Apple Silicon macOS.
# Installs the local model runtime (llama.cpp) + the click helper (cliclick),
# then starts the LFM2-VL model server. No Python anywhere.
set -euo pipefail

# LFM2-24B-A2B is the benchmarked pick (see BENCHMARKS.md): 79% exact-match, ties SOTA,
# on-brand, ~14GB Q4 fits a 24GB GPU or 32GB+ Mac. It's a TEXT model (no vision tower),
# so drive it AX-only (the default `run` mode). Override MODEL=… for a different one.
MODEL="${MODEL:-LiquidAI/LFM2-24B-A2B-GGUF:Q4_K_M}"
PORT="${PORT:-8080}"
CTX="${CTX:-8192}"

say() { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This demo targets macOS." >&2; exit 1
fi
command -v brew >/dev/null || { echo "Homebrew required: https://brew.sh" >&2; exit 1; }

say "Installing llama.cpp (model runtime) and cliclick (click helper)…"
brew list llama.cpp >/dev/null 2>&1 || brew install llama.cpp
brew list cliclick  >/dev/null 2>&1 || brew install cliclick

cat <<'PERMS'

────────────────────────────────────────────────────────────────────────
ONE-TIME macOS PERMISSIONS (cannot be granted from the CLI):
  System Settings ▸ Privacy & Security ▸
    • Accessibility    → add your terminal app   (lets it read the UI tree + click)
    • Screen Recording → add your terminal app   (lets it screenshot the window)
  Quit & reopen the terminal after granting so the grant takes effect.
────────────────────────────────────────────────────────────────────────
PERMS

say "Starting the model server: $MODEL on :$PORT (first run downloads ~14GB)…"
say "Leave this running; open a new terminal and: cargo run -- doctor"
exec llama-server -hf "$MODEL" -c "$CTX" --port "$PORT" -ngl 99 --jinja
