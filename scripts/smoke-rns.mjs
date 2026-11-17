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

// ---- the same link as the Network page sees it ----
const waitForPath = (client, address, timeoutMs) =>
  until(async () => {
    const { mesh } = await client.call({ cmd: "mesh_status" });
    return mesh.peer_paths.some((entry) => entry.address === address) ? mesh : null;
  }, { timeoutMs, intervalMs: 500 });

const meshA = await waitForPath(a, addrB, 30000);
const meshB = await waitForPath(b, addrA, 30000);
const pathA = meshA.peer_paths.find((entry) => entry.address === addrB);
const pathB = meshB.peer_paths.find((entry) => entry.address === addrA);
console.log(
  "mesh: A sees B at",
  pathA.hops,
  "hops via",
  pathA.interface,
  "| B sees A at",
  pathB.hops,
  "hops via",
  pathB.interface,
);
if (pathA.hops < 1 || pathB.hops < 1) {
  throw new Error("a direct link must be at least one hop");
}
const server = meshA.interfaces.find(
  (entry) => entry.kind === "TCPServerInterface" && entry.online,
);
const client = meshB.interfaces.find(
  (entry) => entry.kind === "TCPClientInterface" && entry.online,
);
if (!server) throw new Error("A's TCP server interface is not online");
if (!client) throw new Error("B's TCP client interface is not online");

const byPrefix = await a.call({ cmd: "mesh_paths", query: addrB.slice(0, 8) });
if (!byPrefix.paths.some((entry) => entry.destination === addrB)) {
  throw new Error("mesh_paths did not match the destination prefix");
}
const byInterface = await a.call({ cmd: "mesh_paths", query: server.name });
if (!byInterface.paths.some((entry) => entry.destination === addrB)) {
  throw new Error("mesh_paths did not match the interface name");
}
console.log("mesh paths filter: prefix and interface name");

const { conversation } = await a.call({ cmd: "open_direct", peer_id: addrB });
const body = "reticulum says hello " + Date.now();
await a.call({ cmd: "send_message", conversation_id: conversation.id, body });

const received = await b.waitFor(
  (event) => event.name === "message.new" && event.data.message.body === body,
  45000,
);
console.log("B received:", received.data.message.body, "state:", received.data.message.state);
if (received.data.message.attachments?.length) {
  throw new Error("a plain text message must not carry attachments");
}

const delivered = await a.waitFor(
  (event) => event.name === "message.state" && event.data.state === "delivered",
  45000,
);
console.log("A receipt:", delivered.data.state);

// ---- attachments ----
// One image with a caption, in the standard LXMF image field, over the same
// real link. This is the only check that runs bridge -> LXMF -> Reticulum ->
// bridge, so it is what catches a field shape the engine seam cannot see.
const photo = fs.readFileSync(
  new URL("../apps/desktop/src-tauri/icons/32x32.png", import.meta.url),
);
const caption = "a photo over the mesh " + Date.now();
const uploadUrl =
  baseA.replace(/\/+$/, "") +
  "/attachments?token=" +
  encodeURIComponent(fs.readFileSync(tokenA, "utf8").trim());
const upload = await fetch(uploadUrl, {
  method: "POST",
  headers: {
    "content-type": "image/png",
    "x-rchat-filename": encodeURIComponent("mesh.png"),
  },
  body: photo,
});
if (!upload.ok) {
  throw new Error(`upload failed: ${upload.status} ${await upload.text()}`);
}
const { attachment: uploaded } = await upload.json();
if (!uploaded?.hash) throw new Error("the engine did not accept the image");

const tokenBValue = fs.readFileSync(tokenB, "utf8").trim();
const download = async (base, hash, variant) => {
  const url =
    base.replace(/\/+$/, "") +
    "/attachments/" +
    encodeURIComponent(hash) +
    "?token=" +
    encodeURIComponent(tokenBValue) +
    (variant ? "&variant=thumb" : "");
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`download failed: ${response.status} for ${url}`);
  }
  return Buffer.from(await response.arrayBuffer());
};

// ---- the receiving side sees the image and the same bytes ----
await a.call({
  cmd: "send_message",
  conversation_id: conversation.id,
  body: caption,
  attachments: [uploaded.hash],
});
const arrivedEvent = await b.waitFor(
  (event) =>
    event.name === "message.new" && event.data.message.body === caption,
  45000,
);
const arrived = arrivedEvent.data.message;
if (arrived.kind !== "image") {
  throw new Error("B stored the message as " + arrived.kind + ", not image");
}
if (arrived.attachments?.length !== 1) {
  throw new Error("B stored " + (arrived.attachments?.length ?? 0) + " attachments");
}
const [row] = arrived.attachments;
if (row.hash !== uploaded.hash) {
  throw new Error("hash drifted: " + row.hash + " vs " + uploaded.hash);
}
if (row.size !== photo.length) {
  throw new Error("byte count drifted: " + row.size + " vs " + photo.length);
}
const original = await download(baseB, row.hash);
if (!original.equals(photo)) {
  throw new Error("B's copy of the image differs from A's");
}
if (!row.thumbnail) throw new Error("B did not generate a display thumbnail");
const thumbnail = await download(baseB, row.thumbnail, "thumb");
if (thumbnail.length === 0) throw new Error("B's thumbnail is empty");
if (thumbnail.equals(photo)) {
  throw new Error("the thumbnail is the original, not a resized cache");
}
console.log(
  "attachments: image arrived as",
  row.name,
  row.size,
  "bytes, thumbnail",
  thumbnail.length,
  "bytes",
);

a.close();
b.close();
console.log("rns smoke: OK");
