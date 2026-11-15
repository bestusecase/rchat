<script>
  import { chat } from "./store.svelte.js";

  let showHelp = $state(false);
</script>

<div class="connect">
  <div class="connect-card">
    <div class="connect-brand"><span class="brand-mark big">r</span></div>
    <h1>Welcome to rchat</h1>
    <p class="connect-sub">Fast, offline-first messaging over Reticulum.</p>

    <label class="field-label">
      Engine address
      <input class="field mono" bind:value={chat.base} spellcheck="false" />
    </label>

    <label class="field-label">
      Access token
      <input class="field mono" bind:value={chat.token} spellcheck="false" />
    </label>

    {#if chat.error}
      <p class="form-error">{chat.error}</p>
    {/if}

    <button
      class="btn primary wide"
      onclick={() => chat.connect()}
      disabled={chat.status === "connecting" || !chat.token.trim()}
    >
      {chat.status === "connecting" ? "Connecting…" : "Connect"}
    </button>

    <button class="help-toggle" onclick={() => (showHelp = !showHelp)}>
      Where do I find the token?
    </button>

    {#if showHelp}
      <div class="help-card">
        <p>Start the engine in a terminal:</p>
        <code>cargo run -p rchatd</code>
        <p>It prints the token and writes it to:</p>
        <code>~/.local/share/rchat/token</code>
      </div>
    {/if}
  </div>

  <p class="connect-foot">
    Your history stays on this device. Nothing leaves unless you send it.
  </p>
</div>
