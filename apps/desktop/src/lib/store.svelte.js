import { transportLabel } from "./util.js";

const BASE_KEY = "rchat.base";
const TOKEN_KEY = "rchat.token";

class ChatStore {
  status = $state("idle"); // idle | connecting | connected | error
  error = $state(null);
  base = $state(localStorage.getItem(BASE_KEY) || "http://127.0.0.1:8787");
  token = $state(localStorage.getItem(TOKEN_KEY) || "");
  hello = $state(null);
  identity = $state(null);
  transport = $state("offline");
  conversations = $state([]);
  peers = $state([]);
  messages = $state({});
  activeId = $state(null);
  toasts = $state([]);
  lastSeq = $state(0);

  socket = null;
  nextId = 1;
  pending = new Map();

  get transportLabel() {
    return transportLabel(this.transport);
  }

  get activeConversation() {
    return this.conversations.find((conv) => conv.id === this.activeId) ?? null;
  }

  get activeMessages() {
    return this.messages[this.activeId] ?? [];
  }

  async connect() {
    if (this.status === "connecting") return;
    this.status = "connecting";
    this.error = null;

    const wsBase = this.base.trim().replace(/\/+$/, "").replace(/^http/, "ws");
    const url = `${wsBase}/ws?token=${encodeURIComponent(this.token.trim())}`;

    let socket;
    try {
      socket = new WebSocket(url);
    } catch {
      this.fail("That engine address does not look valid.");
      return;
    }
    this.socket = socket;
    socket.onmessage = (event) => this.receive(event.data);

    try {
      await new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(new Error("The engine did not answer in time.")),
          5000,
        );
        socket.onopen = () => {
          clearTimeout(timer);
          resolve();
        };
        socket.onerror = () => {
          clearTimeout(timer);
          reject(new Error("Could not reach the engine at " + this.base));
        };
        socket.onclose = () => {
          clearTimeout(timer);
          reject(new Error("The engine closed the connection."));
        };
      });
    } catch (err) {
      this.fail(err.message);
      return;
    }

    socket.onclose = () => {
      if (this.status === "connected") {
        // The engine restarted or went away. Try once more shortly; if that
        // fails the connect screen explains what happened.
        this.status = "idle";
        setTimeout(() => this.connect(), 900);
      }
    };

    localStorage.setItem(BASE_KEY, this.base);
    localStorage.setItem(TOKEN_KEY, this.token);
    this.status = "connected";

    try {
      const snapshot = await this.call({ cmd: "snapshot" });
      this.identity = snapshot.identity;
      this.transport = snapshot.transport;
      this.conversations = snapshot.conversations;
      this.peers = snapshot.peers;
      const first = snapshot.conversations[0];
      if (this.activeId) await this.loadMessages(this.activeId);
      else if (first) await this.select(first.id);
    } catch (err) {
      this.toast(err.message);
    }
  }

  fail(message) {
    this.status = "error";
    this.error = message;
    try {
      this.socket?.close();
    } catch {
      /* already closed */
    }
    this.socket = null;
  }

  disconnect() {
    this.socket?.close();
    this.socket = null;
    this.status = "idle";
  }

  receive(raw) {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }

    if (message.type === "hello") {
      this.hello = message;
      this.identity = message.identity;
      this.transport = message.transport;
      return;
    }

    if (message.type === "reply") {
      const waiter = this.pending.get(message.id);
      if (waiter) {
        this.pending.delete(message.id);
        if (message.ok) waiter.resolve(message.data);
        else waiter.reject(new Error(message.error || "command failed"));
      }
      return;
    }

    if (message.type === "event") {
      if (message.seq > this.lastSeq) this.lastSeq = message.seq;
      this.applyEvent(message.name, message.data);
    }
  }

  applyEvent(name, data) {
    switch (name) {
      case "message.new": {
        this.appendMessage(data.message);
        if (data.message.conversation_id === this.activeId && !this.isMine(data.message)) {
          this.call({ cmd: "mark_read", conversation_id: this.activeId }).catch(() => {});
        }
        break;
      }
      case "message.state": {
        this.updateMessageState(data.id, data.state, data.route);
        break;
      }
      case "conversation.updated": {
        this.upsertConversation(data.conversation);
        break;
      }
      case "peer.updated": {
        this.upsertPeer(data.peer);
        break;
      }
      case "identity.updated": {
        this.identity = data.identity;
        break;
      }
      case "transport.state": {
        this.transport = data.transport;
        if (data.address && this.identity) {
          this.identity = { ...this.identity, address: data.address };
        }
        if (data.detail) this.toast(data.detail);
        break;
      }
      case "transport.error": {
        this.toast(data.message);
        break;
      }
      default:
        break;
    }
  }

  call(command, timeoutMs = 20000) {
    if (!this.socket || this.socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("Not connected to the engine."));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("The engine took too long to answer."));
      }, timeoutMs);
      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timer);
          resolve(value);
        },
        reject: (err) => {
          clearTimeout(timer);
          reject(err);
        },
      });
      this.socket.send(JSON.stringify({ id, ...command }));
    });
  }

  appendMessage(message) {
    const existing = this.messages[message.conversation_id] ?? [];
    if (existing.some((item) => item.id === message.id)) return;
    const next = [...existing, message].sort((a, b) => a.ts - b.ts);
    this.messages = { ...this.messages, [message.conversation_id]: next };
  }

  updateMessageState(id, state, route) {
    let changed = false;
    const next = {};
    for (const [conversationId, list] of Object.entries(this.messages)) {
      next[conversationId] = list.map((message) => {
        if (message.id !== id) return message;
        changed = true;
        return { ...message, state, route: route ?? message.route };
      });
    }
    if (changed) this.messages = next;
  }

  upsertConversation(conversation) {
    const list = this.conversations.slice();
    const index = list.findIndex((item) => item.id === conversation.id);
    if (index >= 0) list[index] = conversation;
    else list.unshift(conversation);
    list.sort(
      (a, b) => (b.last_message_at ?? b.created_at) - (a.last_message_at ?? a.created_at),
    );
    this.conversations = list;
  }

  upsertPeer(peer) {
    const list = this.peers.slice();
    const index = list.findIndex((item) => item.id === peer.id || item.address === peer.address);
    if (index >= 0) list[index] = { ...list[index], ...peer };
    else list.push(peer);
    list.sort((a, b) => (b.last_seen ?? 0) - (a.last_seen ?? 0));
    this.peers = list;
  }

  async select(conversationId) {
    this.activeId = conversationId;
    const conversation = this.conversations.find((item) => item.id === conversationId);
    if (conversation) conversation.unread = 0;
    if (!this.messages[conversationId]) {
      await this.loadMessages(conversationId);
    }
    this.call({ cmd: "mark_read", conversation_id: conversationId }).catch(() => {});
  }

  async loadMessages(conversationId) {
    try {
      const data = await this.call({
        cmd: "list_messages",
        conversation_id: conversationId,
        limit: 100,
      });
      this.messages = { ...this.messages, [conversationId]: data.messages };
    } catch (err) {
      this.toast(err.message);
    }
  }

  async sendMessage(body) {
    const text = body.trim();
    if (!text || !this.activeId) return;
    await this.call({
      cmd: "send_message",
      conversation_id: this.activeId,
      body: text,
    });
  }

  async addPeer(address, nickname) {
    const { peer } = await this.call({
      cmd: "add_peer",
      address: address.trim(),
      nickname: nickname?.trim() ? nickname.trim() : null,
    });
    const { conversation } = await this.call({ cmd: "open_direct", peer_id: peer.id });
    await this.select(conversation.id);
    return conversation;
  }

  async setDisplayName(name) {
    const { identity } = await this.call({ cmd: "set_display_name", name });
    this.identity = identity;
  }

  isMine(message) {
    return message?.author_id && message.author_id === this.identity?.id;
  }

  peerLabel(peerId) {
    if (!peerId) return "unknown";
    const peer = this.peers.find((item) => item.id === peerId || item.address === peerId);
    return peer?.nickname || peer?.announced_name || peerId.slice(0, 8);
  }

  conversationTitle(conversation) {
    if (!conversation) return "";
    if (conversation.kind === "group") return conversation.title || "Group";
    const other = conversation.peer_ids?.[0];
    return other ? this.peerLabel(other) : "Empty conversation";
  }

  conversationAddress(conversation) {
    if (!conversation || conversation.kind !== "direct") return null;
    const other = conversation.peer_ids?.[0];
    const peer = this.peers.find((item) => item.id === other || item.address === other);
    return peer?.address ?? null;
  }

  conversationUnread(conversation) {
    return conversation?.unread ?? 0;
  }

  toast(message) {
    if (!message) return;
    const id = Math.random().toString(36).slice(2);
    this.toasts = [...this.toasts, { id, message }];
    setTimeout(() => {
      this.toasts = this.toasts.filter((item) => item.id !== id);
    }, 6000);
  }
}

export const chat = new ChatStore();
