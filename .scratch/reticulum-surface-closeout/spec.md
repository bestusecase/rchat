# Close the Reticulum surface's remaining edges

Status: ready-for-agent
Effort: `reticulum-surface-closeout`
Prerequisite: `reticulum-surface` (phases 1–2 and ticket 13 are resolved),
`mesh-upgrade-honesty` (the upgrade notice and gate)

## Problem Statement

The Reticulum surface works end to end: the bridge reports the live mesh, the engine
serves paths and announce history, the page has all five tabs, interfaces are managed
hot, and the same page runs on the phone. What is left are the edges of that promise —
the places where the app says one thing and the file or the verification says another.

A user upgrades with a hand-tuned `rns/config`. rchat correctly gives that file
precedence and shows the upgrade notice saying the config is in use. Then it rewrites
the file without the user's header comment and without any key it does not itself
generate inside `[reticulum]` and `[logging]` — a blackhole source, a logging
destination, a tuning key from a newer RNS release. The interface blocks survive, so the
notice looks true, but the node's behaviour changes silently. The spec promised that
"anything rchat does not understand is preserved byte-for-byte"; only unknown top-level
sections and unknown interface blocks currently meet that bar.

The rest of the problem is confidence rather than correctness. The contracts that make
the surface safe were verified by hand once and then not again: an unchanged mesh does
not emit events, an announce or an interface change emits exactly one, the path table is
capped without dropping the nearest peers, attach/detach applies without a restart and a
refused interface rolls back. None of those have a repeatable check, so the next
refactor can break them quietly. The Android pass has the same shape: it ran on a
transient tree, reached the desktop through a firewall workaround, did not exercise the
page with the engine stopped, and has not been repeated since the branch was rebased.

## Solution

Make the preservation promise literal where it is practical to keep it: rchat keeps the
file header, and every line inside `[reticulum]` and `[logging]` whose key rchat does not
generate, in place and in order. The keys rchat owns — transport mode, sharing, the
generated ports, the panic flag, the log level — stay rchat's, are never duplicated, and
the user is not shown internal preserved fragments as if they were unmanaged sections.

Back the rest of the surface with checks at the seams that already exist: the bridge's
parse/render pair, the engine's attach-socket import/configure exchange, and the event
loop's fingerprint. Add a scripted hot-apply check so attach/detach/rollback is not a
manual memory. Make the phone pass repeatable over `adb`: build from the current HEAD,
install, drive the real engine API through a forwarded port, and exercise import, toggle,
a TCP message and the page with the engine stopped, using `adb reverse` for the link so
the host firewall does not decide the result.

## User Stories

1. As a user upgrading with a hand-tuned `rns/config`, I want my file header and comments
   kept, so that the notes I wrote for myself survive the first launch.
2. As a node operator, I want unknown `[reticulum]` keys kept, so that blackhole lists or
   tuning from a newer RNS release keep applying after the upgrade.
3. As a node operator, I want unknown `[logging]` keys kept, so that a custom log
   destination, format or timestamp setting does not silently revert.
4. As a node operator, I want comments that sit beside preserved keys kept, so that the
   rewritten file is still understandable.
5. As a user, I want the preserved content to be written back in its original place, so
   that I do not have to hunt for it at the bottom of the file.
6. As a user, I want the config-wins notice to be accurate, so that "your config is in
   use" means nothing was discarded without my knowledge.
7. As a user, I want the keys rchat owns to remain rchat's, so that an upgrade never
   turns my leaf node into a transport node or opens a port I did not ask for.
8. As a user, I want a preserved line that collides with a generated key not to be
   duplicated, so that RNS reads one unambiguous value.
9. As a user, I want the rewrite to be stable across restarts, so that my config does not
   churn or accumulate duplicate fragments.
10. As a user, I want genuinely unmanaged sections to keep appearing under "Managed
    outside rchat", so that I know what rchat does not edit.
11. As a user, I want internal preserved fragments to stay out of that list, so that the
    page does not present a storage detail as a config section I should recognise.
12. As a user with no hand-written config, I want the first-run seeding and the upgrade
    notice to behave exactly as they do now, so that the common path does not change.
13. As a developer, I want preserved fragments to survive the store, so that a later
    apply can re-render the same file without holding the original in memory.
14. As a developer, I want import to stay one-shot, so that a user's later hand edits are
    governed by the owned-rows model rather than re-absorbed unpredictably.
15. As a maintainer, I want a test that fails if the file preamble is dropped, so that
    the most visible fidelity regression is caught at the bridge seam.
16. As a maintainer, I want a test that unknown keys and comments in the generated
    sections survive parse and render, so that the promise is checked line by line.
17. As a maintainer, I want a test that the engine persists those fragments and sends
    them back on the next configure, so that fidelity is not only a pure-function
    property.
18. As a maintainer, I want a test that the client-facing interface list hides internal
    fragments, so that the UI contract cannot regress.
19. As a maintainer, I want a test that an unchanged mesh emits no event, so that idle
    sampling stays cheap and the battery claim remains true.
20. As a maintainer, I want a test that an announce or an interface change emits exactly
    one event, so that the "live, not polled" contract is enforced.
21. As a maintainer, I want a test that stopping the bridge leaves no sampling thread, so
    that repeated restarts cannot accumulate work.
22. As a maintainer, I want a test that the path cap keeps the nearest hops and the most
    recently used entries within a hop count, and reports truncation, so that the Paths
    tab stays honest at scale.
23. As a maintainer, I want a scripted hot-apply check that attaches an interface, sees
    it online, deletes it, sees it gone, and sees a refused apply roll back, so that the
    phase-2 contract is repeatable without the UI.
24. As a maintainer, I want a repeatable Android check that imports a hand-written
    config, toggles an interface and exchanges a message, so that the ticket-13
    acceptance is not a one-off story.
25. As a phone user, I want the page to render honestly while the engine is starting or
    stopped, so that an offline phone shows last-known facts and a banner instead of a
    blank page or a crash.
26. As a maintainer, I want the Android check to need no host firewall change, so that it
    runs on a locked-down machine without weakening it.
27. As a maintainer, I want the final APK rebuilt from the commit that lands this work,
    and the import check repeated, so that the recorded hardware pass matches HEAD.
28. As a maintainer, I want the map, paths and announce fixes from the previous effort to
    stay covered, so that this closeout does not regress them.

## Implementation Decisions

### Config preservation

- **Preserve, do not reconstruct.** Import produces interface rows plus an ordered list
  of preserved fragments. Each fragment carries a placement: the file preamble, the
  lines inside `[reticulum]`, the lines inside `[logging]`, an unknown interface block,
  or an unknown top-level section. The existing extra-section storage is used; no schema
  migration is needed.
- **Reserved placement keys.** Fragments that belong inside a generated section use
  keys beginning with `@` (`@header`, `@reticulum`, `@logging`). Real RNS section names
  and the existing `interfaces/<name>` keys keep their meaning. The `@` prefix is the
  marker for "internal placement, never a section".
- **Generated keys are authoritative.** `[reticulum]` emits `enable_transport`,
  `share_instance`, the shared-instance ports in shared mode, and
  `panic_on_interface_error`; `[logging]` emits `loglevel`. A preserved line whose key
  is one of those is not unknown; it is dropped rather than duplicated. Comments and
  blank lines inside those sections are preserved as free lines.
- **Render order.** Preamble first, then `[reticulum]` with its preserved lines directly
  after the generated keys, then `[logging]` the same way, then `[interfaces]` with
  managed rows followed by preserved unknown blocks, then the other unmanaged sections
  in their original order. Duplicate generated sections in an imported file merge their
  unknown lines in file order; generated keys appear once.
- **Fidelity scope.** Preserved fragments are re-emitted line for line, in order. The
  layout rchat owns — the blank line between sections, the key order inside managed
  interface blocks, the generated keys themselves — may differ. Imported interface
  values survive as rows; their original key order is not promised.
- **One-shot import stays one-shot.** The import marker, the config-wins notice and the
  owned-rows model are unchanged. Content added to `rns/config` after the import is not
  re-absorbed.
- **Client contract.** `configure` carries every fragment, including internal `@` ones.
  `ListInterfaces` omits the internal `@` fragments from `extra`, so the page lists only
  real unmanaged sections. The import reply keeps its existing shape: parsed interfaces,
  `[section, raw]` pairs, and `existed`.
- **No behavior change to the generated values.** Transport mode, sharing, ports and log
  level remain rchat's decisions, as before.

### Verification and harnesses

- **Bridge event loop.** Test the fingerprint path directly: the same payload emits
  nothing the second time; an announce wake or an interface change emits once. Capture
  events at the bridge's existing emit boundary rather than inventing a sink.
- **Path cap.** Test `MESH_PATH_CAP` with a synthetic table larger than the cap: the
  payload keeps the nearest hops first and the most recently used entries within a hop
  count, sets `paths_truncated`, and reports the untruncated total.
- **Sampling lifecycle.** Ensure the sampling thread exits when readiness drops and that
  a restart does not leave an old thread behind. If the check cannot be made
  deterministic at the bridge seam, assert the loop's exit condition rather than a wall
  clock.
- **Hot apply script.** A new script drives a running daemon through the public API: add
  a TCP server, wait for online, delete it, wait for absence from status and config, and
  attempt an apply that must fail (a port already in use) and leave the previous state
  intact. It is the scripted form of the manual phase-2 check.
- **Android script.** A shell entry point builds the debug arm64 APK from the current
  tree, installs over the existing app, writes a hand-written config into the bridge
  directory, launches the app, forwards the engine port, and runs a node assertion
  script over the public API. It uses `adb reverse` for the TCP link so the host firewall
  is irrelevant. A destructive fresh-import reset happens only behind an explicit flag.
  The assertion script covers: two imported rows with their original values, no
  duplicate seed row, AutoInterface online, toggle off/on with the derived transport
  word, a message to a desktop daemon with a delivered receipt, and an empty serial
  device list with a reason.
- **Manual hardware step.** With the engine stopped, open the Network page and confirm
  last-known data plus the offline banner, and confirm a background/foreground cycle
  updates the graph and paths without a reload. Record the result in the ticket.
- **Final rebuild.** Confirm HEAD before building; rebuild and reinstall after the last
  rebase; record the commit hash in the ticket so the pass is attributable.

## Testing Decisions

A good test here asserts the observable contract at an existing seam: the bytes the
bridge parses and renders, the payloads the engine sends and stores, and the events a
client receives. It does not assert private helper shapes or a particular thread schedule
where that would be flaky.

### Bridge seam (python unittest, existing)

- Parse/render round-trip with: a preamble comment, unknown keys and comments in both
  generated sections, an unknown interface block, an unknown top-level section, and a
  preserved key that collides with a generated one.
- A second render of the same parsed result is byte-identical (idempotence).
- Import of a missing file still returns the empty shape.
- Mesh emit: fix a snapshot payload, call the emit path twice, assert zero then one
  event, and assert an explicit wake produces one. Use the existing fingerprint tests as
  prior art.
- Path cap: a synthetic table above the cap produces a capped, ordered payload with
  `paths_truncated` and the true total.
- Sampling lifecycle: assert the loop leaves when readiness drops; keep it deterministic
  by stubbing the wait, not by sleeping.

### Engine seam (rust, attach-socket tests, existing)

- A hand-written config reply carrying `@header`, `@reticulum` and `@logging` fragments
  is persisted in order; the next `configure` carries the same pairs; the subsequent
  render is stable.
- `ListInterfaces` returns real unmanaged sections but not internal `@` fragments.
- The existing announce count, mesh republish and interface rollback tests stay green as
  regression cover.

### Scripted seams

- The hot-apply script runs against the two-daemon dev setup, in the style of the
  existing mesh smoke checks.
- The Android script runs against the installed app over `adb`; its assertions are the
  same public API calls the shared UI makes. Prior art: the rendezvous and mesh scripts,
  including their "skip with a reason" behavior when a capability is missing.

## Out of Scope

- Ticket 14: Android RNode over BLE/USB, and any hardware-dependent radio management.
- The geographic layer and its basemap decision.
- LXMF propagation, stamps and sync state.
- Editing preserved fragments in the UI; they remain read-only content.
- Changing the generated keys or making rchat a transport node.
- Re-reading `rns/config` after the one-shot import.
- Reworking the managed interface block layout or promising key order.
- The peer-profile-card, mobile-attachment and notification efforts; they are separate.
- Pushing to the remote or cutting a release.

## Further Notes

- Current baseline before this effort: 134 rust tests, 39 bridge unit tests, a green
  desktop build, `dev-mesh` + `smoke-mesh` (2-hop relay) and `dev-two-peers` +
  `smoke-rns` (direct link, now with mesh assertions), and a Pixel 8 pass covering import,
  toggle, TCP messages and the page during startup.
- The Pixel pass reached the desktop over `adb reverse` because the host's `ufw` blocks
  inbound LAN TCP. That is an environment fact, not an rchat result; the scripted Android
  check should keep the same approach.
- The branch was rebased repeatedly by a concurrent session while the surface work
  landed. Confirm HEAD before rebuilding the APK and record it in the ticket.
- The upgrade notice and the legacy gate from `mesh-upgrade-honesty` remain the
  authority for what an upgrade says; this effort only makes the preserved set match
  what that notice claims.
- Phase 3's shared UI and ticket 13 are already resolved. This effort closes the code and
  verification edges; it does not reopen the page's design.
