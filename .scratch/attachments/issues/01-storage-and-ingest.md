# 01 — Attachment rows, blob store and the shared ingest path

Status: ready-for-agent

## Scope

The storage foundation everything else uses.

- A new attachments table: owning message id, ordinal, filename, MIME type, byte
  count, content hash, pixel dimensions, thumbnail hash, and a nullable "missing"
  reason. Cascade with the owning message.
- A content-addressed blob directory under the data directory, addressed by hash
  in two-hex-character bucket directories, with deduplication: writing identical
  bytes twice produces one blob and two rows.
- One ingest routine, used by outgoing uploads and incoming bridge events alike:
  write to a temporary file, hash, sniff the MIME type from extension with magic
  bytes taking precedence for the image types rchat displays, record dimensions
  when it is an image, produce one 480 px longest-edge JPEG thumbnail, then commit
  the row.
- Delete the staging file after a successful ingest; a failed ingest leaves no row
  and no blob.
- A startup sweep that removes blobs no row references, covering a crash between
  writing bytes and committing the row.
- `kind` on the owning message becomes `image` or `file` when attachments exist;
  text without attachments stays `text` and its body stays the caption.
- The ingest routine takes an explicit blob root rather than deriving one, so the
  core crate keeps no ownership of where the data directory lives.

## Acceptance

Through the engine seam:

- An ingested image is retrievable by hash, has the recorded byte count and pixel
  dimensions, and its thumbnail decodes to no more than 480 px on the long edge.
- An ingested non-image has a MIME type and no dimensions and no thumbnail.
- The same bytes ingested twice produce one blob and two rows.
- A message with an image attachment reports `kind: "image"` and keeps its caption
  in the body.
- Ingest failure (unwritable root) leaves no row and no blob.
- The sweep removes a blob with no referencing row and keeps one that has a
  reference.
- A missing blob row still reads back with its metadata and a reason.

## Notes

`sha2` is already a dependency of the core crate; an image decoding crate is new
and belongs wherever ingest lives. Path construction must never concatenate a
user-supplied filename.
