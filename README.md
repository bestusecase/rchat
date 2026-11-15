# rchat

A fast, offline-first desktop chat client for [Reticulum](https://reticulum.network).
Discord-style interface, peer-to-peer messaging, no central server.

**Status:** text works end to end over real Reticulum links — announce-based peer
discovery, encrypted delivery, receipts, search, and an offline queue. Voice is
designed for but not built yet.

## Architecture

Two pieces, on purpose:

```
  desktop UI (Svelte 5)  ──local WebSocket + token──►  rchatd (Rust)
                                                         │
                                              stdio JSON │  supervised child
                                                         ▼
                                                   bridge.py (RNS + LXMF)
                                                         │
                                          LoRa / radio / TCP / LAN
```

- **`rchatd`** owns identity, storage, routing policy, and the API. It is the
  product; the window is a view. This is what makes a small always-on node or a
  future mobile client possible without rewriting the engine.
- **`sidecar/bridge.py`** runs the reference Reticulum stack. Reticulum's
  protocol behaviour and crypto are years of work; reimplementing them would be
  a multi-month compatibility risk. A supervised child process keeps the stack
  intact and the daemon native.
- **`apps/desktop`** is a thin, dependency-light interface (about 70 KB of JS
  after a production build).

Messages are addressed with LXMF, so interop with Sideband and Nomad Network is
available through the same stack.

## Requirements

- Rust 1.80+
- Python 3.10+ with Reticulum and LXMF: `pip install rns lxmf`
- Node 20+ for the interface

## Run it

Terminal 1 — the engine:

```bash
cargo run -p rchatd
```

It prints an address, a token, and writes both to `~/.local/share/rchat/`.
The token is also stored in `token` in that directory (mode 0600).

Terminal 2 — the interface:

```bash
cd apps/desktop
npm install
npm run dev
```

Open <http://127.0.0.1:5173>, paste the token, connect. Your address is in the
profile dialog (bottom-left avatar).

## Talk to yourself over a real Reticulum link

```bash
./scripts/dev-two-peers.sh
```

This starts two daemons (ports 8791 and 8792) connected by a Reticulum TCP
interface on 4242, and prints each side's token and address. Open the UI twice
(two browser profiles work), connect each to one daemon, and add the other
peer's address.

## Transports

The bridge picks its interfaces from environment variables:

| Variable | Meaning |
| --- | --- |
| `RCHAT_RNS_AUTO` | `1`/`0` — AutoInterface LAN discovery (on by default) |
| `RCHAT_RNS_TCP_SERVER` | Listen, e.g. `4242` or `0.0.0.0:4242` |
| `RCHAT_RNS_TCP_CLIENT` | Connect, e.g. `host:4242` |
| `RCHAT_RNS_GROUP` | AutoInterface group id |
| `RCHAT_RNS_MODE` | `embedded` (private instance, default) or `shared` |
| `RCHAT_RNS_CONFIGDIR` | Config directory for `shared` mode |

Other process settings: `RCHAT_PORT`, `RCHAT_DATA_DIR`, `RCHAT_TOKEN`,
`RCHAT_PYTHON`, `RCHAT_SIDECAR`.

**Radio:** add an RNode or other interface to `<data-dir>/rns/config` and restart.
The bridge preserves an existing config unless you pass interface variables
explicitly (or set `RCHAT_RNS_RESET_CONFIG=1` to regenerate it).

## What works

- 1:1 conversations over LXMF, direct and opportunistic delivery
- Peer discovery from announces, plus manual add by address
- Delivery receipts, live state (`queued → sending → sent → delivered`)
- Persistent encrypted-at-rest-ready SQLite history and full-text search
- Offline queue that flushes when the mesh comes back
- Display-name changes propagate to peers
- Reconnects automatically if the engine restarts

## Not built yet

- Group conversations exist in the protocol and store, but there is no UI for them
- Attachments, voice notes, live voice
- Mobile clients and background push (the hard problem: Reticulum has no push)
- Multi-device sync

## Voice plan (so the current design does not block it)

1. **IP / LAN:** real calls over direct UDP or WebRTC; Reticulum handles
   discovery, identity and signalling. Opus at 6–16 kbps.
2. **Reticulum over TCP:** unreliable single packets (not Links — retransmits
   would stall audio), 60 ms Opus frames bundled under the 500-byte MTU.
3. **LoRa and radio:** live audio is not realistic. Codec2 voice notes as LXMF
   attachments are, and that is the honest UX.

The pieces already in place for this: per-peer bandwidth class and `last_seen`
on every peer, capability flags on the wire, a versioned event stream for state,
and binary WebSocket frames reserved for media.

## Layout

```
crates/rchat-ipc/   wire types shared by engine and clients
crates/rchat-core/  identity, SQLite store, event log
crates/rchat-rns/   sidecar supervisor and event parsing
crates/rchatd/      daemon: HTTP/WebSocket API, routing, delivery policy
sidecar/bridge.py   Reticulum + LXMF bridge
apps/desktop/       Svelte 5 interface
scripts/            smoke tests and the two-peer dev runner
```

Tests: `cargo test --workspace`. Smoke tests: `node scripts/smoke-offline.mjs`
and `node scripts/smoke-rns.mjs` (see the script headers for arguments).
