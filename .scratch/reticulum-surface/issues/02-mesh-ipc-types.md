# 02 — Mesh types, commands and events in the wire protocol

Status: resolved
Type: task
Blocked by: none

## Goal

Carry mesh state from the engine to any client without breaking existing ones.

## Work

- Add to `crates/rchat-ipc`:
  - `MeshStatus`, `MeshInterface`, `MeshRadioStats`, `MeshCounters`, `PeerPath`,
    `PathEntry`, `AnnounceSighting`, `InterfaceInfo` (phase 2 shape stubbed), all with
    `#[serde(skip_serializing_if = "Option::is_none")]` on optional fields.
  - `ClientCmd::MeshStatus`, `ClientCmd::MeshPaths { query, offset, limit }`,
    `ClientCmd::MeshAnnounces { before, limit }`.
  - `Snapshot.mesh: Option<MeshStatus>` with `#[serde(default)]`.
- Parse the bridge's `ev: mesh` in `crates/rchat-rns`: new
  `BridgeEvent::Mesh(serde_json::Value)` (or a typed struct) and extend
  `BridgeEvent::Announce` with `interface`, `hops`, `aspect`.
- In `crates/rchatd/src/app.rs`:
  - cache the latest mesh payload in `AppState` behind an `RwLock`;
  - build `MeshStatus` = cached interfaces/counters + `path_total` + `peer_paths`
    (path table ∩ `store.list_peers()` addresses) + `other_services`;
  - publish `mesh.updated` with that value;
  - update `peers.bandwidth_class` from the path's interface bitrate
    (`>= 1_000_000` → `lan`, `>= 10_000` → `relay`, else `radio`, unknown when absent)
    and publish `peer.updated` only when it actually changed. This makes the existing
    badge in `Members.svelte` meaningful;
  - serve `MeshPaths` (filter + page the cached table) and `MeshAnnounces` (store query).
- Do not bump `PROTOCOL_VERSION`.

## Acceptance

- `cargo test --workspace` covers serialisation of a `MeshStatus` with zero peers, with
  radio stats, and with unknown fields present.
- An old client that ignores `mesh.updated` and the new `Snapshot.mesh` field still works.
- `MeshPaths` filtering matches hex prefix, announced name and interface name, and pages
  deterministically by hops then recency.
- `bandwidth_class` changes are persisted and only published on change.

## Comments

<!-- outcome -->
- `crates/rchat-ipc/src/lib.rs`: `MeshStatus`, `PeerPath`, `MeshInterface`, `PathEntry`,
  `AnnounceEntry`, `ServiceEntry`; commands `MeshStatus`/`MeshPaths`/`MeshAnnounces`;
  events `mesh.updated`, `announce.received`, `interface.changed`.
- Verified by `mesh_status_roundtrips_and_defaults_are_lenient`,
  `mesh_status_keeps_radio_stats_and_ignores_unknown_fields`, `mesh_commands_parse`,
  `interface_kind_serialises_as_kind`, `command_roundtrip_with_flattened_tag`.
- Deviation: the field is `interface_id`, not the spec's `id`. The request envelope
  already owns `id`, and `command_roundtrip_with_flattened_tag`-style parsing would
  otherwise reject a command that carries both.
- `MeshInterface.kind` deliberately has no serde rename; the bridge emits `kind`.
