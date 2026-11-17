#!/usr/bin/env bash
# The gate. A slice is done when this exits zero; there is no per-slice review
# beyond it.
#
#   ./scripts/verify.sh
#
# It runs rustfmt, clippy at deny-warnings and every portable unit test in the
# repo: the engine workspace, the two shell workspaces, the Python bridge and
# the script tests. It stops at the first failure. It takes no options: a slice
# that needs a different gate is mis-scoped.
#
# The gate deliberately cannot see Reticulum: the namespace runner needs root
# and a veth pair, and the mesh harness needs three live nodes. A slice whose
# behaviour cannot be proven here names the device-acceptance ticket it defers
# to; claiming it here would be worse than skipping it.
#
# A missing toolchain is not a failing slice. When a capability is absent this
# prints one line naming it and exits zero.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [ "$#" -gt 0 ]; then
  printf 'verify: this gate takes no options; run ./scripts/verify.sh\n' >&2
  exit 2
fi

skip() {
  printf 'verify: skipped, the gate did not run (%s)\n' "$1"
  exit 0
}

need() {
  command -v "$1" >/dev/null 2>&1 || skip "$2"
}

need cargo "cargo is not installed"
cargo fmt --version >/dev/null 2>&1 || skip "the rustfmt component is not installed"
cargo clippy --version >/dev/null 2>&1 || skip "the clippy component is not installed"
need node "node is not installed"
need python3 "python3 is not installed"
python3 -c "import RNS, LXMF" >/dev/null 2>&1 \
  || skip "the rns and lxmf Python packages are not installed"

step() {
  local name="$1"
  shift
  printf '\n== %s\n' "$name"
  "$@"
}

# The shell's tauri-build refuses to compile when a packaged input is missing.
# `runtime/`, `resources/rchatd` and `dist/` are release-build outputs and are
# gitignored, so a clean checkout has none of them. The gate compiles and tests
# the Rust shells but does not package the app or build the frontend, so it
# satisfies the existence checks with placeholders and removes only what it
# created.
stub_runtime=0
stub_rchatd=0
stub_dist=0
stub_dist_dir=0
if [ ! -e apps/desktop/src-tauri/runtime ]; then
  mkdir -p apps/desktop/src-tauri/runtime
  stub_runtime=1
fi
if [ ! -e apps/desktop/src-tauri/resources/rchatd ]; then
  mkdir -p apps/desktop/src-tauri/resources
  : > apps/desktop/src-tauri/resources/rchatd
  chmod +x apps/desktop/src-tauri/resources/rchatd
  stub_rchatd=1
fi
if [ ! -e apps/desktop/dist ]; then
  mkdir -p apps/desktop/dist
  stub_dist_dir=1
fi
if [ ! -e apps/desktop/dist/index.html ]; then
  printf '<!doctype html><title>verify</title>\n' > apps/desktop/dist/index.html
  stub_dist=1
fi
cleanup() {
  if [ "$stub_dist" = 1 ]; then rm -f apps/desktop/dist/index.html; fi
  if [ "$stub_dist_dir" = 1 ]; then rm -rf apps/desktop/dist; fi
  if [ "$stub_rchatd" = 1 ]; then rm -f apps/desktop/src-tauri/resources/rchatd; fi
  if [ "$stub_runtime" = 1 ]; then rm -rf apps/desktop/src-tauri/runtime; fi
}
trap cleanup EXIT

step "format: engine" cargo fmt --all -- --check
step "format: desktop shell" \
  cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
step "format: mobile shell" \
  cargo fmt --manifest-path apps/mobile/src-tauri/Cargo.toml --all -- --check

step "clippy: engine" cargo clippy --workspace --all-targets -- -D warnings
step "clippy: desktop shell" \
  cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
step "clippy: mobile shell" \
  cargo clippy --manifest-path apps/mobile/src-tauri/Cargo.toml --all-targets -- -D warnings

step "tests: scripts" node --test scripts/*.test.mjs
step "tests: bridge" python3 -m unittest discover -s sidecar/tests
step "tests: engine" cargo test --workspace
step "tests: desktop shell" cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
step "tests: mobile shell" \
  cargo test --manifest-path apps/mobile/src-tauri/Cargo.toml --lib

printf '\nverify: ok\n'
