# Keep internal preserved fragments out of the interface list

Status: ready-for-agent
Type: task
Blocked by: 01
Effort: `reticulum-surface-closeout`

## Question

The bridge now carries the preamble and the unknown generated-section lines as
`@`-prefixed fragments. The engine must persist and reconfigure them, but the
client must not see them as unmanaged config sections, or the Interfaces tab
will list a storage detail called `@header` under "Managed outside rchat".

## Acceptance

- A successful import persists every `[section, raw]` pair it receives,
  including the `@` ones, in order.
- The next `configure` sent to the bridge carries the same pairs in the same
  order, so the rendered file keeps its preserved content.
- The `extra` array in the `ListInterfaces` reply omits keys beginning with `@`;
  real unmanaged sections and `interfaces/<name>` blocks are still listed.
- The UI needs no change: it continues to render whatever `extra` it receives.
- An engine test drives the attach socket: reply to `import_config` with an
  import that includes `@header`, `@reticulum` and `@logging`, assert the stored
  order, assert the following `configure` payload contains them, and assert the
  `ListInterfaces` reply does not.
- A real config with one unmanaged section keeps that section in the list, so
  the filter is not over-broad.

## Notes

- The reserved prefix is the contract: anything starting with `@` is internal
  placement, never a section. Keep the filter in one place so the two replies
  cannot drift.
- No schema change and no migration; the existing extra-section table and the
  import reply already carry the data.
- Prior art: `a_hand_written_config_is_not_overwritten_by_first_run_seeding`
  and `a_failed_config_import_never_settles_the_legacy_gate` in
  `crates/rchatd/tests/engine.rs`.

## Comments

<!-- outcome -->
