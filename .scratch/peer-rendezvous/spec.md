# Make peers reachable across network changes

Status: ready-for-agent
Effort: `peer-rendezvous`
Prerequisite: `reticulum-surface`. That effort moves interface configuration into
`rchat.db`, renders `rns/config` as an artifact, and publishes the mesh status feed.
This spec consumes those; it does not re-implement them.

## Problem Statement

I moved my laptop and phone to a different Wi-Fi network. Both apps said the Reticulum
connection was working. Nothing I sent arrived, and nothing I should have received
appeared. Neither app said anything was wrong, and the only thing that fixed it was
looking up my phone's new address and typing it into a settings field — the same thing I
had to do after the previous network change.

The reason is structural. The only path between the two devices was a single dialled TCP
link whose target was the phone's local address, stored as configuration; the phone had no
local discovery running, because having a listening interface had silently turned discovery
off. When DHCP handed the phone a new address, the link pointed at an address nobody owned
any more, and there was no second path to fall back on. "Transport: ready" means only that
the Reticulum bridge started and registered an address — not that any interface is online,
not that any peer has a path. The one state that mattered was invisible.

This will happen to every user on every lease change, on every network switch, forever,
because a person's address is stored as product configuration.

## Solution

Devices find each other without anyone typing an address.

- **Same network.** Every platform runs local network discovery by default, from first
  run, and adding any other interface never turns it off. Two devices on one network
  connect with no configuration at all.
- **Different networks.** Both devices dial one shared Reticulum transport node — a relay
  the user names once, preferably by hostname. The stored address belongs to the relay,
  not to a person, so a new lease or a new network changes nothing that is saved.
- **Upgrades.** An existing user's discovery toggle and relay address are carried into
  the interface list once, instead of being dropped on upgrade.
- **Truth.** The transport line stops claiming success when nothing is online. Every
  interface reports whether it is online and for how long it has been down. Every peer
  reports whether a path exists, over which interface, and when it was last heard, so
  "no reply yet" is distinguishable from "this cannot be delivered".

## User Stories

1. As someone who just joined a new Wi-Fi network, I want my laptop and phone to find each
   other automatically, so that I never have to look up and type an address.
2. As someone whose phone received a new DHCP lease, I want existing conversations to keep
   working without editing settings, so that a network change is not an outage.
3. As a user on a network that filters multicast, I want a documented way to connect two
   devices through a relay I name once, so that failed discovery does not strand me.
4. As a user with a relay configured, I want both devices to dial it and neither to store
   the other's address, so that the only address that must be correct is the relay's.
5. As a user upgrading from an older version, I want my saved discovery and relay settings
   carried over, so that upgrading does not silently disconnect me.
6. As a user, I want the status line to mean "the mesh is working", not "a process started",
   so that I can trust it.
7. As a user, I want each interface to show whether it is online and how long it has been
   offline, so that a dead relay is obvious rather than silent.
8. As a user, I want a refused or unreachable relay reported with Reticulum's own reason,
   so that I can fix the address instead of guessing.
9. As a user, I want a conversation to show whether the peer currently has a path and when
   it was last heard, so that I know whether a message can arrive at all.
10. As a user who sent a message while the peer was unreachable, I want it queued and
    delivered when a path returns, so that I do not have to retype it.
11. As a user with a laptop and a phone, I want the same network controls on both, so that
    the phone is not the device I cannot fix.
12. As a user adding a relay, I want the address validated before it is applied, so that a
    typo cannot sit there looking healthy.
13. As a user fixing connectivity, I want interface changes applied without restarting the
    engine, so that I do not drop my session to change one field.
14. As a user, I want discovery to remain on when I add a relay, so that the devices use the
    local link when they are together and the relay when they are apart.
15. As a user, I want to give the relay as a hostname, so that the relay moving addresses
    does not require an edit on every device.
16. As a user, I want to see which interface and how many hops a peer's path uses, so that I
    understand whether I am talking over Wi-Fi or a relay.
17. As a user, I want message content to stay end-to-end encrypted through a relay, so that
    using a third-party node does not expose what I write.
18. As a user with no relay and no local network in common, I want the app to say so plainly,
    so that I stop waiting for something that cannot arrive.
19. As a user, I want stale interfaces to look dead rather than healthy, so that I can delete
    them with confidence.
20. As a user on Android, I want discovery and relay settings to survive backgrounding and
    network changes, so that connectivity returns without babysitting the app.
21. As a first-time user, I want chatting to work with zero network setup when both devices
    share a network, so that configuration is not a prerequisite.
22. As a user who previously hand-edited a Reticulum config, I want those interfaces
    imported and preserved, so that my setup is not lost or duplicated.
23. As a user, I want to see when a peer last announced, so that "offline for two days" is
    visible rather than mysterious.
24. As a user, I want one place that answers "can this person receive a message right now",
    so that I do not have to reason about Reticulum internals.
25. As a user reporting a problem, I want the app to distinguish "no interfaces", "interfaces
    up but no path", and "path established", so that my report is useful.
26. As a developer, I want the multi-node smoke test to prove delivery through a relay with
    no peer addresses configured, so that this regression cannot return unnoticed.
27. As a developer, I want transport state derived from interface state and tested that way,
    so that "ready" can never again mean only "the bridge started".
28. As a developer, I want the legacy migration tested for idempotence and non-destruction,
    so that an upgrade cannot clobber a user's interface list.
29. As a user with both a local link and a relay, I want both available at once, so that
    losing one does not drop messaging.
30. As a user, I want to remove a relay and fall back to local discovery without restarting,
    so that undoing a change is as cheap as making it.

## Implementation Decisions

- **Interfaces are the only source of transport truth.** Database rows are authoritative
  and the generated Reticulum config is an artifact. Production never seeds interfaces from
  environment variables; environment seeding survives only for development scripts behind an
  explicit opt-in.
- **First run creates one enabled local-discovery interface** (an `AutoInterface` row,
  origin `default`) on every platform. Zero network configuration is the default state of a
  fresh install, on desktop and Android alike.
- **Interface kinds are independent.** The old rule where configuring a TCP interface
  silently defaulted discovery off is deleted. Discovery becomes a row the user can toggle,
  never a derived default.
- **Legacy migration is one-shot and additive.** On first start the shell reads its old
  network settings (a discovery flag and one relay string) and asks the engine to create
  interface rows from them with origin `legacy`, then stops supplying interface values by
  environment. The import is guarded by a meta flag, is skipped when the user already has
  rows, never overwrites a same-named row, and never re-runs. A relay string is split into
  host and port; a malformed value is imported *disabled* with the reason recorded, never
  dropped silently.
- **Rendezvous uses stock Reticulum.** A TCP or backbone client interface pointing at a
  transport-enabled node. No new protocol, no new daemon, no `enable_transport` in generated
  configs, and hosts are stored as the string the user gave so DNS names stay preferred.
- **Transport state gains a third meaning.** Besides `offline`, `starting` and `ready`, the
  engine reports a distinct state when the bridge is up but no interface is online
  (recommended wire word: `degraded`), and `ready` now requires at least one online
  interface. The addition is additive on the wire; older clients that do not know the word
  show it verbatim rather than breaking.
- **Interface health is per interface.** Online state plus, for dialled interfaces, when the
  current outage began. This rides on the mesh status the engine already caches and
  publishes; no new polling loop.
- **Peer reachability is derived, not guessed.** From the Reticulum path table and announce
  history: path present or absent, hop count, next hop, receiving interface, last announce.
  It is exposed per peer and consumed by the conversation header. Receipts are never used to
  infer reachability, and a peer with a live path but no recent announce is still reachable.
- **The offline queue keeps its behaviour**; what changes is that the UI states why a message
  is queued and clears the notice when a path returns.
- **Validation lives at the engine boundary:** host/port shape and port range checked before
  an interface is applied, plus RNS's own refusal reason relayed verbatim. A rejected edit is
  not persisted and the form keeps the user's input.
- **No peer address is ever generated.** Nothing in the product writes one device's current
  address into another device's configuration, including during import: an imported legacy
  relay is labelled as such so it can be retired deliberately.

## Testing Decisions

A good test here asserts an outcome a user could observe: a message arrives, or the status
the API reports about itself; never that a particular internal call was made.

- **Highest seam — the three-node dev mesh and mesh smoke scripts.** A real `rnsd` with
  transport enabled and two real daemons dialling it, extended to prove: (a) two leaves
  exchange a message with neither holding the other's address; (b) a leaf is stopped and
  restarted at a different local address, and delivery resumes with the other leaf's
  configuration untouched; (c) removing the relay and re-enabling discovery leaves both
  nodes communicating. The existing two-daemon Reticulum smoke test keeps covering the
  direct point-to-point topology.
- **Engine seam.** Transport-state derivation is tested in the engine's in-process app tests
  by feeding fabricated bridge mesh events — no interfaces, all interfaces offline, one
  online — asserting only what clients are told. Prior art: the mesh cache test that asserts
  only known peers appear in the status.
- **Store seam.** The legacy import is tested through the public store API: rows created with
  the legacy origin, meta flag set, idempotent on a second run, an existing same-named row
  left alone, a malformed relay imported disabled. Prior art: the interface CRUD and
  extra-config round-trip tests.
- **Bridge seam.** Pure helpers for default seeding and config rendering: the default seed
  contains local discovery, and a rendered config keeps discovery when a TCP interface is
  present. Prior art: the TCP client defaults test and the render round-trip tests.
- **Manual device pass, documented like the notification hand pass.** Two real devices on one
  access point with no relay, confirming discovery and two-way delivery; then on separate
  networks with a relay, confirming delivery and the status wording. CI cannot host a second
  physical device.
- **Deliberately not automated:** multicast reachability between machines, public relay DNS
  resolution, and Android doze timing. Each is flaky in a way that would train people to
  ignore the suite.

## Out of Scope

- mDNS/zeroconf peer discovery, or any scheme where a device advertises itself so a peer can
  rediscover a changed address. Deferred: it needs a trusted discovery channel on the same
  link and still cannot cross networks.
- Automatic relay discovery, Reticulum's public-network `discover_interfaces` autoconnect,
  and interface churn from public announce traffic.
- Hosting a relay inside rchat.
- LXMF propagation nodes, stamps and sync state (separate effort).
- Group conversations and multi-device fan-out for one identity.
- Bandwidth classification and topology drawing (belong to `reticulum-surface`).
- Windows, macOS and iOS packaging.

## Further Notes

- **Why not simply fix the address** (the immediate workaround, already applied to the
  affected install): it restores the user for one lease. It leaves the failure mode intact
  and teaches nothing, which is why the immediate fix is not part of this spec.
- **Privacy.** LXMF is end-to-end encrypted, so a relay sees routing metadata and volume, not
  content. Public testnet nodes are shared and can disappear; a DNS-named user-run `rnsd` is
  the durable choice, so no public node should be baked into the product as a default.
- **Evidence from the incident.** The desktop ran packaged 0.1.3, which exports the legacy
  discovery flag and relay address to the engine at launch. The phone moved from
  `192.168.1.216` to `192.168.1.168`; its relay port was open at the new address while the
  desktop kept dialling the old one. The phone ran no local discovery, and the desktop's
  discovery had nothing to hear. Link-local multicast from other clients on that access point
  worked, so the network was never the problem.
- **Schema reach.** Live installs still carry the pre-interface schema (no interface table,
  no import flag). The migration must be correct from exactly that state: interface table
  creation is idempotent and the import keys off the absent flag.
- **Android parity is cheap here** only because the phone ships the same Svelte interface as
  the desktop; the work on the phone side is the shell's settings bridge, not a second UI.

## Implementation notes

- `scripts/smoke-rendezvous.mjs` proves (a) relay delivery with no peer address stored and
  (b) a leaf restart with the other side's rows unchanged. Phase (c) is opt-in
  (`--phases=a,b,c`) because RNS AutoInterface binds one fixed data port per host, so two
  full Reticulum instances on one machine cannot both run discovery; run it on two devices
  or in separate network namespaces.
- The dev scripts configure interfaces through the engine API (`scripts/mesh-config.mjs`),
  not `RCHAT_RNS_*`. They seed a minimal `rns/config` before first start for the same
  one-host port reason; the engine imports it and owns it from then on.
- Transport derivation, per-interface outage age and per-peer reachability are computed in
  `rchatd` from the cached mesh sample, so the UI never infers reachability from receipts.
