# Settle the legacy import gate after the config import

Status: ready-for-agent
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
