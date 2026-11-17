# Agent pipeline

How a spec becomes a reviewed change. Three files do the work:

| File | Decides |
| --- | --- |
| `docs/standards.md` | judgement rules a reviewer applies |
| `scripts/verify.sh` | whether a slice is done |
| `scripts/check-slices.mjs` | whether a plan may be dispatched |

One orchestrator agent plans and reviews; one implementer builds. Prompts carry
no decision logic: every mechanical invariant lives in the checker, every
judgement rule in the rubric. Fable is not used; planning and review here are
bounded by written artefacts.

## Admission

A spec enters the pipeline only when its `Status:` line reads
`ready-for-agent`. A finished feature says `resolved`; a spec that says
`ready-for-agent` after its issues are done is a spec defect, so the filter
improves the specs by refusing them. The plan checker reads the sibling
`spec.md`, so an inadmissible spec cannot be dispatched.

## 1. Plan

The orchestrator writes `.scratch/<feature>/slices.md` using the schema in
[issue-tracker.md](issue-tracker.md). It is numbered slices, numbered from 1,
each with `Touches:`, `Depends on:`, `Verify:` and acceptance bullets. The file
starts at `Status: proposed`.

The orchestrator then runs:

```
node scripts/check-slices.mjs .scratch/<feature>/slices.md
```

Overlapping write sets, a second `sidecar/bridge.py` owner, a nonexistent
dependency, a cycle, a missing verify command, a missing acceptance criterion or
a `Touches:` path that does not exist are rejected here, before anything is
dispatched. So are a spec that does not say `ready-for-agent`, a status the
pipeline does not dispatch, slices not numbered 1, 2, 3 from the first, a
missing `Touches:` or `Depends on:` line, and a dependency the checker cannot
read.

## 2. Approve

The maintainer reads the plan for what the checker cannot judge: is each slice
vertical (does it deliver a behaviour, not a layer), do the acceptance criteria
mean anything, is the named `Verify:` command the right proof. Approval is the
`Status:` line moving to `approved`, not a reply. Editing an approved plan
returns it to `proposed` for another read.

The approval always leaves the file. There is no scope-based trust ratchet: a
protocol change must not arrive ungated because the previous features were
documentation.

## 3. Dispatch

Set `Status: dispatched`, then give each implementer exactly one slice: its
title, touches, dependencies, acceptance criteria, verify command and the spec.
Slices run in parallel only where the checker found disjoint write sets.
`sidecar/bridge.py` is a serial lane — appending a variant to `ClientCmd` or a
field to `Snapshot` is not contested, editing the flat file is.

## 4. Done

A slice is done when its `Verify:` command exits zero. That is the only
per-slice review. The implementer does not change its own acceptance criteria;
if the criteria are wrong, that is a spec defect and goes back a step.

A skip is not done. If `verify.sh` prints `verify: skipped`, the machine lacked
a capability and the gate did not run; the slice's status is unproven, not
green.

The gate cannot see Reticulum. A slice whose behaviour needs root, namespaces or
three live nodes names the device-acceptance ticket it defers to. Claiming a
behaviour is verified because its types compile is a standards violation.

## 5. Review

One review per feature, over the aggregate diff, against the spec, the approved
plan and `docs/standards.md`. The judgements worth a frontier pass are aggregate
ones — is the spec satisfied, is the set coherent, does it conform — and a slice
reviewed in isolation can look correct inside an incoherent whole.

## 6. Classify and repair

Every finding comes back labelled:

- **`spec-defect`** — the spec is missing a sentence, and the finding can name
  it. "The spec was vague" is not enough. Amend the spec first, then re-dispatch
  the affected slices. A defective spec reproduces its defect deterministically,
  so re-running the implementer against it only looks like the implementer is
  failing.
- **`implementation-defect`** — the slice did not meet its stated criteria.
  Re-dispatch with the finding promoted to acceptance criteria.

One retry per class per slice, then that slice escalates to the frontier model.

## 7. Promote

A finding class seen twice becomes a rule:

- judgement → one line in `docs/standards.md`
- mechanical → a step in `scripts/verify.sh` or a rule in
  `scripts/check-slices.mjs`

Promotion is the payoff. Classification costs a few hundred output tokens
inside a review that is already running; promotion is what makes the next
feature cheaper than this one.

## The honest ceiling

After the bridge split, fan-out is one then two — engine side and shell side —
with the bridge as a serial lane. The critical path in the backlog caps the gain
near 2x, and each feature costs one human approval block. The pipeline buys
compounding quality, not speed; do not sell it to itself on throughput.

## First run

The first feature through the pipeline is a pure move: split
`sidecar/bridge.py` into config, interface, mesh and LXMF modules, verified by
the existing smoke harnesses. A pure move has one failure mode, so a failed run
is evidence about the pipeline rather than about the feature. The
`crates/rchat-ipc` split follows opportunistically.
