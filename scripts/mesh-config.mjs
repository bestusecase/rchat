// Point a running daemon at a relay, a peer, or back at local discovery.
//
//   node scripts/mesh-config.mjs BASE TOKEN relay 127.0.0.1:4272
//   node scripts/mesh-config.mjs BASE TOKEN server 4242
//   node scripts/mesh-config.mjs BASE TOKEN client 127.0.0.1:4242
//   node scripts/mesh-config.mjs BASE TOKEN discovery
//
// The production model is store-owned: interfaces are rows the engine renders
// into `rns/config`, and environment seeding is off. The dev scripts use this
// helper instead of RCHAT_RNS_* so they exercise the same path a user does.
//
// Exported for scripts/smoke-rendezvous.mjs.
import fs from "node:fs";
import { pathToFileURL } from "node:url";
import { connect, until } from "./lib.mjs";

const DISCOVERY = "AutoInterface";
const RENDEZVOUS_NAMES = new Set(["Relay", "Peer", "TCP Client", "TCP Server"]);

export function readToken(value) {
  try {
    return fs.readFileSync(value, "utf8").trim();
  } catch {
    return value;
  }
}

export function splitHostPort(value) {
  const text = String(value ?? "").trim();
  const at = text.lastIndexOf(":");
  if (at <= 0) throw new Error(`expected host:port, got "${value}"`);
  const host = text.slice(0, at).trim();
  const port = Number(text.slice(at + 1));
  if (!host) throw new Error(`expected host:port, got "${value}"`);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`port must be between 1 and 65535, got "${value}"`);
  }
  return { host, port };
}

async function interfaces(api) {
  const data = await api.call({ cmd: "list_interfaces" });
  return data.interfaces ?? [];
}

/// Wait until the bridge exists. `degraded` counts: a bridge with nothing
/// online is exactly when an interface edit must be allowed.
export async function waitForBridge(api, timeoutMs = 60000) {
  return until(
    async () => {
      const { mesh } = await api.call({ cmd: "mesh_status" });
      return mesh.transport === "ready" || mesh.transport === "degraded"
        ? mesh
        : null;
    },
    { timeoutMs, intervalMs: 500 },
  );
}

async function create(api, name, kind, config) {
  await api.call({ cmd: "create_interface", name, kind, config }, 30000);
}

async function setEnabled(api, row, enabled) {
  if (row.enabled === enabled) return;
  await api.call(
    { cmd: "set_interface_enabled", interface_id: row.id, enabled },
    30000,
  );
}

async function ensure(api, name, kind, config) {
  const existing = (await interfaces(api)).find((row) => row.name === name);
  if (!existing) {
    await create(api, name, kind, config);
    return;
  }
  if (existing.kind !== kind) {
    throw new Error(
      `"${name}" exists as ${existing.kind}, not ${kind}; delete it first`,
    );
  }
  if (JSON.stringify(existing.config) !== JSON.stringify(config)) {
    await api.call(
      { cmd: "update_interface", interface_id: existing.id, config },
      30000,
    );
  }
  await setEnabled(api, existing, true);
}

async function setDiscovery(api, enabled) {
  const rows = await interfaces(api);
  const auto = rows.filter((row) => row.kind === DISCOVERY);
  if (!auto.length) {
    if (!enabled) return;
    // A store that predates the first-run seed: add the row rather than
    // relying on an environment variable.
    await create(api, DISCOVERY, DISCOVERY, { group_id: "reticulum" });
    return;
  }
  for (const row of auto) await setEnabled(api, row, enabled);
}

/// Apply one topology role. Idempotent, so a restarted script can re-run it.
export async function configureInterfaces(api, role, target) {
  await waitForBridge(api);

  if (role === "discovery") {
    // Bring discovery up first so the mesh is never left with zero paths.
    await setDiscovery(api, true);
    for (const row of await interfaces(api)) {
      if (RENDEZVOUS_NAMES.has(row.name)) {
        await api.call(
          { cmd: "delete_interface", interface_id: row.id },
          30000,
        );
      }
    }
    return role;
  }

  let name;
  let kind;
  let config;
  if (role === "relay" || role === "client") {
    const { host, port } = splitHostPort(target);
    name = role === "relay" ? "Relay" : "Peer";
    kind = "TCPClientInterface";
    config = { target_host: host, target_port: port };
  } else if (role === "server") {
    const port = Number(target);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new Error(`server needs a port between 1 and 65535, got "${target}"`);
    }
    name = "TCP Server";
    kind = "TCPServerInterface";
    config = { listen_ip: "127.0.0.1", listen_port: port };
  } else {
    throw new Error(`unknown role "${role}"`);
  }

  // Add the path before taking discovery down: never zero interfaces by
  // accident, even for a moment.
  await ensure(api, name, kind, config);
  // Keeping AutoInterface as a disabled row (rather than deleting it) is what
  // the UI shows a user who turned discovery off.
  await setDiscovery(api, false);
  return role;
}

const isMain =
  process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  const [base, tokenArg, role, target] = process.argv.slice(2);
  if (!base || !tokenArg || !role) {
    console.error(
      "usage: mesh-config.mjs BASE TOKEN relay HOST:PORT | server PORT | client HOST:PORT | discovery",
    );
    process.exit(1);
  }
  const api = connect(base, readToken(tokenArg));
  try {
    await api.hello;
    await configureInterfaces(api, role, target);
    const { mesh } = await api.call({ cmd: "mesh_status" });
    const live = (mesh.interfaces ?? [])
      .filter((row) => row.online)
      .map((row) => row.name)
      .join(", ");
    console.log(
      `mesh-config: ${role} on ${base} (transport ${mesh.transport}, online: ${
        live || "none"
      })`,
    );
  } finally {
    api.close();
  }
}
