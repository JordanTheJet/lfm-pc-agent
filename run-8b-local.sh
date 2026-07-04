#!/usr/bin/env bash
# Private computer-use: run lfm-pc-agent FULLY ON-DEVICE.
#
# The LFM2.5-8B-A1B model (~1B active, ~6-9 GB) runs locally via llama.cpp (Metal on a Mac,
# CUDA on a GPU box), so the screenshot + accessibility tree NEVER leave this machine. No cloud,
# no API bill, works offline. On the 33-case AX-selection benchmark the 8B scores ~70% at
# 100% safe (see BENCHMARKS.md) — competitive with the 24B at a third the footprint.
#
# Usage:
#   ./run-8b-local.sh                                    # the default TextEdit demo task
#   ./run-8b-local.sh "Open Calculator and compute 12*9" # a custom task
#   ./run-8b-local.sh --verify-file ~/Desktop/zeroclaw-demo.txt
set -euo pipefail

PORT="${PORT:-8090}"
MODEL="${MODEL:-LiquidAI/LFM2.5-8B-A1B-GGUF:Q8_0}"   # cached locally; Q4_K_M is smaller if you prefer

# 1) Serve the 8B locally if it isn't already up.
if ! curl -s -m2 "http://127.0.0.1:${PORT}/v1/models" >/dev/null 2>&1; then
  echo "▶ starting on-device 8B (${MODEL}) on :${PORT} ..."
  nohup llama-server -hf "${MODEL}" --host 127.0.0.1 --port "${PORT}" -ngl 99 -c 8192 --jinja \
    >/tmp/lfm-pc-agent-8b.log 2>&1 &
  for _ in $(seq 1 40); do
    curl -s -m2 "http://127.0.0.1:${PORT}/v1/models" >/dev/null 2>&1 && break; sleep 3
  done
fi
echo "✓ on-device 8B ready on :${PORT} (nothing leaves this machine)"

# 2) Drive the computer with the local 8B (AX-only — the model just picks numbered elements).
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"
exec cargo run --release -- run --url "http://127.0.0.1:${PORT}" --model "${MODEL}" "$@"
