# 13 — Android parity for interface management

Status: ready-for-agent
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
  - the mesh rail button and the Network page work on mobile through the existing pane
    mechanism, with back returning to the conversation list;
  - Settings gains a "Manage interfaces" entry on both desktop and mobile
    (`App.svelte`), so the Interfaces tab is reachable from the gear;
  - graph nodes are `tabindex="0"` with Enter/Space handlers, so they work with touch and
    keyboards;
  - `do_list_devices()` returns an empty list with a reason when pyserial is absent
    (Chaquopy), and the form hides the picker instead of erroring.
- Not done: the Pixel 8 (`39061FDJH0080U`) run. The acceptance items — fresh-install
  AutoInterface import, toggle off/on following transport state, a TCP client reaching a
  desktop daemon, and opening the page while the engine starts — are still unverified on
  hardware. Ticket stays open for that.
