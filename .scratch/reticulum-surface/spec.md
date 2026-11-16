# Reticulum surface and mesh map

Status: ready-for-agent
Effort: `reticulum-surface`
Prerequisite: LXMF propagation is a **separate effort** (`lxmf-propagation`) and ships
first. This spec does not implement it.

## Problem Statement

rchat speaks Reticulum but cannot show it. The only connectivity controls are a LAN
discovery toggle and one TCP relay, and both are buried in a settings dialog that applies
by restarting the engine. Nothing in the app says which interfaces are up, how many hops
away a peer is, which transport node carries the path, or what else is on the mesh. When
connectivity breaks, the user's only remedy is to guess at a text field or hand-edit
`rns/config` — which the desktop build silently rewrites on every launch, because the
shell always exports `RCHAT_RNS_AUTO`. Peer `bandwidth_class` is hardcoded `"unknown"` in
three places, so even the existing badge is decorative. "Is my friend reachable, and
through what?" is unanswerable from inside the app.

## Solution

A first-class Reticulum surface: a new rail destination opening a tabbed page with
Overview, Interfaces, Paths, Announced and Map. The bridge reports live transport state
(interfaces, counters, the path table) on change; the engine caches it, serves queryable
paths, and persists a bounded announce history. The map is an honest topology graph built
from that data — peers arranged by hop distance, first-hop transport nodes between them,
edges labelled with the real interface — never a fabricated multi-hop path, and never a
world map Reticulum cannot support.

Interface configuration moves into `rchat.db`; `rns/config` becomes a generated artifact.
Every RNS interface type gets a bespoke form driven by a schema the bridge owns, applied
hot through `attach`/`reload`/`detach_interface` and persisted only after RNS accepts it,
with automatic rollback on failure. Existing hand-written configs are imported once and
anything rchat does not understand is preserved byte-for-byte. A geographic layer is
deferred, with Sideband-compatible LXMF telemetry chosen as its data source.

## Scope

Give rchat one place that shows the Reticulum mesh the engine is attached to, and lets
the user manage the interfaces that connect it:

- A new rail destination opening a full-page, tabbed surface: **Overview**,
  **Interfaces**, **Paths**, **Announced**, **Map**.
- Fully read-only observability on desktop and Android.
- Interface management (add, edit, enable, disable, delete) for every RNS interface
  type, hot-applied without restarting the engine.
- A **mesh map**: an honest topology graph of known chat peers and the first-hop
  transport nodes that reach them, drawn from the real Reticulum path table.

The RNS layer is the subject. LXMF propagation, stamps and sync state are out of scope
and arrive through the prerequisite effort; the map reserves a `propagation` role so
those nodes can be added without reshaping the model.

## Why the map is a graph, not a world map

Reticulum exposes **no geographic location** for any node, and no global topology. What
it does expose locally:

- `RNS.Transport.path_table`: for each destination, `[timestamp, next_hop, hops,
  expires, random_blobs, receiving_interface, packet_hash, used]`. For a direct
  neighbour `next_hop` equals the destination itself (`Transport.py:2480`).
- `RNS.Transport.path_states` / `path_is_unresponsive()`.
- Interface statistics, including per-interface peers and RNode radio telemetry.

Therefore the graph can show: who you know, how many hops away they are, which
interface that path uses, and the first transport node on the path. It can **not**
reconstruct the full intermediate chain. The layout must never imply otherwise.

A geographic layer is deferred. Its data source is decided: Sideband-compatible LXMF
telemetry (`FIELD_TELEMETRY`, field 2) plus an explicit opt-in "share my location". v1
only reserves optional `location` and `role` fields so that layer is additive.

## Evidence from the current code

| Fact | Where |
| --- | --- |
| Only mesh controls are LAN discovery and one TCP relay | `apps/desktop/src/App.svelte:379-406` |
| Those are applied by restarting the engine | `apps/desktop/src/lib/store.svelte.js:492-503` |
| Desktop shell passes `RCHAT_RNS_AUTO` and `RCHAT_RNS_TCP_CLIENT` to the bridge | `apps/desktop/src-tauri/src/engine.rs:164,173-176` |
| Bridge events are only `state/message/announce/receipt/error/log` | `crates/rchat-rns/src/lib.rs:31-59,339-379` |
| Bridge has an unused `status` command | `sidecar/bridge.py:441-449` |
| Bridge regenerates `rns/config` whenever any interface env var is set | `sidecar/bridge.py:192-208` |
| Because the desktop always sets `RCHAT_RNS_AUTO`, the config is **regenerated every launch**, discarding hand-added interfaces | consequence of the two rows above |
| Peers carry `address`, names, `last_seen`, `bandwidth_class`, `caps` only | `crates/rchat-core/src/store.rs:82-91` |
| `bandwidth_class` is hardcoded `"unknown"` in three paths and never updated | `crates/rchatd/src/app.rs:150,339,391` |
| Wire protocol is snapshot + monotonic events, `PROTOCOL_VERSION = 1` | `crates/rchat-ipc/src/lib.rs:14,124-203` |
| RNS 1.6 has runtime `attach_interface`, `detach_interface`, `reload_interface`, `get_interface_stats` (management enabled by default) | `RNS/Reticulum.py` |
| `path_table` provides hops + first next hop + receiving interface | `RNS/Transport.py:2456-2480,2726-2733` |
| LXMF telemetry is field 2; announce `app_data` carries the display name only | `LXMF/LXMF.py:152-180` |

## Decision log

| # | Decision | Reason |
| --- | --- | --- |
| D1 | Ship the topology graph now; geographic layer later | Only the graph is supported by data Reticulum already carries |
| D2 | Scope is read-only observability **plus** interface management | Explicit product choice; rnsd administration (transport mode, tunnels, blackhole) stays out |
| D3 | `rchat.db` owns interface configuration; `rns/config` is generated | One source of truth, identical on desktop and Android, and it turns the existing config-clobbering bug into defined behaviour |
| D4 | Apply changes hot through RNS's interface management API | No engine restart, no websocket drop; also the only workable path on Android |
| D5 | A bespoke form per interface type, driven by a schema descriptor | Every one of the eleven RNS types is first-class without hand-writing eleven forms twice |
| D6 | The Python bridge owns the type/field schema | It sits next to RNS, versions with it, and its construction error is the authoritative validation message |
| D7 | Common fields first, remaining documented keys under **Advanced** | Beginner-usable, expert-complete |
| D8 | Apply, then persist only on success; roll back to last-good on failure | The database never contains an interface that was not live; next boot is always last-known-good |
| D9 | Import a existing hand-written config once; preserve unparsed content byte-for-byte | No data loss on upgrade |
| D10 | Graph nodes: known LXMF peers + distinct first-hop transports; other destinations behind a toggle | Honest mesh picture without drowning contacts in unnamed hashes |
| D11 | Bridge samples ~2s, pushes on fingerprint change and immediately on announce/interface change | Live without a per-client poll loop or battery cost |
| D12 | New rail destination with tabs; same page on Android | Full page fits a graph and a table; mobile keeps feature parity |
| D13 | Announce history persisted, bounded: 7 days, newest 5000, coalesced per 10-minute bucket | Operationally useful, bounded growth |
| D14 | Network section removed from Settings; its controls become interface rows | One place to change connectivity |
| D15 | Phase 1 read-only + map, phase 2 interface management, phase 3 Android parity | Each phase is shippable and testable |
| D16 | Hand-rolled SVG radial layout, no graph dependency | Keeps the interface light (README's ~70 KB claim) and keyboard accessible |
| D17 | Keep `PROTOCOL_VERSION = 1`; additions are optional fields and new commands/events | Additive changes; a stale browser tab simply lacks the new page |
| D18 | Interface mutations are validated shape-wise in the engine and in RNS in the bridge | UI validation is a convenience, not a guarantee |
| D19 | Links/tunnels/blackhole appear only as counters | Out of product scope (D2) |
| D20 | Multi-hop verification uses a plain `rnsd` transport node, not rchat | rchat stays a leaf node, per D2 |

## Data model

### New tables (`crates/rchat-core/src/store.rs`)

```sql
-- Interfaces rchat manages. `config` is opaque JSON to the engine.
CREATE TABLE IF NOT EXISTS mesh_interfaces (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,   -- section name in rns/config
  kind       TEXT NOT NULL,          -- RNS type, e.g. TCPClientInterface
  config     TEXT NOT NULL,          -- JSON object, key -> string|bool|int
  enabled    INTEGER NOT NULL DEFAULT 1,
  origin     TEXT NOT NULL,          -- 'managed' | 'imported'
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Sections rchat does not understand, preserved verbatim.
CREATE TABLE IF NOT EXISTS mesh_config_extra (
  section TEXT PRIMARY KEY,
  raw     TEXT NOT NULL
);

-- Coalesced announce sightings.
CREATE TABLE IF NOT EXISTS mesh_announces (
  id         INTEGER PRIMARY KEY AUTOINCREMENT,
  address    TEXT NOT NULL,
  name       TEXT,
  aspect     TEXT,
  interface  TEXT,
  hops       INTEGER,
  bucket     INTEGER NOT NULL,       -- ts / 600000
  first_seen INTEGER NOT NULL,
  last_seen  INTEGER NOT NULL,
  count      INTEGER NOT NULL DEFAULT 1,
  UNIQUE(address, bucket)
);
CREATE INDEX IF NOT EXISTS idx_mesh_announces_last ON mesh_announces(last_seen);
```

Bootstrap marker in `meta`: `mesh_config_imported = 1`.

Migration adds tables only; schema marker moves to `2`.

Pruning runs on insert at most once a minute: delete rows older than 7 days, then trim to
the newest 5000 by `last_seen`.

### Mesh status payload

`MeshStatus` is engine state, never persisted:

```jsonc
{
  "transport": "ready",              // offline | starting | ready
  "address": "…32 hex…",             // our LXMF delivery address
  "started_at": 1760000000,          // bridge ready time, seconds
  "interfaces": [                    // whitelisted RNS get_interface_stats()
    { "name": "AutoInterface", "kind": "AutoInterface", "hash": "…",
      "online": true, "bitrate": 10000000, "mtu": 500,
      "rxb": 1234, "txb": 456, "rxs": 12.4, "txs": 3.1,
      "peers": 3, "mode": "full", "ifac": null,
      "radio": { "rssi": -88, "snr": 7.5, "frequency": 868000000, "sf": 8,
                 "bandwidth": 125000, "airtime_short": 1.2 } }
  ],
  "counters": { "rx_packets": 0, "tx_packets": 0, "rx_pps": 0.0, "tx_pps": 0.0,
                "announce_rx": 0, "announce_tx": 0, "links": 0,
                "path_count": 0, "path_unresponsive": 0 },
  "peer_paths": [                    // known store peers ∩ path table, usually small
    { "address": "…", "hops": 2, "next_hop": "…", "interface": "TCP Client",
      "bitrate": 62500, "state": "responsive", "age": 42.0 }
  ],
  "other_services": [                // recent non-LXMF destinations, in-memory only
    { "destination": "…", "aspect": "nomadnetwork.node", "last_seen": 1760000000 }
  ]
}
```

Interfaces are a **whitelist** of the fields above, not a raw dump: `get_interface_stats`
contains bytes and private objects, and a narrower payload stays compatible with future
RNS releases.

`peer_paths` covers only peers already in the `peers` table, so the graph never needs the
full path table. The Paths tab pages the full table through its own command.

## Wire protocol (`crates/rchat-ipc/src/lib.rs`)

Additive only.

```rust
// ClientCmd
MeshStatus,                                        // -> { mesh: MeshStatus }
MeshPaths { query: Option<String>, offset: Option<u32>, limit: Option<u32> },
                                                   // -> { paths, total }
MeshAnnounces { before: Option<i64>, limit: Option<u32> },
                                                   // -> { announces }
DescribeInterfaces,                                // -> { types: [InterfaceType] }  (phase 2)
CreateInterface { name: String, kind: String, config: Value },   // (phase 2)
UpdateInterface { id: String, config: Value },                    // (phase 2)
SetInterfaceEnabled { id: String, enabled: bool },                // (phase 2)
DeleteInterface { id: String },                                   // (phase 2)
ListInterfaces,                                    // -> { interfaces } (phase 2)
ListDevices { kind: String },                      // -> { devices } (phase 2, serial)

// Snapshot gains
mesh: Option<MeshStatus>,                          // serde default

// Events
"mesh.updated"          { mesh }
"announce.received"     { announce }               // after the row is written
"interface.changed"     { interface }               // phase 2
"interface.apply_failed"{ name, error }             // phase 2
```

`MeshPaths` results are ordered by hops then most recently used. `query` matches the
destination hex prefix, the announced name, or the interface name, case-insensitively.

## Bridge protocol (`sidecar/bridge.py`)

### Phase 1

- Sample every 2 s on a daemon thread while the readiness flag is set.
- Fingerprint = interface `(name, online, bitrate, peers)` tuples + `path_count` +
  `announce_rx` + `transport ready` + each peer's `(hops, next_hop, interface)` when the
  engine asks for peers. Cheap; do not fingerprint byte counters (they always change).
- Emit `{"ev":"mesh","mesh":{…}}` only when the fingerprint changes, and immediately from
  `AnnounceHandler.received_announce` and after an interface change.
- The bridge event carries `interfaces`, `counters`, `path_total` and the **full path
  table** in a compact form, capped at 2000 entries (nearest hops first, so known peers
  are never the ones dropped) with `paths_truncated`. The engine caches the latest list
  and serves `MeshPaths` by filtering it; the client-facing `MeshStatus` only carries
  `peer_paths`. No request/response correlation is added to the bridge protocol.
- Announce events gain `[interface]`, `[hops]` and `[aspect]` fields. `aspect` comes from
  `RNS.Transport.destinations` when the destination is still cached, otherwise omitted.
- Register a second, unfiltered announce handler that keeps only a small in-memory ring
  (200) of non-LXMF destinations for the "other services" graph toggle. Do not persist it.

Sampling runs with `path_table_lock` held only for the copy.

### Phase 2

- `describe`: return the type catalogue and field schema (see below).
- `configure`: receive the **full desired interface set** plus preserved sections, render
  `rns/config`, then apply the diff:
  - new name → `attach_interface(name)`
  - changed config → rewrite + `reload_interface(name)`
  - disabled or removed → `detach_interface(name)`
- On any failure, restore the previous config file and re-attach/detach to the previous
  state, then reply `{"ok": false, "name": …, "error": …}`.
- On success, update the bridge's desired set and reply `{"ok": true}`.
- Commands that need an answer (`configure`, `import_config`, `describe`, `list_devices`)
  carry a `token` and are answered by an event with the same `token`. This is the only
  request/response correlation in the bridge protocol; the engine holds a pending map
  with a timeout.
- Renderer details:
  - `[reticulum]` with `enable_transport = No`, `share_instance` per mode, unchanged from
    today's `write_config`.
  - `[logging]` unchanged.
  - `[interfaces]` from store rows, `[[name]]` sections in creation order.
  - Preserved sections re-emitted byte-for-byte after the managed ones.
  - Atomic write: temp file in the same directory, `fsync`, rename.
- Env interface variables are **ignored** once `mesh_config_imported` is set; they are
  only read for the initial seed when no config exists.

### Field schema (`describe`)

```jsonc
{ "types": [ {
    "kind": "TCPClientInterface",
    "label": "TCP client",
    "summary": "Connect to a peer or relay over TCP.",
    "fields": [
      { "key": "target_host", "label": "Host", "widget": "text",
        "required": true, "placeholder": "relay.example", "common": true,
        "help": "Hostname or IP of the relay." },
      { "key": "target_port", "label": "Port", "widget": "port",
        "required": true, "default": 4242, "common": true },
      { "key": "kiss_framing", "label": "KISS framing", "widget": "bool",
        "default": false, "common": false },
      { "key": "i2p_tunneled", "label": "Via I2P", "widget": "bool",
        "default": false, "common": false }
    ] } ] }
```

`widget` is one of `text`, `mono`, `secret`, `port`, `int`, `bool`, `select`, `serial`.
Conditional visibility uses `show_if: { key, equals }`. `common: false` fields render
under **Advanced**. Unknown keys already present in an imported interface are preserved
and shown in Advanced as editable rows, so an RNS release that adds a field never loses
data.

## Interface management semantics

- **Ownership.** Rows are the truth. The config file is regenerated on every apply and on
  bridge start.
- **Import.** On first run after upgrade: if `rns/config` exists, parse `[[section]]`
  blocks line-based (not through ConfigObj, which does not round-trip comments), write
  parseable interfaces to `mesh_interfaces` with `origin = 'imported'`, and every other
  section verbatim to `mesh_config_extra`. If no config exists, seed rows from the
  interface env vars the bridge already receives.
- **Names.** Section names are user-visible and unique. Generated as `<Type label>`,
  `<Type label> 2`, … Reject `[`, `]`, newlines and empty names. 1–64 characters.
- **Apply failure UX.** The editor keeps unsaved values, shows RNS's error verbatim next
  to the offending field when it can be mapped, otherwise in a banner, with **Retry** and
  **Revert**. No global "settings saved" message that hides a failure.
- **Unmanaged sections.** Listed read-only under "Managed outside rchat", with the
  reason (could not parse / unsupported).
- **Deletion** detaches first, then deletes the row; confirm in the UI because it can
  cut the last path to a peer.

## UI

### Navigation

- New rail button (mesh glyph) between Home and Add peer in `Sidebar.svelte`.
- Selecting it replaces the chat area with `Network.svelte`; mobile uses the same pane
  mechanism as a conversation (`data-pane`), and back returns to the conversation list.
- Tabs: Overview, Interfaces, Paths, Announced, Map. Tab state is local to the page and
  remembered for the session.
- Page works with transport offline: show last known status plus a banner naming the
  bridge state and the detail string.

### Overview

- Transport state, your LXMF address (copy), bridge uptime.
- Counters: Rx/Tx packets and rates, announces, active links, path count, unresponsive
  paths.
- Interface chips: name, online dot, bitrate, peers. Clicking one opens the Interfaces tab.

### Paths

- Table: destination (short + copy), name when known, hops, next hop, interface, state,
  age.
- Search box (`MeshPaths.query`), 100 rows per page, infinite scroll. Total count shown.
- Updates on `mesh.updated`, throttled to at most once per second.

### Announced

- Table from `mesh_announces`: name, address, aspect, interface, hops, first seen, last
  seen, count.
- Filter by name/address. Newest first. Live updates from `announce.received`.

### Map

Pure SVG, deterministic layout, no dependency.

- Self at the centre. Ring per hop count (1, 2, 3, …), radius growing linearly.
- Known peers placed on their hop ring at a stable angle derived from the address hash,
  sorted by angle so refreshes do not reorder the picture.
- For `hops > 1`, the peer's first-hop transport is a small diamond on the same spoke at
  the inner ring position; peers sharing a next hop share the diamond.
- Edge label = receiving interface; edge style distinguishes direct vs relayed.
- Peers with no live path are drawn dashed on an outer "unseen" arc, using `last_seen`.
- "Show other services" adds non-LXMF path entries as small unnamed dots (from the ring
  buffer), labelled by short hash and aspect when known.
- Cap: 300 drawn nodes. Beyond that, draw the nearest peers and a "+N more" chip that
  switches to the Paths tab.
- Interaction: click a peer → open/create its direct conversation; click a transport or
  service → detail popover (hash, hops, interface, first/last seen) with copy address.
  Nodes are focusable and reachable by keyboard; the graph has a text alternative list
  below it for screen readers.

### Interfaces

- List rows: name, type label, online dot, bitrate, Rx/Tx rate, peers, radio summary for
  RNode, and origin badge.
- Add: type picker, then the bespoke form.
- Edit: same form, prefilled; changes go through apply-then-persist.
- Toggle enable, delete with confirmation.
- Serial device picker via `ListDevices` for `RNodeInterface`/`KISSInterface`.
- `AutoInterface` form: group id, discovery scope note, `interface_enabled`.
- Radio fields use units (MHz, kHz, dBm) and show measured RSSI/SNR/airtime read-only
  when the hardware reports them.

## Failure modes

| Situation | Behaviour |
| --- | --- |
| Bridge not running | Page renders last known status; banner says the bridge is offline; interface edits disabled with the reason |
| RNS refuses an interface | Config and live state rolled back; error shown verbatim; database unchanged |
| Config write fails (disk, permissions) | Same rollback, error names the path |
| Import finds unparseable content | Preserved verbatim and listed read-only; nothing dropped |
| Path table empty | Graph shows only self and a hint that no announces have arrived |
| Huge path table | MeshStatus stays small (`peer_paths` only); Paths tab pages; graph caps at 300 |
| Shared-instance mode | Interface management may be refused by RNS (`I2P`, local interfaces); the row is marked read-only with the reason |
| Engine restart | Same rows regenerate the same config; no re-import (marker set) |

## Privacy and security

- Everything here is local. No mesh data leaves the device, and no remote management is
  enabled or exposed (RNS remote management stays off).
- The page is behind the same loopback + token boundary as the rest of the API.
- Announce history stores addresses and names already present in `peers`; it adds
  timing, not new exposure. It is included in the existing data directory and inherits
  its 0700 permissions.
- Location is never collected in this phase. The reserved field is unused and null.

## Validation

- Rust: `cargo test --workspace` for the store migration, announce coalescing/pruning,
  `MeshStatus` serialisation, and path query filtering.
- Python: `sidecar/tests/` with stdlib `unittest` for config render (round-trip,
  passthrough preservation, ordering, atomic write) and the schema descriptor.
- Scripted bridge test: attach a `TCPClientInterface` to a local listener, assert it comes
  online, detach it, assert it disappears — no engine restart.
- `scripts/smoke-rns.mjs`: assert `mesh_status` reports both peers, the TCP interface and
  a path with `hops >= 1`; assert `mesh_paths` filtering.
- New `scripts/dev-mesh.sh`: A and C are rchat daemons, B is a plain `rnsd` with
  `enable_transport = Yes` between them. Verifies a 2-hop peer and a shared transport
  diamond on the graph. Documented in the README.
- Manual visual check of the graph at 1 and 2 hops, with the bridge stopped, and after an
  interface apply failure.

## Delivery

- **Phase 1** (tickets 01–08): bridge status, IPC, announce history, page shell, Paths,
  Announced, Map, dev mesh. Read-only, ships on desktop and Android via the shared UI.
- **Phase 2** (tickets 09–12): interface store, config rendering and import, describe and
  hot apply, bespoke forms, Settings migration.
- **Phase 3** (ticket 13): Android interface management parity for TCP and AutoInterface.
- **Deferred** (ticket 14): Android RNode over BLE/USB, pending hardware.
- **Separate effort**: LXMF propagation support, which must land before the map can show
  propagation nodes. The graph reserves the role now.

## Open items

- Basemap choice for the geographic layer (bundled world outline vs. optional tiles) is
  deferred with that layer.
- Whether to expose `enable_transport` later, so rchat itself can relay. Explicitly out
  of scope now (D2/D20).
