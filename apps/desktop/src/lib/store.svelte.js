import { peerReachable, shortAddress, transportLabel } from "./util.js";
import { boot, hasTauri, isDesktop, invoke, listen, loadBoot } from "./desktop.js";

const BASE_KEY = "rchat.base";
const TOKEN_KEY = "rchat.token";
/// How long a peer-card draft waits for the next keystroke before it is saved.
const PEER_CARD_SAVE_DELAY_MS = 500;

/// A closed card. Drafts live here so they can be debounced and flushed.
function emptyPeerCard() {
  return {
    peerId: null,
    nickname: "",
    notes: "",
    editing: false,
    save: "idle", // idle | saving | saved | error
  };
}

class ChatStore {
  status = $state(boot ? "starting" : "idle"); // idle | starting | connecting | connected | error
  error = $state(null);
  // web | desktop | mobile. `desktop` stays a plain boolean: it gates features
  // that only exist in the desktop shell, while `shell` means "native host".
  platform = $state(isDesktop ? "desktop" : "web");
  booted = $state(!hasTauri || isDesktop);
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
  /// The peer profile card. Drafts live here, so a write can be debounced and
  /// flushed on close; the peer itself is always read live from `peers`.
  peerCard = $state(emptyPeerCard());
  // The Reticulum surface. `mesh` is the engine's last known picture and stays
  // visible while the bridge is down; tabs fetch their own larger lists.
  mesh = $state(null);
  meshUpdatedAt = $state(0);
  meshPaths = $state({
    rows: [],
    total: 0,
    truncated: false,
    query: "",
    loading: false,
    error: null,
  });
  meshAnnounces = $state([]);
  /// Announce events waiting for the next batched render.
  pendingAnnounces = new Map();
  announceTimer = null;
  announcesLoading = $state(false);
  networkPane = $state(false);
  networkTab = $state("overview");
  transportDetail = $state(null);
  interfaces = $state([]);
  interfaceExtra = $state([]);
  /// Why interface edits are unavailable, straight from the bridge.
  interfaceReadOnly = $state(null);
  interfaceTypes = $state(null);
  devices = $state({ serial: [], reason: null, loaded: false });
  windowFocused = $state(true);
  windowVisible = $state(true);
  keyboardInset = $state(0);
  update = $state({
    status: "idle", // idle | checking | current | available | downloading | ready | error
    current: "",
    latest: null,
    notes: null,
    downloaded: 0,
    total: null,
    installKind: null, // appimage | deb | other
    autoUpdates: false,
    pendingRestart: false,
    packageVersion: null,
    error: null,
  });

  socket = null;
  nextId = 1;
  pending = new Map();
  retryTimer = null;
  updateTimer = null;
  peerSaveTimer = null;
  /// Pending notes writes, chained so they cannot reorder and so closing can
  /// wait for the last keystroke to land.
  peerNotesChain = null;
  desktopWired = false;
  mobileWired = false;
  bootWired = false;
  restartGraceUntil = 0;

  get shell() {
    return this.platform !== "web";
  }

  get mobile() {
    return this.platform === "mobile";
  }

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

  get meshTransport() {
    return this.mesh?.transport ?? this.transport;
  }

  /// Edits are legal while the bridge is up, even when no interface is online:
  /// a degraded mesh is exactly when a user needs to fix an interface.
  get meshUp() {
    return this.meshTransport === "ready" || this.meshTransport === "degraded";
  }

  get needsName() {
    return Boolean(
      this.shell &&
        this.status === "connected" &&
        this.identity &&
        !this.identity.display_name_set,
    );
  }

  /// Resolve the shell this page is running in and wire its native hooks.
  ///
  /// Desktop answers synchronously (its boot blob is injected); mobile has to
  /// await the `boot` command because a foreground service owns the engine.
  /// Idempotent: `onMount` may run again after a webview reload.
  async bootShell() {
    if (this.bootWired) return;
    this.bootWired = true;

    if (this.desktop) {
      await this.initDesktop();
      this.booted = true;
      return;
    }

    if (!hasTauri) {
      this.booted = true;
      return;
    }

    let info = null;
    try {
      info = await loadBoot();
    } catch {
      /* the command is missing or the shell refused to answer */
    }

    if (info?.role === "mobile") {
      this.platform = "mobile";
      this.initMobile();
      this.applyBoot(info);
      this.status = "starting";
      if (info.error) this.desktopError = info.error;
      else await this.loadDesktopSettings();
    }
    this.booted = true;
  }

  applyBoot(info) {
    if (info?.base) this.base = info.base;
    if (info?.token) this.token = info.token;
    if (info?.error) this.desktopError = info.error;
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

    listen("update-progress", (data) => {
      if (this.update.status !== "downloading") return;
      this.update.downloaded = data?.downloaded ?? 0;
      this.update.total = data?.total ?? null;
    });

    listen("update-installed", () => {
      this.update.pendingRestart = true;
    });

    window.addEventListener("focus", () => this.handleBrowserFocus(true));
    window.addEventListener("blur", () => this.handleBrowserFocus(false));

    await this.loadDesktopSettings();
    await this.loadUpdateState();
    this.scheduleUpdateCheck();
  }

  handleBrowserFocus(focused) {
    if (this.desktop) return; // the shell reports the authoritative state
    this.windowFocused = focused;
    if (focused) this.clearActivity();
  }

  /// Mobile visibility, keyboard and wake-up wiring.
  ///
  /// Android has no window-state event: the WebView reports pause/resume
  /// through `visibilitychange` and focus/blur. The soft keyboard shrinks the
  /// visual viewport, so its height feeds a CSS variable the composer uses to
  /// stay above it.
  initMobile() {
    if (this.mobileWired) return;
    this.mobileWired = true;
    document.documentElement.classList.add("mobile");

    const syncVisibility = () => {
      const hidden = document.visibilityState === "hidden";
      this.windowVisible = !hidden;
      if (hidden) {
        this.windowFocused = false;
        if (this.unreadTotal > 0) this.notifyActivity(this.unreadTotal, false);
        return;
      }
      this.windowFocused = document.hasFocus();
      this.clearActivity();
      if (this.activeId) {
        this.call({
          cmd: "mark_read",
          conversation_id: this.activeId,
        }).catch(() => {});
      }
    };

    document.addEventListener("visibilitychange", syncVisibility);
    window.addEventListener("focus", () => {
      this.windowFocused = true;
      this.windowVisible = document.visibilityState !== "hidden";
      this.clearActivity();
    });
    window.addEventListener("blur", () => {
      this.windowFocused = false;
    });

    const viewport = window.visualViewport;
    if (viewport) {
      const syncKeyboard = () => {
        // With `interactive-widget=resizes-content` the layout viewport already
        // shrinks and this is zero; older WebViews only shrink the visual one.
        const inset = Math.round(
          window.innerHeight - viewport.height - viewport.offsetTop,
        );
        this.keyboardInset = inset > 40 ? inset : 0;
        document.documentElement.style.setProperty(
          "--kb-inset",
          this.keyboardInset + "px",
        );
      };
      viewport.addEventListener("resize", syncKeyboard);
      viewport.addEventListener("scroll", syncKeyboard);
      syncKeyboard();
    }

    syncVisibility();
  }

  async loadDesktopSettings() {
    if (!this.shell) return;
    try {
      this.desktopSettings = await invoke("get_desktop_settings");
    } catch {
      /* the shell may still be attaching */
    }
  }

  async loadUpdateState() {
    if (!this.desktop) return;
    try {
      const state = await invoke("update_state");
      this.update.current = state?.runningVersion ?? "";
      this.update.installKind = state?.installKind ?? null;
      this.update.autoUpdates = Boolean(state?.autoUpdates);
      this.update.pendingRestart = Boolean(state?.pendingRestart);
      this.update.packageVersion = state?.packageVersion ?? null;
      if (this.update.pendingRestart) this.update.status = "ready";
    } catch {
      /* older shell or the command is unavailable */
    }
  }

  scheduleUpdateCheck() {
    if (!this.desktop || this.updateTimer) return;
    const tick = () => {
      // Deb installs are updated by their apt timer, never by the app.
      if (this.update.installKind === "appimage") this.checkForUpdate(false);
    };
    setTimeout(tick, 8000);
    this.updateTimer = setInterval(tick, 6 * 60 * 60 * 1000);
  }

  async checkForUpdate(manual = false) {
    if (!this.desktop || !this.update.installKind) return;
    if (this.update.installKind !== "appimage") {
      if (manual) this.toast("Debian installs update themselves from the rchat apt repository.");
      return;
    }
    if (this.update.status === "checking" || this.update.status === "downloading") return;
    this.update.status = "checking";
    this.update.error = null;
    try {
      const result = await invoke("check_for_update");
      if (result?.current) this.update.current = result.current;
      if (result?.available) {
        this.update.status = "available";
        this.update.latest = result.latest;
        this.update.notes = result.notes;
      } else {
        this.update.status = "current";
        this.update.latest = null;
        this.update.notes = null;
        if (manual) this.toast("rchat is up to date.");
      }
    } catch (err) {
      this.update.status = "error";
      this.update.error = err?.message ?? String(err);
      // A background check stays silent; a manual one explains itself.
      if (manual) this.toast("Could not check for updates: " + this.update.error);
    }
  }

  async installUpdate() {
    if (!this.desktop || this.update.status !== "available") return;
    this.update.status = "downloading";
    this.update.downloaded = 0;
    this.update.total = null;
    try {
      await invoke("install_update");
      this.update.status = "ready";
      this.update.pendingRestart = true;
      this.toast(`rchat ${this.update.latest} is ready. Restart to finish.`);
    } catch (err) {
      this.update.status = "error";
      this.update.error = err?.message ?? String(err);
      this.toast("Update failed: " + this.update.error);
    }
  }

  async restartForUpdate() {
    if (!this.desktop) return;
    try {
      await invoke("restart_for_update");
    } catch (err) {
      this.toast(err?.message ?? String(err));
    }
  }

  async connect() {
    if (this.status === "connecting" || this.status === "connected") return;
    if (this.retryTimer) {
      clearTimeout(this.retryTimer);
      this.retryTimer = null;
    }
    if (!this.base || !this.token) {
      if (this.shell) {
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
      if (this.status === "connected" || this.shell) {
        // The engine restarted or went away. Reconnect: the address is stable
        // across restarts, and the shell may still be starting one.
        this.status = this.shell ? "starting" : "idle";
        this.scheduleReconnect(this.shell ? 900 : 1200);
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
      if (snapshot.mesh) {
        this.mesh = snapshot.mesh;
        this.meshUpdatedAt = Date.now();
      }
      const first = snapshot.conversations[0];
      if (this.activeId) await this.loadMessages(this.activeId);
      // A phone opens on the conversation list, not on whichever thread
      // happens to be newest.
      else if (first && !this.mobile) await this.select(first.id);
      if (this.shell) await this.loadDesktopSettings();
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
    if (this.mobile) {
      // The engine belongs to a foreground service on Android; asking it again
      // is enough. `boot` waits for the service endpoint to appear.
      this.desktopError = null;
      this.status = "starting";
      try {
        const info = await loadBoot();
        this.applyBoot(info);
        if (!info?.base || !info?.token) {
          this.desktopError = info?.error ?? "The local engine is not ready yet.";
          return false;
        }
        this.disconnect();
        this.status = "starting";
        this.scheduleReconnect(250);
        return true;
      } catch (err) {
        this.desktopError = err?.message ?? String(err);
        return false;
      }
    }
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

  async setMuted(muted) {
    if (!this.shell) return;
    await invoke("set_muted", { muted });
    if (this.desktopSettings) this.desktopSettings.muted = muted;
  }

  async openBackgroundSettings() {
    if (!this.mobile) return;
    await invoke("open_background_settings");
  }

  async setAutostart(enabled) {
    if (!this.desktop) return;
    await invoke("set_autostart", { enabled });
    if (this.desktopSettings) this.desktopSettings.autostart = enabled;
  }

  notifyActivity(count, chime) {
    // The phone cannot post from here: a backgrounded webview is suspended, so
    // its JavaScript never sees the event at all. The engine raises message
    // notifications from Rust instead, and only the desktop shell needs this.
    if (!this.desktop) return;
    invoke("notify_activity", { count, chime }).catch(() => {});
  }

  clearActivity() {
    if (!this.shell) return;
    // Unconditional on mobile: the notification was posted by the shell, so
    // this page has no reliable counter to skip the call with.
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
    if (this.shell) {
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
    // Last known mesh data is deliberately kept for the offline banner; the
    // lists are refetched on reconnect.
    this.meshPaths = { rows: [], total: 0, truncated: false, query: "", loading: false, error: null };
    this.meshAnnounces = [];
    this.pendingAnnounces = new Map();
    if (this.announceTimer) clearTimeout(this.announceTimer);
    this.announceTimer = null;
    this.interfaces = [];
    this.interfaceExtra = [];
    this.interfaceReadOnly = null;
    this.interfaceTypes = null;
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
      case "mesh.updated": {
        if (data?.mesh) {
          this.mesh = data.mesh;
          this.meshUpdatedAt = Date.now();
        }
        break;
      }
      case "announce.received": {
        this.upsertAnnounce(data.announce);
        break;
      }
      case "interface.changed": {
        this.loadInterfaces().catch(() => {});
        break;
      }
      case "interface.apply_failed": {
        const name = data.name ? `"${data.name}"` : "An interface";
        this.toast(`${name} was not applied: ${data.error ?? "Reticulum refused it"}`);
        break;
      }
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
      case "attachment.evicted": {
        this.markAttachmentMissing(
          data.hash,
          data.reason ?? "Attachment removed to save space",
        );
        break;
      }
      case "conversation.updated": {
        this.upsertConversation(data.conversation);
        if (this.shell && (!this.windowFocused || !this.windowVisible)) {
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
        this.transportDetail = data.detail ?? null;
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

  /// The engine dropped a blob. Replace every mention with its reason so the
  /// bubble says so instead of showing a broken image.
  markAttachmentMissing(hash, reason) {
    if (!hash) return;
    let changed = false;
    const next = {};
    for (const [conversationId, list] of Object.entries(this.messages)) {
      next[conversationId] = list.map((message) => {
        const attachments = message.attachments ?? [];
        if (!attachments.some((item) => item.hash === hash)) return message;
        changed = true;
        return {
          ...message,
          attachments: attachments.map((item) =>
            item.hash === hash
              ? { ...item, hash: "", thumbnail: null, reason }
              : item,
          ),
        };
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

  // ---- Reticulum surface --------------------------------------------------

  openNetwork(tab = "overview") {
    if (this.mobile) this.activeId = null;
    this.networkPane = true;
    this.networkTab = tab;
  }

  closeNetwork() {
    this.networkPane = false;
  }

  async openPeer(peerId) {
    const { conversation } = await this.call({ cmd: "open_direct", peer_id: peerId });
    this.networkPane = false;
    await this.select(conversation.id);
  }

  async requestMeshStatus() {
    const { mesh } = await this.call({ cmd: "mesh_status" });
    if (mesh) {
      this.mesh = mesh;
      this.meshUpdatedAt = Date.now();
    }
    return mesh;
  }

  async loadMeshPaths({ reset = false, query = this.meshPaths.query } = {}) {
    const offset = reset ? 0 : this.meshPaths.rows.length;
    this.meshPaths = { ...this.meshPaths, loading: true, error: null, query };
    try {
      const data = await this.call({
        cmd: "mesh_paths",
        query: query || null,
        offset,
        limit: 100,
      });
      this.meshPaths = {
        rows: reset ? data.paths : [...this.meshPaths.rows, ...data.paths],
        total: data.total,
        truncated: Boolean(data.truncated),
        query,
        loading: false,
        error: null,
      };
    } catch (err) {
      this.meshPaths = {
        ...this.meshPaths,
        loading: false,
        error: err?.message ?? String(err),
      };
    }
  }

  setMeshPathQuery(query) {
    this.meshPaths = { ...this.meshPaths, rows: [], total: 0, query };
  }

  /// Refresh the rows already on screen without collapsing an infinite scroll
  /// back to the first page.
  async refreshMeshPaths() {
    const limit = Math.min(500, Math.max(100, this.meshPaths.rows.length));
    try {
      const data = await this.call({
        cmd: "mesh_paths",
        query: this.meshPaths.query || null,
        offset: 0,
        limit,
      });
      this.meshPaths = {
        ...this.meshPaths,
        rows: data.paths,
        total: data.total,
        truncated: Boolean(data.truncated),
        error: null,
      };
    } catch (err) {
      this.meshPaths = {
        ...this.meshPaths,
        error: err?.message ?? String(err),
      };
    }
  }

  async loadAnnounces({ reset = false } = {}) {
    const before = reset
      ? null
      : this.meshAnnounces[this.meshAnnounces.length - 1]?.last_seen;
    if (this.announcesLoading) return;
    this.announcesLoading = true;
    try {
      const data = await this.call({
        cmd: "mesh_announces",
        before: before ?? null,
        limit: 100,
      });
      this.meshAnnounces = reset
        ? data.announces
        : [...this.meshAnnounces, ...data.announces];
    } catch (err) {
      this.toast(err?.message ?? String(err));
    } finally {
      this.announcesLoading = false;
    }
  }

  upsertAnnounce(announce) {
    if (!announce?.address) return;
    this.pendingAnnounces.set(`${announce.address}:${announce.bucket}`, announce);
    if (this.announceTimer) return;
    // Announce storms arrive in bursts; batch them into one render.
    this.announceTimer = setTimeout(() => {
      this.announceTimer = null;
      this.flushAnnounces();
    }, 250);
  }

  flushAnnounces() {
    if (!this.pendingAnnounces.size) return;
    const pending = this.pendingAnnounces;
    this.pendingAnnounces = new Map();
    const list = this.meshAnnounces.slice();
    const index = new Map(
      list.map((item, at) => [`${item.address}:${item.bucket}`, at]),
    );
    for (const [key, announce] of pending) {
      const at = index.get(key);
      if (at != null) list[at] = announce;
      else list.unshift(announce);
    }
    list.sort((a, b) => (b.last_seen ?? 0) - (a.last_seen ?? 0));
    this.meshAnnounces = list.slice(0, 500);
  }

  async loadInterfaces() {
    try {
      const data = await this.call({ cmd: "list_interfaces" });
      this.interfaces = data.interfaces ?? [];
      this.interfaceExtra = data.extra ?? [];
      this.interfaceReadOnly = data.read_only ?? null;
    } catch (err) {
      // Offline: keep whatever we had and let the page explain why edits fail.
      if (this.transport === "ready") this.toast(err?.message ?? String(err));
    }
  }

  async describeInterfaces() {
    if (this.interfaceTypes) return this.interfaceTypes;
    const data = await this.call({ cmd: "describe_interfaces" });
    this.interfaceTypes = data.types ?? [];
    this.interfaceReadOnly = data.read_only ?? this.interfaceReadOnly;
    return this.interfaceTypes;
  }

  async listDevices(kind = "serial") {
    if (this.devices.loaded) return this.devices;
    try {
      const data = await this.call({ cmd: "list_devices", kind });
      this.devices = {
        serial: data.devices ?? [],
        reason: data.reason ?? null,
        loaded: true,
      };
    } catch (err) {
      this.devices = { serial: [], reason: err?.message ?? String(err), loaded: true };
    }
    return this.devices;
  }

  async createInterface(name, kind, config) {
    const { interface: row } = await this.call(
      {
        cmd: "create_interface",
        name,
        kind,
        config,
      },
      30000,
    );
    await this.loadInterfaces();
    return row;
  }

  async updateInterface(interfaceId, config) {
    const { interface: row } = await this.call(
      {
        cmd: "update_interface",
        interface_id: interfaceId,
        config,
      },
      30000,
    );
    await this.loadInterfaces();
    return row;
  }

  async setInterfaceEnabled(interfaceId, enabled) {
    const { interface: row } = await this.call(
      {
        cmd: "set_interface_enabled",
        interface_id: interfaceId,
        enabled,
      },
      30000,
    );
    await this.loadInterfaces();
    return row;
  }

  async deleteInterface(interfaceId) {
    await this.call({ cmd: "delete_interface", interface_id: interfaceId }, 30000);
    await this.loadInterfaces();
  }

  meshInterface(name) {
    return this.mesh?.interfaces?.find((item) => item.name === name) ?? null;
  }

  /// The engine's reachability row for a peer, or null when it has no address
  /// yet (or an older engine did not send the rows).
  peerReachability(peer) {
    const address = peer?.address;
    if (!address) return null;
    return (
      this.mesh?.peer_reachability?.find((row) => row.address === address) ?? null
    );
  }

  /// Contacts are addressed by hash, or by their row id in some replies.
  peerByAddress(address) {
    if (!address) return null;
    return (
      this.peers.find(
        (item) => item.address === address || item.id === address,
      ) ?? null
    );
  }

  async select(conversationId) {
    this.activeId = conversationId;
    const conversation = this.conversations.find((item) => item.id === conversationId);
    if (conversation) conversation.unread = 0;
    if (!this.messages[conversationId]) {
      await this.loadMessages(conversationId);
    }
    this.call({ cmd: "mark_read", conversation_id: conversationId }).catch(() => {});
    if (this.shell && this.windowFocused && this.windowVisible) {
      this.clearActivity();
    }
  }

  /// Leave the conversation for the list. Only the phone has a list pane to
  /// return to; the desktop keeps the conversation selected.
  backToList() {
    if (!this.mobile) return;
    this.activeId = null;
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

  async sendMessage(body, attachments = []) {
    const text = body.trim();
    if ((!text && attachments.length === 0) || !this.activeId) return;
    const command = {
      cmd: "send_message",
      conversation_id: this.activeId,
      body: text,
    };
    if (attachments.length) command.attachments = attachments;
    await this.call(command, 60000);
  }

  /// Upload one chosen file and return its descriptor. Raw bytes go over HTTP,
  /// never the socket: the WebSocket stays a control channel.
  ///
  /// The filename travels percent-encoded because a header cannot carry an
  /// arbitrary name, and the engine uses it only for display.
  async uploadAttachment(file) {
    if (!this.base || !this.token) {
      throw new Error("Not connected to the engine.");
    }
    const url =
      this.base.replace(/\/+$/, "") +
      "/attachments?token=" +
      encodeURIComponent(this.token);
    let response;
    try {
      response = await fetch(url, {
        method: "POST",
        headers: {
          "content-type": file.type || "application/octet-stream",
          "x-rchat-filename": encodeURIComponent(file.name || "attachment"),
        },
        body: file,
      });
    } catch {
      throw new Error("Could not reach the engine to upload " + file.name + ".");
    }
    if (!response.ok) {
      const reason = (await response.text()).trim();
      throw new Error(
        reason || "The engine refused " + (file.name || "that file") + ".",
      );
    }
    const data = await response.json();
    if (!data?.attachment?.hash) {
      throw new Error("The engine did not accept " + (file.name || "that file") + ".");
    }
    return data.attachment;
  }

  /// The download route, with the token this client already holds. `variant`
  /// is "thumb" for the display cache and unset for the original.
  attachmentUrl(attachment, variant = null) {
    const hash = attachment?.hash;
    if (!hash) return "";
    const base = this.base.replace(/\/+$/, "");
    const target =
      variant === "thumb" && attachment.thumbnail ? attachment.thumbnail : hash;
    let url =
      base +
      "/attachments/" +
      encodeURIComponent(target) +
      "?token=" +
      encodeURIComponent(this.token);
    if (variant === "thumb" && !attachment.thumbnail) url += "&variant=thumb";
    return url;
  }

  /// Hand one attachment's bytes to the shell, which owns where they land. A
  /// cancelled dialog is not an error and returns false.
  async saveAttachment(attachment) {
    if (!attachment?.hash || attachment.reason) return false;
    if (this.shell) {
      const saved = await invoke("save_attachment", {
        hash: attachment.hash,
        name: attachment.name || "attachment",
      });
      if (saved) this.toast("Saved " + (attachment.name || "attachment"));
      return Boolean(saved);
    }
    // In a browser a download is the save action; the interface never holds
    // the bytes itself.
    const link = document.createElement("a");
    link.href = this.attachmentUrl(attachment);
    link.download = attachment.name || "attachment";
    link.rel = "noopener";
    document.body.appendChild(link);
    link.click();
    link.remove();
    return true;
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
    const peer = this.peerById(peerId);
    return (
      peer?.nickname || peer?.announced_name || shortAddress(peer?.address ?? peerId)
    );
  }

  peerById(peerId) {
    if (!peerId) return null;
    return (
      this.peers.find((item) => item.id === peerId || item.address === peerId) ?? null
    );
  }

  // ---- peer profile card --------------------------------------------------

  /// The peer the card is showing, read live so an announce that renames them
  /// while the card is open is reflected at once.
  get cardPeer() {
    return this.peerById(this.peerCard.peerId);
  }

  get cardHeadline() {
    const id = this.peerCard.peerId;
    if (!id) return "";
    return this.peerLabel(id);
  }

  get cardSubtitle() {
    const peer = this.cardPeer;
    const id = this.peerCard.peerId;
    if (!id) return "";
    return peer?.announced_name || shortAddress(peer?.address ?? id);
  }

  get cardReachable() {
    return peerReachable(
      this.cardPeer,
      this.transport,
      this.peerReachability(this.cardPeer),
    );
  }

  /// Open the card for another peer. Returns false for your own identity,
  /// which the caller answers with the profile dialog.
  openPeerCard(peerId) {
    const peer = this.peerById(peerId);
    const id = peer?.id ?? peerId;
    if (!id || id === this.identity?.id) return false;
    this.clearPeerSaveTimer();
    this.peerCard = {
      ...emptyPeerCard(),
      peerId: id,
      nickname: peer?.nickname ?? "",
      notes: peer?.notes ?? "",
    };
    return true;
  }

  /// Close the card once its last draft lands. A failed write keeps the card
  /// (and the draft) open, so closing never silently drops notes.
  async closePeerCard() {
    if (!(await this.flushPeerCard())) return false;
    this.clearPeerSaveTimer();
    this.peerCard = emptyPeerCard();
    return true;
  }

  /// Save anything pending. Returns false when a write failed, so the caller
  /// can keep the drafts instead of losing them.
  async flushPeerCard() {
    if (!this.peerCard.peerId) return true;
    this.clearPeerSaveTimer();
    const nickname = this.peerCard.editing
      ? this.commitPeerNickname()
      : Promise.resolve(true);
    const [nicknameSaved, notesSaved] = await Promise.all([
      nickname,
      this.writePeerNotes(),
    ]);
    return nicknameSaved && notesSaved;
  }

  startPeerNicknameEdit() {
    if (!this.peerCard.peerId) return;
    this.peerCard = {
      ...this.peerCard,
      nickname: this.cardPeer?.nickname ?? "",
      editing: true,
      save: "idle",
    };
  }

  draftPeerNickname(value) {
    if (!this.peerCard.peerId) return;
    this.peerCard = { ...this.peerCard, nickname: value, save: "idle" };
  }

  /// Enter or clicking away: keep the alias, or drop it when the field is empty.
  /// The field stays open until the write lands, so a failure keeps the draft.
  async commitPeerNickname() {
    const { peerId, editing, nickname } = this.peerCard;
    if (!peerId || !editing) return true;
    const alias = nickname.replace(/[\r\n]+/g, " ").trim();
    if (alias === (this.cardPeer?.nickname ?? "")) {
      this.peerCard = { ...this.peerCard, editing: false, nickname: alias };
      return true;
    }
    if (!(await this.writePeerNickname(peerId, alias))) return false;
    if (this.peerCard.peerId !== peerId) return true;
    this.peerCard = { ...this.peerCard, editing: false, nickname: alias };
    return true;
  }

  /// Esc: back out without touching the stored alias.
  cancelPeerNicknameEdit() {
    this.peerCard = {
      ...this.peerCard,
      nickname: this.cardPeer?.nickname ?? "",
      editing: false,
      save: "idle",
    };
  }

  async writePeerNickname(peerId, alias) {
    this.peerCard = { ...this.peerCard, save: "saving" };
    try {
      const { peer } = await this.call({
        cmd: "set_peer_nickname",
        peer_id: peerId,
        nickname: alias || null,
      });
      this.upsertPeer(peer);
      this.markPeerCard(this.peerCard.peerId === peerId ? "saved" : null);
      return true;
    } catch (err) {
      this.markPeerCard(this.peerCard.peerId === peerId ? "error" : null);
      this.toast(err?.message ?? String(err));
      return false;
    }
  }

  draftPeerNotes(value) {
    if (!this.peerCard.peerId) return;
    this.peerCard = { ...this.peerCard, notes: value, save: "idle" };
    this.clearPeerSaveTimer();
    this.peerSaveTimer = setTimeout(() => {
      this.peerSaveTimer = null;
      void this.writePeerNotes();
    }, PEER_CARD_SAVE_DELAY_MS);
  }

  async writePeerNotes() {
    const { peerId, notes } = this.peerCard;
    if (!peerId) return true;
    const text = notes.trimEnd();
    if (text === (this.peerById(peerId)?.notes ?? "")) return true;
    // Serialise writes: typing through a slow save must not reorder them, and
    // awaiting this promise waits for every write queued before it too.
    const previous = this.peerNotesChain ?? Promise.resolve(true);
    const chain = previous.then(
      () => this.sendPeerNotes(peerId, text),
      () => this.sendPeerNotes(peerId, text),
    );
    this.peerNotesChain = chain;
    void chain.finally(() => {
      if (this.peerNotesChain === chain) this.peerNotesChain = null;
    });
    return chain;
  }

  async sendPeerNotes(peerId, text) {
    this.peerCard = { ...this.peerCard, save: "saving" };
    try {
      const { peer } = await this.call({
        cmd: "set_peer_notes",
        peer_id: peerId,
        notes: text || null,
      });
      this.upsertPeer(peer);
      this.markPeerCard(this.peerCard.peerId === peerId ? "saved" : null);
      return true;
    } catch (err) {
      this.markPeerCard(this.peerCard.peerId === peerId ? "error" : null);
      this.toast(err?.message ?? String(err));
      return false;
    }
  }

  markPeerCard(save) {
    if (!save) return;
    this.peerCard = { ...this.peerCard, save };
  }

  clearPeerSaveTimer() {
    if (this.peerSaveTimer) {
      clearTimeout(this.peerSaveTimer);
      this.peerSaveTimer = null;
    }
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
