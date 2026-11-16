# 12 — Retire the old network settings

Status: resolved
Type: task
Blocked by: 09, 11

## Goal

One place to change connectivity, and no stale env vars silently rewriting the config.

## Work

- Remove the Network section from `apps/desktop/src/App.svelte` (LAN discovery toggle and
  TCP relay field) and the `saveNetwork` / `autoDiscovery` / `relayDraft` plumbing in
  `store.svelte.js`; keep the Settings dialog for desktop/mobile preferences and updates,
  with a "Manage interfaces" button that opens the Network page on the Interfaces tab.
- Tauri shell (`apps/desktop/src-tauri/src/engine.rs`): stop setting `RCHAT_RNS_AUTO` and
  `RCHAT_RNS_TCP_CLIENT`; drop `save_network_settings` and the `auto_discovery` /
  `tcp_relay` settings fields (keep a read-only migration read of the old values).
- Migration: on first start after upgrade the bridge imports the existing `rns/config`,
  which already contains the interfaces those env vars produced. If no config exists,
  seed from the env vars once. Set `mesh_config_imported` so later env changes are
  ignored.
- The engine must send `configure` on every bridge start (ready or reconnect) so a
  restarted bridge gets the same interfaces.
- README: replace the "add an RNode to `<data-dir>/rns/config`" advice with the
  Interfaces tab, and document that hand-written config is imported once and then
  regenerated; the preserved-sections behaviour is the escape hatch.

## Acceptance

- A user upgrading with a TCP relay configured keeps that relay after the upgrade, with
  no env vars in the child process.
- Changing the relay in the new Interfaces tab survives an app restart.
- Grep shows no remaining `RCHAT_RNS_AUTO` / `RCHAT_RNS_TCP_CLIENT` writes from the
  desktop shell, and `save_network_settings` is gone.
- A hand-written extra section in `rns/config` survives ten restarts byte-for-byte.
- No UI path still calls `restart_engine` merely to change connectivity.

## Comments

<!-- outcome -->
- `apps/desktop/src-tauri/src/engine.rs` no longer sets `RCHAT_RNS_AUTO` or
  `RCHAT_RNS_TCP_CLIENT`, and no longer takes `Settings` to spawn the engine.
  `save_network_settings` is gone.
- `restart_engine` still exists, but only as engine recovery after a crash or update —
  nothing in the connectivity UI calls it any more; interfaces apply in place.
- `crates/rchatd/src/main.rs` still forwards `RCHAT_RNS_*` from its own environment to the
  bridge. That is the daemon's opt-in dev path and the import seed source
  (`seed_interfaces`), not the desktop shell.
