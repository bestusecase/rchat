<script>
  import { onDestroy } from "svelte";
  import { chat } from "./store.svelte.js";
  import { dayLabel, formatBytes, formatTime, shortAddress } from "./util.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  let { onBack = null } = $props();
  let viewport = $state();
  let input = $state();
  let draft = $state("");
  let sending = $state(false);
  let uploading = $state(false);
  let attachError = $state(null);
  /// Chosen but unsent: `{id, file, name, size, preview}`. Uploads happen at
  /// send time, so removing a chip costs nothing.
  let chips = $state([]);
  /// The mobile two-choice sheet. A desktop has no camera, so it never opens.
  let attachMenu = $state(false);
  /// The image open in the full-screen viewer, or null.
  let viewer = $state(null);
  let addInput = $state();
  /// The one place camera capture is declared. A QR scanner would reuse it.
  let cameraInput = $state();
  let dragging = $state(false);

  const conversation = $derived(chat.activeConversation);
  const canSend = $derived(draft.trim().length > 0 || chips.length > 0);

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
    const inset = chat.keyboardInset;
    if (!viewport || !id) return;
    void count;
    void inset;
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

  onDestroy(clearChips);

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

  function isImage(attachment) {
    return attachment.mime?.startsWith("image/");
  }

  function pictures(message) {
    return (message.attachments ?? []).filter(
      (item) => isImage(item) && item.reason === undefined,
    );
  }

  function documents(message) {
    return (message.attachments ?? []).filter(
      (item) => !(isImage(item) && item.reason === undefined),
    );
  }

  /// What a screen reader hears for the attachment block, so an image message
  /// is not an empty bubble with a picture in it.
  function attachmentLabel(message) {
    const pics = pictures(message);
    const docs = documents(message);
    const caption = message.body?.trim();
    if (pics.length && !docs.length) {
      return caption ? `Image: ${caption}` : `Image: ${pics[0].name}`;
    }
    if (docs.length === 1 && !pics.length) return `File: ${docs[0].name}`;
    return caption ? `Attachments: ${caption}` : `Attachments: ${(message.attachments ?? []).length}`;
  }

  function imageAlt(message, picture) {
    const caption = message.body?.trim();
    return caption ? `Image: ${caption}` : `Image: ${picture.name}`;
  }

  /// A stable aspect box stops the thread from jumping while thumbnails load.
  function ratioStyle(attachment) {
    if (!attachment.width || !attachment.height) return "";
    return `aspect-ratio: ${attachment.width} / ${attachment.height};`;
  }

  function openViewer(attachment) {
    if (attachment.reason) return;
    viewer = attachment;
  }

  function closeViewer() {
    viewer = null;
  }

  function onWindowKeydown(event) {
    if (event.key === "Escape" && viewer) {
      event.preventDefault();
      closeViewer();
    }
  }

  async function save(attachment) {
    try {
      await chat.saveAttachment(attachment);
    } catch (err) {
      chat.toast(err?.message ?? String(err));
    }
  }

  /// Add chosen files to the chips. Uploading waits for send, so a change of
  /// mind is free.
  function addFiles(list) {
    const incoming = Array.from(list ?? []).filter(Boolean);
    if (incoming.length === 0) return;
    attachError = null;
    const room = 4 - chips.length;
    if (room <= 0) {
      attachError = "A message can carry at most four attachments.";
      return;
    }
    if (incoming.length > room) {
      attachError = "A message can carry at most four attachments.";
    }
    const added = incoming.slice(0, room).map((file) => ({
      id: Math.random().toString(36).slice(2),
      file,
      name: file.name || "attachment",
      size: file.size,
      mime: file.type,
      preview: file.type?.startsWith("image/") ? URL.createObjectURL(file) : null,
    }));
    chips = [...chips, ...added];
    if (addInput) addInput.value = "";
    if (cameraInput) cameraInput.value = "";
  }

  function removeChip(chip) {
    if (chip.preview) URL.revokeObjectURL(chip.preview);
    chips = chips.filter((item) => item.id !== chip.id);
  }

  function clearChips() {
    for (const chip of chips) {
      if (chip.preview) URL.revokeObjectURL(chip.preview);
    }
    chips = [];
  }

  function openPicker() {
    attachError = null;
    if (chat.mobile) {
      // Android's document picker cannot reach the camera, so the plus asks
      // which one is wanted. Desktop goes straight to the picker.
      attachMenu = !attachMenu;
      return;
    }
    addInput?.click();
  }

  function pickCamera() {
    attachMenu = false;
    cameraInput?.click();
  }

  function pickFiles() {
    attachMenu = false;
    addInput?.click();
  }

  function onPick(event) {
    addFiles(event.currentTarget.files);
  }

  function onPaste(event) {
    const files = event.clipboardData?.files;
    if (files?.length) {
      event.preventDefault();
      addFiles(files);
    }
  }

  function onDrop(event) {
    dragging = false;
    event.preventDefault();
    if (event.dataTransfer?.files?.length) addFiles(event.dataTransfer.files);
  }

  function onDragOver(event) {
    if (!event.dataTransfer?.types?.includes("Files")) return;
    event.preventDefault();
    dragging = true;
  }

  function onDragLeave(event) {
    if (event.currentTarget.contains(event.relatedTarget)) return;
    dragging = false;
  }

  async function submit() {
    if (sending || uploading) return;
    const text = draft.trim();
    if (!text && chips.length === 0) return;
    sending = true;
    attachError = null;
    try {
      const hashes = [];
      if (chips.length) {
        uploading = true;
        for (const chip of chips) {
          const descriptor = await chat.uploadAttachment(chip.file);
          hashes.push(descriptor.hash);
        }
        uploading = false;
      }
      await chat.sendMessage(text, hashes);
      draft = "";
      clearChips();
    } catch (err) {
      // The chips stay: a failed upload leaves the same choice on screen.
      attachError = err?.message ?? String(err);
      chat.toast(attachError);
    } finally {
      uploading = false;
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

<svelte:window onkeydown={onWindowKeydown} />

<section
  class="chat"
  class:dragging
  ondragover={onDragOver}
  ondragleave={onDragLeave}
  ondrop={onDrop}
>
  <header class="chat-head">
    {#if chat.mobile && onBack}
      <button
        class="icon-button back-button"
        title="Back to conversations"
        aria-label="Back to conversations"
        onclick={onBack}
      >
        <Icon name="back" size={22} />
      </button>
    {/if}
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

              {#if (row.message.attachments ?? []).length}
                {@const pics = pictures(row.message)}
                {@const docs = documents(row.message)}
                <div
                  class="attachments"
                  role="group"
                  aria-label={attachmentLabel(row.message)}
                >
                  {#if pics.length === 1}
                    <button
                      type="button"
                      class="attach-single"
                      title="Open image"
                      onclick={() => openViewer(pics[0])}
                    >
                      <img
                        src={chat.attachmentUrl(pics[0], "thumb")}
                        alt={imageAlt(row.message, pics[0])}
                        style={ratioStyle(pics[0])}
                        loading="lazy"
                      />
                    </button>
                  {:else if pics.length > 1}
                    <div class="attach-grid" data-count={Math.min(pics.length, 4)}>
                      {#each pics.slice(0, 4) as picture, index (picture.hash)}
                        <button
                          type="button"
                          class="attach-cell"
                          title="Open image"
                          onclick={() => openViewer(picture)}
                        >
                          <img
                            src={chat.attachmentUrl(picture, "thumb")}
                            alt={imageAlt(row.message, picture)}
                            loading="lazy"
                          />
                          {#if index === 3 && pics.length > 4}
                            <span class="attach-more">+{pics.length - 4}</span>
                          {/if}
                        </button>
                      {/each}
                    </div>
                  {/if}

                  {#each docs as attachment (attachment.hash + attachment.name + attachment.ordinal)}
                    {#if attachment.reason}
                      <div class="attach-missing" role="note">
                        <Icon name="alert" size={15} />
                        <span class="attach-missing-text">
                          <span class="attach-name">{attachment.name}</span>
                          <span class="attach-size">{attachment.reason}</span>
                        </span>
                      </div>
                    {:else}
                      <div class="attach-card">
                        <span class="attach-glyph" aria-hidden="true">
                          <Icon name="file" size={20} />
                        </span>
                        <span class="attach-meta">
                          <span class="attach-name">{attachment.name}</span>
                          <span class="attach-size">{formatBytes(attachment.size)}</span>
                        </span>
                        <button
                          type="button"
                          class="attach-save"
                          title="Save {attachment.name}"
                          aria-label="Save {attachment.name}"
                          onclick={() => save(attachment)}
                        >
                          <Icon name="download" size={16} />
                        </button>
                      </div>
                    {/if}
                  {/each}
                </div>
              {/if}

              {#if row.message.body}
                <p class="message-text">{row.message.body}</p>
              {/if}
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

  {#if chips.length || attachError}
    <div class="composer-chips">
      {#each chips as chip (chip.id)}
        <span class="chip" class:uploading>
          {#if chip.preview}
            <img class="chip-thumb" src={chip.preview} alt="" />
          {:else}
            <span class="chip-glyph" aria-hidden="true"><Icon name="file" size={15} /></span>
          {/if}
          <span class="chip-name" title={chip.name}>{chip.name}</span>
          <span class="chip-size">{formatBytes(chip.size)}</span>
          <button
            type="button"
            class="chip-remove"
            title="Remove {chip.name}"
            aria-label="Remove {chip.name}"
            onclick={() => removeChip(chip)}
          >
            <Icon name="close" size={13} />
          </button>
        </span>
      {/each}
      {#if attachError}
        <span class="chip-error" role="alert">{attachError}</span>
      {/if}
    </div>
  {/if}

  <form class="composer" onsubmit={(event) => { event.preventDefault(); submit(); }}>
    <input
      bind:this={addInput}
      type="file"
      multiple
      hidden
      onchange={onPick}
    />
    <input
      bind:this={cameraInput}
      type="file"
      accept="image/*"
      capture="environment"
      hidden
      onchange={onPick}
    />
    <div class="composer-add-wrap">
      <button
        type="button"
        class="composer-add"
        class:active={attachMenu}
        title="Attach a photo or file"
        aria-label="Attach a photo or file"
        aria-expanded={chat.mobile ? attachMenu : undefined}
        onclick={openPicker}
      >
        <Icon name="plus" size={20} />
      </button>
      {#if attachMenu}
        <div class="attach-menu" role="menu">
          <button type="button" role="menuitem" onclick={pickCamera}>
            <Icon name="camera" size={18} />
            Camera
          </button>
          <button type="button" role="menuitem" onclick={pickFiles}>
            <Icon name="file" size={18} />
            Photo or file
          </button>
        </div>
        <button
          type="button"
          class="attach-menu-backdrop"
          aria-label="Close attachment menu"
          onclick={() => (attachMenu = false)}
        ></button>
      {/if}
    </div>
    <textarea
      bind:this={input}
      bind:value={draft}
      onkeydown={onKeydown}
      onpaste={onPaste}
      placeholder={'Message ' + chat.conversationTitle(conversation)}
      rows="1"
      spellcheck="false"
    ></textarea>
    <button
      type="submit"
      class="composer-send"
      class:ready={canSend}
      disabled={!canSend || sending}
      title={uploading ? "Uploading…" : "Send"}
    >
      <Icon name="send" size={18} />
    </button>
  </form>

  <div class="composer-hint">
    <Icon name="lock" size={12} />
    End-to-end encrypted by Reticulum · Enter to send, Shift+Enter for a new line
  </div>

  {#if viewer}
    <div
      class="viewer"
      role="dialog"
      aria-modal="true"
      aria-label={viewer.name ? `Image: ${viewer.name}` : "Image"}
      onclick={closeViewer}
    >
      <button
        type="button"
        class="viewer-close"
        title="Close"
        aria-label="Close"
        onclick={closeViewer}
      >
        <Icon name="close" size={20} />
      </button>
      <img
        class="viewer-image"
        src={chat.attachmentUrl(viewer)}
        alt={viewer.name ? `Image: ${viewer.name}` : "Image"}
        onclick={(event) => event.stopPropagation()}
      />
      <button
        type="button"
        class="viewer-save"
        onclick={(event) => {
          event.stopPropagation();
          save(viewer);
        }}
      >
        <Icon name="download" size={16} />
        Save
      </button>
    </div>
  {/if}
</section>
