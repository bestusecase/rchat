# rchat

A fast, offline-first chat client for [Reticulum](https://reticulum.network), on
desktop and Android. Discord-style interface, peer-to-peer messaging, no central
server.

**Status:** text works end to end over real Reticulum links — announce-based peer
discovery, encrypted delivery, receipts, search, and an offline queue. The desktop
app is packaged for Linux, and a standalone Android app runs the engine in-process
and notifies for new messages while backgrounded. Voice is designed for but not
built yet.

## Architecture

The pieces, on purpose:

```
  desktop:  Svelte UI ──WS + token──► rchatd ──stdio─────► bridge.py
  Android:  Svelte UI ──WS + token──► rchatd ──Chaquopy──► bridge.py
                                       (linked in-process)

  bridge.py ──► LoRa / radio / TCP / LAN
```

- **`rchatd`** owns identity, storage, routing policy, and the API. It is the
  product; the window is a view. Being a library is what let the Android client
  reuse the engine unchanged.
- **`sidecar/bridge.py`** runs the reference Reticulum stack. Reticulum's
  protocol behaviour and crypto are years of work; reimplementing them would be
  a multi-month compatibility risk. A supervised child process on desktop — and
  Chaquopy on Android — keeps the stack intact and the daemon native.
- **`apps/desktop`** is a thin, dependency-light interface (about 70 KB of JS
  after a production build).
- **`apps/mobile`** is the Android shell: the same Svelte interface, `rchatd`
  linked in-process, and the Python bridge under Chaquopy, so the APK is
  self-contained.

Messages are addressed with LXMF, so interop with Sideband and Nomad Network is
available through the same stack.

## Requirements

For the packaged desktop app: Linux x86_64, a desktop with a system tray, and
WebKitGTK 4.1. Python, Reticulum and LXMF are bundled, so nothing else is
needed.

For development:

- Rust 1.80+
- Python 3.10+ with Reticulum and LXMF: `pip install rns lxmf`
- Node 20+ for the interface
- Tauri 2 tooling: `cargo install tauri-cli --version "^2" --locked`, plus the
  usual WebKitGTK/GTK build dependencies

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

## Desktop app

The packaged app is a thin Tauri shell around the same interface and engine. It
starts a private engine on a free loopback port, injects the address and token,
and opens straight into chat — there is no connect screen and no token to paste.
A second launch focuses the existing window.

```bash
cd apps/desktop
npm install
cargo tauri build
```

Output:

- `apps/desktop/src-tauri/target/release/bundle/appimage/rchat_0.1.0_amd64.AppImage`
- `apps/desktop/src-tauri/target/release/bundle/deb/rchat_0.1.0_amd64.deb`

What the shell does:

- **Engine lifecycle.** One engine per data directory. It reuses a healthy
  running engine, otherwise starts the bundled `rchatd` on a free port. Quit
  stops the engine and its Python bridge cleanly.
- **Tray.** Closing the window hides it; the tray stays resident. Left-click
  toggles the window; the menu has Open, Mute sounds, Start at login, Quit.
- **Notifications.** No banners and no message previews. A message in a
  conversation that is not on screen plays one coalesced chime. A tray dot and
  launcher badge (or a Cinnamon taskbar needs-attention hint) appear only while
  the window is hidden or unfocused. Nothing happens for the open conversation.
- **Network.** The settings panel has LAN discovery and one TCP relay. Applying
  them restarts the engine in place.
- **Updates.** AppImage installs check GitHub releases for a signed manifest and
  update on request with a restart; Debian installs ship a signed apt source and
  a daily systemd timer, so they upgrade with the rest of the system. Details in
  `RELEASING.md`.
- **First run.** Asks for a display name, then opens the conversation list.

Data lives in `~/.local/share/rchat` and survives uninstalling the app.
Start-at-login is opt-in and off by default.

The shell bundles a private Python 3.12 interpreter plus RNS 1.6 and LXMF 1.3,
so it never touches the system Python. Staging happens automatically in
`apps/desktop/scripts/prepare-bundle.mjs` before every desktop build.

Platform scope: Linux x86_64 desktop and Android arm64. Windows, macOS, Flatpak
and iOS are not packaged yet.

## Android app

`apps/mobile` is a Tauri 2 app. It links `rchatd` as a library, so the engine runs
in the app's own process and survives the activity going to the background, and it
runs `bridge.py` under Chaquopy, so the APK needs no external Python.

New-message notifications are posted from Rust, not from the page: a backgrounded
WebView has its JavaScript suspended, so the engine's event stream is the only
reliable trigger. Posting is gated on the activity being visible, the display
interactive and the keyguard unlocked, so nothing appears while you are reading
the conversation. The notification carries a count, never message content.

```bash
cd apps/mobile
cargo tauri android build --debug --target aarch64 --apk
```

Needs the Android SDK, NDK and a JDK alongside the Tauri Android tooling. The
generated native project in `src-tauri/gen/android` is checked in because the
Kotlin service, activity and notification code live there.

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
- On Android, notifications while backgrounded, gated on the screen being off or
  the app not being in front

## Not built yet

- Group conversations exist in the protocol and store, but there is no UI for them
- Attachments, voice notes, live voice
- iOS, and notifications without a running engine (Reticulum has no push, so
  Android keeps a foreground service alive instead)
- Tapping a notification opens the app, not the conversation that caused it
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
apps/desktop/src-tauri/  Tauri shell: window, tray, engine supervisor, packaging
apps/mobile/        Android app: same interface, rchatd in-process, Chaquopy bridge
apps/mobile/src-tauri/gen/android/  Kotlin service, activity and notifications
scripts/            smoke tests and the two-peer dev runner
```

Tests: `cargo test --workspace`. The mobile app is a separate workspace, so its
tests run with `cd apps/mobile/src-tauri && cargo test --lib`. Smoke tests:
`node scripts/smoke-offline.mjs` and `node scripts/smoke-rns.mjs` (see the script
headers for arguments).
