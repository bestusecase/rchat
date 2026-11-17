# 05 — Deferred: open the conversation from a notification

Status: deferred

## Scope

Tapping a conversation's notification opens that conversation rather than the
app's last state.

Not started with the thread work because the shared interface has no routing
hook, so this is cross-platform UI work rather than notification work. The
notification side is already prepared: the snapshot knows the conversation id, so
the child's content intent can carry it as an extra.

## Related deferred item

A preference to hide message content from notifications entirely, leaving only a
count. Android's per-channel settings and its lock-screen privacy setting cover
part of this, but there is currently no way to keep content out of an unlocked
shade. Adding it means a persisted setting and a row in the shared settings
interface that also appears on desktop.

## Acceptance (when picked up)

- Tapping a child opens the app with that conversation selected, from cold start
  and from a backgrounded app.
- Tapping the summary opens the conversation list.
- A hidden-content preference leaves sender-free, body-free notifications while
  keeping counts.
