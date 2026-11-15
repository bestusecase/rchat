#!/usr/bin/env bash
# Run two rchat daemons on one machine so you can talk to yourself over a real
# Reticulum TCP link. Useful for development and for demos without hardware.
#
#   ./scripts/dev-two-peers.sh
#
# Then start the UI (cd apps/desktop && npm run dev), connect to the address and
# token printed below, and add the other peer's address from its profile dialog.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PORT_A="${PORT_A:-8791}"
PORT_B="${PORT_B:-8792}"
TCP_PORT="${TCP_PORT:-4242}"

cargo build -q -p rchatd

rm -rf .dev/a .dev/b
mkdir -p .dev

PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT INT TERM

RCHAT_RNS_AUTO=0 RCHAT_RNS_TCP_SERVER="$TCP_PORT" \
  ./target/debug/rchatd --data-dir .dev/a --port "$PORT_A" > .dev/a.log 2>&1 &
PIDS+=($!)

sleep 1.5

RCHAT_RNS_AUTO=0 RCHAT_RNS_TCP_CLIENT="127.0.0.1:$TCP_PORT" \
  ./target/debug/rchatd --data-dir .dev/b --port "$PORT_B" > .dev/b.log 2>&1 &
PIDS+=($!)

sleep 2

echo
echo "  A  http://127.0.0.1:$PORT_A   token: $(cat .dev/a/token)"
echo "     address: $(curl -s "http://127.0.0.1:$PORT_A/health" | grep -o '"address":"[a-f0-9]*"' | cut -d'"' -f4)"
echo "  B  http://127.0.0.1:$PORT_B   token: $(cat .dev/b/token)"
echo "     address: $(curl -s "http://127.0.0.1:$PORT_B/health" | grep -o '"address":"[a-f0-9]*"' | cut -d'"' -f4)"
echo
echo "  Start the UI:  cd apps/desktop && npm run dev"
echo "  Logs:          .dev/a.log  .dev/b.log"
echo "  Ctrl-C stops both daemons."
echo

wait
