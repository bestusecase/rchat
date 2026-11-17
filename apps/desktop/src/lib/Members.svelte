<script>
  import { chat } from "./store.svelte.js";
  import { peerReachable, shortAddress } from "./util.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  let { onPeer = null } = $props();

  const peers = $derived(chat.peers);
</script>

<aside class="members">
  <header class="members-head">
    Peers
    <span class="section-count">{peers.length}</span>
  </header>

  <div class="members-scroll">
    {#each peers as peer (peer.id)}
      <button
        class="member"
        type="button"
        title="Open {chat.peerLabel(peer.id)}'s profile"
        onclick={() => onPeer?.(peer.id)}
      >
        <Avatar
          id={peer.id}
          label={chat.peerLabel(peer.id)}
          size={32}
          status={peerReachable(peer, chat.transport, chat.peerReachability(peer))
            ? "online"
            : "offline"}
        />
        <div class="member-text">
          <span class="member-name">{chat.peerLabel(peer.id)}</span>
          <span class="member-sub">
            {shortAddress(peer.address)}
            {#if peer.bandwidth_class && peer.bandwidth_class !== "unknown"}
              <span class="bandwidth-chip" data-class={peer.bandwidth_class}>
                {peer.bandwidth_class}
              </span>
            {/if}
          </span>
        </div>
      </button>
    {:else}
      <p class="members-empty">
        Peers show up here when they announce themselves on the mesh. You can also
        add one by address.
      </p>
    {/each}
  </div>

  <footer class="members-foot">
    <span class="secure-line">
      <Icon name="lock" size={13} />
      Encrypted by Reticulum
    </span>
  </footer>
</aside>
