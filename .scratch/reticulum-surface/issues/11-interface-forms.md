# 11 — Interface management UI and bespoke forms

Status: resolved
Type: task
Blocked by: 10

## Goal

Make the Interfaces tab the one place to add, edit, enable and delete every kind of
Reticulum interface, with a tailored form per type.

## Work

- `NetworkInterfaces.svelte`:
  - list rows with name, type label, online dot, bitrate, Rx/Tx rate, peers, radio
    summary (RSSI/SNR/airtime when reported), origin badge (`rchat` / `imported` /
    `outside rchat`);
  - Add button → type picker grouped as Local, TCP, Radio, Other;
  - per-row Edit, Enable/Disable, Delete (confirm text names the risk of losing the last
    path);
  - read-only rows for unmanaged sections with the reason.
- `InterfaceForm.svelte`: renders the descriptor.
  - widgets: `text`, `mono`, `secret` (reveal toggle), `port`, `int` (units in the
    label), `bool`, `select`, `serial` (populated by `ListDevices`);
  - `required` enforced; `show_if` honoured; common fields first, Advanced collapsed by
    default and opened automatically when an unknown key or a validation error lives
    there;
  - save → engine call → on `ok` update the row; on failure keep values, map the RNS
    error to the nearest field by key name when possible, otherwise show a banner with
    Retry/Revert.
- AutoInterface form: group id plus `interface_enabled`; note that it discovers peers on
  the local link.
- Radio forms (RNode/KISS): frequency (MHz), bandwidth (kHz), spreading factor, coding
  rate, TX power, airtime limits — common; advanced keys as declared. Show measured radio
  stats read-only from the Interfaces row.
- Empty state explains that without an interface rchat is local-only, with an Add button.
- Mobile: same components; hide `serial` widgets with the platform reason when
  unavailable.

## Acceptance

- Adding a TCP client through the form connects and shows online within a few seconds.
- A duplicate section name is rejected in the form before submit and again by the engine.
- A failing edit leaves the interface connected, shows RNS's message, and Retry succeeds
  after fixing the field.
- Advanced fields round-trip: open an imported interface, change one advanced value,
  save, reload the page, and the value persists.
- Delete removes the row, the config section, and the live interface.
- Keyboard and screen-reader users can complete add and edit without the graph.

## Comments

<!-- outcome -->
- `apps/desktop/src/lib/InterfaceForm.svelte` is descriptor-driven: required fields first,
  `common: false` and imported-but-unknown keys under Advanced with a guessed widget, a
  serial picker fed by `listDevices`, and a duplicate-name check before submit (still
  enforced by the engine and the DB unique index, per D18).
- Apply failure keeps the form's values, shows a banner and a toast, and offers Retry;
  `interface.apply_failed` arrives even when the user has navigated away.
- Shared-instance and I2P rows are read-only: banner, badge and disabled controls, with
  delete left enabled as the escape hatch.
- Read-only rows come from the bridge (`READ_ONLY_KINDS`, `SHARED_READ_ONLY_REASON`), and
  the engine refuses create/update/enable for them as well as the UI; covered by
  `read_only_reasons_come_from_the_bridge_descriptor`.
- Deviation from the spec: the error banner names the row rather than quoting RNS, because
  `_attach_interface` returns a bare `False` (see ticket 09).
