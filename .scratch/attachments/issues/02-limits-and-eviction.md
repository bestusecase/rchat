# 02 — Size limits, refusal, placeholder and two-pass eviction

Status: done

## Scope

The policy that keeps one message from consuming a link or a disk.

- A per-message ceiling on total attachment bytes, chosen by the target peer's
  bandwidth class: 8 MB for `lan` and `relay`, 1 MB for `unknown`, 64 KB for
  `radio`. At most four attachments per message.
- Refusal at send time with a message naming the class and the actual size. No
  message row is created and nothing reaches the bridge.
- A caption of any length up to the existing text limit, or no caption at all,
  when attachments are present.
- An inbound ceiling applied to bridge events: an oversized attachment becomes a
  row with no blob and a reason, while the message and its caption are preserved
  and delivered.
- A 500 MB total budget for attachments, enforced after ingest by eviction in two
  passes — oldest received blobs first, and only if that is not enough, oldest sent
  blobs. Eviction clears the blob and the thumbnail and records a reason; message
  text, timestamps, state and route are untouched.
- The budget is a constant with a process-level override so tests can force
  eviction without writing a large amount of data.

## Acceptance

Through the engine seam, with a forced budget:

- An over-limit send is refused, the reply carries the reason and the class, and no
  message row exists afterwards.
- A peer with no known path gets the stricter `unknown` ceiling.
- Attachments totalling exactly the limit are accepted; one byte more is refused.
- Five attachments are refused regardless of size.
- An over-limit inbound attachment yields a message with a row whose reason is
  recorded, and the caption intact.
- With the budget exceeded, received blobs are evicted oldest-first and sent blobs
  are only touched once no received blob remains.
- An evicted attachment's message still reports its original timestamp, body and
  receipt state, and its attachment reports a reason.
- Eviction frees the thumbnail as well as the blob.

## Notes

Bandwidth class is already refreshed from the path table; this ticket only adds a
consumer. The policy lives in the engine, not in the interface, so a client cannot
bypass it.
