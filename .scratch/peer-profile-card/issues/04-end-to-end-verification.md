# 04 — End-to-end verification on desktop and Android

Status: resolved
Type: task
Blocked by: 02, 03

## Goal

Prove the feature works on both shipped shells against a real engine, including
the cases automated tests cannot reach.

## Work

- Run the workspace test suite and confirm the store, migration, engine and IPC
  tests from tickets 01 and 02 pass.
- Desktop pass with the packaged-style workflow (engine plus interface):
  - open the card from a message avatar, a message author name and a members
    row;
  - rename, commit with Enter, commit by clicking away, cancel with Esc, clear
    to reset;
  - write a multi-line note, close, reopen, restart the engine, confirm both
    nickname and note survive;
  - confirm your own avatar still opens the profile dialog and that Add peer
    still works as before;
  - confirm a peer announce arriving after a rename does not overwrite it, and
    after a clear does not restore the old name.
- Android pass on the Pixel 8:
  - same three surfaces, rename and note on the touch keyboard;
  - back gesture closes the card, notes field stays above the keyboard;
  - force-stop and relaunch the app, confirm nickname and note persist.
- Regressions to check explicitly: message search unchanged, conversation
  selection unchanged, members list ordering unchanged, no dialog stacking when
  the profile dialog is already open.
- Record anything that differs between desktop and Android; if a difference is
  intentional, note why.

## Acceptance

- Automated suite green.
- Every desktop and Android checklist item passes, with any deviation written
  up rather than silently accepted.
- The card's data survives an engine restart on both platforms.

## Comments

- **Automated suite**: `cargo test --workspace` green at `22623aa`, run in a
  clean worktree because the working tree now carries another session's
  half-wired `ClientCmd::DismissInterfaceNotice` and does not compile. 47 core,
  14 ipc, 2 rns, 22 rchadt, 21 attachments, 5 engine; `socket_bridge`'s live
  bridge test is `ignored` without RNS installed. `npm run build` (126 modules)
  and `cargo check` in `apps/mobile/src-tauri` are clean.
- **Desktop**: the pass on `8f43587` stands (three surfaces, rename by
  Enter/blur/Esc/clear, multi-line note, restart persistence, announce does not
  overwrite a rename, own identity opens the profile dialog). Re-checked the
  typing path in a browser against the engine on 8799 after the focus fix below:
  per-keystroke typing accumulates in both the nickname input and the notes
  field, Enter commits, and the note flushes on close.
- **Android** (Pixel 8, `39061FDJH0080U`, debug arm64 build from the `22623aa`
  tree, Gboard visible):
  - Opened the card from a message avatar and from the message author name.
    Renamed with Enter, with a tap on the notes (blur), and with Esc
    (`KEYCODE_ESCAPE` cancels the edit and leaves the card open); cleared the
    field to fall back to the announced name; a 49-character alias was refused
    with `a nickname can be at most 48 characters`, the field stayed open and the
    badge showed `Not saved`.
  - Typed a two-line note with the soft keyboard: it saved after the debounce
    (`Saved`), the card sat fully above the keyboard (card bottom 341 CSS px in a
    485 px viewport, keyboard up), and closing then reopening kept the newlines.
  - System back closed the card and left the conversation open; a back press
    then closed the profile dialog, then left the conversation — layered as
    intended. Your own message avatar opened the profile dialog, not the card.
  - Force-stop and relaunch kept both the alias and the note.
  - Unchanged: search still disabled (`Search (coming soon)`), conversation
    selection and previews behave as before, and only one dialog can exist at a
    time.
- **Bug found on hardware and fixed in `22623aa`**: the card's focus effect read
  the whole card state, so it re-ran on every draft change. It reselected the
  nickname input mid-word and moved focus back to the dialog after the first
  character typed in the notes, so a touch keyboard lost all but one letter.
  The earlier desktop pass missed it because it set the field in one shot. The
  effect now only moves focus that is not already in place.
- **Deviations**:
  - The members row is desktop-only (`{#if !chat.mobile}` around `Members`), so
    the phone has two of the three surfaces, not three.
  - Tapping the notes while the nickname is being edited commits the alias but
    returns focus to the dialog, so the notes need a second tap.
  - One back press was swallowed after closing the card with its Close button and
    opening the profile dialog; not reproduced in three timed open/close cycles.
  - The device fixtures were seeded: the phone had no second node to converse
    with, so a peer learned from a real LAN announce got a conversation and one
    message written into its DB, and the peer's alias was reset. Announce
    behaviour was still observed for real — four announces updated
    `announced_name` and left the local alias alone — and
    clear-survives-an-announce is covered by the engine test.
  - The phone's app data was wiped twice by reinstalls (once as a fresh install
    at 23:15, once by a parallel session at 23:40), which ended the fixtures
    rather than the checks.
- **Unexplained, recorded rather than dismissed**: in the first device profile a
  peer learned from an announce appeared with `nickname = 'Desk'` and no
  announced name, although the only writer of that column is
  `set_peer_nickname` and the announce path stores names in `announced_name`.
  It never reproduced: on a wiped profile the same peer's announce wrote
  `announced_name` only, and four further announces left a deliberately set
  alias alone. Worth a second look if an alias ever appears that nobody typed.
