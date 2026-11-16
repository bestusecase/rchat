# 03 — Wire types, send command and blob HTTP routes

Status: ready-for-agent

## Scope

What a client can say about attachments and how it moves the bytes.

- An attachment descriptor on the wire: filename, MIME type, byte count, hash,
  dimensions, thumbnail hash, and an optional reason when the bytes are absent.
- Messages carry an ordered list of descriptors; older clients that omit the field
  still parse.
- The send command grows an ordered list of hashes alongside the caption. The
  engine rejects a hash it has no row for, and a send that has neither text nor
  attachments fails with the existing empty-message error.
- The identity payload advertises that this client supports attachments.
- `POST` upload route on the loopback server, authenticated with the same token as
  the socket: raw bytes, content type and filename supplied by the client, answer
  carries the descriptor. The route's request body limit is raised above the
  largest permitted attachment so the engine's own refusal, not the transport's,
  is what the user sees.
- `GET` download route by hash, with a thumbnail variant, immutable cache headers
  and the same token. Unknown hashes answer 404 without leaking which hashes exist.
- Binary WebSocket frames remain unused.

## Acceptance

Through the engine seam:

- Uploading bytes returns a descriptor; a subsequent send referencing that hash
  produces one message with one attachment and the caption in the body.
- Uploading more than the transport limit fails with the engine's own error text.
- Sending an unknown hash is rejected and creates no message.
- Sending with no caption and one attachment succeeds; sending with neither fails.
- A snapshot and a message event both carry descriptors for a message that has
  them, and omit the field entirely when it has none.
- A downloaded original returns the exact ingested bytes; the thumbnail variant
  returns a decodable image no larger than 480 px.
- A download with a bad or missing token is refused, as the socket already is.
- A download of an evicted or absent blob answers 404 rather than an empty body.

## Notes

The preview surface and the full-screen viewer both consume the download route, so
the token has to be accepted as a query parameter — the existing socket route
already does that.
