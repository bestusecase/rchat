<script>
  import { onMount } from "svelte";
  import { chat } from "./store.svelte.js";
  import { shortAddress, shortHash } from "./util.js";

  let scrollBox = $state();
  let query = $state("");
  let sortAsc = $state(true);
  let selected = $state(null);
  let copied = $state(false);
  let refreshAt = 0;
  let mounted = false;
  let queryTimer = null;

  const rows = $derived.by(() => {
    const list = chat.meshPaths.rows.slice();
    if (!sortAsc) list.sort((a, b) => b.hops - a.hops || a.age - b.age);
    return list;
  });

  const canLoadMore = $derived(chat.meshPaths.rows.length < chat.meshPaths.total);

  function labelFor(path) {
    if (path.name) return path.name;
    const peer = chat.peerByAddress(path.destination);
    return peer?.nickname || peer?.announced_name || null;
  }

  async function load(reset) {
    if (!chat.meshUp) return;
    await chat.loadMeshPaths({ reset, query });
  }

  function onSearch() {
    clearTimeout(queryTimer);
    queryTimer = setTimeout(() => {
      chat.setMeshPathQuery(query.trim());
      load(true);
    }, 250);
  }

  function onScroll() {
    if (!scrollBox || chat.meshPaths.loading || !canLoadMore) return;
    const remaining = scrollBox.scrollHeight - scrollBox.scrollTop - scrollBox.clientHeight;
    if (remaining < 240) load(false);
  }

  function open(path) {
    const peer = chat.peerByAddress(path.destination);
    if (peer) chat.openPeer(peer.id).catch((err) => chat.toast(err.message));
    else selected = path;
  }

  async function copy(value) {
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      setTimeout(() => (copied = false), 1600);
    } catch {
      chat.toast("Could not access the clipboard.");
    }
  }

  onMount(() => {
    mounted = true;
    refreshAt = Date.now();
    load(true);
  });

  // A mesh refresh can carry the whole table; cap refetches to once a second.
  $effect(() => {
    const stamp = chat.meshUpdatedAt;
    if (!mounted || !stamp) return;
    const now = Date.now();
    if (now - refreshAt < 1000) return;
    refreshAt = now;
    chat.refreshMeshPaths();
  });
</script>

<div class="net-tab">
  <div class="net-toolbar">
    <label class="search-box">
      <input
        class="field"
        bind:value={query}
        oninput={onSearch}
        placeholder="Filter by address, name or interface"
        spellcheck="false"
      />
    </label>
    <span class="net-total">
      {chat.meshPaths.total} {chat.meshPaths.total === 1 ? "destination" : "destinations"}
      {#if chat.meshPaths.truncated}<span class="net-total-note">(nearest 2000 shown)</span>{/if}
    </span>
  </div>

  {#if chat.meshPaths.error}
    <p class="form-error">{chat.meshPaths.error}</p>
  {/if}

  {#if chat.meshPaths.rows.length === 0 && !chat.meshPaths.loading}
    <p class="net-empty">
      {#if !chat.meshUp}
        The bridge is offline. The last known paths stay visible once it reports again.
      {:else if chat.meshPaths.query}
        No destinations match “{chat.meshPaths.query}”.
      {:else}
        No announces yet. Destinations appear once a peer announces itself.
      {/if}
    </p>
  {:else}
    <div class="net-table-wrap" bind:this={scrollBox} onscroll={onScroll}>
      <table class="net-table">
        <thead>
          <tr>
            <th>Destination</th>
            <th>Name</th>
            <th>
              <button class="th-sort" onclick={() => (sortAsc = !sortAsc)}>
                Hops {sortAsc ? "↑" : "↓"}
              </button>
            </th>
            <th>Next hop</th>
            <th>Interface</th>
            <th>State</th>
            <th>Age</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as path (path.destination)}
            <tr
              class="net-row"
              class:selected={selected?.destination === path.destination}
              onclick={() => open(path)}
              tabindex="0"
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  open(path);
                }
              }}
            >
              <td class="mono">{shortHash(path.destination)}</td>
              <td>{labelFor(path) ?? "—"}</td>
              <td>{path.hops}</td>
              <td class="mono">{shortHash(path.next_hop)}</td>
              <td>{path.interface}</td>
              <td>
                <span class="net-state" data-state={path.state}>{path.state}</span>
              </td>
              <td>{Math.max(0, Math.round(path.age))}s</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if chat.meshPaths.loading}
        <p class="net-loading">Loading…</p>
      {:else if canLoadMore}
        <p class="net-loading">Scroll for more</p>
      {/if}
    </div>
  {/if}

  {#if selected}
    <div class="net-popover" role="dialog" aria-label="Destination details">
      <div class="net-popover-head">
        <strong>Destination</strong>
        <button class="icon-button" title="Close" onclick={() => (selected = null)}>×</button>
      </div>
      <dl class="net-detail">
        <dt>Address</dt>
        <dd class="mono break">{selected.destination}</dd>
        <dt>Hops</dt>
        <dd>{selected.hops}</dd>
        <dt>Next hop</dt>
        <dd class="mono break">{selected.next_hop}</dd>
        <dt>Interface</dt>
        <dd>{selected.interface}</dd>
        <dt>State</dt>
        <dd>{selected.state}</dd>
        <dt>Age</dt>
        <dd>{Math.max(0, Math.round(selected.age))}s</dd>
      </dl>
      <button class="btn ghost" onclick={() => copy(selected.destination)}>
        {copied ? "Copied" : "Copy address"}
      </button>
    </div>
  {/if}
</div>
