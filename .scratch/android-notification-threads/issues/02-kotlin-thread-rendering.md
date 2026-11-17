# 02 — Render the thread in Kotlin

Status: ready-for-agent

## Scope

Turn the snapshot payload into Android notifications.

- Parse the snapshot; for each conversation build a messaging style thread with
  one sender identity per distinct author, the conversation title as the
  notification title, the conversation's unseen count as the number, in a single
  notification group.
- Build the summary: app name, `<n> conversations · <m> new messages` with
  singular forms, a styled list of at most five `<conversation> — <latest body>`
  lines, one per conversation, newest conversation first.
- Stable child notification ids derived from the conversation id, never colliding
  with the foreground-service notification id or the summary id.
- Private visibility with a content-free public version carrying the count.
- Full bodies, no pre-truncation.
- Reuse the existing silent message channel unchanged; no new channel id.

## Acceptance

- Two conversations produce a summary plus two children, and the children nest
  under the summary in the shade.
- A single conversation still produces a summary row plus one child.
- Collapsed children show the newest message with an ellipsis; expanding shows
  the thread with wrapped text.
- A locked, secure lock screen shows only the public version.
- Singular and plural summary forms are correct for one and for several.
