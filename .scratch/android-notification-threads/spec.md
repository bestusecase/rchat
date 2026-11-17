# Android notification threads

Status: resolved

## Problem Statement

When rchat is backgrounded on Android, an incoming message produces a
notification that says only `3 new messages`. It names no sender, no
conversation and no content, so the person holding the phone has to unlock it
and open the app to learn whether the message matters. Messages from every
conversation collapse into one number, so a chatty peer drowns out a single
important one. The v1 posture — that message text never leaves the app — was a
deliberate privacy trade-off, but in practice it makes the notification useless
as a glance surface, which is the only reason a notification exists.

## Solution

The notification becomes a conversation thread.

Each conversation that has unseen messages gets its own child notification under
the app's single rchat notification, titled with the conversation — the peer's
name for a direct chat — listing the messages that arrived while nobody could
see the app, oldest at the top and newest at the bottom, each labelled with its
sender and body. The parent summarises the whole set: `2 conversations · 5 new
messages`, with one line per conversation. A long body ellipsizes when the
notification is collapsed and shows in full when it is expanded. With the screen
locked, only the count is visible; the text appears once the device is unlocked.

## User Stories

1. As someone with the phone in their pocket, I want the notification to name
   the sender, so that I can decide whether to stop what I am doing.
2. As someone glancing at the shade, I want to see the message text, so that I
   can answer a question without opening the app.
3. As someone with several active conversations, I want a separate notification
   per conversation, so that one person cannot bury another.
4. As someone with several active conversations, I want a summary that counts
   conversations and messages, so that I know the size of what is waiting.
5. As someone with several active conversations, I want the summary to list one
   line per conversation, so that I can see who wrote most recently.
6. As someone receiving a burst from one person, I want those messages stacked in
   one notification in the order they were sent, so that the conversation reads
   naturally.
7. As someone receiving a burst, I want the newest message at the bottom, so that
   the collapsed notification shows the latest thing said.
8. As someone receiving a long message, I want it to end in an ellipsis rather
   than break the layout, so that the notification stays readable.
9. As someone receiving a long message, I want to expand the notification and
   read more of it, so that I can judge whether it needs an immediate reply.
10. As someone whose phone is locked on a desk, I want the lock screen to show no
    message text, so that bystanders learn nothing about my conversations.
11. As someone whose phone is locked, I want the lock screen to still say how
    many messages are waiting, so that I know to pick it up.
12. As someone with a device configured to hide sensitive notification content, I
    want the system setting respected rather than overridden by the app.
13. As someone reading the app, I want no notification for messages I am already
    looking at, so that the shade does not fill with things I have seen.
14. As someone who glanced at the app and then pocketed the phone, I want the
    next notification to contain only messages that arrived after I stopped
    looking, so that the count is not inflated by already-read text.
15. As someone who reads a conversation in the app, I want the notification
    cleared, so that I stop being told about it.
16. As someone with many unread messages in one conversation, I want the
    notification bounded rather than unbounded, so that a flood does not bloat
    the shade or the app's memory.
17. As someone with many unread messages, I want the count to remain truthful
    even when the notification shows only a few of them, so that I can tell how
    far behind I am.
18. As someone who mutes notifications, I want nothing to appear, so that the
    setting means what it says.
19. As someone whose phone has been rebooted or whose app was killed by the
    system, I want a stale notification never to show threads the running app
    cannot explain, so that what I see always matches what the app believes.
20. As someone who sends messages myself, I never want my own message to raise a
    notification.
21. As someone using a group conversation, I want the notification titled with
    the conversation name and each message attributed to its author, so that the
    thread still makes sense.
22. As someone receiving a message from a new peer with no name yet, I want the
    notification to fall back to the same short identifier the conversation list
    shows, so that the two surfaces agree.
23. As someone tapping a notification, I want the app to open, so that I can go
    from glance to reply in one tap.
24. As someone who denies the notification permission, I want the app to keep
    working silently, so that the refusal is not fatal.
25. As a maintainer, I want the notification decisions covered by fast tests that
    need no device, so that behaviour does not regress unnoticed.
26. As a maintainer, I want one definition of "the user can see the app", so that
    the notification policy cannot drift between its two call sites.
27. As a maintainer, I want the notification channel left alone, so that no user
    is surprised by a banner or a sound that was not there before.
28. As a reader of the repository, I want the documentation to stop claiming that
    notifications carry no content, so that it describes the product truthfully.

## Implementation Decisions

**Source of thread content.** Threads are accumulated in the engine host
process's memory from the engine's `message.new` event stream. The store is not
used as the source: a conversation's `unread` is a counter, marking a
conversation read publishes no event, and no per-message read state exists, so
"which messages are unread" is not answerable from the database. The
accumulator's lifetime is the process's, which is acceptable because the
notification cannot outlive it in any meaningful way.

**Shape of the state.** The notification module owns a single value: an ordered
map of conversation id to a bounded, oldest-first list of message entries, each
entry carrying sender id, resolved sender label, body and timestamp. Everything
the Kotlin side renders is derived from it, so no notification state lives on the
Kotlin side except the set of ids currently posted.

**Bounds.** A conversation keeps at most the eight most recent incoming
messages; trimming drops from the oldest end. Eight caps shade bloat and process
memory; it is not a claim that the expanded shade renders no more, so trimming
can hide text the platform could have displayed. The conversation's true unread
count is preserved separately and still drives the notification number and the
summary totals.

**Ordering.** Entries are ordered by the sender's timestamp, with arrival order
breaking ties. That is the order the conversation view shows, and the order the
platform's stack renders: oldest at top, newest at bottom, with no sorting at
render time.

**Truncation.** Bodies are stored whole and never pre-truncated. The platform's
collapsed/expanded rendering is the truncation mechanism; the escape hatch for a
body longer than the expanded shade can render is opening the app. A message body
may be up to 12 000 characters, so pathological lengths are expected to clip.

**Structure.** One summary notification and one child per conversation, all in a
single notification group. The summary reuses the app's existing message
notification id. Child ids are allocated from the lowest free integer at or above
the reserved base the first time a conversation appears, and kept for the life of
the process, so two conversations can never share one. Stability across process
restarts is not needed: the service cancels every notification before it promotes
itself, so a new process never inherits a child.

**Summary content.** Title is the app name. Text is `<n> conversations · <m> new
messages`, with singular forms for one of either. A styled list carries at most
five lines of `<conversation> — <latest body>`, newest conversation first.

**Child content.** A messaging style thread: one sender identity per distinct
author, the conversation title taken from the stored conversation title when
present, otherwise from the peer identity. Sender labels follow the existing peer
label rule — nickname, then announced name, then the first eight characters of
the identity — so the notification and the conversation list agree by
construction.

**Numbers.** The summary carries the total unread count across every
conversation, since it spans conversations and has no single conversation's count
to carry; each child carries its own conversation's unread count. Public versions
carry the same number as the notification they stand in for.

**Visibility.** Notifications are private, with a content-free public version
that carries only the count. This keeps the body off a secure lock screen while
letting the platform's own "hide sensitive content" preference take effect,
which an always-public notification would override.

**Suppression.** Nothing is posted while one of the app's activities is resumed,
the display is interactive and the keyguard is unlocked. The same predicate is
consulted at the moment an incoming message would be appended to its thread,
under the accumulator's lock, not only before posting, so a message read in the
app can never resurface in a later notification. It is exposed as a single static
boolean accessor on the JNI bridge object so both call sites share one definition
of visibility. A post the predicate refuses also forgets the accumulator, so a
stale snapshot cannot seed the next post.

**Payload boundary.** Each event produces one snapshot of every non-empty
conversation, serialized as a single payload across the JNI boundary, replacing
the count-only call. One crossing per event keeps the interface small and makes
the Kotlin side a pure function of the snapshot.

**Failure behaviour.** A lagged event feed skips at most one notification; the
next event recomputes the whole snapshot, so the thread self-heals. A failure to
post is logged and never disturbs the engine.

**Lifecycle.** Coming to the foreground cancels the summary and every child. The
service cancels all of the app's notifications while starting, before promoting
itself to the foreground, so a sticky restart cannot leave threads the new
process cannot account for. Conversations absent from a snapshot have their
children cancelled, which covers trimming and reading.

**Channel.** The existing message channel is reused unchanged: default
importance, no sound, no vibration, no lights, badge enabled. Channel importance
is effectively permanent once created, so reusing it guarantees no user is
surprised by a banner they did not ask for; anyone who wants more can raise it in
system settings, and that choice survives because no new channel id is
introduced.

**Muting.** Muting continues to suppress all notification output, before any
state is accumulated.

**Tap behaviour.** Unchanged: any notification opens the app. No conversation
routing exists in the shared interface and none is added here.

**Unchanged.** The notification permission request at startup, the
foreground-service notification and its channel, and the engine's event feed
itself.

## Testing Decisions

A good test describes observable behaviour: an event and a visibility state go
in, and the notification the user would receive comes out — which conversations
appear, in what order their messages sit, what the counts say, and what is
suppressed. Tests must not assert on internal maps, on JNI call counts, or on the
order in which the summary and children happen to be posted.

**One seam.** The notification state machine in the mobile shell's Rust code,
exposed as a pure function of event payload, mute state and a visibility
provider that returns the snapshot to be posted. The provider is a closure, so
tests can flip visibility between events without Android or threads; sender label
resolution reads from the store as it does in production. This mirrors the
existing tests in the notification module, which already test "event JSON in,
decision out", and reuses that prior art rather than introducing new test
infrastructure.

**Deliberately not covered by automated tests.** The Kotlin rendering — group,
summary, styles, sender identities, icon, visibility flag. There are no Kotlin
test sources in the mobile app and no Robolectric dependency; adding that harness
to assert we called Android's API with our own arguments would be all
implementation detail. The Android API surface is declarative, so it is verified
once, by hand, on the device.

**Device acceptance pass.** On the Pixel 8, with a desktop daemon on the same LAN,
background the app and check: a burst from one peer stacks in send order with the
newest last; a message from a second peer creates a second child; the summary
reads `<n> conversations · <m> new messages` and lists one line per conversation;
a long body ellipsizes collapsed and expands; the lock screen shows the count
only; reading a conversation clears it; several messages beyond the bound leave a
truthful count with the oldest dropped; and forcing the process to stop and
restart leaves no stale notification.

## Out of Scope

- Tapping a notification to open the conversation that caused it.
- A user preference to hide message content from notifications.
- Message previews on desktop, which remain count-and-chime by design.
- Notification actions — inline reply, mark as read, quick reactions.
- Attachments, voice notes and live voice in notifications.
- Per-conversation muting.
- Notification shortcuts and avatars, since the app has neither avatars nor
  dynamic shortcuts.
- iOS.

## Further Notes

The repository currently documents the opposite of this design in two places —
that notifications have no previews, and that the message notification carries a
count and never message content — and lists the missing conversation deep link as
a known gap. All of it needs correcting in the same change.

`VISIBILITY_PUBLIC` was previously chosen on the reasoning that the body never
left the app. That reasoning ends with this change, which is why the visibility
decision inverts.

The eventual deep-link follow-up inherits a constraint from here: the snapshot
knows the conversation id, so the child's content intent can carry it as an
extra, but the shared interface has no routing hook yet and adding one touches
the cross-platform UI rather than notifications.

Group conversations exist in the store and protocol without any user interface.
The title and per-author attribution rules above are written so that a group
renders correctly the day that interface lands.

## Comments

**2026-11-15 — revisions from the review of `ea777e6`.** The full record lives in
`.scratch/android-notification-repairs/spec.md`. This spec stays the design
record; the paragraphs it got wrong are corrected above, and the revisions are:

- **Ordering.** The device pass disproved "arrival order is timestamp order":
  two messages can overtake each other in transit. Entries are ordered by the
  sender's timestamp with arrival order breaking ties, and the paragraph above
  now says so.
- **Numbers.** "Both styles carry the conversation's unseen count" was
  ambiguous, and a summary spanning conversations has no single conversation's
  count. The summary now carries the total unread count; each child carries its
  own conversation's count; the public versions mirror them.
- **Child ids.** Hashing the conversation id let two ids that hash alike
  overwrite each other's child. Ids are now allocated for the process, and the
  summary's five-line cap counts conversations by position rather than by id.
- **Bounds rationale.** Eight caps shade bloat and memory; it was never a claim
  that the expanded shade renders no more. Trimming can hide text the platform
  could have displayed, and the count remains the truthful part.
- **Suppression.** The visibility sample is taken at commit time under the
  accumulator's lock, not before it; a post the predicate refuses also forgets
  the accumulator.
- **Vocabulary.** The quantity is called unread everywhere, matching the store.
- **Missing timestamps.** An event whose message has no integer timestamp is
  malformed and ignored rather than treated as the oldest message.

