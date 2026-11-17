# 03 — Peer profile card interface

Status: resolved
Type: task
Blocked by: 02

## Goal

Clicking a peer's avatar, name or members-list row opens a card where their
nickname can be edited and notes written, with changes saved as you type.

## Work

- Interface state:
  - track the open peer id, a nickname draft, a notes draft and a save
    indicator;
  - resolve everything else live from `chat.peers` by id, never from a snapshot
    taken at open time;
  - expose open/close/save actions; debounce note writes (~500 ms) and flush on
    close; a failed write raises a toast and keeps the draft.
- Card component:
  - reuse the existing overlay/dialog markup so Esc, click-outside and the
    Android back guard behave like the other dialogs;
  - top to bottom: avatar at a large size (reusing `Avatar.svelte`, including the
    members-list reachability dot), nickname headline, display-name subtitle,
    notes textarea;
  - the subtitle is always the peer's announced display name, falling back to
    the short address when they have none; the headline falls back the same way;
  - the headline is a button with a pencil affordance beside it; activating
    either turns it into a select-all input;
  - Enter or blur commits, Esc restores the previous value, an empty value
    clears the alias (send `null`);
  - a quiet "Saved" indicator after a successful write; no Save button anywhere
    on the card;
  - aria: dialog role with an accessible name, labelled rename control, notes
    textarea labelled;
  - on Android the textarea stays above the keyboard via the tracked keyboard
    inset.
- Click wiring:
  - message avatar and message author name in the thread;
  - members-list peer row;
  - messages authored by the local identity route to the existing profile
    dialog instead;
  - leave the sidebar conversation row and the chat header unchanged.
- Escape handling must not close the card while the rename input is in its
  cancel path, and closing the card must flush the pending note.

## Acceptance

- All three surfaces open the card for the right peer; your own name opens the
  profile dialog.
- The headline shows an alias when set, otherwise the announced name, otherwise
  the short address; the subtitle always shows the display name or that
  fallback.
- Renaming updates the thread, sidebar and members list immediately, with no
  reload and no second name visible.
- Esc during editing restores the old name and does not close the card; a
  second Esc closes it.
- Clearing the nickname restores the display-name headline.
- Notes typed and the card closed are present after reopening; a multi-line note
  keeps its newlines.
- With the engine stopped or the bridge offline, opening the card and editing
  do not throw; a failed write surfaces as a toast.
- On the phone the back gesture closes the card, and the notes field is visible
  above the keyboard.

## Comments

- Shipped in `8f43587`: `PeerCard.svelte`, `peerCard` state and the chained notes
  writes in `store.svelte.js`, and the wiring in `App.svelte`, `Chat.svelte`,
  `Members.svelte`, `Icon.svelte` and `app.css`.
- Fixed in `22623aa`: the focus effect re-ran on every draft change, so the
  nickname input was reselected per keystroke and the notes field blurred after
  the first character. Touch typing lost all but one letter.
- The members list is desktop-only, so the phone has the avatar and author-name
  surfaces only. Laptop/mobile verification is recorded in `04`.
