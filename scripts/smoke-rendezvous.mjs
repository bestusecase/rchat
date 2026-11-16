// Peer rendezvous smoke test: a real rnsd transport node and two real rchatd
// leaves, configured only through the engine's interface API.
//
//   node scripts/smoke-rendezvous.mjs                 # phases a and b
//   node scripts/smoke-rendezvous.mjs --phases=a,b,c  # add discovery, two hosts
//
// Phases:
//   (a) the leaves exchange a message through the relay with neither leaf's
//       interface rows holding the other's Reticulum address;
//   (b) leaf C is stopped and restarted; delivery resumes and A's interface
//       rows and peer configuration are unchanged;
//   (c) both leaves delete the relay row and re-enable discovery, then exchange
//       a message over the direct local path.
//
// (a) always runs, because (b) and (c) start from it. This is a developer script:
// it starts processes, writes `.dev/rendezvous`, and kills them on exit.
//
// One host cannot reproduce a DHCP lease change, so (b) restarts the same
// identity and asserts the stored side is inert (no address was ever written
// into A). The real address-change pass is the documented two-device manual
// check. Phase (c) is off by default for the same reason: RNS AutoInterface
// binds one fixed data port per host, so two full Reticulum instances on one
// machine cannot discover each other. Run it on two devices, or in separate
// network namespaces.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { connect, sleep, until } from "./lib.mjs";
import { configureInterfaces } from "./mesh-config.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DEV = process.env.RENDEZVOUS_DIR || path.join(ROOT, ".dev", "rendezvous");
const RELAY_TCP = Number(process.env.RELAY_TCP || 4282);
const PORT_A = Number(process.env.PORT_A || 8781);
const PORT_C = Number(process.env.PORT_C || 8783);

function phaseSet() {
  const args = process.argv.slice(2);
  let value = process.env.PHASES || "a,b";
  const inline = args.find((arg) => arg.startsWith("--phases="));
  if (inline) value = inline.slice("--phases=".length);
  const at = args.indexOf("--phases");
  if (at >= 0 && args[at + 1]) value = args[at + 1];
  return new Set(
    value
      .split(",")
      .map((part) => part.trim())
      .filter(Boolean),
  );
}

const phases = phaseSet();
const runB = phases.has("b");
const runC = phases.has("c");

const children = [];
let stopping = false;

function start(name, command, args, env) {
  const log = fs.openSync(path.join(DEV, name + ".log"), "a");
  const child = spawn(command, args, {
    cwd: ROOT,
    env,
    stdio: ["ignore", log, log],
  });
  child.label = name;
  child.on("exit", (code, signal) => {
    if (!stopping) console.log(`[${name}] exited (${signal ?? code})`);
  });
  children.push(child);
  return child;
}

async function stop(child) {
  if (!child || child.exitCode != null) return;
  const done = new Promise((resolve) => child.once("exit", resolve));
  child.kill("SIGTERM");
  await Promise.race([done, sleep(4000)]);
  if (child.exitCode == null) child.kill("SIGKILL");
  await Promise.race([done, sleep(1000)]);
}

async function stopAll() {
  stopping = true;
  await Promise.all(children.map((child) => stop(child)));
}

process.on("SIGINT", async () => {
  await stopAll();
  process.exit(130);
});
process.on("exit", () => {
  for (const child of children) {
    if (child.exitCode == null) child.kill("SIGKILL");
  }
});

// Production never seeds interfaces from the environment; make sure the
// caller's shell cannot silently switch this test to the old path.
const cleanEnv = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !key.startsWith("RCHAT_RNS_")),
);

function ensureBinary() {
  const binary = process.env.RCHATD || path.join(ROOT, "target", "debug", "rchatd");
  if (fs.existsSync(binary)) return binary;
  console.log("building rchatd…");
  const built = spawnSync("cargo", ["build", "-q", "-p", "rchatd"], {
    cwd: ROOT,
    stdio: "inherit",
  });
  if (built.status !== 0) process.exit(1);
  return binary;
}

function ensureRnsd() {
  const found = spawnSync("sh", ["-c", "command -v rnsd"], {
    encoding: "utf8",
  }).stdout.trim();
  if (!found) {
    throw new Error("rnsd is required: pip install rns");
  }
  return found;
}

async function waitToken(name) {
  const tokenPath = path.join(DEV, name, "token");
  return until(
    () => {
      try {
        const token = fs.readFileSync(tokenPath, "utf8").trim();
        return token || null;
      } catch {
        return null;
      }
    },
    { timeoutMs: 30000, intervalMs: 100 },
  );
}

async function connectLeaf(name, port) {
  const token = await waitToken(name);
  const base = `http://127.0.0.1:${port}`;
  const api = await until(
    async () => {
      let client;
      try {
        client = connect(base, token);
        await client.hello;
        return client;
      } catch {
        client?.close();
        return null;
      }
    },
    { timeoutMs: 60000, intervalMs: 250 },
  );
  return { name, dir: path.join(DEV, name), base, token, api };
}

/// Render a minimal `rns/config` before first start. Two Reticulum instances
/// on one machine cannot both bind AutoInterface's fixed data port, so each
/// leaf starts from a hand-written relay config (which the engine imports)
/// instead of the first-run discovery row. A real first run has no such
/// same-host conflict.
function seedConfig(name, rows) {
  const dir = path.join(DEV, name, "rns");
  fs.mkdirSync(dir, { recursive: true });
  const lines = [
    "[reticulum]",
    "  enable_transport = No",
    "  share_instance = No",
    "  panic_on_interface_error = No",
    "",
    "[logging]",
    "  loglevel = 3",
    "",
    "[interfaces]",
  ];
  for (const row of rows) {
    lines.push(`  [[${row.name}]]`);
    lines.push(`    type = ${row.kind}`);
    lines.push("    interface_enabled = True");
    for (const [key, value] of Object.entries(row.config ?? {})) {
      lines.push(`    ${key} = ${value}`);
    }
    lines.push("");
  }
  fs.writeFileSync(path.join(dir, "config"), lines.join("\n").trimEnd() + "\n");
}

function launch(label, port, binary, dirName = label) {
  return start(
    label,
    binary,
    ["--data-dir", path.join(DEV, dirName), "--port", String(port)],
    cleanEnv,
  );
}

async function launchLeaf(name, port, binary) {
  fs.mkdirSync(path.join(DEV, name), { recursive: true });
  seedConfig(name, [
    {
      name: "Relay",
      kind: "TCPClientInterface",
      config: { target_host: "127.0.0.1", target_port: RELAY_TCP },
    },
  ]);
  const child = launch(name, port, binary);
  const leaf = await connectLeaf(name, port);
  leaf.child = child;
  // `hello` can be sent before the bridge has registered the address.
  leaf.address = await until(
    async () => {
      const { mesh } = await leaf.api.call({ cmd: "mesh_status" });
      return mesh.address ?? null;
    },
    { timeoutMs: 60000, intervalMs: 250 },
  );
  return leaf;
}

async function restartLeaf(leaf, port, binary) {
  await stop(leaf.child);
  leaf.child = launch(leaf.name + "-restart", port, binary, leaf.name);
  const next = await connectLeaf(leaf.name, port);
  leaf.api.close();
  leaf.api = next.api;
  const address = await until(
    async () => {
      const { mesh } = await next.api.call({ cmd: "mesh_status" });
      return mesh.address ?? null;
    },
    { timeoutMs: 60000, intervalMs: 250 },
  );
  if (address !== leaf.address) {
    throw new Error("a restart changed the leaf's identity");
  }
  return address;
}

async function waitTransport(leaf, timeoutMs = 60000) {
  return until(
    async () => {
      const { mesh } = await leaf.api.call({ cmd: "mesh_status" });
      return mesh.transport === "ready" ? mesh : null;
    },
    { timeoutMs, intervalMs: 500 },
  );
}

async function waitPath(leaf, address, { hops = null, timeoutMs = 90000 } = {}) {
  return until(
    async () => {
      const { mesh } = await leaf.api.call({ cmd: "mesh_status" });
      const found = mesh.peer_paths.find((entry) => entry.address === address);
      if (!found) return null;
      if (hops != null && found.hops !== hops) return null;
      return found;
    },
    { timeoutMs, intervalMs: 500 },
  );
}

async function exchange(from, to, label) {
  const body = `${label} ${Date.now()}`;
  const { conversation } = await from.api.call({
    cmd: "open_direct",
    peer_id: to.address,
  });
  const sent = await from.api.call({
    cmd: "send_message",
    conversation_id: conversation.id,
    body,
  });
  const received = await to.api.waitFor(
    (event) => event.name === "message.new" && event.data.message.body === body,
    60000,
  );
  await from.api.waitFor(
    (event) =>
      event.name === "message.state" &&
      event.data.id === sent.message.id &&
      event.data.state === "delivered",
    60000,
  );
  return { body, received, id: sent.message.id };
}

/// The user-owned part of a peer row. `bandwidth_class` is derived from the
/// current path and legitimately changes when a leaf reconnects.
function peerConfig(peers) {
  return peers.map((peer) => ({
    id: peer.id,
    address: peer.address,
    nickname: peer.nickname,
  }));
}

async function phaseA(a, c) {
  console.log("\n(a) relay delivery with no peer address in either interface list");
  await configureInterfaces(a.api, "relay", `127.0.0.1:${RELAY_TCP}`);
  await configureInterfaces(c.api, "relay", `127.0.0.1:${RELAY_TCP}`);
  await waitTransport(a);
  await waitTransport(c);

  const pathA = await waitPath(a, c.address);
  const pathC = await waitPath(c, a.address);
  if (pathA.hops < 2 || pathC.hops < 2) {
    throw new Error(
      `expected a relayed path, got A->C ${pathA.hops} hops and C->A ${pathC.hops}`,
    );
  }
  console.log(
    `  A sees C at ${pathA.hops} hops via ${pathA.interface}; C sees A at ${pathC.hops}`,
  );

  const rows = await a.api.call({ cmd: "list_interfaces" });
  const configured = JSON.stringify(rows.interfaces ?? []);
  if (configured.includes(c.address) || configured.includes(a.address)) {
    throw new Error("an interface row holds a peer's Reticulum address");
  }

  await exchange(a, c, "rendezvous a");
  console.log("(a) OK: delivered through the relay");
}

async function phaseB(a, c, binary) {
  console.log("\n(b) leaf restart with A's configuration untouched");
  const rowsBefore = JSON.stringify(
    (await a.api.call({ cmd: "list_interfaces" })).interfaces ?? [],
  );
  const peersBefore = JSON.stringify(
    peerConfig((await a.api.call({ cmd: "snapshot" })).peers),
  );
  const sightingBefore =
    (
      await a.api.call({ cmd: "mesh_announces", limit: 500 })
    ).announces.find((row) => row.address === c.address)?.count ?? 0;

  await stop(c.child);
  await restartLeaf(c, PORT_C, binary);
  await waitTransport(c);
  // A's path table may still hold the pre-restart route. Wait for C's fresh
  // announce to be recorded, so the delivery below is not a stale success.
  await until(
    async () => {
      const { announces } = await a.api.call({
        cmd: "mesh_announces",
        limit: 500,
      });
      const row = announces.find((entry) => entry.address === c.address);
      return row && row.count > sightingBefore ? row : null;
    },
    { timeoutMs: 90000, intervalMs: 500 },
  );
  await waitPath(a, c.address);
  await waitPath(c, a.address);

  await exchange(a, c, "rendezvous b");

  const rowsAfter = JSON.stringify(
    (await a.api.call({ cmd: "list_interfaces" })).interfaces ?? [],
  );
  const peersAfter = JSON.stringify(
    peerConfig((await a.api.call({ cmd: "snapshot" })).peers),
  );
  if (rowsAfter !== rowsBefore) {
    throw new Error("A's interface rows changed across C's restart");
  }
  if (peersAfter !== peersBefore) {
    throw new Error("A's stored peer configuration changed across C's restart");
  }
  console.log("(b) OK: delivery resumed; A stored nothing new");
  console.log(
    "    (a real lease change needs two devices; this asserts the stored side is inert)",
  );
}

async function phaseC(a, c) {
  console.log("\n(c) relay removed and local discovery re-enabled");
  // A separate group keeps a running rchat install out of the test. RNS
  // AutoInterface uses one fixed data port per host, so this phase needs the
  // leaves on separate machines (or separate network namespaces).
  const groupId = "rchat-rendezvous";
  await configureInterfaces(a.api, "discovery", null, { groupId });
  await configureInterfaces(c.api, "discovery", null, { groupId });
  await waitTransport(a, 120000);
  await waitTransport(c, 120000);

  let pathA;
  let pathC;
  try {
    pathA = await waitPath(a, c.address, { hops: 1, timeoutMs: 120000 });
    pathC = await waitPath(c, a.address, { hops: 1, timeoutMs: 120000 });
  } catch {
    throw new Error(
      "discovery did not form a direct path. RNS AutoInterface supports one " +
        "instance per host, so phase (c) needs two machines or separate " +
        "network namespaces; run `--phases=a,b` for the one-host phases.",
    );
  }
  console.log(
    `  direct paths: A->C via ${pathA.interface}, C->A via ${pathC.interface}`,
  );

  await exchange(a, c, "rendezvous c");
  console.log("(c) OK: delivered over local discovery");
}

let a;
let c;
try {
  const binary = ensureBinary();
  const rnsd = ensureRnsd();

  fs.rmSync(DEV, { recursive: true, force: true });
  fs.mkdirSync(path.join(DEV, "relay"), { recursive: true });
  fs.writeFileSync(
    path.join(DEV, "relay", "config"),
    `# Generated by scripts/smoke-rendezvous.mjs
[reticulum]
  enable_transport = Yes
  share_instance = No
  panic_on_interface_error = No

[logging]
  loglevel = 3

[interfaces]
  [[Leaves]]
    type = TCPServerInterface
    interface_enabled = True
    listen_ip = 127.0.0.1
    listen_port = ${RELAY_TCP}
`,
  );

  console.log(
    `relay rnsd on 127.0.0.1:${RELAY_TCP}; leaves on ${PORT_A} and ${PORT_C}`,
  );
  start("relay", rnsd, ["--config", path.join(DEV, "relay")], cleanEnv);
  await sleep(2000);

  a = await launchLeaf("a", PORT_A, binary);
  c = await launchLeaf("c", PORT_C, binary);
  console.log("A:", a.address, "| C:", c.address);

  await phaseA(a, c);
  if (runB) await phaseB(a, c, binary);
  if (runC) await phaseC(a, c);

  console.log("\nrendezvous smoke: OK");
} finally {
  try {
    a?.api.close();
    c?.api.close();
  } catch {
    /* already closed */
  }
  await stopAll();
}
