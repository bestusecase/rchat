# 07 — Announced tab

Status: resolved
Type: task
Blocked by: 03, 04

## Goal

Show the durable announce history so a user can tell whether a node is around, how often
it announces, and over what.

## Work

- `NetworkAnnounced.svelte`: table with name, address (short + copy), aspect, interface,
  hops, first seen, last seen, count.
- Filter by name and address (client-side over the fetched page is fine; server-side when
  the query is non-empty).
- Newest first, 100 rows per page, infinite scroll mapped to `MeshAnnounces { before,
  limit }`.
- Live prepend from `announce.received` without refetching; coalesce bursts with a short
  timer so a busy mesh does not re-render per announce.
- Format times relative ("2 min ago"), with the absolute time in the title attribute.
- Empty state explains that announces are kept for 7 days and capped at 5000.

## Acceptance

- After the dev mesh announces, both peers appear with the right interface and hop count.
- A repeated announce inside 10 minutes increments the count of the existing row instead
  of adding a duplicate.
- Live updates do not disturb scroll position or the active filter.
- Rows older than 7 days are absent after a restart that triggered pruning.

## Comments

<!-- outcome -->
- `apps/desktop/src/lib/NetworkAnnounced.svelte`; live `announce.received` events go
  through `upsertAnnounce` into `pendingAnnounces` and are flushed 250 ms later by
  `announceTimer`/`flushAnnounces`, so an announce storm is one render and the list's
  scroll position and filter survive.
- `disconnect()` clears the pending map and the timer.
- Post-verification fix: the address cell gained the copy affordance the spec asks for
  (`⧉` / `✓`, with an aria-label naming the full destination).
