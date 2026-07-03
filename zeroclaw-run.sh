#!/usr/bin/env bash
# Wrapper so ZeroClaw can run the lfm-pc-agent PC-control demo as a scheduled/triggered task.
#
# Register with ZeroClaw (paused by default so it never grabs your keyboard unexpectedly):
#   zeroclaw cron add '0 3 * * *' '/abs/path/to/zeroclaw-run.sh'
#   zeroclaw cron pause <id>            # then `resume`/`update` or fire it when you want
#
# Prereqs at run time:
#   * a model server reachable at $LFM_URL (default: the local 24B from ./setup.sh,
#     or point at a remote GPU host, e.g. LFM_URL=http://YOUR_GPU_HOST:8080)
#   * the running process must have macOS Accessibility + Screen Recording granted
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/target/release/lfm-pc-agent"
[ -x "$BIN" ] || BIN="$DIR/target/debug/lfm-pc-agent"
URL="${LFM_URL:-http://127.0.0.1:8080}"
MODEL="${LFM_MODEL:-lfm2-24b-a2b}"
TASK="${PC_TASK:-Open TextEdit, type 'Hello from ZeroClaw driving a local Liquid LFM.', then save the document to the Desktop with the name zeroclaw-demo.txt}"

if [ ! -x "$BIN" ]; then
  echo "lfm-pc-agent binary not built — run: (cd '$DIR' && cargo build --release)" >&2
  exit 1
fi
if ! curl -sf -m5 "$URL/v1/models" >/dev/null 2>&1; then
  echo "model server not reachable at $URL — start it: (cd '$DIR' && ./setup.sh)" >&2
  exit 1
fi

echo "[zeroclaw-run] $(date '+%F %T')  model=$MODEL url=$URL"
exec "$BIN" run "$TASK" --url "$URL" --model "$MODEL" --yes \
  --verify-file "$HOME/Desktop/zeroclaw-demo.txt"
