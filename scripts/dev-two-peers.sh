#!/usr/bin/env bash
# Run two rchat daemons on one machine so you can talk to yourself over a real
# Reticulum TCP link. Useful for development and for demos without hardware.
#
#   ./scripts/dev-two-peers.sh
#
# Then start the UI (cd apps/desktop && npm run dev), connect to the address and
# token printed below, and add the other peer's address from its profile dialog.
#
# The direct link is configured through the engine's interface API (a TCP
# server on A, a client on B), with local discovery turned off so the two
# daemons can only reach each other through that link.
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

./target/debug/rchatd --data-dir .dev/a --port "$PORT_A" > .dev/a.log 2>&1 &
PIDS+=($!)

sleep 1.5

./target/debug/rchatd --data-dir .dev/b --port "$PORT_B" > .dev/b.log 2>&1 &
PIDS+=($!)

wait_token() {
  local path="$1"
  for _ in $(seq 1 100); do
    [ -s "$path" ] && return 0
    sleep 0.1
  done
  echo "timed out waiting for $path" >&2
  return 1
}

wait_token .dev/a/token
wait_token .dev/b/token

node scripts/mesh-config.mjs "http://127.0.0.1:$PORT_A" .dev/a/token server "$TCP_PORT"
node scripts/mesh-config.mjs "http://127.0.0.1:$PORT_B" .dev/b/token client "127.0.0.1:$TCP_PORT"

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
