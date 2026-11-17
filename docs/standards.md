# Standards

Review has a referent: this list. It holds only rules that an exit code cannot
decide and only rules worth blocking a merge over. If a check can decide it, it
belongs in `scripts/verify.sh` or `scripts/check-slices.mjs`, not here.

Rules are added by promotion — a finding class seen twice becomes a rule — so
every line below has a real complaint behind it. Keep the list short. A rubric
nobody reads is not applied, and review becomes theatre.

## Error handling

1. **A refusal is written where the reason is known.** The layer that knows why
   something is invalid names it: the bridge for Reticulum shape, the engine for
   product policy, the shell for presentation. Upper layers add context; they do
   not re-derive the reason. Why: duplicated validation drifts, and the second
   copy is always the one that is wrong.
2. **An `unwrap` is a claim, not a shortcut.** Use `unwrap`, `expect` or
   `panic!` only where the failure is locally impossible and the immediate code
   makes that evident. If proving it takes more than the surrounding lines,
   return the error instead. Why: the daemon is long-lived and the bridge is a
   child process; a panic is a user-visible crash, not a debug print.
3. **A failure that crosses a boundary keeps its meaning.** Client-visible
   errors say what was refused and what the user can do next. Internal error
   types, absolute paths and stack details do not cross the wire. Why: desktop
   and phone show the same string, and a leaky one cannot be fixed on one
   platform alone.
4. **Degrading and retrying are decisions, not accidents.** When a capability is
   unavailable, retry or recover in the layer that can, then say what is
   degraded and what still works. Never silently substitute an empty result for
   a failed one, and never surface an error the engine could have handled. Why:
   in an offline-first app "offline" has to look different from "broken", and
   every surfaced failure costs the user an action.

## Module boundaries

5. **The window is a view.** Policy — routing, retry, normalisation, attachment
   lifetime — lives in `rchatd` and `rchat-core`; shells render state and send
   commands. Why: Android links the same engine, so a rule placed in one shell
   exists in the other shell or in neither.
6. **The wire crate is a leaf.** `crates/rchat-ipc` holds types and their serde
   rules and depends on nothing else in the workspace. Why: both shells compile
   it, and behaviour placed there cannot be exercised without inventing a
   client.
7. **A domain owns its storage.** Tables, queries and their invariants stay in
   `rchat-core`; `rchatd` orchestrates them, and the bridge never writes SQLite.
   Why: two writers to one table is how a column grows two meanings.
8. **The bridge is the Reticulum seam, not the policy layer.** Reticulum
   mechanics belong in `sidecar/bridge.py`; product decisions — retry budgets,
   eviction, routing — belong in Rust. Why: the bridge is the serial lane and
   the only part of the product that cannot be unit-tested cheaply.
9. **A shared concept gets one home.** When two layers need the same thing, it
   moves down to the lowest crate that can own it. Copied structs and parallel
   enums are not boundaries. Why: conversions are where fields get silently
   dropped, and only one copy can be the truth.

## Test placement

10. **A unit test sits beside the code it proves.** In-crate proofs live in
    `#[cfg(test)] mod tests` next to their subject; `tests/` is for a process,
    API or workspace boundary that needs a real spawn. Why: a test far from its
    subject is rerun rarely and updated last.
11. **A test asserts an observed behaviour and states the promise.** Its name
    says what a user or caller observes, and a failing assertion says which
    promise broke, not merely which values differ. Why: `verify.sh` is the only
    per-slice review, so its output has to be readable by whoever broke it.
12. **What the gate cannot prove is declared, not implied.** Behaviour needing
    root, network namespaces or three live nodes names the device or network
    ticket that accepts it; the in-tree test skips with the unmet requirement
    instead of pretending. Why: a green suite that never touched Reticulum is
    worse than one that admits what it did not test.
13. **Prove it at the lowest layer that can see it.** A wire round-trip belongs
    in `rchat-ipc`, a routing rule in `rchatd`, a rendering rule in the shell
    that renders. Why: duplicate coverage hides which layer changed and doubles
    the cost of every wire tweak.

## Public surface

14. **Within a protocol version, wire changes are additive.** New fields and
    command variants are optional and defaulted. A rename, a removal or a
    changed meaning bumps `PROTOCOL_VERSION` and updates both shells in the same
    change. Why: desktop and phone update independently, and a peer can be
    running last month's app.
15. **`pub` is a promise.** Expose an item from a crate only when another crate
    or a client depends on it; a helper used once is private. Every public item
    carries a doc comment saying what it promises and for whom. Why: each
    exported item is a compatibility obligation that no gate can see.
