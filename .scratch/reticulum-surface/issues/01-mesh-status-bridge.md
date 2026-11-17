# 01 — Mesh status from the bridge

Status: resolved
Type: task
Blocked by: none

## Goal

Make the Reticulum bridge report live transport state in a form the engine can cache and
the UI can render: interfaces, counters and the path table.

## Work

- Add a sampling thread in `sidecar/bridge.py`, started once the router is ready and
  stopped on shutdown. Every 2 s while ready:
  - copy `RNS.Transport.path_table` under `path_table_lock` into plain tuples;
  - read `RNS.Reticulum.get_interface_stats()` and whitelist the fields in the spec
    (`name`, `short_name`, `type`, `hash`, `online`, `bitrate`, `mtu`, `rxb`, `txb`,
    `rxs`, `txs`, `peers`, `mode`, `ifac_netname`, and the `r_*` radio stats grouped
    under `radio`); never pass raw stats through;
  - read counters (`rx_packets`, `tx_packets`, `rx_pps`, `tx_pps`, `announce_rx`,
    `announce_tx`, `link_count`, `path_count`, unresponsive count via
    `path_is_unresponsive` over the copied table).
- Emit `{"ev":"mesh","mesh":{...}}` only when the fingerprint changes. Fingerprint
  excludes byte counters; it covers interface identity/online/bitrate/peers, path count
  and the `(destination, hops, next_hop, interface)` set.
- Emit immediately from `AnnounceHandler.received_announce` and after an interface
  change, not only on the next tick.
- Extend the announce event with `interface`, `hops`, `aspect` (aspect best-effort from
  `RNS.Transport.destinations`).
- Add an unfiltered announce handler that keeps an in-memory ring of the 200 most recent
  non-`lxmf.delivery` destinations, exposed inside the mesh event as `other_services`.
- Cap the path list at 2000 entries sorted by hops ascending, set `paths_truncated`, and
  never truncate a destination that is in `[interfaces]` peers.
- All of this runs with the GIL-friendly pure-Python reads the bridge already uses; no
  new dependency.

## Acceptance

- With two TCP-connected daemons, each bridge emits a mesh event that names the TCP
  interface as online and lists the peer path at 1 hop.
- No event is emitted when nothing changed for 10 s, and exactly one event is emitted on
  an announce or an interface up/down.
- `rns/config` behaviour is untouched by this ticket.
- Bridge start/stop leaves no sampling thread running.

## Comments

<!-- outcome -->
- Bridge: `mesh_snapshot()` builds the payload, `mesh_fingerprint()` reduces it to the
  parts that matter, and `mesh_loop()`/`emit_mesh()` publish `mesh.updated` only when the
  fingerprint moves or `_mesh_wake` fires. `now_ms()`, `INTERFACE_MODES` and
  `_interface_payload()` translate RNS constants; byte counters are excluded from the
  fingerprint so idle traffic never emits.
- Verified by `sidecar/tests/test_bridge.py` (`test_interface_whitelist_drops_extra_stats`,
  `test_fingerprint_ignores_byte_counters_but_sees_structure`) and a live run of
  `scripts/dev-mesh.sh` on 2026-11-15.
- `rns/config` rendering is untouched: `render_config()` is only called from apply/import.
- Post-verification fix: RNS `Transport.destinations_map` only holds *locally
  registered* destinations, so the original `_aspect_for()` returned `None` for every
  remote announce and the non-LXMF service ring stayed empty. Aspects are now recovered
  from the announced identity (`RNS.Destination.hash(identity, "lxmf", "delivery")`)
  and cached in a bounded map; `ServiceAnnounceHandler` keeps non-LXMF destinations even
  when the aspect is unknown. `sidecar/tests/test_bridge.py` covers the identity match,
  the learned-map lookup, and the service ring.
- The 2000-path cap now sorts `(hops, age)` so it keeps the *most* recently used entries
  within a hop count, matching the engine's order.
