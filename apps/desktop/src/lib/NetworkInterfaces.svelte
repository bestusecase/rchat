<script>
  import { onMount } from "svelte";
  import { chat } from "./store.svelte.js";
  import { formatBitrate, formatOutage, formatRate } from "./util.js";
  import InterfaceForm from "./InterfaceForm.svelte";

  let mode = $state("list"); // list | picker | form
  let editing = $state(null);
  let selectedType = $state(null);
  let error = $state("");
  let busy = $state(null);

  const types = $derived(chat.interfaceTypes ?? []);
  const groups = $derived.by(() => {
    const out = [];
    for (const type of types) {
      const group = type.group || "Other";
      if (!out.includes(group)) out.push(group);
    }
    return out;
  });

  function liveFor(row) {
    return chat.meshInterface(row.name);
  }

  function typeLabel(kind) {
    return types.find((type) => type.kind === kind)?.label ?? kind;
  }

  // Where the row came from: created here, imported from `rns/config`, carried
  // over from the old discovery/relay settings, or the first-run default.
  const ORIGIN_LABELS = {
    managed: "rchat",
    imported: "imported",
    legacy: "legacy",
    default: "default",
  };

  function originLabel(origin) {
    return ORIGIN_LABELS[origin] ?? origin;
  }

  // Why RNS will not let rchat manage this row, if it won't. The bridge owns
  // this; the page only renders it.
  function readOnlyReason(row) {
    return types.find((type) => type.kind === row.kind)?.read_only ?? null;
  }

  function radioSummary(iface) {
    const radio = iface?.radio;
    if (!radio) return null;
    const parts = [];
    if (radio.rssi != null) parts.push(`${Math.round(radio.rssi)} dBm`);
    if (radio.snr != null) parts.push(`${radio.snr.toFixed(1)} dB`);
    if (radio.airtime_short != null) parts.push(`${radio.airtime_short.toFixed(0)}% airtime`);
    return parts.join(" · ") || null;
  }

  function extraReason(section) {
    if (String(section).startsWith("interfaces/")) {
      return "could not be parsed as a known Reticulum interface";
    }
    return "not an interface section, kept verbatim";
  }

  async function load() {
    error = "";
    await chat.loadInterfaces();
    if (!chat.interfaceTypes && chat.meshUp) {
      try {
        await chat.describeInterfaces();
      } catch (err) {
        error = err?.message ?? String(err);
      }
    }
  }

  function startAdd() {
    selectedType = null;
    editing = null;
    mode = "picker";
    if (!types.length) {
      chat
        .describeInterfaces()
        .catch((err) => (error = err?.message ?? String(err)));
    }
  }

  function pick(type) {
    selectedType = type;
    editing = null;
    mode = "form";
  }

  function startEdit(row) {
    selectedType = types.find((type) => type.kind === row.kind) ?? {
      kind: row.kind,
      label: row.kind,
      summary: "Unknown interface type. Existing keys stay editable.",
      fields: [],
    };
    editing = row;
    mode = "form";
  }

  function saved() {
    mode = "list";
    editing = null;
    selectedType = null;
  }

  function cancelled() {
    mode = "list";
    editing = null;
    selectedType = null;
  }

  async function toggle(row) {
    busy = row.id;
    error = "";
    try {
      await chat.setInterfaceEnabled(row.id, !row.enabled);
    } catch (err) {
      error = err?.message ?? String(err);
    } finally {
      busy = null;
    }
  }

  async function remove(row) {
    if (
      !window.confirm(
        `Delete "${row.name}"? Deleting an interface can cut the last path to a peer.`,
      )
    ) {
      return;
    }
    busy = row.id;
    error = "";
    try {
      await chat.deleteInterface(row.id);
    } catch (err) {
      error = err?.message ?? String(err);
    } finally {
      busy = null;
    }
  }

  onMount(load);

  // The bridge may come back after the page opened; retry the catalogue then.
  $effect(() => {
    if (chat.meshUp && !chat.interfaceTypes) load();
  });
</script>

<div class="net-tab">
  {#if !chat.meshUp}
    <p class="net-banner" role="status">
      <strong>The bridge is offline.</strong>
      <span>Interface edits are disabled until Reticulum is running.</span>
    </p>
  {:else if chat.interfaceReadOnly}
    <p class="net-banner" role="status">
      <strong>Read-only.</strong>
      <span>{chat.interfaceReadOnly}</span>
    </p>
  {/if}

  {#if error}
    <p class="form-error">{error}</p>
  {/if}

  {#if mode === "picker"}
    <div class="net-section-head">
      <h2>Add an interface</h2>
      <button class="link-button" onclick={cancelled}>Cancel</button>
    </div>
    {#if types.length === 0}
      <p class="net-empty">
        The interface catalogue is unavailable. Start the bridge and try again.
      </p>
    {:else}
      {#each groups as group (group)}
        <h3 class="net-group-title">{group}</h3>
        <div class="type-grid">
          {#each types.filter((type) => (type.group || "Other") === group) as type (type.kind)}
            <button
              class="type-card"
              onclick={() => pick(type)}
              disabled={!!type.read_only || !!chat.interfaceReadOnly}
              title={type.read_only ?? chat.interfaceReadOnly ?? ""}
            >
              <span class="type-card-label">{type.label}</span>
              <span class="type-card-summary">{type.summary}</span>
              {#if type.read_only}<span class="badge muted">read-only</span>{/if}
            </button>
          {/each}
        </div>
      {/each}
    {/if}
  {:else if mode === "form"}
    <InterfaceForm
      type={selectedType}
      row={editing}
      existingNames={chat.interfaces.map((row) => row.name)}
      onSaved={saved}
      onCancel={cancelled}
    />
  {:else}
    <div class="net-section-head">
      <h2>Interfaces</h2>
      <button
        class="btn primary"
        onclick={startAdd}
        disabled={!chat.meshUp || !types.length || !!chat.interfaceReadOnly}
        title={chat.interfaceReadOnly ?? ""}
      >
        Add interface
      </button>
    </div>

    {#if chat.interfaces.length === 0 && chat.interfaceExtra.length === 0}
      <p class="net-empty">
        No interfaces yet. Without one, rchat is local-only. Add a TCP relay, or
        turn on LAN discovery with AutoInterface.
      </p>
    {:else}
      <ul class="iface-list">
        {#each chat.interfaces as row (row.id)}
          {@const live = liveFor(row)}
          {@const radio = radioSummary(live)}
          {@const locked = readOnlyReason(row)}
          <li class="iface-row" class:disabled={!row.enabled}>
            <span class="dot" data-state={live?.online ? "ready" : "offline"}></span>
            <div class="iface-main">
              <div class="iface-title">
                <strong>{row.name}</strong>
                <span class="badge">{typeLabel(row.kind)}</span>
                <span class="badge origin" data-origin={row.origin}>
                  {originLabel(row.origin)}
                </span>
                {#if !row.enabled}<span class="badge muted">disabled</span>{/if}
                {#if locked}
                  <span class="badge muted" title={locked}>read-only</span>
                {/if}
              </div>
              {#if row.note}
                <div class="iface-note">{row.note}</div>
              {/if}
              <div class="iface-meta">
                {#if locked}
                  {locked}
                {:else if live}
                  {#if live.online}
                    {formatBitrate(live.bitrate)}
                    · ↓ {formatRate(live.rxs)} / ↑ {formatRate(live.txs)}
                    {#if live.peers != null}· {live.peers} peers{/if}
                    {#if radio}· {radio}{/if}
                  {:else}
                    <span class="iface-down">
                      Offline{#if live.down_since} for {formatOutage(live.down_since)}{/if}
                    </span>
                  {/if}
                {:else}
                  Not attached
                {/if}
              </div>
            </div>
            <div class="iface-actions">
              <button
                class="link-button"
                onclick={() => startEdit(row)}
                disabled={busy === row.id || !!locked || !!chat.interfaceReadOnly}
                title={locked ?? chat.interfaceReadOnly ?? ""}
              >
                Edit
              </button>
              <button
                class="link-button"
                onclick={() => toggle(row)}
                disabled={busy === row.id || !!locked || !!chat.interfaceReadOnly}
                title={locked ?? chat.interfaceReadOnly ?? ""}
              >
                {row.enabled ? "Disable" : "Enable"}
              </button>
              <button class="link-button danger" onclick={() => remove(row)} disabled={busy === row.id}>
                Delete
              </button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    {#if chat.interfaceExtra.length > 0}
      <div class="net-section-head">
        <h2>Managed outside rchat</h2>
        <span class="section-count">{chat.interfaceExtra.length}</span>
      </div>
      <p class="net-empty">
        These sections are preserved byte-for-byte in <code>rns/config</code> and
        are not editable here.
      </p>
      <ul class="iface-list">
        {#each chat.interfaceExtra as block (block.section)}
          <li class="iface-row readonly">
            <div class="iface-main">
              <div class="iface-title">
                <strong>{block.section}</strong>
                <span class="badge muted">read-only</span>
              </div>
              <div class="iface-meta">{extraReason(block.section)}</div>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>
