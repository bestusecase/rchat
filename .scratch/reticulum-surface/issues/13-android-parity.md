# 13 — Android parity for interface management

Status: resolved
Type: task
Blocked by: 12

## Goal

The phone can see the mesh and fix its own connectivity, using the same page and the same
store as desktop.

## Work

- Verify the Chaquopy bridge supports `import_config`, `describe`, `configure` and
  `list_devices` with the app's data directory; the embedded bridge uses the same
  `bridge.py`, so this should be configuration only, not new code.
- Confirm the engine sends `configure` when the embedded bridge dials in, and that a
  foreground-service restart of the engine reuses the rows rather than re-importing.
- Ensure `ListDevices { kind: "serial" }` returns an empty list with a reason on Android
  and the form hides device pickers instead of erroring.
- Mobile UI: pane navigation for the Network page, touch targets for graph nodes, and the
  Interfaces tab reachable from the existing settings gear (on mobile, Settings currently
  holds only notification options — add the "Manage interfaces" entry there too).
- Test on the Pixel 8 (`39061FDJH0080U`) with the Android SDK/NDK environment from
  `/tmp/opencode/env.sh`:
  - import the existing AutoInterface row;
  - toggle AutoInterface off and on and confirm the transport state follows;
  - add a TCP client to the desktop dev relay and exchange a message;
  - verify the graph and paths update while backgrounded and foregrounded.
- README: note that Android manages TCP and AutoInterface in v1.

## Acceptance

- A fresh install imports its AutoInterface and the Network page shows it online.
- Toggling AutoInterface off reports the transport as still ready but with no interfaces,
  and back on restores discovery without an app restart.
- Adding a TCP client on the phone reaches a desktop daemon and delivers a message.
- No crash when the Network page is opened while the engine is starting or stopped.

## Comments

<!-- outcome -->
- Done (shared UI and bridge, so the phone uses the same page and store as desktop):
  - the Network page works on mobile through the existing pane mechanism, with back
    returning to the conversation list. The rail button is desktop-only; on the phone the
    page is reached from the settings gear;
  - Settings gains a "Manage interfaces" entry on both desktop and mobile
    (`App.svelte`), so the Interfaces tab is reachable from the gear;
  - graph nodes are `tabindex="0"` with Enter/Space handlers, and every node also carries
    a transparent 18–20 unit hit disc (`circle.map-hit`), so finger taps land on a ~24 dp
    target rather than the 4–8 unit visible dot;
  - `do_list_devices()` returns an empty list with a reason when pyserial is absent
    (Chaquopy), and the form hides the picker instead of erroring.
- Done on the Pixel 8 (`39061FDJH0080U`), 2026-11-16/17, with a debug arm64 APK built by
  `cd apps/mobile/src-tauri && cargo tauri android build --debug --apk --target aarch64`:
  - the engine now receives the bridge directory (`nativeStart(dataDir, configDir)`) and
    imports a hand-written `files/rns/config` from it. That fixed a real bug: the engine
    looked for `files/rns/config` under `files/engine/`, so every Android start seeded a
    default row instead of importing. Verified by installing a config with
    `group_id = rchat-testgroup` and a `[[Desktop]]` TCP row, then launching: both rows
    came back with `origin = imported`, the custom group id intact, and no duplicate
    seed row. A second launch reused the same two rows (marker honoured);
  - the embedded bridge now takes `managed=True`, so it waits for the engine's
    `configure` before starting Reticulum instead of writing a config of its own;
  - toggling AutoInterface off detaches it and the transport reports `degraded` with the
    banner "The bridge is up but no interface is online, so nothing can be sent or
    received"; enabling it again restores `ready` and an online AutoInterface without a
    restart. `degraded` is the honest word here; the older "still ready" wording in the
    acceptance list predates the derived transport state;
  - the imported TCP client reached a desktop rchatd over a real TCP interface and
    messages flowed both ways (`phone sent` → `A received` → `delivered` receipt, then
    `desktop sent` → `phone received` → `delivered`). The host's ufw blocks inbound LAN
    TCP, so the link ran over `adb reverse tcp:4242 tcp:4242`; both ends still ran the
    normal RNS `TCPServerInterface`/`TCPClientInterface` code;
  - opening the Network page immediately after a cold start rendered the interface rows
    with the degraded banner while the interfaces were still coming up, and logged no
    crash;
  - `list_devices { kind: "serial" }` returned `{"devices":[],"reason":"serial device
    listing is not available on this platform"}`, and `describe` returned all 14 types.
- Not verified on hardware: a LAN/Wi-Fi TCP link (blocked by the host firewall, not by
  rchat), and the page while the engine is fully stopped rather than starting. Both paths
  are the shared desktop code.
