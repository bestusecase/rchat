// Three-node mesh smoke test, for the topology scripts/dev-mesh.sh starts.
// Usage: node scripts/smoke-mesh.mjs BASE_A TOKEN_A BASE_C TOKEN_C
//
// Asserts that A sees C two hops away behind the relay, that the path table is
// queryable, that announce history kept the sighting, and that a message
// crosses the relay with neither leaf holding the other's address.
import fs from "node:fs";
import { connect, until } from "./lib.mjs";

const [baseA, tokenA, baseC, tokenC] = process.argv.slice(2);
if (!baseA || !tokenA || !baseC || !tokenC) {
  console.error("usage: smoke-mesh.mjs BASE_A TOKEN_A BASE_C TOKEN_C");
  process.exit(1);
}

function readToken(value) {
  try {
    return fs.readFileSync(value, "utf8").trim();
  } catch {
    return value;
  }
}

const a = connect(baseA, readToken(tokenA));
const c = connect(baseC, readToken(tokenC));

const [helloA, helloC] = await Promise.all([a.hello, c.hello]);
const addressA = helloA.identity.address;
const addressC = helloC.identity.address;
console.log("A:", addressA, "| C:", addressC);
if (!addressA || !addressC) throw new Error("a daemon has no Reticulum address yet");

const mesh = await until(async () => {
  const { mesh } = await a.call({ cmd: "mesh_status" });
  const path = mesh.peer_paths.find((entry) => entry.address === addressC);
  if (!path) return null;
  return path.hops >= 2 && path.next_hop !== addressC ? mesh : null;
}, { timeoutMs: 45000, intervalMs: 500 });

const path = mesh.peer_paths.find((entry) => entry.address === addressC);
console.log(
  "A sees C at",
  path.hops,
  "hops via",
  path.interface,
  "next hop",
  path.next_hop,
);

if (!mesh.interfaces.some((iface) => iface.online)) {
  throw new Error("A reports no online interface");
}
if (
  !mesh.interfaces.some(
    (iface) => iface.kind === "TCPClientInterface" && iface.online,
  )
) {
  throw new Error("A's TCP client interface is not online");
}

// Deliverable, not just routed: both leaves must know a path before A sends.
await until(async () => {
  const { mesh } = await c.call({ cmd: "mesh_status" });
  return mesh.peer_paths.some((entry) => entry.address === addressA);
}, { timeoutMs: 45000, intervalMs: 500 });

const { interfaces } = await a.call({ cmd: "list_interfaces" });
const configured = JSON.stringify(interfaces);
if (configured.includes(addressC) || configured.includes(addressA)) {
  throw new Error("an interface row holds a peer's Reticulum address");
}
console.log("interface rows hold no peer address: ok");

const byPrefix = await a.call({ cmd: "mesh_paths", query: addressC.slice(0, 10), limit: 50 });
if (byPrefix.total !== 1) {
  throw new Error(`mesh_paths prefix query returned ${byPrefix.total} rows`);
}
if (byPrefix.paths[0].destination !== addressC) {
  throw new Error("mesh_paths returned the wrong destination");
}
console.log("mesh_paths filter: ok, name:", byPrefix.paths[0].name ?? "(unknown)");

const byInterface = await a.call({ cmd: "mesh_paths", query: "TCP Client", limit: 50 });
if (!byInterface.paths.some((entry) => entry.destination === addressC)) {
  throw new Error("mesh_paths interface filter did not include C");
}
console.log("mesh_paths interface filter: ok");

const announces = await a.call({ cmd: "mesh_announces", limit: 200 });
const sighting = announces.announces.find((entry) => entry.address === addressC);
if (!sighting) throw new Error("announce history does not contain C");
if (sighting.hops !== path.hops) {
  throw new Error(`announce hops ${sighting.hops} != path hops ${path.hops}`);
}
console.log(
  "announce history: ok, count:",
  sighting.count,
  "interface:",
  sighting.interface,
);

// The whole point of the relay: a message arrives with no peer address in
// either leaf's configuration.
const body = "mesh relay hello " + Date.now();
const { conversation } = await a.call({ cmd: "open_direct", peer_id: addressC });
const sent = await a.call({
  cmd: "send_message",
  conversation_id: conversation.id,
  body,
});
console.log("A sent:", sent.message.id, "state:", sent.message.state);

const received = await c.waitFor(
  (event) =>
    event.name === "message.new" && event.data.message.body === body,
  60000,
);
console.log("C received:", received.data.message.body);

await a.waitFor(
  (event) =>
    event.name === "message.state" &&
    event.data.id === sent.message.id &&
    event.data.state === "delivered",
  60000,
);
console.log("A receipt: delivered");

a.close();
c.close();
console.log("mesh smoke: OK");
