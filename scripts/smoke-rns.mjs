// Two-daemon Reticulum smoke test.
// Usage: node scripts/smoke-rns.mjs BASE_A TOKEN_A BASE_B TOKEN_B
//
// Expects daemon A to expose a TCP server interface and daemon B to connect to
// it. The dev scripts create those rows through the engine's interface API:
//   node scripts/mesh-config.mjs BASE_A TOKEN_A server 4242
//   node scripts/mesh-config.mjs BASE_B TOKEN_B client 127.0.0.1:4242
import fs from "node:fs";
import { connect, until } from "./lib.mjs";

const [baseA, tokenA, baseB, tokenB] = process.argv.slice(2);
if (!baseA || !tokenA || !baseB || !tokenB) {
  console.error("usage: smoke-rns.mjs BASE_A TOKEN_A BASE_B TOKEN_B");
  process.exit(1);
}

const a = connect(baseA, fs.readFileSync(tokenA, "utf8").trim());
const b = connect(baseB, fs.readFileSync(tokenB, "utf8").trim());

const [helloA, helloB] = await Promise.all([a.hello, b.hello]);
const addrA = helloA.identity.address;
const addrB = helloB.identity.address;
console.log("A:", addrA, "| B:", addrB);

if (!addrA || !addrB) {
  throw new Error("one of the daemons has no Reticulum address yet");
}

// Both sides learn each other through LXMF announces.
await until(async () => {
  const snapA = await a.call({ cmd: "snapshot" });
  const snapB = await b.call({ cmd: "snapshot" });
  const seenByA = snapA.peers.some((p) => p.address === addrB);
  const seenByB = snapB.peers.some((p) => p.address === addrA);
  return seenByA && seenByB ? true : { seenByA, seenByB };
}, { timeoutMs: 40000 });

console.log("announces exchanged");

const { conversation } = await a.call({ cmd: "open_direct", peer_id: addrB });
const body = "reticulum says hello " + Date.now();
await a.call({ cmd: "send_message", conversation_id: conversation.id, body });

const received = await b.waitFor(
  (event) => event.name === "message.new" && event.data.message.body === body,
  45000,
);
console.log("B received:", received.data.message.body, "state:", received.data.message.state);

const delivered = await a.waitFor(
  (event) => event.name === "message.state" && event.data.state === "delivered",
  45000,
);
console.log("A receipt:", delivered.data.state);

a.close();
b.close();
console.log("rns smoke: OK");
