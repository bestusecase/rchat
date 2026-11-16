<script>
  import { onMount } from "svelte";
  import { chat } from "./store.svelte.js";
  import Icon from "./Icon.svelte";
  import NetworkOverview from "./NetworkOverview.svelte";
  import NetworkInterfaces from "./NetworkInterfaces.svelte";
  import NetworkPaths from "./NetworkPaths.svelte";
  import NetworkAnnounced from "./NetworkAnnounced.svelte";
  import NetworkMap from "./NetworkMap.svelte";

  let { onBack = null } = $props();

  const tabs = [
    { id: "overview", label: "Overview" },
    { id: "interfaces", label: "Interfaces" },
    { id: "paths", label: "Paths" },
    { id: "announced", label: "Announced" },
    { id: "map", label: "Map" },
  ];

  const transportText = $derived(
    chat.meshTransport === "ready"
      ? "Reticulum online"
      : chat.meshTransport === "degraded"
        ? "Mesh degraded"
        : chat.meshTransport === "starting"
          ? "Connecting mesh"
          : "Bridge offline",
  );

  onMount(() => {
    // The snapshot normally carries the mesh, but a bridge that came up after
    // the snapshot would otherwise leave the page empty.
    if (!chat.mesh) chat.requestMeshStatus().catch(() => {});
  });
</script>

<section class="chat network" aria-label="Reticulum network">
  <header class="chat-head">
    {#if chat.mobile}
      <button class="icon-button back-button" title="Back" onclick={onBack}>
        <Icon name="back" size={20} />
      </button>
    {/if}
    <div class="chat-title-block">
      <h1>Reticulum</h1>
      <div class="chat-sub">
        <span class="dot" data-state={chat.meshTransport}></span>
        {transportText}
      </div>
    </div>
    <div class="chat-actions">
      <span class="transport-pill" data-state={chat.meshTransport}>{transportText}</span>
    </div>
  </header>

  {#if chat.meshTransport !== "ready"}
    <div class="net-banner" role="status" data-state={chat.meshTransport}>
      <strong>
        {#if chat.meshTransport === "degraded"}
          The bridge is up, but no interface is online.
        {:else if chat.meshTransport === "starting"}
          The Reticulum bridge is starting.
        {:else}
          The Reticulum bridge is offline.
        {/if}
      </strong>
      <span>
        {#if chat.meshTransport === "degraded"}
          {chat.transportDetail ||
            "Nothing can be sent or received yet. Fix an interface below; discovery and a relay both work."}
        {:else}
          {chat.transportDetail ||
            "Showing the last known mesh. Interface changes are disabled until it is back."}
        {/if}
      </span>
    </div>
  {/if}

  <div class="network-tabs" role="tablist" aria-label="Reticulum views">
    {#each tabs as tab (tab.id)}
      <button
        class="network-tab"
        class:active={chat.networkTab === tab.id}
        role="tab"
        aria-selected={chat.networkTab === tab.id}
        onclick={() => (chat.networkTab = tab.id)}
      >
        {tab.label}
      </button>
    {/each}
  </div>

  <div class="network-body">
    {#if chat.networkTab === "overview"}
      <NetworkOverview onInterfaces={() => (chat.networkTab = "interfaces")} />
    {:else if chat.networkTab === "interfaces"}
      <NetworkInterfaces />
    {:else if chat.networkTab === "paths"}
      <NetworkPaths />
    {:else if chat.networkTab === "announced"}
      <NetworkAnnounced />
    {:else if chat.networkTab === "map"}
      <NetworkMap onOpenPaths={() => (chat.networkTab = "paths")} />
    {/if}
  </div>
</section>
