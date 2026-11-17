# Shrink the mobile settings payload and lock the two-field shape

Status: ready-for-agent
Type: task
Effort: `mesh-upgrade-honesty`

## Question

Remove the dead `autoDiscovery` and `tcpRelay` fields from the mobile settings
reply, and assert the same two-field shape on both shells so the shared UI
cannot drift from either one.

## Acceptance

- The mobile `get_desktop_settings` reply contains exactly `muted` and
  `autostart`.
- The legacy `desktop.json` reader and `LegacyNetwork` type remain as migration
  input and are not part of the live settings API.
- Desktop and mobile expose the same two-field shape; sharing a type is optional
  and must not pull engine dependencies into the standalone desktop shell.
- Serialization tests assert the exact key set on both shells.
- `npm --prefix apps/desktop run build` and both Tauri `cargo check`s pass.

## Notes

- Mobile `autostart` stays `false`; Android has no login autostart.
- See `.scratch/mesh-upgrade-honesty/spec.md`, decisions "The mobile settings
  payload shrinks" and "One settings shape, two small types".
