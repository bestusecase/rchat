# Slices: Reticulum surface closeout

Status: proposed
Effort: `reticulum-surface-closeout`

Spec: [spec.md](spec.md). Tickets: `issues/`.

Two tickets are merged into the bridge slice because they share
`sidecar/tests/test_bridge.py`, and the plan has no dependency-ordering escape
from a shared write set. The README paragraphs from tickets 01, 04 and 05
become one documentation slice, for the same reason. The device record stays
separate because its proof is the phone, not the gate.

## 1. Keep the hand-written config through bridge import and render

Touches: sidecar/bridge.py, sidecar/tests/test_bridge.py

Depends on: none

Verify: ./scripts/verify.sh

- Parsing returns the file preamble as `@header` and the unknown lines of
  `[reticulum]` and `[logging]` as `@reticulum` and `@logging`, alongside the
  interface rows and the existing extra sections; the import reply shape is
  unchanged.
- A preserved line whose key the renderer generates for that section is
  dropped, not duplicated; comments and blank lines inside those sections are
  kept as free lines.
- Rendering writes the preamble first, then `[reticulum]` and `[logging]` with
  their preserved lines after the generated keys, then `[interfaces]`, then the
  unmanaged sections in their original order.
- Parsing and rendering the same input twice is byte-identical, including when
  the input repeats a generated section; duplicate sections merge their unknown
  lines in file order and emit the generated keys once.
- A missing config still imports as the empty shape, and a config with only
  generated keys produces no fragments.
- Bridge tests cover the preamble with comments, unknown keys in both generated
  sections, a generated-key collision, idempotence, duplicate sections and the
  generated-only config.
- The mesh sampling contract is proven at the existing emit boundary: one
  payload emitted twice produces zero then one event, an announce wake and an
  interface change each produce exactly one, and byte counters alone produce
  none; the sampling loop leaves when readiness drops, asserted without a
  wall-clock sleep.
- A synthetic path table above the 2000-entry cap yields exactly 2000 paths
  ordered nearest hops first and most recently used within a hop count, with
  `paths_truncated` true and the untruncated total; a table at the cap is not
  truncated.
- No engine, IPC or shell change: engine persistence and the client filter are
  slice 2, and the import reply already carries every pair.

## 2. Persist internal fragments and hide them from the client

Touches: crates/rchatd/src/app.rs, crates/rchatd/tests/engine.rs

Depends on: 1

Verify: ./scripts/verify.sh

- A successful import persists every `[section, raw]` pair, including `@`
  fragments, in the order received; the next `configure` carries the same pairs
  in the same order.
- The `extra` array of `ListInterfaces` omits keys beginning with `@`, so the
  page lists only real unmanaged sections; `interfaces/<name>` blocks and real
  sections are still listed.
- An attach-socket test replies to `import_config` with `@header`, `@reticulum`
  and `@logging`, asserts the stored order, asserts the following `configure`
  payload contains them, and asserts `ListInterfaces` does not.
- A config with one unmanaged section keeps that section in the list, so the
  filter is not over-broad.
- No shell change: the UI renders whatever `extra` it receives.

## 3. Script the hot-apply interface check

Touches: +scripts/smoke-hot-apply.mjs

Depends on: none

Verify: ./scripts/verify.sh

- Against a running daemon it creates a TCP server on a free port, waits until
  `mesh_status` reports it online and the row appears in `rns/config`, deletes
  it, and waits until both are gone with no row left.
- It then attempts an apply that must fail because its own server already binds
  the port, asserts the refusal names the interface, and asserts the previously
  working interface is still online with its stored row unchanged.
- It exits non-zero on any failed assertion, closes its client on every path,
  and removes the rows it created even when an assertion fails.
- It reuses `scripts/lib.mjs` and the public command vocabulary the UI uses; no
  engine, bridge or IPC change.
- It runs against the leaves of `scripts/dev-mesh.sh` or
  `scripts/dev-two-peers.sh` with only the base URL and token; the README names
  it in slice 5.

## 4. Make the Android network check repeatable

Touches: +scripts/smoke-android.sh, +scripts/smoke-android.mjs

Depends on: 1, 2

Verify: ./scripts/verify.sh

- The shell entry point builds the debug arm64 APK from the current tree,
  installs it over the existing app without clearing data, writes a
  hand-written `rns/config` with a unique `group_id` and a TCP client row,
  launches the app, forwards the engine port and runs the node assertions.
- The assertions cover: both rows imported with their original values and
  `origin = imported`, no duplicate seed row, AutoInterface online, toggling it
  off gives the derived `degraded` transport with no online interface and back
  on gives `ready`, a message to a desktop daemon arrives with a `delivered`
  receipt, `list_devices` returns an empty serial list with a reason, and
  `describe` returns every type.
- The phone uses `adb reverse` for the TCP link; the forward and reverse
  mappings are removed on exit and the engine port comes from the app's own
  endpoint file.
- `--fresh` is the only path that clears app data; the default run is
  non-destructive and reuses the identity and rows.
- With `adb`, the device or the SDK/NDK environment missing, the script prints
  one reason and exits 0, matching the rendezvous scripts.
- The manual steps still required are in the script's usage text and in ticket
  05: the page with the engine stopped, and a background/foreground cycle.
- No engine, bridge or shell change; the assertions are the public API calls
  the shared UI makes.

## 5. Document the new seams

Touches: README.md

Depends on: 1, 3, 4

Verify: ./scripts/verify.sh

- The config-import paragraph says the file header and unknown keys inside
  `[reticulum]` and `[logging]` are kept, and that rchat's generated keys remain
  the only authority for transport mode, sharing, ports and log level.
- The mesh section names `scripts/smoke-hot-apply.mjs` and the command that
  runs it against the `dev-*` leaves.
- The Android section names `scripts/smoke-android.sh`, the `adb reverse` link
  and the `--fresh` flag.
- No paragraph claims behaviour the code does not have.

## 6. Record the final hardware pass

Touches: .scratch/reticulum-surface-closeout/issues/05-repeatable-android-check.md, .scratch/reticulum-surface/issues/13-android-parity.md

Depends on: 5

Verify: ./scripts/smoke-android.sh

- With the Pixel 8 attached and a desktop daemon running, confirm HEAD, rebuild
  and install from that commit, and run the non-destructive Android check; a
  skip is not a pass.
- Record the commit hash, the phone serial and the date in the outcome of
  ticket 05, and append a pointer from
  `.scratch/reticulum-surface/issues/13-android-parity.md` so the older
  acceptance does not claim a tree it did not run on.
- Complete and record the manual observations: the Network page with the engine
  stopped shows last-known data plus the offline banner, and a
  background/foreground cycle refreshes the graph and paths without a reload.
- Confirm HEAD is unchanged before and after the run; a failure becomes its own
  ticket-sized change, and the check is rerun rather than recorded as a partial
  pass.
- The gate still passes: `./scripts/verify.sh`.
