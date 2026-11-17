<script>
  import { chat } from "./store.svelte.js";
  import Avatar from "./Avatar.svelte";
  import Icon from "./Icon.svelte";

  let { onClose } = $props();
  let dialog = $state();
  let input = $state();
  let cancelled = $state(false);

  const card = $derived(chat.peerCard);

  $effect(() => {
    if (!dialog) return;
    if (card.editing) {
      // Select what is there: typing replaces the alias, clearing resets it.
      input?.focus();
      input?.select();
      return;
    }
    // Announcing the card on open, and returning focus when editing ends, so
    // an Esc-cancel does not strand focus at the top of the page.
    dialog.focus();
  });

  function startEdit() {
    cancelled = false;
    if (chat.peerCard.editing) {
      input?.focus();
      input?.select();
      return;
    }
    chat.startPeerNicknameEdit();
  }

  function onNicknameKeydown(event) {
    if (event.key === "Enter") {
      event.preventDefault();
      void chat.commitPeerNickname();
    } else if (event.key === "Escape") {
      // Esc discards the edit, not the whole card.
      event.preventDefault();
      event.stopPropagation();
      cancelled = true;
      chat.cancelPeerNicknameEdit();
    }
  }

  function onNicknameBlur() {
    if (cancelled) {
      cancelled = false;
      return;
    }
    void chat.commitPeerNickname();
  }

  function onDialogKeydown(event) {
    event.stopPropagation();
    if (event.key === "Escape") onClose();
  }
</script>

<div class="overlay" role="presentation" onclick={onClose}>
  <div
    class="dialog peer-card"
    role="dialog"
    aria-modal="true"
    aria-label="Peer profile"
    tabindex="-1"
    bind:this={dialog}
    onclick={(event) => event.stopPropagation()}
    onkeydown={onDialogKeydown}
  >
    <div class="peer-card-head">
      <Avatar
        id={card.peerId}
        label={chat.cardHeadline}
        size={72}
        status={chat.cardReachable ? "online" : "offline"}
      />
      <div class="peer-card-names">
        <div class="peer-card-name-row">
          {#if card.editing}
            <input
              bind:this={input}
              class="field peer-card-nickname"
              value={card.nickname}
              maxlength="48"
              spellcheck="false"
              aria-label="Nickname"
              oninput={(event) => chat.draftPeerNickname(event.currentTarget.value)}
              onkeydown={onNicknameKeydown}
              onblur={onNicknameBlur}
            />
          {:else}
            <button class="peer-card-headline" title="Rename this peer" onclick={startEdit}>
              {chat.cardHeadline}
            </button>
          {/if}
          <button
            class="icon-button peer-card-edit"
            title="Rename this peer"
            aria-label="Rename this peer"
            onclick={startEdit}
          >
            <Icon name="pencil" size={16} />
          </button>
        </div>
        <span class="peer-card-subtitle">{chat.cardSubtitle}</span>
      </div>
    </div>

    <label class="field-label">
      Notes
      <textarea
        class="field peer-card-notes"
        rows="5"
        placeholder="Anything worth remembering about them."
        value={card.notes}
        oninput={(event) => chat.draftPeerNotes(event.currentTarget.value)}
      ></textarea>
    </label>

    <div class="peer-card-foot">
      <span class="peer-card-status" aria-live="polite">
        {#if card.save === "saving"}
          Saving…
        {:else if card.save === "saved"}
          Saved
        {:else if card.save === "error"}
          Not saved
        {/if}
      </span>
      <button class="btn ghost" onclick={onClose}>Close</button>
    </div>
  </div>
</div>
