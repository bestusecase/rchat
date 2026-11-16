<script>
  import { chat } from "./store.svelte.js";
  import {
    formatBitrate,
    formatOutage,
    formatRate,
    formatUptime,
    shortAddress,
  } from "./util.js";

  let { onInterfaces = null } = $props();
  let copied = $state(false);

  const mesh = $derived(chat.mesh);
  const counters = $derived(mesh?.counters ?? {});
  const interfaces = $derived(mesh?.interfaces ?? []);

  const countersView = $derived([
    { label: "Packets in / out", value: `${counters.rx_packets ?? 0} / ${counters.tx_packets ?? 0}` },
    { label: "Rate (pps)", value: `${formatRate(counters.rx_pps)} / ${formatRate(counters.tx_pps)}` },
    { label: "Announces", value: `${counters.announce_rx ?? 0} in / ${counters.announce_tx ?? 0} out` },
    { label: "Active links", value: counters.links ?? 0 },
    { label: "Known paths", value: counters.path_count ?? 0 },
    { label: "Unresponsive", value: counters.path_unresponsive ?? 0 },
  ]);

  function peerName(address) {
    const found = chat.peerByAddress(address);
    return found?.nickname || found?.announced_name || shortAddress(address);
  }

  function peerId(address) {
    return peer(address)?.id ?? address;
  }

  async function copyAddress() {
    const address = mesh?.address ?? chat.identity?.address;
    if (!address) return;
    try {
      await navigator.clipboard.writeText(address);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      chat.toast("Could not access the clipboard.");
    }
  }
</script>

<div class="net-overview">
  <div class="net-grid">
    <div class="net-card">
      <div class="net-card-title">Transport</div>
      <div class="net-card-value" data-state={chat.meshTransport}>
        {chat.meshTransport}
      </div>
      <div class="net-card-note">
        {#if chat.meshTransport === "ready"}
          Up {formatUptime(mesh?.started_at)}
        {:else if chat.meshTransport === "degraded"}
          Bridge up, but no interface is online. Nothing can move until one is.
        {:else}
          {chat.transportDetail || "No bridge connected"}
        {/if}
      </div>
    </div>

    <div class="net-card wide">
      <div class="net-card-title">Your LXMF address</div>
      <div class="copy-row">
        <code class="address-box">
          {mesh?.address || chat.identity?.address || "starting the mesh…"}
        </code>
        <button
          class="btn ghost"
          onclick={copyAddress}
          disabled={!mesh?.address && !chat.identity?.address}
          title="Copy address"
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <div class="net-card-note">
        Share this so peers can reach you. It comes from your Reticulum identity.
      </div>
    </div>
  </div>

  <div class="net-counters">
    {#each countersView as counter (counter.label)}
      <div class="net-counter">
        <span class="net-counter-value">{counter.value}</span>
        <span class="net-counter-label">{counter.label}</span>
      </div>
    {/each}
  </div>

  <div class="net-section-head">
    <h2>Interfaces</h2>
    <button class="link-button" onclick={onInterfaces}>Manage</button>
  </div>

  {#if interfaces.length === 0}
    <p class="net-empty">
      No interfaces are up. Without one, rchat is local-only. Add a TCP relay or
      LAN discovery on the Interfaces tab.
    </p>
  {:else}
    <div class="chip-row">
      {#each interfaces as iface (iface.hash || iface.name)}
        <button class="iface-chip" onclick={onInterfaces}>
          <span class="dot" data-state={iface.online ? "ready" : "offline"}></span>
          <span class="iface-chip-name">{iface.name}</span>
          <span class="iface-chip-meta">
            {formatBitrate(iface.bitrate)}
            {#if iface.peers != null}· {iface.peers} peers{/if}
            {#if !iface.online && iface.down_since}
              · offline for {formatOutage(iface.down_since)}
            {/if}
          </span>
        </button>
      {/each}
    </div>
  {/if}

  <div class="net-section-head">
    <h2>Known peers</h2>
    <span class="section-count">{mesh?.peer_paths?.length ?? 0}</span>
  </div>

  {#if (mesh?.peer_paths?.length ?? 0) === 0}
    <p class="net-empty">
      No peer paths yet. Peers appear once they announce and a path is known.
    </p>
  {:else}
    <ul class="peer-path-list">
      {#each mesh.peer_paths as path (path.address)}
        <li>
          <button class="peer-path" onclick={() => chat.openPeer(peerId(path.address))}>
            <span class="peer-path-name">{peerName(path.address)}</span>
            <span class="peer-path-meta">
              {path.hops === 1 ? "direct" : path.hops + " hops"} · {path.interface}
            </span>
            <span class="peer-path-hash mono">{shortAddress(path.address)}</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>
