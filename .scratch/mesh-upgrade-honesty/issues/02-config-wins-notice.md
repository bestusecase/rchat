# Show a dismissible notice when a config wins over old settings

Status: resolved
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

## Comments

<!-- outcome -->
- `Store::upgrade_notice` derives the notice from source `config` plus explicit
  old settings; `ListInterfaces` carries it as `notice {kind, message}` and
  `DismissInterfaceNotice` writes `mesh_upgrade_notice_dismissed`.
- The Network page renders it above the interface list as a dismissible
  informational banner.
- Covered by the store notice/dismissal tests and
  `a_config_winning_upgrade_tells_clients_what_was_not_imported`.
- Which fields were skipped is recorded in `mesh_upgrade_skipped` while
  `desktop.json` still carries them, so a later settings save cannot erase the
  notice. Dismissal is the only removal.
