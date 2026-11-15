<script>
  import { onMount } from "svelte";
  import { chat } from "./lib/store.svelte.js";
  import { shortAddress } from "./lib/util.js";
  import Connect from "./lib/Connect.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import Chat from "./lib/Chat.svelte";
  import Members from "./lib/Members.svelte";
  import Icon from "./lib/Icon.svelte";

  let dialog = $state(null);
  let peerAddress = $state("");
  let peerName = $state("");
  let nameDraft = $state("");
  let dialogError = $state("");
  let busy = $state(false);
  let copied = $state(false);
  let autoDiscovery = $state(true);
  let relayDraft = $state("");
  let mutedDraft = $state(false);
  let autostartDraft = $state(false);
  let updatePercent = $derived(
    chat.update.total
      ? Math.min(100, Math.round((chat.update.downloaded / chat.update.total) * 100))
      : 0,
  );

  onMount(() => {
    chat.initDesktop();
    if (!chat.desktop || chat.token) chat.connect();
  });

  $effect(() => {
    if (!chat.desktop && chat.status === "idle" && chat.token) chat.connect();
  });

  function openPeer() {
    dialog = "peer";
    peerAddress = "";
    peerName = "";
    dialogError = "";
  }

  function openProfile() {
    dialog = "profile";
    nameDraft = chat.identity?.display_name ?? "";
    dialogError = "";
    copied = false;
  }

  function openSettings() {
    const settings = chat.desktopSettings;
    autoDiscovery = settings?.autoDiscovery ?? true;
    relayDraft = settings?.tcpRelay ?? "";
    mutedDraft = settings?.muted ?? false;
    autostartDraft = settings?.autostart ?? false;
    dialogError = "";
    dialog = "settings";
  }

  function close() {
    if (busy || chat.needsName) return;
    dialog = null;
  }

  function handleKey(event) {
    if (chat.needsName) return;
    if (event.key === "Escape") close();
  }

  async function submitPeer() {
    dialogError = "";
    const address = peerAddress.trim().toLowerCase();
    if (!/^[0-9a-f]{32}$/.test(address)) {
      dialogError = "Enter the peer's 32-character hex address.";
      return;
    }
    busy = true;
    try {
      await chat.addPeer(address, peerName);
      dialog = null;
    } catch (err) {
      dialogError = err.message;
    } finally {
      busy = false;
    }
  }

  async function saveName() {
    dialogError = "";
    if (!nameDraft.trim()) {
      dialogError = "A display name is required.";
      return;
    }
    busy = true;
    try {
      await chat.setDisplayName(nameDraft.trim());
      dialog = null;
    } catch (err) {
      dialogError = err.message;
    } finally {
      busy = false;
    }
  }

  async function applyNetwork() {
    dialogError = "";
    busy = true;
    try {
      await chat.saveNetwork(autoDiscovery, relayDraft.trim());
      dialog = null;
    } catch (err) {
      dialogError = err?.message ?? String(err);
    } finally {
      busy = false;
    }
  }

  async function toggleMute() {
    mutedDraft = !mutedDraft;
    try {
      await chat.setMuted(mutedDraft);
    } catch (err) {
      dialogError = err?.message ?? String(err);
    }
  }

  async function toggleAutostart() {
    autostartDraft = !autostartDraft;
    try {
      await chat.setAutostart(autostartDraft);
    } catch (err) {
      dialogError = err?.message ?? String(err);
    }
  }

  async function copyAddress() {
    try {
      await navigator.clipboard.writeText(chat.identity?.address ?? "");
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      chat.toast("Could not access the clipboard.");
    }
  }
</script>

<svelte:window onkeydown={handleKey} />

{#if chat.status !== "connected"}
  <Connect />
{:else}
  <div class="app">
    <Sidebar onAddPeer={openPeer} onProfile={openProfile} onSettings={openSettings} />

    {#if chat.activeConversation}
      <Chat />
    {:else}
      <section class="chat placeholder">
        <div class="chat-empty-mark"><Icon name="wave" size={30} /></div>
        <h2>Pick up where you left off</h2>
        <p>Choose a conversation on the left, or add a peer by their Reticulum address.</p>
        <div class="placeholder-actions">
          <button class="btn primary" onclick={openPeer}>Add a peer</button>
          <button class="btn ghost" onclick={openProfile}>Your address</button>
        </div>
      </section>
    {/if}

    <Members />
  </div>
{/if}

{#if dialog === "peer"}
  <div
    class="overlay"
    role="presentation"
    onclick={close}
    onkeydown={(event) => event.key === "Escape" && close()}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      <h2>Add a peer</h2>
      <p class="hint">
        Paste their LXMF address. Peers on the same mesh also appear automatically once
        they announce.
      </p>
      <label class="field-label">
        Address
        <input
          class="field mono"
          bind:value={peerAddress}
          placeholder="32 hex characters"
          spellcheck="false"
        />
      </label>
      <label class="field-label">
        Nickname
        <input class="field" bind:value={peerName} placeholder="optional" />
      </label>
      {#if dialogError}<p class="form-error">{dialogError}</p>{/if}
      <div class="dialog-actions">
        <button class="btn ghost" onclick={close} disabled={busy}>Cancel</button>
        <button class="btn primary" onclick={submitPeer} disabled={busy}>
          {busy ? "Adding…" : "Add peer"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if dialog === "profile"}
  <div
    class="overlay"
    role="presentation"
    onclick={close}
    onkeydown={(event) => event.key === "Escape" && close()}
  >
    <div
      class="dialog"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      <h2>Your profile</h2>
      <label class="field-label">
        Display name
        <input class="field" bind:value={nameDraft} placeholder="Your name" />
      </label>
      <div class="field-label">
        Reticulum address
        <div class="copy-row">
          <span class="address-box">{chat.identity?.address || "Starting the mesh…"}</span>
          <button
            class="btn ghost"
            onclick={copyAddress}
            disabled={!chat.identity?.address}
            title="Copy address"
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      </div>
      <p class="hint">
        Share this address so others can reach you. It is derived from your Reticulum
        identity, not your display name.
      </p>
      {#if dialogError}<p class="form-error">{dialogError}</p>{/if}
      <div class="dialog-actions">
        <button class="btn ghost" onclick={close} disabled={busy}>Cancel</button>
        <button class="btn primary" onclick={saveName} disabled={busy}>
          {busy ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if chat.needsName}
  <div class="overlay">
    <div
      class="dialog first-run"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onkeydown={(event) => event.stopPropagation()}
    >
      <div class="first-run-mark"><span class="brand-mark big">r</span></div>
      <h2>What should people call you?</h2>
      <p class="hint">
        This is the name peers see when you announce. You can change it later in
        your profile.
      </p>
      <input
        class="field"
        bind:value={nameDraft}
        placeholder="Your name"
        onkeydown={(event) => {
          if (event.key === "Enter") saveName();
        }}
      />
      {#if dialogError}<p class="form-error">{dialogError}</p>{/if}
      <button class="btn primary wide" onclick={saveName} disabled={busy}>
        {busy ? "Saving…" : "Start chatting"}
      </button>
    </div>
  </div>
{/if}

{#if dialog === "settings"}
  <div
    class="overlay"
    role="presentation"
    onclick={close}
    onkeydown={(event) => event.key === "Escape" && close()}
  >
    <div
      class="dialog settings-dialog"
      role="dialog"
      aria-modal="true"
      tabindex="-1"
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => event.stopPropagation()}
    >
      <h2>Settings</h2>

      {#if chat.desktop}
        <div class="settings-section">
          <div class="settings-title">Network</div>
          <button
            class="toggle-row"
            type="button"
            aria-pressed={autoDiscovery}
            onclick={() => (autoDiscovery = !autoDiscovery)}
          >
            <span class="toggle-text">
              <strong>Find peers on this network</strong>
              <small>Uses Reticulum's local discovery. Turn off to stay isolated.</small>
            </span>
            <span class="switch" data-on={autoDiscovery}></span>
          </button>
          <label class="field-label">
            TCP relay
            <input
              class="field mono"
              bind:value={relayDraft}
              placeholder="relay.example:4242"
              spellcheck="false"
            />
          </label>
          <p class="hint">
            Leave empty to stay on your local network. Applying these settings
            restarts the engine for a moment.
          </p>
        </div>

        <div class="settings-section">
          <div class="settings-title">Desktop</div>
          <button
            class="toggle-row"
            type="button"
            aria-pressed={mutedDraft}
            onclick={toggleMute}
          >
            <span class="toggle-text">
              <strong>Mute sounds</strong>
              <small>Unread dots and the taskbar hint still work.</small>
            </span>
            <span class="switch" data-on={mutedDraft}></span>
          </button>
          <button
            class="toggle-row"
            type="button"
            aria-pressed={autostartDraft}
            onclick={toggleAutostart}
          >
            <span class="toggle-text">
              <strong>Start at login</strong>
              <small>Launch rchat in the tray when you sign in.</small>
            </span>
            <span class="switch" data-on={autostartDraft}></span>
          </button>
        </div>

        <div class="settings-section">
          <div class="settings-title">Updates</div>
          <div class="update-row">
            <span class="toggle-text">
              <strong>rchat {chat.update.current || chat.update.packageVersion || ""}</strong>
              {#if chat.update.installKind === "appimage"}
                {#if chat.update.status === "checking"}
                  <small>Checking the release feed…</small>
                {:else if chat.update.status === "available"}
                  <small>Version {chat.update.latest} is available to download.</small>
                {:else if chat.update.status === "downloading"}
                  <small>Downloading {chat.update.latest}…</small>
                {:else if chat.update.status === "ready"}
                  <small>Version {chat.update.latest} is installed. Restart to use it.</small>
                {:else if chat.update.status === "error"}
                  <small>{chat.update.error}</small>
                {:else if chat.update.status === "current"}
                  <small>You're on the latest version.</small>
                {:else}
                  <small>Downloaded in the app, applied on restart.</small>
                {/if}
              {:else if chat.update.installKind === "deb"}
                {#if chat.update.pendingRestart}
                  <small>Version {chat.update.packageVersion} is installed. Restart to use it.</small>
                {:else if chat.update.autoUpdates}
                  <small>
                    Automatic: a daily system job upgrades rchat from the signed rchat
                    repository.
                  </small>
                {:else}
                  <small>Add the rchat apt repository to get updates with your other packages.</small>
                {/if}
              {:else}
                <small>Automatic updates are available in packaged builds.</small>
              {/if}
            </span>
          </div>
          {#if chat.update.status === "downloading"}
            <div
              class="update-progress"
              role="progressbar"
              aria-valuemin="0"
              aria-valuemax="100"
              aria-valuenow={updatePercent}
            >
              <span style="width: {updatePercent}%"></span>
            </div>
          {/if}
          {#if chat.update.notes && chat.update.status === "available"}
            <p class="update-notes">{chat.update.notes}</p>
          {/if}
          <div class="update-actions">
            {#if chat.update.installKind === "appimage"}
              {#if chat.update.status === "available"}
                <button class="btn primary" onclick={() => chat.installUpdate()}>
                  Download {chat.update.latest}
                </button>
              {:else if chat.update.status === "downloading"}
                <button class="btn ghost" disabled>{updatePercent}%</button>
              {:else if chat.update.status === "ready"}
                <button class="btn primary" onclick={() => chat.restartForUpdate()}>
                  Restart to update
                </button>
              {:else}
                <button
                  class="btn ghost"
                  onclick={() => chat.checkForUpdate(true)}
                  disabled={chat.update.status === "checking"}
                >
                  {chat.update.status === "checking" ? "Checking…" : "Check now"}
                </button>
              {/if}
            {:else if chat.update.installKind === "deb" && chat.update.pendingRestart}
              <button class="btn primary" onclick={() => chat.restartForUpdate()}>
                Restart to update
              </button>
            {/if}
          </div>
        </div>
      {:else}
        <p class="hint">
          Network and desktop options are available in the rchat desktop app.
        </p>
      {/if}

      {#if dialogError}<p class="form-error">{dialogError}</p>{/if}
      <div class="dialog-actions">
        <button class="btn ghost" onclick={close} disabled={busy}>Cancel</button>
        {#if chat.desktop}
          <button class="btn primary" onclick={applyNetwork} disabled={busy}>
            {busy ? "Restarting…" : "Apply"}
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<div class="toasts">
  {#each chat.toasts as toast (toast.id)}
    <div class="toast">{toast.message}</div>
  {/each}
</div>
