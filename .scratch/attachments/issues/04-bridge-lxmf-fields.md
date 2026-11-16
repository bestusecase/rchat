# 04 — Bridge: standard LXMF fields in both directions

Status: ready-for-agent

## Scope

The sidecar half of the feature, and the only code that speaks to other LXMF
clients.

- On send, the bridge accepts a list of `{name, path}` in the send command, reads
  the bytes itself, and sets the standard image field for a single image or the
  standard file-attachments field otherwise. Bytes never cross the JSON channel.
- On receipt, the bridge reads the image and file-attachments fields, enforces the
  inbound byte ceiling before writing anything, and writes each payload into a
  staging directory inside the data directory.
- The message event gains a list of `{path, name, ordinal, size}` and an optional
  reason per entry when the ceiling was exceeded. The event stays small: no bytes
  in JSON.
- Unknown or unreadable field payloads are skipped rather than failing delivery of
  the text.
- A staging file the engine never collects is cleaned up on the next bridge start.
- A small script, run under the bundled runtime, that packs and unpacks a message
  carrying both field types and asserts the tuples survive — the format guard.

## Acceptance

Format guard, no network:

- A message built with the image field round-trips through pack and unpack with its
  name and bytes intact.
- A message built with the file-attachments field round-trips with every entry in
  order.
- A field payload of an unexpected shape is refused by the bridge's own decoder
  rather than crashing the process.

Two-peer smoke over a real Reticulum TCP link:

- A small image with a caption sent from one engine arrives at the other as one
  message whose caption is the caption and whose single attachment has the same
  byte count and content hash as the original.
- The receiving side has a thumbnail on disk and the original retrievable by hash.
- A plain text message still arrives as before, with no attachment list.

## Notes

The field constants are already defined in the vendored LXMF; the bridge currently
discards fields entirely, which is the whole of the gap on the receive side.
Interop with a client that sends a filename as bytes rather than text must not
fail delivery.
