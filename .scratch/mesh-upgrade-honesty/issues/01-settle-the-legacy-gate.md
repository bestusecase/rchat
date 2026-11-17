# Settle the legacy import gate after the config import

Status: resolved
Type: task
Effort: `mesh-upgrade-honesty`

## Question

Close `mesh_network_imported` in the same start that successfully reads a
hand-written `rns/config`, and record which source won, without touching
`mesh_config_imported`.

## Acceptance

- The startup legacy-import step on a config-wins start leaves the legacy gate
  open and creates no interface rows; it must not touch
  `mesh_config_imported`.
- The bridge import still sets `mesh_config_imported` exactly as it does today.
- After a successful bridge `import_config` that produced at least one row, the
  engine closes the legacy gate and records source `config`.
- A successful import that produced no rows leaves the gate open, so first-run
  seeding still creates the default discovery row.
- A failed import never settles the gate.
- A second settle is a no-op; a normal legacy import still records
  `legacy`/`default`/`empty` exactly once.
- Store tests cover every branch through the public API; app tests assert only
  what clients are told.

## Notes

- Keep the two flags independent. Do not reuse `mesh_config_imported` as the
  legacy gate.
- Prior art: `a_present_rns_config_is_imported_before_anything_is_seeded`.
- See `.scratch/mesh-upgrade-honesty/spec.md`, decisions "The two import flags
  stay separate" and "The legacy gate settles with the config import".

## Comments

<!-- outcome -->
- `Store::settle_network_import` closes `mesh_network_imported` and records
  `mesh_network_source` after the bridge import succeeds with rows.
  `import_network_once` records `legacy`/`default`/`empty` in the same start and
  never touches `mesh_config_imported`.
- A failed import leaves both gates open and seeds nothing; existing rows still
  drive that session. A config with no interface rows still gets first-run
  discovery.
- Covered by the store source/settle tests, `a_config_winning_upgrade_...`,
  `a_failed_config_import_...`, `a_config_with_no_interfaces_...`, and the
  startup assertion in `a_hand_written_config_is_not_overwritten_...`.
- An install that closed the gate before `mesh_network_source` existed is
  backfilled once from the row origins; an empty or managed-only row list is
  not guessed into one of the four facts.
