# Make agent work accountable from slice plan to review

Status: ready-for-agent
Effort: `agent-pipeline`

Prerequisite: none. This effort adds process, one script and one checker. It
changes no wire type, no engine behaviour and no shell.

## Problem Statement

Code quality in this repo is bounded by spec quality, not by who writes the
code. The specs are already closed contracts — problem, solution, user stories,
implementation decisions, testing decisions, out of scope — and a cheap
implementer given a closed contract does fine. But nothing in the repo converts
a review complaint into a rule, so the same complaint is paid for again on the
next feature. There is no `docs/standards.md`, no `CONTRIBUTING`, one
twenty-line `AGENTS.md` that talks only about the issue tracker, and no
`rustfmt.toml`, `clippy.toml` or CI workflow anywhere. A reviewer asked to
"check this meets standards" has no referent, so it reviews against its own
preferences. The codebase then drifts toward whichever model reviewed it last,
and no one can say when that happened.

Verification has the same shape. The ninety unit tests are real and portable,
but the namespace runner needs root and a veth pair, and the mesh harness needs
three live nodes. Anything touching Reticulum behaviour therefore leaves the
suite and becomes a manual pass that is easy to skip and impossible to diff.

Work that has been planned is planned in conversation and dies there. A slice
list exists as scrollback, so by the time a diff is ready the reviewer compares
it to whatever the planner is remembered to have said. Two implementers can be
dispatched against the same file without anyone noticing, and `sidecar/bridge.py`
is where that hurts: 2131 lines, seventy-one top-level definitions, no module
boundaries, and every wire change edits it.

Finally, the backlog cannot tell finished work from unstarted work. Five specs
say `Status: ready-for-agent`; `attachments` and `reticulum-surface` still say it
with their issues done.

## Solution

The repo gains a written standard, a mechanical gate, and a plan that is a file
before it is a dispatch.

`docs/standards.md` holds only rules that an exit code cannot decide and that
the maintainer would block a merge over. Everything else moves into
`scripts/verify.sh`, which runs formatting, clippy at deny-warnings and the unit
tests and fails on the first violation. "Done" stops being an opinion.

An agent plans the work into `.scratch/<feature>/slices.md` — numbered slices,
each naming the files it touches, the slices it depends on, its acceptance
criteria and the exact command that proves it. A checker reads that file and
rejects the plan before anything dispatches: overlapping write sets, a dependency
that does not exist, two owners of `sidecar/bridge.py`, a missing verify command,
a `Touches:` path that is not in the repo. The maintainer reads the plan, applies
what the checker cannot judge — is this slice vertical, do these criteria mean
anything — and approves the file. The approval is a status line, not a reply.

Implementation then runs on the cheap model, slice by slice, in parallel only
where the write sets are disjoint. A slice is done when its verify command exits
zero; that is the only per-slice review there is. The whole feature gets one
review on the frontier model, against the spec, the plan and the rubric.

Each finding comes back labelled. A spec defect amends the spec before anything
is re-dispatched, because a defective spec reproduces its defect deterministically
and re-running the implementer against it only looks like the implementer is
failing. An implementation defect re-dispatches with the finding promoted to
acceptance criteria. A finding class seen twice becomes a rubric rule if it is
judgement and a check if it is mechanical, so the pipeline gets cheaper and the
specs get better at the same time.

## User Stories

1. As the maintainer, I want a written standard I would actually block a merge
   over, so that review has a referent instead of a reviewer's preferences.
2. As the maintainer, I want every rule an exit code can decide to be decided by
   an exit code, so that review attention is spent on judgement.
3. As the maintainer, I want one command that says whether a slice is done, so
   that "done" stops being an opinion.
4. As the maintainer, I want that command to name the unmet requirement and exit
   zero when a capability is missing, so that a machine without root does not
   produce a failure that looks like a bug.
5. As the maintainer, I want the slice plan written to a file before anything
   dispatches, so that I approve a plan rather than a chat message.
6. As the maintainer, I want each slice to name the files it touches, so that I
   can see a collision before it happens.
7. As the maintainer, I want a plan rejected automatically when two slices claim
   the same file, so that I never pay for a merge I could have prevented.
8. As the maintainer, I want `sidecar/bridge.py` to have exactly one owner in any
   plan, so that the one flat file every wire change touches is never edited
   concurrently.
9. As the maintainer, I want every slice to be vertical, so that each one
   delivers a behaviour instead of a layer.
10. As the maintainer, I want each slice to carry its acceptance criteria and its
    verify command, so that the implementer and the reviewer agree on done before
    work begins.
11. As the maintainer, I want dependency edges declared and checked, so that a
    slice is never dispatched against something that does not exist yet.
12. As the maintainer, I want a cycle in the plan to be rejected, so that a
    deadlock is a planning error rather than a dispatch-time discovery.
13. As the maintainer, I want a `Touches:` path that is not in the repo to be
    rejected, so that a rename cannot quietly invalidate the disjointness claim.
14. As the maintainer, I want the checker to be testable without a repository, so
    that its rules are proven rather than asserted.
15. As the maintainer, I want one review per feature over the whole diff, so that
    integration is judged rather than assembled from separately approved parts.
16. As the maintainer, I want per-slice review to be mechanical only, so that
    frontier passes are not spent re-reading context to re-reach the same
    conclusion.
17. As the maintainer, I want every finding labelled spec defect or implementation
    defect, so that I fix the input when the input was the problem.
18. As the maintainer, I want a spec defect to be valid only when the missing
    sentence can be named, so that "the spec was vague" cannot absorb every
    criticism.
19. As the maintainer, I want a recurring finding to become a written rule, so
    that the same complaint is paid for once.
20. As the maintainer, I want a mechanical finding to become a check, so that it
    never needs a reviewer again.
21. As the maintainer, I want the approval gate to leave an artifact even when I
    wave a plan through, so that it cannot silently decay into a conversation.
22. As the maintainer, I want the gate to apply to every feature, so that a
    protocol change cannot arrive ungated because the previous features were
    documentation.
23. As the maintainer, I want a spec to enter the pipeline only when its status
    says it is ready, so that finished and unstarted work are distinguishable.
24. As the maintainer, I want to know the realistic ceiling of this pipeline, so
    that I do not mistake process discipline for throughput.
25. As the maintainer, I want the first run of the pipeline to be a pure move, so
    that a failed run means the pipeline is wrong rather than the feature is hard.
26. As an implementing agent, I want the slice I am given to be closed and
    disjoint, so that I never guess at a decision or negotiate a file with another
    agent.
27. As a reviewing agent, I want a rubric and an approved plan to compare against,
    so that my findings are about this project rather than about my taste.
28. As a future maintainer, I want to read why a rule exists, so that I change it
    deliberately instead of discovering it by violating it.

## Implementation Decisions

- **The rubric is judgement only.** `docs/standards.md` holds rules an exit code
  cannot decide, and only rules the maintainer would block a merge over. Target
  ten to fifteen. Four categories are required: error handling, module
  boundaries, test placement, and public surface. Anything mechanical belongs in
  the verify script, which is the dividing line that keeps the rubric short.
- **One gate command, no flags.** `scripts/verify.sh` runs formatting, clippy at
  deny-warnings and the unit tests, and exits non-zero on the first failure. A
  slice that needs a different gate is a slice that is mis-scoped, so there is no
  per-slice variation to encode.
- **The gate cannot see Reticulum.** The namespace runner needs root and a veth
  pair; the mesh harness needs three live nodes. A slice whose behaviour cannot
  be proven in-process must name the device-acceptance ticket it defers to, and
  claiming a behaviour is verified when only its types compile is a rubric
  violation rather than a gap in the script.
- **The plan is a file with a schema.** `.scratch/<feature>/slices.md` carries a
  `Status:` of `proposed`, `approved` or `dispatched`, then one numbered section
  per slice with `Touches:`, `Depends on:`, `Verify:` and acceptance bullets.
  This extends the convention in `docs/agents/issue-tracker.md`, which today
  describes only `spec.md` and `issues/`.
- **Slices are vertical.** A slice delivers one behaviour across wire, engine,
  bridge and shell, or names the layers it deliberately omits. A slice that stops
  at a layer boundary is a layer change, integrates badly, and is the reason the
  plan is slices rather than stages.
- **Disjointness is declared, then checked.** The checker reads the plan and the
  repository together; overlap, cycles, a second `sidecar/bridge.py` owner, a
  missing verify command, a missing acceptance criterion and a nonexistent
  `Touches:` path are all rejected mechanically, before dispatch and without a
  model in the loop.
- **The checker is the only new seam.** One pure function from plan text to
  violations; the file read and the process exit are thin wrappers. Prompts make
  no decisions, because every mechanical invariant lives in the checker and every
  judgement rule lives in prose. A prompt with logic would need a seam it cannot
  have.
- **`sidecar/bridge.py` is the serial lane.** It is flat and every wire change
  edits it, so at most one slice owns it. Appending a variant to `ClientCmd` or a
  field to `Snapshot` is safe concurrently because it is append-at-tail, so those
  are not treated as contested.
- **One review per feature.** The aggregate diff is reviewed once, against the
  spec, the approved plan and the rubric. Per-slice review is the verify command
  and nothing more; the judgements worth making are about the set, and a slice in
  isolation can look correct inside an incoherent whole.
- **Classification precedes re-dispatch.** A `spec-defect` amends the spec first,
  then re-dispatches the affected slices. An `implementation-defect` re-dispatches
  with the finding text promoted to acceptance criteria. One retry per class,
  then that slice escalates to the frontier model.
- **Promotion is the payoff.** A finding class seen twice becomes a rubric rule if
  it is judgement and a verify check if it is mechanical. Classification costs a
  few hundred output tokens inside a review that is already running; promotion is
  what makes the cost fall feature over feature.
- **Approval is permanent and always leaves an artifact.** Scope-based gating was
  considered and rejected: a trust ratchet cannot tell a documentation feature
  from a protocol change, and a gate that writes nothing when waved through
  reverts to the conversational gate it replaced.
- **Work enters by status.** A spec is admissible when its `Status:` line reads
  `ready-for-agent`. A spec whose status disagrees with its slices is a spec
  defect, so the admission filter improves the specs by refusing them.
- **Agents are configured, not scripted.** One orchestrator that plans and
  reviews, one implementer that builds. Fable is not used: it costs twice what
  Opus costs for planning and review work that is bounded by a written spec and a
  written rubric.

## Testing Decisions

A good test here asserts something a developer observes: a plan with overlapping
write sets is refused and names both slices and the shared path; a plan whose
dependency target does not exist is refused; a plan with no verify command is
refused.

- **Plan seam.** `checkSlices` is tested table-driven: one accepted plan and one
  fixture per rejected invariant, asserting the violation kind and the slices it
  names. Prior art: `scripts/smoke-rendezvous-netns.test.mjs`, which proves the
  skip path by injecting `exec` rather than touching the machine.
- **Path existence** is tested with an injected predicate, so the tests need no
  repository and no fixture files on disk.
- **The verify script is not unit-tested.** It is a fixed command list, so its
  contract is its exit code: proven by running it on a clean tree, and again with
  a deliberate clippy warning to prove it fails.
- **The skip path.** A missing capability prints one line naming the unmet
  requirement and exits zero. Prior art: the discovery runner's capability probe,
  which the effort already established must not turn an absent capability into a
  failure.
- **Acceptance for the whole effort is a real run.** The `bridge.py` split is
  planned into slices, approved, implemented, verified and reviewed, and the
  outcome is recorded in the effort directory the way the notification hand pass
  is recorded in the README. No test can assert that planning is good; only a run
  can.
- **Deliberately not automated:** whether a slice is genuinely vertical, whether
  acceptance criteria mean anything, and whether a labelled spec defect is a true
  omission. These are judgement, which is exactly why they stay with a human.

## Out of Scope

- The `crates/rchat-ipc` module split and the `sidecar/bridge.py` split. Each is
  its own effort; together they are the first application of this pipeline.
- Restructuring `Snapshot`, or any change to the wire format. That would require
  a `PROTOCOL_VERSION` bump and a lockstep desktop and mobile update.
- Splitting `sidecar/bridge.py` into per-domain modules as part of this effort.
- CI beyond one workflow that runs the verify script.
- Automatic slice decomposition. An agent proposes slices; a human approves them.
- Per-slice frontier review.
- Optimising the pipeline's own token cost. At this repo's volume the difference
  between the cheapest and most expensive arrangement is a few dollars a feature.

## Further Notes

- **Why the rubric is short.** A long rubric is not read, so it is not applied,
  and review becomes theatre. Ten to fifteen rules that are actually enforced beat
  fifty that are aspirational. Rules are added only by promotion, which means
  every rule on the list has a complaint behind it.
- **Why one review, not one per slice.** The judgements worth frontier tokens are
  aggregate ones: is the spec satisfied, is the set coherent, does it conform.
  Per-slice review structurally cannot make them and costs a context read each.
- **Why the gate never loosens.** Trust is the wrong axis. The risk is that one
  inherently dangerous change slips through after a run of safe ones, and a ratchet
  has no way to see the difference.
- **Why the refactors go first.** A pure move has one failure mode, so a bad run
  is unambiguous evidence about the pipeline. A feature touching rendezvous
  behaviour has at least three, and two of them are environmental.
- **The honest ceiling.** After the bridge split, fan-out is one then two —
  engine-side and shell-side — with the bridge as a serial lane, capped near 2x
  by the critical path in the existing backlog. The approval round-trip costs one
  human block per feature. The payoff is compounding quality, not speed, and the
  pipeline should not be sold to itself on throughput.
- **The admission filter needs a sweep.** `attachments` and `reticulum-surface`
  both say `ready-for-agent` while their slices are done. Until that is corrected,
  the pipeline will keep offering finished work.
- **First application.** `sidecar/bridge.py` splits into config, interfaces, mesh
  and lxmf modules as a pure move, verified by the existing smoke harnesses. The
  `crates/rchat-ipc` split into per-domain modules with `pub use` re-exports
  follows opportunistically. Both are invisible to clients, which is why they are
  safe to run first.
