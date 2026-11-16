# 03 — Durable, bounded announce history

Status: resolved
Type: task
Blocked by: none

## Goal

Answer "who was on this mesh, when, and over which interface" without unbounded growth.

## Work

- Add the `mesh_announces` table and index from the spec to `Store::migrate`, bump the
  schema marker to `2`.
- `record_announce(address, name, aspect, interface, hops, ts)`:
  - bucket = `ts / 600_000`;
  - `INSERT ... ON CONFLICT(address, bucket) DO UPDATE SET last_seen, count = count + 1,
    name = coalesce(excluded.name, name), interface = ..., hops = ...`;
  - prune at most once a minute: delete rows with `last_seen < now - 7 days`, then delete
    all but the newest 5000 by `last_seen`.
- `list_announces(before, limit)` newest first, clamped to 500.
- Engine: on `BridgeEvent::Announce`, write the row, then publish
  `announce.received { announce }` in addition to the existing `peer.updated`.
- Ignore our own address before writing (same check as today).

## Acceptance

- Store test: 1000 announces for one address inside one bucket produce one row with
  `count = 1000`.
- Store test: rows older than 7 days are removed; a 6000-row table trims to 5000 newest.
- Pruning does not run more than once per minute under a burst of announces.
- Engine test: an announce produces one `announce.received` event and does not create a
  peer for an invalid address.

## Comments

<!-- outcome -->
- `crates/rchat-core/src/store.rs`: `announces` table, bucket coalescing and the prune
  pass; bridge `AnnounceHandler`/`ServiceAnnounceHandler` feed it.
- Verified by `announces_coalesce_inside_a_bucket`, `announces_prune_by_age_and_count`
  (asserts 6000 rows trim to exactly 5000 by counting through the store's private `conn`,
  so the check cannot pass vacuously), `announce_pruning_is_throttled`,
  `announce_history_pages_newest_first`.
