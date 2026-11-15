<script>
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

  $effect(() => {
    if (chat.status === "idle" && chat.token) chat.connect();
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

  function close() {
    if (!busy) dialog = null;
  }

  function handleKey(event) {
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
    <Sidebar onAddPeer={openPeer} onProfile={openProfile} />

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

<div class="toasts">
  {#each chat.toasts as toast (toast.id)}
    <div class="toast">{toast.message}</div>
  {/each}
</div>
