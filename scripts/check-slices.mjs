#!/usr/bin/env node
// Reject a slice plan before anything is dispatched. Every rule here is
// mechanical, so it runs without a model:
//
//   node scripts/check-slices.mjs .scratch/<feature>/slices.md
//
// Exit 0 accepts the plan, exit 1 lists the violations and exit 2 means the
// command itself could not run. Whether a slice is genuinely vertical, or
// whether an acceptance criterion means anything, is judgement and stays with
// the maintainer.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const STATUSES = new Set(["proposed", "approved", "dispatched"]);
const BRIDGE = "sidecar/bridge.py";

/// The first `Status:` value in a document, or null.
function firstStatus(text) {
  for (const raw of text.split(/\r?\n/)) {
    const match = /^Status:\s*(.*)$/.exec(raw.trim());
    if (match) return match[1].trim();
  }
  return null;
}

/// Read a plan into slices and headings the schema does not allow.
function parsePlan(text) {
  const slices = [];
  const malformed = [];
  let status = null;
  let current = null;
  let started = false;

  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();

    const statusLine = /^Status:\s*(.*)$/.exec(line);
    if (statusLine && !started) {
      status = statusLine[1].trim();
      continue;
    }

    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      if (heading[1].length !== 2) {
        current = null;
        continue;
      }
      const numbered = /^(\d+)\.\s*(.*)$/.exec(heading[2].trim());
      if (!numbered) {
        malformed.push(heading[2].trim());
        current = null;
        continue;
      }
      current = {
        number: Number(numbered[1]),
        touches: [],
        dependencies: [],
        malformedDependencies: [],
        hasDependencies: false,
        verify: "",
        acceptance: [],
      };
      slices.push(current);
      started = true;
      continue;
    }

    if (!current) continue;

    const field = /^(Touches|Depends on|Verify):\s*(.*)$/.exec(line);
    if (field) {
      const value = field[2].trim();
      if (field[1] === "Touches") {
        for (const part of value.split(",")) {
          const touch = parseTouch(part);
          if (touch) current.touches.push(touch);
        }
      } else if (field[1] === "Depends on") {
        current.hasDependencies = true;
        for (const token of value.split(",").map((part) => part.trim())) {
          if (!token) continue;
          if (/^\d+$/.test(token)) current.dependencies.push(Number(token));
          else if (!["-", "none", "—"].includes(token)) {
            current.malformedDependencies.push(token);
          }
        }
      } else {
        current.verify = value;
      }
      continue;
    }

    const bullet = /^[-*]\s+(.*)$/.exec(line);
    if (bullet && bullet[1].trim()) current.acceptance.push(bullet[1].trim());
  }

  return { status, slices, malformed };
}

/// `+path` declares a file this feature creates. Backticks are allowed because
/// plans are read as markdown.
function parseTouch(raw) {
  const value = raw.trim().replace(/^`+|`+$/g, "").replace(/^\.\//, "");
  if (!value) return null;
  if (value.startsWith("+")) {
    const path = value.slice(1).replace(/^\.\//, "");
    return path ? { path, created: true } : null;
  }
  return { path: value, created: false };
}

/** Check a plan against every mechanical rule. Pure: the repository is a
 * predicate, so the rules are testable without a checkout. `specStatus` is
 * supplied by the caller that can read the sibling spec; when omitted, the
 * admission check is skipped and only the plan is judged. */
export function checkSlices(text, options = {}) {
  return validate(parsePlan(text), options);
}

function validate(
  { status, slices, malformed },
  { pathExists = () => true, specStatus } = {},
) {
  const violations = [];

  if (specStatus === null) {
    violations.push({ kind: "missing-spec", slices: [] });
  } else if (specStatus !== undefined && specStatus !== "ready-for-agent") {
    violations.push({ kind: "spec-not-ready", slices: [] });
  }

  if (!STATUSES.has(status)) {
    violations.push({ kind: "unknown-status", slices: [] });
  }
  if (slices.length === 0) {
    violations.push({ kind: "no-slices", slices: [] });
  }
  for (const title of malformed) {
    violations.push({ kind: "malformed-slice", slices: [], title });
  }

  const counts = new Map();
  for (const slice of slices) {
    counts.set(slice.number, (counts.get(slice.number) ?? 0) + 1);
  }
  for (const [number, count] of counts) {
    if (count > 1) violations.push({ kind: "duplicate-slice", slices: [number] });
  }

  const numbering = [...counts.keys()].sort((a, b) => a - b);
  if (numbering.some((number, index) => number !== index + 1)) {
    violations.push({ kind: "slice-numbering", slices: numbering });
  }

  for (const slice of slices) {
    if (slice.touches.length === 0) {
      violations.push({ kind: "missing-touches", slices: [slice.number] });
    }
    if (!slice.hasDependencies) {
      violations.push({ kind: "missing-dependencies", slices: [slice.number] });
    }
    if (!slice.verify) {
      violations.push({ kind: "missing-verify", slices: [slice.number] });
    }
    if (slice.acceptance.length === 0) {
      violations.push({ kind: "missing-acceptance", slices: [slice.number] });
    }
  }

  const numbers = new Set(slices.map((slice) => slice.number));
  for (const slice of slices) {
    for (const dependency of new Set(slice.dependencies)) {
      if (!numbers.has(dependency)) {
        violations.push({
          kind: "unknown-dependency",
          slices: [slice.number],
          dependency,
        });
      }
    }
    for (const dependency of slice.malformedDependencies) {
      violations.push({
        kind: "malformed-dependency",
        slices: [slice.number],
        dependency,
      });
    }
  }

  for (const cycle of findCycles(slices)) {
    violations.push({ kind: "dependency-cycle", slices: cycle });
  }

  const checked = new Set();
  for (const slice of slices) {
    for (const { path, created } of slice.touches) {
      const key = `${slice.number}\0${path}`;
      if (checked.has(key)) continue;
      checked.add(key);
      if (!created && !pathExists(path)) {
        violations.push({ kind: "missing-touch", slices: [slice.number], path });
      }
    }
  }

  const owners = new Map();
  for (const slice of slices) {
    for (const { path } of slice.touches) {
      if (!owners.has(path)) owners.set(path, new Set());
      owners.get(path).add(slice.number);
    }
  }
  for (const [path, owning] of owners) {
    if (owning.size < 2) continue;
    violations.push({
      kind: path === BRIDGE ? "bridge-contested" : "overlap",
      slices: [...owning].sort((a, b) => a - b),
      path,
    });
  }

  return violations;
}

/// Depth-first search over declared dependencies. A back edge closes a cycle;
/// the reported list starts at the smallest slice number so it is stable.
function findCycles(slices) {
  const present = new Set(slices.map((slice) => slice.number));
  const edges = new Map();
  for (const slice of slices) {
    const deps = edges.get(slice.number) ?? [];
    for (const dependency of slice.dependencies) {
      if (present.has(dependency)) deps.push(dependency);
    }
    edges.set(slice.number, [...new Set(deps)]);
  }

  const state = new Map();
  const stack = [];
  const cycles = [];
  const seen = new Set();

  const visit = (node) => {
    state.set(node, "visiting");
    stack.push(node);
    for (const dependency of edges.get(node) ?? []) {
      const mark = state.get(dependency) ?? "unvisited";
      if (mark === "visiting") {
        const cycle = rotate(stack.slice(stack.indexOf(dependency)));
        const key = cycle.join(",");
        if (!seen.has(key)) {
          seen.add(key);
          cycles.push(cycle);
        }
      } else if (mark === "unvisited") {
        visit(dependency);
      }
    }
    stack.pop();
    state.set(node, "done");
  };

  for (const { number } of slices) {
    if ((state.get(number) ?? "unvisited") === "unvisited") visit(number);
  }
  return cycles;
}

function rotate(cycle) {
  const offset = cycle.indexOf(Math.min(...cycle));
  return [...cycle.slice(offset), ...cycle.slice(0, offset)];
}

function listSlices(numbers) {
  if (numbers.length === 0) return "the plan";
  if (numbers.length === 1) return `slice ${numbers[0]}`;
  return `slices ${numbers.slice(0, -1).join(", ")} and ${numbers.at(-1)}`;
}

function describe(violation) {
  const slices = listSlices(violation.slices);
  switch (violation.kind) {
    case "overlap":
      return `${slices} share ${violation.path}; split the write set`;
    case "bridge-contested":
      return `${slices} share ${violation.path}; the bridge is a serial lane`;
    case "missing-touch":
      return `${slices} touches ${violation.path}, which is not in the repository`;
    case "unknown-dependency":
      return `${slices} depends on slice ${violation.dependency}, which is not in the plan`;
    case "malformed-dependency":
      return `${slices} has a dependency the checker cannot read: ${violation.dependency}`;
    case "dependency-cycle":
      return violation.slices.length === 1
        ? `${slices} depends on itself`
        : `${slices} form a dependency cycle`;
    case "missing-verify":
      return `${slices} has no Verify command`;
    case "missing-acceptance":
      return `${slices} has no acceptance criteria`;
    case "missing-touches":
      return `${slices} names no files`;
    case "missing-dependencies":
      return `${slices} has no Depends on line`;
    case "slice-numbering":
      return `${slices} must be numbered 1, 2, 3, … from the first slice`;
    case "duplicate-slice":
      return `${slices} is declared more than once`;
    case "malformed-slice":
      return `"## ${violation.title}" is not a numbered slice`;
    case "unknown-status":
      return `Status must be one of ${[...STATUSES].join(", ")}`;
    case "missing-spec":
      return "there is no spec.md next to the plan";
    case "spec-not-ready":
      return "the feature's spec.md does not say ready-for-agent";
    case "no-slices":
      return "the plan has no numbered slices";
    default:
      return `${violation.kind}${violation.path ? `: ${violation.path}` : ""}`;
  }
}

/** Run the checker for a plan path and return a process status: 0 accepted,
 * 1 violations, 2 the command could not run. */
export async function main({
  argv = process.argv.slice(2),
  readFile = (path) => readFileSync(path, "utf8"),
  pathExists = (path) => existsSync(join(ROOT, path)),
  log = console.log,
  error = console.error,
} = {}) {
  const planPath = argv[0];
  if (!planPath) {
    error("usage: node scripts/check-slices.mjs <slices.md>");
    return 2;
  }

  let text;
  try {
    text = readFile(planPath);
  } catch (err) {
    error(`${planPath}: cannot read plan: ${err.message}`);
    return 2;
  }

  const plan = parsePlan(text);
  const specPath = join(dirname(planPath), "spec.md");
  let specStatus;
  try {
    specStatus = firstStatus(readFile(specPath)) ?? "";
  } catch {
    specStatus = null;
  }
  const violations = validate(plan, { pathExists, specStatus });
  if (violations.length === 0) {
    const count = plan.slices.length;
    log(`${planPath}: ok (${count} ${count === 1 ? "slice" : "slices"})`);
    return 0;
  }

  for (const violation of violations) {
    error(`${planPath}: ${describe(violation)}`);
  }
  return 1;
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  process.exitCode = await main();
}
