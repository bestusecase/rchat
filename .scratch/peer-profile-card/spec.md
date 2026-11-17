# Peer profile card: local nicknames and notes

Status: ready-for-agent
Effort: `peer-profile-card`

## Problem Statement

rchat lets you talk to a peer but not remember anything about them. Their name is
whatever they last announced over the mesh, which is often a callsign, a device
name, or nothing at all — and it can change whenever they announce again. There
is nowhere to write "the person from the Tuesday radio net", and no way to give
someone a name that means something to you without pretending it is theirs.

Clicking a peer's avatar or name does nothing at all, even though those are the
most obvious "who is this?" affordances in the interface. The only peer-level
surface is the Add peer dialog, whose nickname field is write-once in practice:
it also stamps the peer as freshly seen, so it cannot be used to rename someone
honestly.

The available profile surfaces are also misleading. The interface calls the
generated initials circle a profile picture, but nothing about a peer is
displayed larger than a list thumbnail, so there is no place to look at a peer
as a subject rather than as a row.

## Solution

Clicking a peer's avatar or name — in a message or in the members list — opens a
peer profile card.

The card shows the peer large: an enlarged version of the avatar, their nickname
as the headline, their display name underneath in smaller text, and a free-form
notes field. The nickname is a **local alias** that starts out as the peer's
display name and only becomes yours once you edit it. Clicking the headline (or
the pencil beside it) turns it into an input; Enter or clicking away saves, Esc
discards, and clearing it removes your alias so the headline follows their
display name again.

Notes and nicknames are saved as you type, with no Save button, and live in the
engine's database next to the rest of the conversation state. They survive
engine restarts, they are never sent over the mesh, and an announce from the
peer never overwrites them.

Clicking your own avatar or name keeps opening the existing profile dialog: the
card is for other people.

## User Stories

1. As someone reading a conversation, I want to click a peer's avatar in a
   message, so that I can see who they are and write a note about them.
2. As someone reading a conversation, I want to click a peer's name in a
   message, so that I do not have to aim at a small circle.
3. As someone browsing the members list, I want to click a peer's row, so that I
   can inspect someone I have not messaged yet.
4. As someone opening the card, I want to see a large version of the peer's
   avatar, so that the card reads as a profile rather than a tooltip.
5. As someone opening the card, I want to see their nickname as the headline,
   so that I immediately see the name I know them by.
6. As someone who has never renamed a peer, I want their announced display name
   to be the default nickname, so that the card is useful without any setup.
7. As someone who has never renamed a peer, I want their later announce to
   change the headline, so that I am not stuck with a stale name.
8. As someone opening the card, I want their display name in smaller text
   underneath, so that I can tell my alias from their real name.
9. As someone opening the card, I want the display name shown even when it is
   identical to the headline, so that the layout never shifts under me.
10. As someone opening the card for a peer with no announced name, I want the
    short address shown instead of a blank line, so that the card never looks
    broken.
11. As someone opening the card for a peer with no announced name, I want the
    headline to fall back to their short address, so that I can still tell
    which peer I am looking at.
12. As someone who wants to rename a peer, I want to click their nickname, so
    that I can edit it in place.
13. As someone who wants to rename a peer, I want a pencil affordance next to
    the nickname, so that I can discover that it is editable.
14. As someone renaming a peer, I want Enter or clicking away to save, so that
    the interaction matches normal text fields.
15. As someone renaming a peer, I want Esc to discard my edit, so that I can
    back out without retyping the old name.
16. As someone renaming a peer, I want clearing the field to remove my alias,
    so that the headline goes back to their display name.
17. As someone renaming a peer, I want whitespace trimmed and newlines refused,
    so that I cannot create a name that breaks list layout.
18. As someone renaming a peer, I want a length limit, so that a long paste
    cannot overflow the sidebar or the members list.
19. As someone renaming a peer, I want the change to apply everywhere at once,
    so that the sidebar, thread and members list agree.
20. As someone renaming a peer, I want the peer never to learn about it, so that
    my private name for them stays private.
21. As someone writing notes, I want a multi-line field, so that I can write
    more than one sentence.
22. As someone writing notes, I want the text saved shortly after I stop typing,
    so that I never lose it by closing the card.
23. As someone writing notes, I want the note saved when I close the card, so
    that the last keystrokes are not dropped.
24. As someone writing notes, I want no Save button, so that the card behaves
    like a scratchpad rather than a form.
25. As someone writing notes, I want a quiet confirmation that the note was
    saved, so that I trust it persisted.
26. As someone writing notes, I want a failure surfaced as a toast, so that I
    know if the engine did not take the write.
27. As someone who restarts rchat, I want my nicknames and notes to still be
    there, so that they are worth writing.
28. As a user receiving a new announce from a peer, I want my nickname and notes
    left untouched, so that their announcements cannot erase my work.
29. As a user who cleared a nickname, I want it to stay cleared after the next
    announce, so that the reset is durable.
30. As a user on Android, I want the same card as the desktop, so that both
    clients behave the same way.
31. As an Android user, I want the system back gesture to close the card, so
    that it behaves like the other dialogs.
32. As an Android user, I want the notes field to stay above the keyboard, so
    that I can see what I am typing.
33. As someone who clicks their own avatar or name, I want my existing profile
    dialog, so that I can change my real display name instead of aliasing
    myself.
34. As someone who clicks a peer with no stored record, I want the card to open
    anyway, so that old data or an unexpected author id does not dead-end.
35. As someone writing a note about a peer with no stored record, I want saving
    to create that record, so that the note is kept.
36. As someone who renames a peer and closes the card, I want the headline, the
    sidebar name and the thread name to already reflect it, so that I never see
    two names for one person.
37. As someone using the card while offline from the mesh, I want it to work
    normally, so that notes do not depend on connectivity.
38. As someone reading a name in a message, I want clicking my own name to never
    create a note about myself, so that the card's meaning stays "other people".
39. As someone searching messages later, I want notes excluded from message
    search results, so that search means "what was said", not "what I wrote".
40. As someone using a screen reader, I want the card announced as a dialog and
    the rename control labelled, so that I can use it without a pointer.

## Implementation Decisions

**Schema and migration.** `peers` gains a nullable `notes TEXT` column. The
existing migration is `CREATE TABLE IF NOT EXISTS`-only and therefore does not
alter existing databases, so the new column is added by a guarded `ALTER TABLE`
that first checks the table's column list; the guard makes it safe on every
startup. Schema metadata version moves from 2 to 3. Existing peer rows keep
their id, nickname, announced name and timestamps.

**IPC types.** `PeerInfo` gains `notes: Option<String>`, serialised with a
default so payloads from an older engine still parse and an older client still
parses a new snapshot. Because peers already travel in the snapshot and in
`peer.updated`, the interface needs no new read path: notes arrive with the peer.

**IPC commands.** Two narrow commands rather than reusing Add peer:

- `set_peer_nickname` with `peer_id` and an optional `nickname`.
- `set_peer_notes` with `peer_id` and an optional `notes`.

Both return `{ "peer": PeerInfo }` and publish `peer.updated` with the full peer,
so every view updates from one event. Add peer keeps its current behaviour
(including the `last_seen` stamp) and is not used for renames.

**Validation and normalisation.** Nicknames are trimmed, newlines are replaced
with spaces, and the result is capped at 48 characters — the same limit as the
local display name. An empty result is stored as NULL, which is what "clears the
override" means. Notes are stored as written apart from trailing whitespace, are
allowed to contain newlines, and have no length cap. An empty note is stored as
NULL. A `peer_id` with no stored peer creates a minimal peer row (id, and
address when the id looks like a 32-character hex LXMF address) before the
write, mirroring what incoming-message handling already does.

**Alias semantics.** `nickname` means one thing: the override the user typed.
The effective name is `nickname`, else the peer's announced display name, else
the short address. Nothing is copied from the announce into `nickname`; the
headline therefore follows the peer's later renames until the user types an
alias. `peerLabel` already implements this precedence and remains the single
source for all displayed peer names.

**Announce interaction.** The peer upsert path must leave `notes` untouched and
must not resurrect a nickname the user cleared. This is an explicit requirement
on the upsert statement, not an accident of column ordering.

**Interface state.** The interface store owns the open card's peer id, an
in-progress nickname draft, a notes draft, and a save indicator. The card renders
from live store data by peer id, never from a snapshot captured at open time, so
an announce that renames the peer while the card is open is reflected
immediately. Writes are debounced (roughly half a second) and flushed when the
card closes; a failed write raises a toast and leaves the draft intact.

**Card composition.** A centred modal dialog built from the same overlay and
dialog primitives as the existing Add peer, profile and settings dialogs,
therefore inheriting Esc-to-close, click-outside-to-close and the mobile back
guard. Top to bottom: enlarged avatar (with the same reachability dot the
members list computes), editable nickname headline, display-name subtitle,
notes textarea. The name is a button until clicked, then an input; the pencil is
a separate labelled control that focuses the same input.

**Click wiring.** Three surfaces open the card: a message avatar, a message
author name, and a members-list row. A message authored by the local identity
routes to the existing profile dialog instead. The sidebar conversation row and
the chat header keep their current behaviour.

**Avatar as a seam for real pictures.** The card reuses the existing avatar
component at a larger size rather than drawing its own. When real profile
pictures arrive, that component gains an image source and the card picks it up
without restructuring.

**Accessibility and mobile.** The card is a modal dialog with a label; the
rename control has an accessible name; focus moves into the field when editing
starts and the field is select-all on entry. The notes field scrolls above the
Android keyboard using the keyboard inset the interface already tracks.

**Explicitly not changed.** The Add peer dialog, the local profile dialog, and
message search. Notes do not join the message full-text index.

## Testing Decisions

A good test here asserts externally observable behaviour: what a store call
writes and reads back, what a command returns, and what event it emits. It does
not assert on SQL text, private helpers, or argument ordering. The data layer is
where a mistake destroys user data, so that is where the automated coverage
goes. Two existing seams cover the feature; no new seam is introduced.

**Seam 1 — the store (`rchat-core`).** Highest existing seam for persistence and
alias precedence, using the existing in-memory store helper for behaviour and a
temporary file for persistence and migration. Cases:

- Setting a nickname then reading the peer back returns it.
- Clearing the nickname yields an empty nickname, and the effective label falls
  back to the announced name.
- Notes round-trip, including newlines.
- Reopening the same file-backed database returns the same nickname and notes.
- An announce-style upsert after a rename keeps the nickname; after a clear it
  keeps the nickname empty; it never changes notes.
- A database created with the previous schema is opened by the new code and the
  `notes` column exists, with pre-existing peer fields intact.

Prior art: the existing store tests use the in-memory store and already cover
identity, conversations, message ordering and event history; `tempfile` is
already a dev-dependency for on-disk cases.

**Seam 2 — engine command handling (`rchatd`).** Exercises validation,
persistence, the returned payload and the published event through the same
command path the socket uses. Cases:

- `set_peer_nickname` returns the updated peer and records a `peer.updated`
  event whose payload carries the new nickname.
- An over-long nickname is rejected and nothing is written.
- An empty nickname clears the alias.
- `set_peer_notes` persists multi-line notes and emits `peer.updated`.
- A command for an unknown peer creates the record and succeeds.

Prior art: the existing engine tests construct an in-memory app state and call
command handling directly, and the store's event history is already used to
assert published events.

**Not automated.** The interface has no test harness. The card's behaviour is
verified manually on desktop and on Android: open from all three surfaces,
rename, cancel with Esc, clear to reset, write notes, close and reopen, restart
the engine, and confirm both persist; confirm your own avatar still opens the
profile dialog, and that the back gesture closes the card on the phone.

## Out of Scope

- Real image profile pictures. The card only enlarges the generated avatar; the
  avatar component is reused so images can be added later without reshaping the
  card.
- Changing a peer's actual display name, or telling them they were renamed.
  Nicknames are local-only and never cross the mesh.
- Notes in message search, or any search UI.
- Opening the card from the sidebar conversation row or the chat header.
- Group details, member roles, or any group-level profile surface.
- Editing or deleting messages, and attachments.

## Further Notes

- The repository has no `GLOSSARY.md` and no ADRs yet, so there is no existing
  vocabulary or decision to contradict. This spec uses the project's current
  terms: peer, announce, display name (what a peer announces), nickname (the
  local alias), members list, engine, bridge.
- The distinction between display name and nickname is the load-bearing part of
  this feature. Implementations should not introduce a third term for either.
- Android and desktop share one interface, so every interaction decision here
  applies to both unless stated otherwise.
- There is no peer deletion, so notes and nicknames have no orphan lifecycle to
  design yet.
