<script>
  import { onMount } from "svelte";
  import { chat } from "./store.svelte.js";
  import { absoluteTime, formatRelative, shortHash } from "./util.js";

  let filter = $state("");
  let scrollBox = $state();
  let mounted = false;

  const rows = $derived.by(() => {
    const needle = filter.trim().toLowerCase();
    if (!needle) return chat.meshAnnounces;
    return chat.meshAnnounces.filter(
      (row) =>
        (row.name ?? "").toLowerCase().includes(needle) ||
        row.address.toLowerCase().includes(needle),
    );
  });

  const canLoadMore = $derived(chat.meshAnnounces.length > 0 && !filter);

  async function load(reset) {
    await chat.loadAnnounces({ reset });
  }

  function onScroll() {
    if (!scrollBox || !canLoadMore || chat.announcesLoading) return;
    if (chat.meshAnnounces.length < 100) return;
    const remaining = scrollBox.scrollHeight - scrollBox.scrollTop - scrollBox.clientHeight;
    if (remaining < 240) load(false);
  }

  onMount(() => {
    mounted = true;
    load(true);
  });

  // A reconnect refills the list; live announces arrive through the store.
  $effect(() => {
    if (!mounted) return;
    if (chat.meshUpdatedAt) {
      // Only refetch when the list is empty, so live prepends are not lost.
      if (chat.meshAnnounces.length === 0) load(true);
    }
  });
</script>

<div class="net-tab">
  <div class="net-toolbar">
    <label class="search-box">
      <input
        class="field"
        bind:value={filter}
        placeholder="Filter by name or address"
        spellcheck="false"
      />
    </label>
    <span class="net-total">
      {rows.length} {rows.length === 1 ? "announce" : "announces"}
    </span>
  </div>

  {#if rows.length === 0}
    <p class="net-empty">
      {#if !chat.meshUp}
        The bridge is offline. Announce history is kept for 7 days, so it will
        return with the connection.
      {:else if filter}
        Nothing matches “{filter}”.
      {:else}
        No announces yet. History is kept for 7 days and capped at the newest 5000
        sightings, coalesced into ten-minute buckets.
      {/if}
    </p>
  {:else}
    <div class="net-table-wrap" bind:this={scrollBox} onscroll={onScroll}>
      <table class="net-table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Address</th>
            <th>Aspect</th>
            <th>Interface</th>
            <th>Hops</th>
            <th>First seen</th>
            <th>Last seen</th>
            <th>Count</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as row (row.address + ":" + row.bucket)}
            <tr class="net-row-static">
              <td>{row.name || "—"}</td>
              <td class="mono">{shortHash(row.address)}</td>
              <td>{row.aspect || "—"}</td>
              <td>{row.interface || "—"}</td>
              <td>{row.hops ?? "—"}</td>
              <td title={absoluteTime(row.first_seen)}>{formatRelative(row.first_seen)}</td>
              <td title={absoluteTime(row.last_seen)}>{formatRelative(row.last_seen)}</td>
              <td>{row.count}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if chat.announcesLoading}
        <p class="net-loading">Loading…</p>
      {/if}
    </div>
  {/if}
</div>
