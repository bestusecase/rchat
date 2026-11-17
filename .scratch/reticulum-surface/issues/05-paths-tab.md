# 05 — Paths tab

Status: resolved
Type: task
Blocked by: 02, 04

## Goal

Expose the full Reticulum path table: every destination the engine knows how to reach,
how far away it is, and through which interface and next hop.

## Work

- `NetworkPaths.svelte`: table with destination (short hash + copy), name when the
  address is a known peer, hops, next hop, interface, state, age.
- Search input (debounced 250 ms) mapping to `MeshPaths.query`; 100 rows per page with
  infinite scroll; total from the reply, shown as "N destinations".
- Fetch on first open and after `mesh.updated`, throttled to at most once per second.
- Sort by hops ascending then most recently used, as the engine returns it; a header
  click may reverse hops locally without a refetch.
- Row click opens the peer conversation when the destination is a known peer; otherwise
  opens a detail popover with the full hash and copy action.
- Empty state distinguishes "no announces yet" from "bridge offline".

## Acceptance

- With the dev mesh (ticket 08), A shows C at 2 hops with the relay's interface and next
  hop, and B at 1 hop.
- Filtering by hex prefix, by displayed name and by interface name returns the expected
  rows.
- 150 rows page correctly and the scroll position survives a `mesh.updated` refresh.
- A destination that is not a peer shows a popover, never a broken conversation.

## Comments

<!-- outcome -->
- `apps/desktop/src/lib/NetworkPaths.svelte` with `loadMeshPaths`/`refreshMeshPaths`/
  `setMeshPathQuery` in the store and the `mesh_paths` handler in the engine.
- Filtering and paging are covered by `mesh_paths_filter_and_page` (hex prefix, displayed
  name, interface name; pages by hops then recency).
- Known duplication: this tab and `NetworkAnnounced` both implement infinite scroll with
  slightly different guards; left alone rather than forced into one helper.
- Post-verification fix: every destination cell now carries a copy button (also for known
  peers, not only the non-peer popover), with `stopPropagation` so copying does not open
  the row.
