<script>
  import { chat } from "./store.svelte.js";
  import { shortAddress } from "./util.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  let { onAddPeer, onProfile, onSettings } = $props();
</script>

<div class="shell-left">
  <nav class="rail">
    <div class="brand" title="rchat">
      <span class="brand-mark">r</span>
    </div>

    <button class="rail-button active" title="Home">
      <Icon name="home" size={22} />
    </button>

    <div class="rail-gap"></div>

    <button class="rail-button" title="Add a peer" onclick={onAddPeer}>
      <Icon name="plus" size={22} />
    </button>

    <button class="profile-button" title="Your profile" onclick={onProfile}>
      <Avatar
        id={chat.identity?.id}
        label={chat.identity?.display_name || "you"}
        size={38}
        status={chat.transport === "ready" ? "online" : "offline"}
      />
    </button>
  </nav>

  <aside class="sidebar">
    <header class="sidebar-head">
      <div>
        <div class="wordmark">rchat</div>
        <div class="engine-line">
          <span class="dot" data-state={chat.transport}></span>
          {chat.transportLabel}
        </div>
      </div>
      <button class="icon-button" title="Add a peer" onclick={onAddPeer}>
        <Icon name="plus" size={18} />
      </button>
    </header>

    <div class="sidebar-scroll">
      <div class="section-label">
        Direct messages
        <span class="section-count">{chat.conversations.length}</span>
      </div>

      {#if chat.conversations.length === 0}
        <div class="sidebar-empty">
          <p>No conversations yet.</p>
          <button class="link-button" onclick={onAddPeer}>Add your first peer</button>
        </div>
      {:else}
        <div class="conversation-list">
          {#each chat.conversations as conversation (conversation.id)}
            <button
              class="conversation"
              class:active={conversation.id === chat.activeId}
              onclick={() => chat.select(conversation.id)}
            >
              <Avatar
                id={conversation.peer_ids?.[0] ?? conversation.id}
                label={chat.conversationTitle(conversation)}
                size={38}
              />
              <span class="conversation-text">
                <span class="conversation-name">{chat.conversationTitle(conversation)}</span>
                <span class="conversation-preview">
                  {conversation.preview || "No messages yet"}
                </span>
              </span>
              {#if conversation.unread > 0}
                <span class="unread-pill">{conversation.unread > 99 ? "99+" : conversation.unread}</span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <footer class="sidebar-foot">
      <Avatar
        id={chat.identity?.id}
        label={chat.identity?.display_name || "you"}
        size={34}
        status={chat.transport === "ready" ? "online" : "offline"}
      />
      <div class="me-text">
        <span class="me-name">{chat.identity?.display_name || "you"}</span>
        <span class="me-address">{shortAddress(chat.identity?.address)}</span>
      </div>
      <button class="icon-button" title="Settings and network" onclick={onSettings}>
        <Icon name="gear" size={18} />
      </button>
    </footer>
  </aside>
</div>
