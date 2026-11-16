# 07 — Notification lines and documentation

Status: ready-for-agent

## Scope

The surfaces that describe a message in text, and the docs that describe the
product.

- The Android notification thread line for a message with an attachment reads the
  attachment kind, or the filename when there is exactly one file, instead of the
  empty body it would show today.
- The lock screen continues to show only the count. No image is rendered in a
  notification, and no new notification channel is introduced.
- Search continues to index captions only; nothing about filenames becomes
  searchable.
- The README stops listing attachments as unbuilt, gains a short section describing
  what a user can now do and the size behaviour, and drops the voice-note mention
  where it conflates attachments with audio.
- The plus button's "coming in a later update" tooltip is gone (the interface ticket
  removes the disabled state; this ticket makes sure no stale promise survives).

## Acceptance

- With the app backgrounded, an incoming image raises a notification whose thread
  line reads as an image, and an incoming PDF raises one naming the file.
- A locked screen shows the count only, for an attachment message as for text.
- Reading the conversation clears the notification as before.
- The README contains no claim that attachments do not exist, and describes the
  limits a user would actually hit.

## Notes

The notification text is accumulated from the engine's event stream in the mobile
shell, which already carries the message body; the attachment descriptors on the
event are the only new input.
