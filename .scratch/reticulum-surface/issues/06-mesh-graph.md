# 06 — Mesh graph

Status: resolved
Type: task
Blocked by: 04

## Goal

Draw the mesh the way Reticulum actually reports it: peers by hop distance, first-hop
transport nodes, and the interface each path uses — with no invented topology.

## Work

- `NetworkMap.svelte`, hand-rolled SVG, no new dependency.
- Layout:
  - self at the centre; one ring per hop count; radius = `14 + hop * 12` units of a fixed
     calc based on the largest hop count seen (max 6 rings, deeper peers collapse onto the
    last ring with their true number as a label);
  - stable angle from `hash(address + ring)` so nodes do not move between refreshes;
  - for `hops > 1`, a diamond is placed at the inner ring on that spoke for the next-hop
    transport; peers sharing a next hop share the diamond;
  - direct peers (`hops == 1`) connect straight to the centre;
  - peers with no live path draw dashed on an outer arc, using `last_seen`;
  - edges are labelled with the receiving interface and styled direct vs relayed;
  - "Show other services" toggle adds non-LXMF path entries from `other_services` as
    small unnamed dots (label: short hash, plus aspect when the bridge knew it).
- Cap at 300 drawn nodes; beyond that draw the lowest-hop peers plus a "+N more" chip that
  switches to the Paths tab.
- Interaction and accessibility:
  - peer click → `openDirect` and close the pane;
  - transport/service click → detail popover (hash, hops, interface, first/last seen) with
    copy;
  - nodes are `<g tabindex="0" role="link">` with keyboard activation;
  - render a visually-hidden text list of the same nodes and edges below the SVG so the
    graph is never the only representation.
- Empty state: self plus "No announces yet. Peers appear here once they announce."
- Pan/zoom only if it stays dependency-free and does not fight the page scroll; otherwise
  fixed layout with a "fit" reset is acceptable.

## Acceptance

- Two-node dev setup: both peers at 1 hop on the TCP spoke.
- Three-node dev mesh: C appears at 2 hops behind a transport diamond shared by nothing
  else; the edge labels name the correct interfaces.
- Nodes keep their positions across a `mesh.updated` that only changes counters.
- Keyboard-only user can focus every node and open a peer conversation.
- With the bridge offline, the last known graph stays visible with a muted "stale" note.

## Comments

<!-- outcome -->
- `apps/desktop/src/lib/NetworkMap.svelte`: peers placed by `hashAngle(address)` and
  `ringFor(hops)`; a transport diamond sits at the circular mean of its peers' spoke
  angles (`meanAngle`) on the hop-1 ring, so every relayed edge stays a spoke. Unknown
  destinations sit outside the outer ring as `unseen`, dead ones as `unresponsive`.
- Every node is a `tabindex="0"` `role="link"` with an Enter/Space handler, so the graph
  is reachable without a pointer.
- Visual checks only (no automated test for the layout).
