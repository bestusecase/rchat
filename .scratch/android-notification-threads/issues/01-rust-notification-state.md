# 01 — Notification state machine in the engine host

Status: ready-for-agent

## Scope

Replace the count-only notification trigger with the accumulator described in
the spec.

- A notification state value in the mobile shell's Rust code: ordered map of
  conversation id to an oldest-first list of at most eight incoming message
  entries, each with sender id, sender label, body and timestamp, plus that
  conversation's unseen count.
- Append on `message.new` only when the author is not this identity, the app is
  not muted, and the user cannot see the app.
- Sender labels resolved with the existing peer label rule; conversation titles
  from the stored conversation, falling back to the peer label.
- One snapshot of every non-empty conversation per event, serialized, across a
  single JNI call replacing the count-only call.
- A static boolean accessor on the JNI bridge object exposing the existing
  "activity resumed, display interactive, keyguard unlocked" predicate, consulted
  before appending.
- Clearing the accumulated state when the app becomes visible and when the engine
  stops.

## Acceptance

- Unit tests at the state-machine seam: send order preserved; the bound trims
  from the head while the count stays truthful; own messages never append; muted
  suppresses; a message arriving while the app is visible is never appended;
  clearing empties the snapshot; a second conversation produces a second entry;
  a malformed or unrelated event is ignored.
- A lagged event feed leaves the next snapshot well-formed.
- No behaviour change on desktop or in the browser client.
