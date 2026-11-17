# Make the mesh upgrade and discovery edges honest

Status: ready-for-agent
Effort: `mesh-upgrade-honesty`
Prerequisite: `peer-rendezvous`. That effort made interfaces authoritative,
added `degraded`, per-interface health and per-peer reachability, and left three
edges unfinished. This spec closes those edges; it does not revisit the
precedence decisions it made.

## Problem Statement

An upgrading user can have both an old `desktop.json` relay and a hand-written
`rns/config`. rchat gives the config precedence, which is correct: the config
may contain an RNode or a backbone link that the old app-level setting never
knew about. But the old discovery flag and relay string are then dropped without
a word, and the README says they are imported "once" without mentioning the
exception. A user whose only path used to be that relay sees it vanish and has
no way to tell whether rchat forgot it or deliberately ignored it.

The one-shot legacy import also does not actually settle in that case. The first
start leaves the import flag unset so the config can be read; only the second
start notices the imported rows and closes the gate. Nothing user-visible goes
wrong today, but the gate is not closed in the start that made the decision, so
the outcome is re-derived on the next start, and any future code that adds work
to that path inherits a flag whose meaning is "eventually done, unless a config
existed".

The mobile shell still reports `autoDiscovery` and `tcpRelay` from
`get_desktop_settings`, even though the shared UI reads neither and the engine
owns interfaces now. The fields do nothing, but they are the contract the next
mobile screen would trust, and a control wired to them would look healthy while
changing nothing.

The scripted rendezvous check stops at the relay. Discovery is the zero-config
half of the feature, and the only proof that it works across a network boundary
is a manual two-device pass. RNS AutoInterface binds one fixed data port per
host, so the script cannot run two leaves on one machine; the last manual step
therefore stays manual by accident rather than by choice.

## Solution

An upgrade says what it did. When a hand-written `rns/config` wins and the old
app settings contained something explicit, the Network page shows one
informational notice naming what was not imported and where to add it. The
legacy import gate closes in the same start that reads the config, so the
outcome is recorded once and never re-derived. The notice is dismissible, and a
dismissal is permanent because the fact never changes.

The mobile settings payload contains only what the UI uses. Deleting the dead
network fields makes the shell's contract match the screen, and a test on each
shell keeps the two shapes from drifting apart again.

Discovery becomes scriptable. A Linux-only runner puts one leaf in a separate
network namespace connected by a veth pair, so two full Reticulum instances can
run AutoInterface on one machine. The relay-free scenario then proves that two
leaves with no relay and no stored peer address still exchange a message. Where
the host cannot create a namespace, the runner says it skipped and exits
successfully, because a missing capability is not a test failure.

## User Stories

1. As a user upgrading with a hand-written `rns/config`, I want to be told once
   that my old discovery/relay settings were not imported, so that a missing
   relay is an explained decision rather than a mystery.
2. As that user, I want the notice to name which settings were skipped, so that
   I know whether to add an AutoInterface row, a TCP row, or both.
3. As that user, I want a way to dismiss the notice, so that a settled upgrade
   stops occupying the Network page.
4. As a user who dismisses it, I want it to stay dismissed across restarts, so
   that I do not have to dismiss the same fact twice.
5. As a user with no hand-written config, I want the legacy import to behave
   exactly as it does today, so that the common upgrade path does not change.
6. As a user whose old toggle was off and whose config has interfaces, I want
   the notice to say that discovery was not turned on, so that I can choose to
   add it.
7. As a user with a malformed old relay, I want the existing imported-disabled
   row with its reason to keep working, so that this spec does not weaken the
   existing repair path.
8. As a developer, I want the legacy-import gate to close in the first start
   that reads a config, so that "done" means done and the old settings are not
   reconsidered forever.
9. As a developer, I want the interface origin to keep one meaning per word, so
   that `legacy` always means "from the old app settings" and `imported` always
   means "from a hand-written config".
10. As a mobile user, I want the phone's settings contract to expose only
    settings that do something, so that a future screen cannot show a toggle
    that silently changes nothing.
11. As a developer, I want the mobile and desktop settings payloads to have the
    same shape, so that the shared UI cannot read a field one shell forgot.
12. As a developer, I want a script that proves discovery across a network
    boundary without a relay, so that the zero-config path is guarded by more
    than a manual pass.
13. As a developer without root or network-namespace support, I want that script
    to skip clearly instead of failing, so that the suite stays trustworthy on
    machines that cannot host it.
14. As a developer, I want the namespace runner to use an isolated discovery
    group, so that the test cannot pass because of real devices on the LAN.
15. As a user reporting a connectivity problem, I want the README to state that
    a hand-written config wins over old app settings, so that support advice
    matches what the app does.

## Implementation Decisions

- **Precedence does not change.** A hand-written `rns/config` remains
  authoritative. No legacy row is created when the config defines interfaces,
  including a disabled one. The new notice is the only trace of the skipped
  settings, so the interface list stays a list of things that are actually in
  use.
- **The two import flags stay separate.** `mesh_network_imported` guards the old
  `desktop.json` settings; `mesh_config_imported` guards the bridge's read of
  `rns/config`. Neither may be used as the other's gate, and neither is set
  before the work it describes has succeeded.
- **The legacy gate settles with the config import.** When a config exists,
  startup leaves the gate open as it does now. After the bridge reports a
  successful `import_config`, the engine closes the gate and records the source
  (`config`, `legacy`, `default`, or `empty`) in a meta key. If the import
  produced no interface rows, the gate stays open and the existing first-run
  seeding runs, so a config containing only extra sections still yields the
  default discovery row. A failed import never settles the gate, so the next
  start retries.
- **Source is a fact, not a guess.** `legacy` means at least one row came from
  the old app settings; `default` means only first-run defaults were seeded;
  `config` means the bridge imported at least one interface from `rns/config`;
  `empty` means the user explicitly chose local-only with discovery off and no
  relay.
- **The notice is derived from explicit settings only.** It appears when the
  source is `config` and the old settings carried a non-empty relay or an
  explicit discovery-off. A default-on discovery flag is not a saved choice and
  does not produce a notice. The message names only what was present: relay,
  discovery, or both.
- **The notice rides on the existing interfaces reply.** `ListInterfaces` gains
  an optional `notice` object with a stable kind and a human message; no new
  polling, no event. A new client command dismisses it by writing a meta flag.
  Dismissal is idempotent. The Network page renders it above the interface list
  as an informational banner, not a toast or a modal: a toast can be missed and
  a modal overstates a non-error.
- **Origin wording is documented, not widened.** `origin: legacy` stays reserved
  for rows created from old app settings; `origin: imported` stays reserved for
  rows read out of `rns/config`. A config-wins upgrade therefore has no `legacy`
  row, and that is correct; the notice explains the absence.
- **The mobile settings payload shrinks.** `SettingsView` loses
  `auto_discovery` and `tcp_relay`. The legacy `desktop.json` reader stays,
  because it is migration input, but it is no longer part of the shell's live
  settings API. `LegacyNetwork` and `SettingsView` are separate types with
  separate purposes.
- **One settings shape, two small types.** Both shells expose exactly `muted`
  and `autostart`, and each gets a serialization test asserting that key set.
  The desktop shell is a standalone Tauri workspace and does not link
  `rchat-ipc`; a two-field shared struct is not worth pulling engine
  dependencies into it. Sharing a type is allowed only if it stays dependency
  free. The mobile value for `autostart` stays `false`, which is honest: Android
  has no login autostart.
- **The namespace runner is a companion script.** `scripts/smoke-rendezvous.mjs`
  stays runnable without privileges. A new Linux-only runner sets up a
  discovery-only case — one leaf on the host, one in a namespace, no relay at
  all — and reuses the existing client helpers (`scripts/lib.mjs`,
  `scripts/mesh-config.mjs`) rather than duplicating them.
- **The namespace topology is one namespace, not two.** The host runs leaf A;
  leaf C runs in a namespace reachable over the veth. A small forwarder inside
  the namespace exposes leaf C's loopback HTTP/WS port on the veth address, so
  the Node orchestrator can drive both leaves through the same client.
  Supporting two namespaces would need a second forwarder and buys nothing here.
- **The scripted discovery test is isolated from the real LAN.** Both leaves use
  a per-run AutoInterface `group_id` (`rchat-smoke-<pid>`), so a device on the
  physical network cannot make the test pass. The runner also asserts that the
  leaves' interface rows never contain the other leaf's Reticulum address.
- **No privileges, no failure.** The runner checks for `ip netns` plus write
  access (root or passwordless sudo) and for IPv6 on the veth. When either is
  missing it prints one line naming the unmet requirement and exits 0. CI opts
  in explicitly; the default `node scripts/smoke-rendezvous.mjs` is unchanged.
- **Cleanup is unconditional.** The runner removes the namespace, veth and
  forwarder on success, failure, SIGINT and SIGTERM. A leftover namespace would
  break the next run, and a leftover AutoInterface process would hold the data
  port this spec exists to work around.

## Testing Decisions

A good test here asserts an outcome a user or a developer can observe: a notice
appears once, a flag is settled, a message crosses a link with no relay.

- **Store seam.** Through the public store API: a config-wins start leaves the
  gate open and creates no rows; settling after an import with rows records
  `config` and closes the gate; settling with no imported rows leaves the gate
  open so first-run seeding still runs; settling after a failed import is never
  called; a second settle is a no-op; `mesh_config_imported` is untouched by all
  of it. Prior art: the existing `a_present_rns_config_is_imported_before_anything_is_seeded`
  test.
- **Notice seam.** The interfaces reply contains the notice only when the source
  is `config` and the old settings had an explicit relay or discovery-off; it
  names the fields that were present; it is absent for a normal legacy import;
  dismissal removes it and survives a reopen; dismissal is idempotent.
- **Engine seam.** App tests drive startup with a fabricated `rns/config` and a
  fabricated legacy payload and assert only what clients are told. The existing
  transport-derivation tests must keep passing unchanged. Prior art: the
  fabricated bridge-event tests for `degraded`.
- **Shell seam.** A Rust test serialises the mobile settings view and asserts
  its keys are exactly `muted` and `autostart`; a matching desktop test asserts
  the same shape. No test reaches into Tauri state.
- **Script seam.** `node scripts/smoke-rendezvous-netns.mjs` proves the
  relay-free discovery case: both leaves start with one AutoInterface row and no
  relay, exchange a message, and neither stores the other's address. The
  capability check is unit-testable by forcing the probe to fail and asserting
  the skip message and exit code. The full run is a Linux acceptance check, not
  part of `cargo test`.
- **Manual device pass, documented like the notification hand pass.** Two real
  devices on one access point with no relay, confirming discovery and two-way
  delivery; then on separate networks with one named relay, confirming delivery
  and the `degraded`/`ready` wording. Record it in the README next to the
  existing notification hand pass. CI cannot host a second physical device.
- **Deliberately not automated:** multicast reachability between physical
  machines, public relay DNS resolution, Android doze timing, and any
  namespace setup on Windows or macOS. Each is flaky or unavailable in a way
  that would train people to ignore the suite.

## Out of Scope

- Changing the config-wins rule, merging old settings into a hand-written config,
  or rewriting a user's `rns/config`.
- Deleting or rewriting `desktop.json` after migration; the engine ignores it
  once the gate is closed and the file is harmless.
- A general notification or banner system; the upgrade notice is one derived
  fact on one page.
- Automatic relay discovery, hosting a relay, mDNS/zeroconf, and public testnet
  defaults (still out of scope from `peer-rendezvous`).
- Two-namespace or container-based runners, and any non-Linux namespace
  emulation.
- Mobile-specific network controls beyond the existing interface management;
  the phone uses the same Network page as the desktop.
- The two-device pass itself is a release check, not code. This spec documents
  how to run it and where to record the result; it cannot perform the pass in
  CI.

## Further Notes

- **Why not a disabled legacy row.** The recorded decision is that a config
  already in place stays authoritative and no legacy rows are created. A
  disabled row would also imply rchat owns it, and the user would have to delete
  something they never wrote. A notice states the fact without adding state.
- **Why not a toast.** The upgrade happens on a start the user may not be
  watching, and a toast disappears before the Network page is opened. A banner
  on the page that owns interfaces is where the user goes to fix the
  consequence.
- **Why settle after the bridge import.** The gate must stay open until the
  config has actually been read, or a failed import would permanently suppress
  the legacy settings. It must close in the same start that succeeded, or the
  source is re-derived on every start and the second-start behaviour becomes
  part of the contract by accident.
- **Why the namespace runner uses the host for leaf A.** Keeping one leaf on the
  host means one forwarder, one veth end and one set of existing client calls.
  A discovery-only case needs no relay, so the topology stays as small as it can
  be while still crossing a network boundary.
- **Why IPv6 matters.** RNS AutoInterface discovers over IPv6 link-local
  multicast. A veth pair forwards multicast, but the runner must leave IPv6
  enabled on it; a silently IPv4-only veth would make discovery fail for a
  reason that looks like an rchat bug.
- **Relation to the two-device pass.** The namespace runner proves the code path
  and the assertions. It does not prove that a particular access point forwards
  multicast, which is why the manual pass stays. The runner makes the manual
  pass a confirmation rather than the only evidence.
- **README correction.** The current "Transports" section says the old discovery
  flag and relay string are imported once, full stop. That is true only when no
  `rns/config` exists. The sentence becomes a precedence rule, and the notice
  makes the exception visible in the app as well.
