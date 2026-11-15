<script>
  import { chat } from "./store.svelte.js";
  import { dayLabel, formatTime, shortAddress } from "./util.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  let viewport = $state();
  let input = $state();
  let draft = $state("");
  let sending = $state(false);

  const conversation = $derived(chat.activeConversation);

  const rows = $derived.by(() => {
    const list = chat.activeMessages;
    const out = [];
    let lastDay = null;
    let previous = null;
    for (const message of list) {
      const day = dayLabel(message.ts);
      if (day !== lastDay) {
        out.push({ kind: "day", key: "day-" + day + "-" + message.ts, label: day });
        lastDay = day;
      }
      const grouped =
        previous !== null &&
        previous.author_id === message.author_id &&
        message.ts - previous.ts < 5 * 60 * 1000;
      out.push({ kind: "msg", key: message.id, message, grouped });
      previous = message;
    }
    return out;
  });

  $effect(() => {
    const id = chat.activeId;
    const count = chat.activeMessages.length;
    if (!viewport || !id) return;
    void count;
    requestAnimationFrame(() => {
      viewport.scrollTop = viewport.scrollHeight;
    });
  });

  $effect(() => {
    const value = draft;
    if (!input) return;
    void value;
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 168) + "px";
  });

  function stateIcon(state) {
    switch (state) {
      case "failed":
        return "alert";
      case "delivered":
        return "checks";
      case "sent":
        return "check";
      default:
        return "clock";
    }
  }

  function stateLabel(state) {
    switch (state) {
      case "queued":
        return "Waiting for a connection";
      case "sending":
        return "Sending";
      case "sent":
        return "Sent";
      case "delivered":
        return "Delivered";
      case "failed":
        return "Not delivered — the peer may be unreachable";
      default:
        return state;
    }
  }

  async function submit() {
    if (sending || !draft.trim()) return;
    sending = true;
    try {
      await chat.sendMessage(draft);
      draft = "";
    } catch (err) {
      chat.toast(err.message);
    } finally {
      sending = false;
    }
  }

  function onKeydown(event) {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  }
</script>

<section class="chat">
  <header class="chat-head">
    <div class="chat-title-block">
      <h1>{chat.conversationTitle(conversation)}</h1>
      <span class="chat-sub">
        {#if conversation?.kind === "direct"}
          {shortAddress(chat.conversationAddress(conversation))}
        {:else}
          {conversation?.peer_ids?.length ?? 0} peers
        {/if}
      </span>
    </div>

    <div class="chat-actions">
      <span class="transport-pill" data-state={chat.transport}>
        <span class="dot" data-state={chat.transport}></span>
        {chat.transport === "ready" ? "Mesh online" : chat.transport === "starting" ? "Connecting" : "Offline"}
      </span>
      <button class="icon-button" title="Search (coming soon)" disabled>
        <Icon name="search" size={18} />
      </button>
      <button class="icon-button voice" title="Voice calls are planned for a later update" disabled>
        <Icon name="phone" size={18} />
      </button>
    </div>
  </header>

  <div class="messages" bind:this={viewport}>
    {#if rows.length === 0}
      <div class="chat-empty">
        <div class="chat-empty-mark">
          <Icon name="wave" size={30} />
        </div>
        <h2>This is the beginning</h2>
        <p>Messages are delivered peer to peer over Reticulum. Say hello.</p>
      </div>
    {:else}
      {#each rows as row (row.key)}
        {#if row.kind === "day"}
          <div class="day-separator">
            <span>{row.label}</span>
          </div>
        {:else}
          <article class="message" class:grouped={row.grouped}>
            {#if row.grouped}
              <span class="message-time-hover">{formatTime(row.message.ts)}</span>
            {:else}
              <Avatar
                id={row.message.author_id}
                label={chat.isMine(row.message) ? chat.identity?.display_name : chat.peerLabel(row.message.author_id)}
                size={40}
              />
            {/if}

            <div class="message-body">
              {#if !row.grouped}
                <div class="message-meta">
                  <span class="message-author">
                    {chat.isMine(row.message) ? chat.identity?.display_name || "you" : chat.peerLabel(row.message.author_id)}
                  </span>
                  <span class="message-time">{formatTime(row.message.ts)}</span>
                  {#if row.message.route === "reticulum"}
                    <span class="route-chip">mesh</span>
                  {/if}
                </div>
              {/if}
              <p class="message-text">{row.message.body}</p>
            </div>

            {#if chat.isMine(row.message)}
              <span class="message-state" data-state={row.message.state} title={stateLabel(row.message.state)}>
                <Icon name={stateIcon(row.message.state)} size={14} />
              </span>
            {/if}
          </article>
        {/if}
      {/each}
    {/if}
  </div>

  <form class="composer" onsubmit={(event) => { event.preventDefault(); submit(); }}>
    <button
      type="button"
      class="composer-add"
      title="Attachments are coming in a later update"
      disabled
    >
      <Icon name="plus" size={20} />
    </button>
    <textarea
      bind:this={input}
      bind:value={draft}
      onkeydown={onKeydown}
      placeholder={'Message ' + chat.conversationTitle(conversation)}
      rows="1"
      spellcheck="false"
    ></textarea>
    <button
      type="submit"
      class="composer-send"
      class:ready={draft.trim().length > 0}
      disabled={!draft.trim() || sending}
      title="Send"
    >
      <Icon name="send" size={18} />
    </button>
  </form>

  <div class="composer-hint">
    <Icon name="lock" size={12} />
    End-to-end encrypted by Reticulum · Enter to send, Shift+Enter for a new line
  </div>
</section>
