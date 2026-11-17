// Rules for the slice-plan checker. The plan text and the repository are both
// injected, so these prove the invariants without a checkout on disk:
//
//   node --test scripts/check-slices.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { checkSlices, main } from "./check-slices.mjs";

const exists = () => true;
const none = () => false;

function check(text, pathExists = exists) {
  return checkSlices(text, { pathExists });
}

const accepted = `# Slices: carry a reaction

Status: proposed

## 1. Carry a reaction from the wire to the store

Touches: crates/rchat-ipc/src/lib.rs, crates/rchat-core/src/store.rs

Depends on: none

Verify: ./scripts/verify.sh

- A reaction round-trips through the wire type.
- The store returns the reaction with its message.

## 2. Show a reaction in the shell

Touches: apps/desktop/src/App.svelte

Depends on: 1

Verify: ./scripts/verify.sh

- The message card shows the reaction.
- An old message without a reaction renders unchanged.
`;

function plan({ status = "proposed", slices } = {}) {
  return `# Slices: fixture\n\nStatus: ${status}\n\n${slices}`;
}

function slice(number, { touches = "crates/rchat-core/src/store.rs", depends = "none", verify = "./scripts/verify.sh", body = "- the behaviour holds" } = {}) {
  return `## ${number}. A slice

Touches: ${touches}

Depends on: ${depends}

Verify: ${verify}

${body}
`;
}

test("an accepted plan produces no violations", () => {
  assert.deepEqual(check(accepted), []);
});

test("a file the feature creates is declared with a plus and needs no existing path", () => {
  const text = plan({
    slices: slice(1, { touches: "+sidecar/config.py, sidecar/bridge.py" }),
  });
  const violations = checkSlices(text, {
    pathExists: (path) => path === "sidecar/bridge.py",
  });
  assert.deepEqual(violations, []);
});

test("two slices writing one file are refused and name both slices and the path", () => {
  const text = plan({
    slices: [
      slice(1, { touches: "crates/rchat-core/src/store.rs" }),
      slice(2, { touches: "crates/rchat-core/src/store.rs" }),
    ].join("\n"),
  });
  assert.deepEqual(check(text), [
    { kind: "overlap", slices: [1, 2], path: "crates/rchat-core/src/store.rs" },
  ]);
});

test("the bridge has at most one owner", () => {
  const text = plan({
    slices: [
      slice(1, { touches: "sidecar/bridge.py" }),
      slice(2, { touches: "sidecar/bridge.py" }),
    ].join("\n"),
  });
  assert.deepEqual(check(text), [
    { kind: "bridge-contested", slices: [1, 2], path: "sidecar/bridge.py" },
  ]);
});

test("a dependency on a slice that does not exist is refused", () => {
  const text = plan({
    slices: slice(1, { depends: "2" }),
  });
  assert.deepEqual(check(text), [
    { kind: "unknown-dependency", slices: [1], dependency: 2 },
  ]);
});

test("a slice without a Depends on line is refused", () => {
  const text = plan({
    slices: `## 1. A slice

Touches: crates/rchat-core/src/store.rs

Verify: ./scripts/verify.sh

- the behaviour holds
`,
  });
  assert.deepEqual(check(text), [
    { kind: "missing-dependencies", slices: [1] },
  ]);
});

test("slices are numbered from 1 without gaps", () => {
  const text = plan({
    slices: [
      slice(1, { touches: "crates/rchat-core/src/store.rs" }),
      slice(3, { touches: "crates/rchatd/src/app.rs" }),
    ].join("\n"),
  });
  assert.deepEqual(check(text), [
    { kind: "slice-numbering", slices: [1, 3] },
  ]);
});

test("a plan whose first slice is not 1 is refused", () => {
  const text = plan({ slices: slice(2) });
  assert.deepEqual(check(text), [
    { kind: "slice-numbering", slices: [2] },
  ]);
});

test("a dependency that is not a slice number is refused", () => {
  const text = plan({ slices: slice(1, { depends: "the wire ticket" }) });
  assert.deepEqual(check(text), [
    {
      kind: "malformed-dependency",
      slices: [1],
      dependency: "the wire ticket",
    },
  ]);
});

test("a dependency cycle is refused and names the slices in it", () => {
  const text = plan({
    slices: [
      slice(1, { depends: "2", touches: "crates/rchat-ipc/src/lib.rs" }),
      slice(2, { depends: "3", touches: "crates/rchat-core/src/store.rs" }),
      slice(3, { depends: "1", touches: "crates/rchatd/src/app.rs" }),
    ].join("\n"),
  });
  assert.deepEqual(check(text), [
    { kind: "dependency-cycle", slices: [1, 2, 3] },
  ]);
});

test("a self-dependency is a cycle", () => {
  const text = plan({ slices: slice(1, { depends: "1" }) });
  assert.deepEqual(check(text), [
    { kind: "dependency-cycle", slices: [1] },
  ]);
});

test("a slice without a verify command is refused", () => {
  const text = plan({ slices: slice(1, { verify: "" }) });
  assert.deepEqual(check(text), [{ kind: "missing-verify", slices: [1] }]);
});

test("a slice without acceptance criteria is refused", () => {
  const text = plan({ slices: slice(1, { body: "" }) });
  assert.deepEqual(check(text), [
    { kind: "missing-acceptance", slices: [1] },
  ]);
});

test("a slice that names no files is refused", () => {
  const text = plan({ slices: slice(1, { touches: "" }) });
  assert.deepEqual(check(text), [{ kind: "missing-touches", slices: [1] }]);
});

test("a Touches path missing from the repository is refused", () => {
  const text = plan({
    slices: slice(1, { touches: "crates/rchat-core/src/stroe.rs" }),
  });
  assert.deepEqual(check(text, () => false), [
    {
      kind: "missing-touch",
      slices: [1],
      path: "crates/rchat-core/src/stroe.rs",
    },
  ]);
});

test("a status the pipeline does not dispatch is refused", () => {
  const text = plan({ status: "in-progress", slices: slice(1) });
  assert.deepEqual(check(text), [{ kind: "unknown-status", slices: [] }]);
});

test("a plan with no slices is refused", () => {
  const text = plan({ slices: "" });
  assert.deepEqual(check(text), [{ kind: "no-slices", slices: [] }]);
});

test("two slices with the same number are refused", () => {
  const text = plan({
    slices: [
      slice(1, { touches: "crates/rchat-core/src/store.rs" }),
      slice(1, { touches: "crates/rchatd/src/app.rs" }),
    ].join("\n"),
  });
  assert.deepEqual(check(text), [{ kind: "duplicate-slice", slices: [1] }]);
});

test("an unnumbered section is refused rather than silently ignored", () => {
  const text = plan({ slices: `${slice(1)}\n## Notes\n\nNo number, no slice.\n` });
  assert.deepEqual(check(text), [
    { kind: "malformed-slice", slices: [], title: "Notes" },
  ]);
});

test("the command reads the plan, prints each violation and exits non-zero", async () => {
  const lines = [];
  const code = await main({
    argv: ["plan.md"],
    readFile: (path) =>
      path.endsWith("spec.md")
        ? "Status: ready-for-agent\n"
        : plan({
            slices: [
              slice(1, { touches: "crates/rchat-core/src/store.rs" }),
              slice(2, { touches: "crates/rchat-core/src/store.rs" }),
            ].join("\n"),
          }),
    pathExists: exists,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 1);
  assert.equal(lines.length, 1);
  assert.match(lines[0], /^plan\.md: /);
  assert.match(lines[0], /1 and 2/);
  assert.match(lines[0], /crates\/rchat-core\/src\/store\.rs/);
});

test("the command exits zero on an accepted plan", async () => {
  const lines = [];
  const code = await main({
    argv: ["plan.md"],
    readFile: (path) =>
      path.endsWith("spec.md") ? "Status: ready-for-agent\n" : accepted,
    pathExists: exists,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 0);
  assert.deepEqual(lines, ["plan.md: ok (2 slices)"]);
});

test("the command refuses a plan whose spec is not ready", async () => {
  const lines = [];
  const code = await main({
    argv: ["plan.md"],
    readFile: (path) =>
      path.endsWith("spec.md") ? "Status: resolved\n" : accepted,
    pathExists: exists,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 1);
  assert.match(lines[0], /spec\.md does not say ready-for-agent/);
});

test("the command refuses a plan with no spec beside it", async () => {
  const lines = [];
  const code = await main({
    argv: ["plan.md"],
    readFile: (path) => {
      if (path.endsWith("spec.md")) throw new Error("no such file");
      return accepted;
    },
    pathExists: exists,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 1);
  assert.match(lines[0], /no spec\.md next to the plan/);
});

test("the command refuses to run without a plan path", async () => {
  const lines = [];
  const code = await main({
    argv: [],
    readFile: none,
    pathExists: none,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 2);
  assert.match(lines[0], /usage:/);
});

test("an unreadable plan is a command error, not a violation", async () => {
  const lines = [];
  const code = await main({
    argv: ["missing.md"],
    readFile: () => {
      throw new Error("no such file");
    },
    pathExists: none,
    log: (line) => lines.push(line),
    error: (line) => lines.push(line),
  });
  assert.equal(code, 2);
  assert.match(lines[0], /cannot read/);
});
