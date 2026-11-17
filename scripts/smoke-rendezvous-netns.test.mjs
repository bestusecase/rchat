// Capability checks for smoke-rendezvous-netns.mjs. The full run needs
// namespaces, so this file proves the skip path on any machine:
//
//   node --test scripts/smoke-rendezvous-netns.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { capabilityProblem, main } from "./smoke-rendezvous-netns.mjs";

const ipVersion = () => ({ status: 0, stdout: "ip utility, iproute2\n", stderr: "" });
const ok = () => ({ status: 0, stdout: "", stderr: "" });

function execWith({ ip = ipVersion, sudo = ok, netns = ok } = {}) {
  return (command, args) => {
    if (command === "ip" && args[0] === "-V") return ip();
    if (command === "sudo" && args[0] === "-n" && args[1] === "true") return sudo();
    if (command === "ip" && args[0] === "netns" && args[1] === "list") return netns();
    if (command === "sudo" && args.includes("netns") && args.includes("list")) {
      return netns();
    }
    return { status: 1, stdout: "", stderr: `unexpected ${command} ${args.join(" ")}` };
  };
}

test("a non-Linux host is skipped, not failed", () => {
  const problem = capabilityProblem({
    platform: "darwin",
    uid: 0,
    exec: execWith(),
  });
  assert.match(problem, /Linux-only/);
});

test("a host without the ip command is skipped", () => {
  const problem = capabilityProblem({
    platform: "linux",
    uid: 0,
    exec: execWith({
      ip: () => ({ status: 1, stdout: "", stderr: "not found" }),
    }),
  });
  assert.match(problem, /ip command is not installed/);
});

test("a host without root or passwordless sudo is skipped", () => {
  const problem = capabilityProblem({
    platform: "linux",
    uid: 1000,
    exec: execWith({
      sudo: () => ({ status: 1, stdout: "", stderr: "a password is required" }),
    }),
  });
  assert.match(problem, /root or passwordless sudo/);
});

test("a host that cannot list namespaces is skipped", () => {
  const problem = capabilityProblem({
    platform: "linux",
    uid: 0,
    exec: execWith({
      netns: () => ({ status: 1, stdout: "", stderr: "not permitted" }),
    }),
  });
  assert.match(problem, /ip netns is unavailable/);
});

test("root with ip available can run the case", () => {
  assert.equal(
    capabilityProblem({ platform: "linux", uid: 0, exec: execWith() }),
    null,
  );
});

test("passwordless sudo with ip available can run the case", () => {
  assert.equal(
    capabilityProblem({ platform: "linux", uid: 1000, exec: execWith() }),
    null,
  );
});

test("the skip path prints one line and exits successfully", async () => {
  const lines = [];
  const code = await main({
    platform: "darwin",
    uid: 0,
    exec: execWith(),
    log: (line) => lines.push(line),
  });
  assert.equal(code, 0);
  assert.deepEqual(lines, [
    "smoke-rendezvous-netns: skipped (network namespaces are Linux-only)",
  ]);
});
