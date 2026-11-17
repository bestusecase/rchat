# Show a dismissible notice when a config wins over old settings

Status: ready-for-agent
Type: task
Blocked by: 01
Effort: `mesh-upgrade-honesty`

## Question

When the legacy gate settles with source `config` and the old app settings had
an explicit relay or discovery-off, tell the user once on the Network page and
let them dismiss the notice permanently.

## Acceptance

- `ListInterfaces` includes an optional notice with a stable kind and a message
  that names only the skipped fields that were actually present.
- No notice for a normal legacy import, for a config-wins upgrade with no
  explicit old settings, or after dismissal.
- A new client command dismisses the notice by writing a meta flag; dismissal
  is idempotent and survives a restart.
- The Network page shows the notice above the interface list as an informational
  banner with a dismiss action, not a toast or modal.
- Tests assert the reply contents and dismissal persistence; no test reaches
  into UI internals.

## Notes

- The notice is derived, not stored as a row.
- See `.scratch/mesh-upgrade-honesty/spec.md`, decisions "The notice is derived
  from explicit settings only" and "The notice rides on the existing interfaces
  reply".
