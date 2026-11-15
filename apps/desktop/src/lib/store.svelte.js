import { transportLabel } from "./util.js";
import { boot, isDesktop, invoke, listen } from "./desktop.js";

const BASE_KEY = "rchat.base";
const TOKEN_KEY = "rchat.token";

class ChatStore {
  status = $state(boot ? "starting" : "idle"); // idle | starting | connecting | connected | error
  error = $state(null);
  desktop = isDesktop;
  desktopError = $state(boot?.error ?? null);
  desktopSettings = $state(null);
  base = $state(
    boot?.base || localStorage.getItem(BASE_KEY) || "http://127.0.0.1:8787",
  );
  token = $state(boot?.token || localStorage.getItem(TOKEN_KEY) || "");
  hello = $state(null);
  identity = $state(null);
  transport = $state("offline");
  conversations = $state([]);
  peers = $state([]);
  messages = $state({});
  activeId = $state(null);
  toasts = $state([]);
  lastSeq = $state(0);
  windowFocused = $state(true);
  windowVisible = $state(true);

  socket = null;
  nextId = 1;
  pending = new Map();
  retryTimer = null;
  desktopWired = false;
  activityCount = 0;
  restartGraceUntil = 0;

  get transportLabel() {
    return transportLabel(this.transport);
  }

  get activeConversation() {
    return this.conversations.find((conv) => conv.id === this.activeId) ?? null;
  }

  get activeMessages() {
    return this.messages[this.activeId] ?? [];
  }

  get unreadTotal() {
    return this.conversations.reduce(
      (total, conversation) => total + (conversation.unread ?? 0),
      0,
    );
  }

  get needsName() {
    return Boolean(
      this.desktop &&
        this.status === "connected" &&
        this.identity &&
        !this.identity.display_name_set,
    );
  }

  async initDesktop() {
    if (!this.desktop || this.desktopWired) return;
    this.desktopWired = true;

    listen("window-state", (state) => {
      const wasFocused = this.windowFocused;
      this.windowFocused = Boolean(state.focused);
      this.windowVisible = Boolean(state.visible);
      if (this.windowFocused && this.windowVisible) {
        this.clearActivity();
        if (wasFocused === false && this.activeId) {
          this.call({
            cmd: "mark_read",
            conversation_id: this.activeId,
          }).catch(() => {});
        }
      } else if (this.unreadTotal > 0) {
        this.notifyActivity(this.unreadTotal, false);
      }
    });

    listen("settings-changed", (view) => {
      if (view) this.desktopSettings = view;
    });

    listen("engine-restarted", (payload) => {
      if (payload?.base && payload.base !== this.base) {
        this.base = payload.base;
        this.disconnect();
      }
    });

    window.addEventListener("focus", () => this.handleBrowserFocus(true));
    window.addEventListener("blur", () => this.handleBrowserFocus(false));

    await this.loadDesktopSettings();
  }

  handleBrowserFocus(focused) {
    if (this.desktop) return; // the shell reports the authoritative state
    this.windowFocused = focused;
    if (focused) this.clearActivity();
  }

  async loadDesktopSettings() {
    if (!this.desktop) return;
    try {
      this.desktopSettings = await invoke("get_desktop_settings");
    } catch {
      /* the shell may still be attaching */
    }
  }

  async connect() {
    if (this.status === "connecting" || this.status === "connected") return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    if (!this.base || !this.token) {
      if (this.desktop) {
        this.status = "starting";
        this.desktopError =
          this.desktopError ?? "The local engine is not ready yet.";
        this.scheduleReconnect(1500);
      } else {
        this.fail("The engine address and token are required.");
      }
      return;
    }

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
          6000,
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
      if (this.socket !== socket) return;
      this.socket = null;
      if (this.status === "connected" || this.desktop) {
        // The engine restarted or went away. Reconnect: the address is stable
        // across restarts, and the desktop shell may still be starting one.
        this.status = this.desktop ? "starting" : "idle";
        if (!this.desktop) this.desktopError = this.desktopError;
        this.scheduleReconnect(this.desktop ? 900 : 1200);
      }
    };

    localStorage.setItem(BASE_KEY, this.base);
    localStorage.setItem(TOKEN_KEY, this.token);
    this.status = "connected";
    this.desktopError = null;
    this.restartGraceUntil = 0;

    try {
      const snapshot = await this.call({ cmd: "snapshot" });
      this.identity = snapshot.identity;
      this.transport = snapshot.transport;
      this.conversations = snapshot.conversations;
      this.peers = snapshot.peers;
      const first = snapshot.conversations[0];
      if (this.activeId) await this.loadMessages(this.activeId);
      else if (first) await this.select(first.id);
      if (this.desktop) await this.loadDesktopSettings();
    } catch (err) {
      this.toast(err.message);
    }
  }

  scheduleReconnect(delay) {
    if (this.retryTimer) return;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.connect();
    }, delay);
  }

  async retryEngine() {
    if (!this.desktop) return false;
    this.desktopError = null;
    this.restartGraceUntil = Date.now() + 15000;
    this.status = "starting";
    try {
      const result = await invoke("restart_engine");
      this.applyEngineResult(result);
      this.disconnect();
      this.status = "starting";
      this.scheduleReconnect(250);
      return true;
    } catch (err) {
      this.desktopError = err?.message ?? String(err);
      this.scheduleReconnect(1800);
      return false;
    }
  }

  applyEngineResult(result) {
    if (result?.base) this.base = result.base;
    if (result?.token) this.token = result.token;
    if (result?.settings) this.desktopSettings = result.settings;
    localStorage.setItem(BASE_KEY, this.base);
    localStorage.setItem(TOKEN_KEY, this.token);
  }

  async saveNetwork(autoDiscovery, tcpRelay) {
    if (!this.desktop) return;
    this.restartGraceUntil = Date.now() + 15000;
    const result = await invoke("save_network_settings", {
      autoDiscovery,
      tcpRelay,
    });
    this.applyEngineResult(result);
    this.disconnect();
    this.status = "starting";
    this.scheduleReconnect(250);
  }

  async setMuted(muted) {
    if (!this.desktop) return;
    await invoke("set_muted", { muted });
    if (this.desktopSettings) this.desktopSettings.muted = muted;
  }

  async setAutostart(enabled) {
    if (!this.desktop) return;
    await invoke("set_autostart", { enabled });
    if (this.desktopSettings) this.desktopSettings.autostart = enabled;
  }

  notifyActivity(count, chime) {
    this.activityCount = count;
    if (!this.desktop) return;
    invoke("notify_activity", { count, chime }).catch(() => {});
  }

  clearActivity() {
    if (this.activityCount === 0) {
      if (!this.desktop) return;
    }
    this.activityCount = 0;
    if (!this.desktop) return;
    invoke("clear_activity").catch(() => {});
  }


  fail(message) {
    this.error = message;
    try {
      this.socket?.close();
    } catch {
      /* already closed */
    }
    this.socket = null;
    if (this.desktop) {
      // A shell-started engine may still be coming up. Keep trying instead of
      // parking the user on an error screen.
      this.status = "starting";
      if (Date.now() < this.restartGraceUntil) {
        // Applying network settings restarts the engine on purpose. The socket
        // is expected to drop for a few seconds; stay on the calm "Starting
        // rchat" screen instead of reporting a failure.
        this.scheduleReconnect(500);
        return;
      }
      this.desktopError = message;
      this.scheduleReconnect(1200);
    } else {
      this.status = "error";
    }
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
        const incoming = !this.isMine(data.message);
        const isOpen = data.message.conversation_id === this.activeId;
        const hidden = !this.windowFocused || !this.windowVisible;
        this.appendMessage(data.message);
        if (!incoming) break;
        if (isOpen) {
          // Never alert for the conversation on screen. Mark it read while the
          // window is in front; if hidden, leave it unread until it is shown.
          if (!hidden) {
            this.call({
              cmd: "mark_read",
              conversation_id: this.activeId,
            }).catch(() => {});
          }
          break;
        }
        // A conversation the user cannot see: one coalesced chime, with the
        // shell adding a tray dot and taskbar hint only while hidden.
        this.notifyActivity(Math.max(this.unreadTotal + 1, 1), true);
        break;
      }
      case "message.state": {
        this.updateMessageState(data.id, data.state, data.route);
        break;
      }
      case "conversation.updated": {
        this.upsertConversation(data.conversation);
        if (this.desktop && (!this.windowFocused || !this.windowVisible)) {
          if (this.unreadTotal > 0) {
            this.notifyActivity(this.unreadTotal, false);
          }
        }
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
    if (this.desktop && this.windowFocused && this.windowVisible) {
      this.clearActivity();
    }
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
