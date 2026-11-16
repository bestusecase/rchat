# 09 — Interface store, config rendering and hot apply

Status: resolved
Type: task
Blocked by: 02

## Goal

Move interface configuration into the engine, render `rns/config` from it, and apply
edits without restarting the engine.

## Work

- Store (`crates/rchat-core/src/store.rs`): add `mesh_interfaces` and `mesh_config_extra`
  with the spec's columns; CRUD helpers (`list_interfaces`, `create_interface`,
  `update_interface_config`, `set_interface_enabled`, `delete_interface`,
  `extra_config`, `replace_extra_config`); meta key `mesh_config_imported`.
- Engine (`crates/rchatd/src/app.rs`): new commands `ListInterfaces`, `CreateInterface`,
  `UpdateInterface`, `SetInterfaceEnabled`, `DeleteInterface`. Each mutation loads the
  rows, applies the candidate in memory, sends the full desired set to the bridge, and
  persists only after `ok`. Validate name shape (1–64 chars, no `[`/`]`/newline) and that
  `config` is a JSON object.
- Bridge (`sidecar/bridge.py`):
  - `import_config`: if `rns/config` exists, split it line-based into `[[section]]`
    blocks; parse sections whose `type` is a known RNS interface into rows (name, type,
    ordered key/value), keep everything else (reticulum, logging, unknown sections)
    verbatim. Answer with rows + extra.
  - `configure`: render the whole file (managed `[interfaces]` + preserved sections),
    write atomically (temp + fsync + rename, mode 0600), then diff against the running
    set and call `attach_interface` / `reload_interface` / `detach_interface`.
  - rollback: keep the previous rendered text and applied set; on any failure rewrite the
    previous text and restore the interfaces, then answer `ok: false` with the RNS error.
  - On bridge start with no `import_config` call and rows already imported, wait for the
    engine's `configure` before starting the router, so the first announce uses the right
    interfaces.
- Remove the env-driven regeneration path (`bridge.py:192-208`) for interfaces once
  `mesh_config_imported` is set; keep seeding from env only when no config exists.
- Permissions and paths: respect `RCHAT_RNS_CONFIGDIR`, never write outside it.

## Acceptance

- Creating a TCP client row attaches it live: the interface appears in `mesh_status`
  without an engine or bridge restart, and it is in `rns/config` after the call.
- Editing its port reloads it; disabling detaches it; deleting removes the section.
- A port already in use makes the call fail, leaves the row absent (create) or unchanged
  (edit), and the previously working interfaces stay up.
- Importing a config with an unknown section and a `[[My RNode]]` leaves the RNode as a
  managed row and the unknown section byte-identical in the rewritten file.
- Two consecutive starts do not duplicate sections or reorder them.
- Store tests cover CRUD and the import marker; a Python unittest covers render +
  passthrough round-trip.

## Comments

<!-- outcome -->
- Store: `mesh_interfaces` + `mesh_config_extra`. Bridge: `render_config()`,
  `parse_config_text()` (line-based, so comments survive) and `write_config_text()`
  (single `os.write` loop). `apply_configure()` attaches/reloads/detaches, rolls back
  from `_applied_map()` — which holds only enabled rows, so a rollback cannot re-attach
  what the user just disabled.
- Engine: create/update/enable/delete plus the `configure` command; `apply_or_report()`
  emits `interface.apply_failed` on every apply path.
- Tests: `interfaces_crud_and_meta`, `extra_config_round_trips_in_order`,
  `test_render_round_trips_managed_rows`, `test_unknown_interface_and_sections_are_preserved`,
  `test_shared_configure_writes_the_client_config_only`.
- Caveat: RNS's own refusal text cannot be shown verbatim. `_attach_interface` logs and
  returns `False`, so the reply names the refused row (`InterfaceFailure.name`) instead of
  quoting RNS.
