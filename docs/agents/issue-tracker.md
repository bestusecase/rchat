# Issue tracker: Local Markdown

Issues and specs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The spec is `.scratch/<feature-slug>/spec.md`
- Implementation issues are one file per ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`, never a single combined tickets file
- Triage state is recorded as a `Status:` line near the top of each issue file
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## Slice plans

A feature that will be dispatched to agents is planned into
`.scratch/<feature-slug>/slices.md` before any work starts. The file is the
dispatch contract; a conversation about slices is not.

````markdown
# Slices: bridge split

Status: proposed

## 1. Move the config helpers out of the bridge

Touches: sidecar/bridge.py, +sidecar/config.py

Depends on: none

Verify: ./scripts/verify.sh

- The bridge still starts and writes the same `rns/config`.
- `python3 -m unittest discover -s sidecar/tests` passes.
````

Rules:

- `Status:` is `proposed`, `approved` or `dispatched`. Only the maintainer moves
  it to `approved`; the orchestrator moves it to `dispatched`. Editing an
  approved plan returns it to `proposed`.
- One `## <n>. <behaviour>` section per slice, numbered 1, 2, 3, … in order. A
  `##` heading without a number is a planning error, not a note.
- `Touches:` lists every file the slice will write, comma-separated and relative
  to the repo root. Prefix a path with `+` when the feature creates it, so the
  checker does not demand it already exists.
- `Depends on:` lists slice numbers, or `none`. The line is required even when
  the slice depends on nothing. A dependency must name a slice in this plan, and
  the edges must not form a cycle.
- `Verify:` is one command — `./scripts/verify.sh` — unless the slice cannot be
  proven by it, in which case the slice names the device-acceptance ticket it
  defers to.
- Acceptance bullets are the only place the slice says what done means. At
  least one is required.
- `sidecar/bridge.py` has at most one owner across the whole plan.

Check a plan before dispatch:

```
node scripts/check-slices.mjs .scratch/<feature-slug>/slices.md
```

The checker reads the sibling `spec.md` too: its `Status:` must say
`ready-for-agent`, or the plan is refused. It enforces the mechanical rules
above. It cannot tell whether a slice is vertical or whether an acceptance
criterion means anything; that is the maintainer's read at approval. See
`pipeline.md` for the rest of the process.

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a file with one **child** file per ticket.

- **Map**: `.scratch/<effort>/map.md` (the Notes / Decisions-so-far / Fog body).
- **Child ticket**: `.scratch/<effort>/issues/NN-<slug>.md`, numbered from `01`, with the question in the body. A `Type:` line records the ticket type (`research`/`prototype`/`grilling`/`task`); a `Status:` line records `claimed`/`resolved`.
- **Blocking**: a `Blocked by: NN, NN` line near the top. A ticket is unblocked when every file it lists is `resolved`.
- **Frontier**: scan `.scratch/<effort>/issues/` for files that are open, unblocked, and unclaimed; first by number wins.
- **Claim**: set `Status: claimed` and save before any work.
- **Resolve**: append the answer under an `## Answer` heading, set `Status: resolved`, then append a context pointer (gist + link) to the map's Decisions-so-far in `map.md`.

## Note on duplication

`../docs/agents/issue-tracker.md` in the workspace root holds the same
conventions with paths prefixed `rchat/`, for sessions started outside this
repo. Keep the two in sync.
