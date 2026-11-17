# 02 — Engine commands for nickname and notes

Status: resolved
Type: task
Blocked by: 01

## Goal

Give the interface one narrow, honest way to rename a peer locally and to save
notes, without reusing Add peer and without lying about when the peer was last
seen.

## Work

- Add two `ClientCmd` variants:
  - `set_peer_nickname { peer_id, nickname: Option<String> }`
  - `set_peer_notes { peer_id, notes: Option<String> }`
- Normalisation in the command handler, mirroring the local display-name rules:
  - nickname: trim, replace newlines with spaces, reject on a 48-character cap
    (count characters, not bytes), store an empty result as None;
  - notes: keep newlines and interior spacing, strip trailing whitespace, store
    an empty result as None; no length cap.
- When `peer_id` has no stored peer, create a minimal row first: id, address only
  when the id looks like a 32-character hex LXMF address, no announce name, no
  nickname, no notes.
- Both commands return `{ "peer": PeerInfo }` and publish `peer.updated` with
  the full peer.
- Do not change `add_peer`, the announce path, or `last_seen` semantics.

## Acceptance

- Engine test: `set_peer_nickname` returns the updated peer and records a
  `peer.updated` event whose payload carries the new nickname.
- Engine test: an over-long nickname is rejected, no write happens, and the
  previous nickname is unchanged.
- Engine test: a whitespace-only nickname clears the alias.
- Engine test: `set_peer_notes` persists multi-line notes and emits
  `peer.updated`.
- Engine test: a command for an unknown peer id creates the record and succeeds.
- Engine test: after a rename, announce-style peer updates leave nickname and
  notes intact.
- Command round-trip test for both variants in the IPC crate, matching the
  existing flattened-tag test style.

## Comments

- Shipped in `8f43587`: `ClientCmd::SetPeerNickname` / `SetPeerNotes`,
  `normalise_nickname` (rejects over 48 chars instead of truncating) and
  `normalise_notes` in `crates/rchatd/src/app.rs`, plus the announce fix that
  sets `peer.nickname = None` before `upsert_peer` and publishes the stored row.
  Six tests under `// ---- peer profile ----` and the IPC round-trip test.
