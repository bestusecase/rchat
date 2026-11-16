# 04 — Network page shell and navigation

Status: resolved
Type: task
Blocked by: 02

## Goal

Put the Reticulum surface in the interface as a first-class destination, with empty tabs
that later tickets fill.

## Work

- Add a mesh glyph to `Icon.svelte`.
- Add a rail button in `Sidebar.svelte` (between Home and Add peer) that selects the
  network pane. Keep Home behaviour unchanged.
- Create `apps/desktop/src/lib/Network.svelte`: header with transport state, tab bar
  (Overview, Interfaces, Paths, Announced, Map), and the five tab bodies as child
  components (`NetworkOverview`, `NetworkInterfaces`, `NetworkPaths`,
  `NetworkAnnounced`, `NetworkMap`). Only Overview is populated in this ticket; the rest
  render an empty state.
- Wire the pane:
  - desktop/web: replace the chat area while the pane is active, keep `Members`;
  - mobile: reuse the existing `data-pane` mechanism and the back-guard so Android back
    returns to the conversation list.
- `store.svelte.js`: add `mesh`, `meshPaths`, `meshAnnounces`, `networkPane` state;
  apply `Snapshot.mesh`, `mesh.updated`, `announce.received`; clear on disconnect; expose
  `openNetwork()` / `closeNetwork()` and a `requestMeshStatus()` helper.
- Overview content: transport state, address with copy, uptime, counters, interface chips
  linking to the Interfaces tab, and an offline banner using the transport detail string.
- Do not touch Settings yet.

## Acceptance

- The page opens from the rail on desktop, web and Android; back on Android returns to
  the list, not out of the app.
- Overview renders from `Snapshot.mesh` without waiting for a `mesh.updated` event, and
  shows the offline banner when the bridge is not ready.
- With the bridge stopped, the page still opens and does not throw.
- No change to existing conversation behaviour or layout at desktop width.

## Comments

<!-- outcome -->
- `apps/desktop/src/lib/Network.svelte` plus the mesh rail button in `Sidebar.svelte`
  (`onNetwork`). Mobile reuses the conversation pane: `data-pane` in `App.svelte`, and the
  back button calls `chat.closeNetwork()`, which returns to the list.
- Overview renders from the snapshot (`chat.meshTransport` falls back to
  `chat.transport`), and `Network.svelte` refetches `mesh_status` when a bridge comes up
  after the snapshot.
- Not re-verified this session: opening the page with the bridge stopped.
