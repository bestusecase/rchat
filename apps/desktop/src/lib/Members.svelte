<script>
  import { chat } from "./store.svelte.js";
  import { peerReachable, shortAddress } from "./util.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  const peers = $derived(chat.peers);
</script>

<aside class="members">
  <header class="members-head">
    Peers
    <span class="section-count">{peers.length}</span>
  </header>

  <div class="members-scroll">
    {#each peers as peer (peer.id)}
      <div class="member">
        <Avatar
          id={peer.id}
          label={peer.nickname || peer.announced_name || peer.id}
          size={32}
          status={peerReachable(peer, chat.transport, chat.peerReachability(peer))
            ? "online"
            : "offline"}
        />
        <div class="member-text">
          <span class="member-name">
            {peer.nickname || peer.announced_name || peer.id.slice(0, 8)}
          </span>
          <span class="member-sub">
            {shortAddress(peer.address)}
            {#if peer.bandwidth_class && peer.bandwidth_class !== "unknown"}
              <span class="bandwidth-chip" data-class={peer.bandwidth_class}>
                {peer.bandwidth_class}
              </span>
            {/if}
          </span>
        </div>
      </div>
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
