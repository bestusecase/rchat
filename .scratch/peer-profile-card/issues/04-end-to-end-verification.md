# 04 — End-to-end verification on desktop and Android

Status: ready-for-agent
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
