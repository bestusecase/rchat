// Offline smoke test: identity, peers, conversations, messages, search.
// Usage: node scripts/smoke-offline.mjs http://127.0.0.1:8791 /tmp/rchat-t1/token
import fs from "node:fs";
import { connect } from "./lib.mjs";

const [base, tokenPath] = process.argv.slice(2);
if (!base || !tokenPath) {
  console.error("usage: smoke-offline.mjs BASE TOKEN_FILE");
  process.exit(1);
}

const token = fs.readFileSync(tokenPath, "utf8").trim();
const client = connect(base, token);

const hello = await client.hello;
console.log("hello:", hello.version, "transport:", hello.transport);

const snapshot = await client.call({ cmd: "snapshot" });
console.log("snapshot:", snapshot.conversations.length, "conversations,", snapshot.peers.length, "peers");

const { peer } = await client.call({
  cmd: "add_peer",
  address: "ab".repeat(16),
  nickname: "smoke-peer",
});
console.log("peer added:", peer.id, peer.nickname);

const { conversation } = await client.call({ cmd: "open_direct", peer_id: peer.id });
console.log("conversation:", conversation.id);

const sent = await client.call({
  cmd: "send_message",
  conversation_id: conversation.id,
  body: "hello from the smoke test",
});
console.log("send state:", sent.message.state);

const list = await client.call({ cmd: "list_messages", conversation_id: conversation.id });
console.log("messages:", list.messages.map((m) => m.body));

const found = await client.call({ cmd: "search", query: "smoke" });
console.log("search hits:", found.messages.length);

await client.call({ cmd: "mark_read", conversation_id: conversation.id });
client.close();
console.log("offline smoke: OK");
