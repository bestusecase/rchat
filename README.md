# rchat

A fast, offline-first chat client for [Reticulum](https://reticulum.network), on
desktop and Android. Modern interface, peer-to-peer messaging, no central
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
- **`apps/desktop`** is a thin, dependency-light interface (about 50 KB of JS
  gzipped after a production build; no graph or chart library).
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
- **Notifications.** Desktop alerts stay quiet about content: no banners and no
  message previews. A message in a conversation that is not on screen plays one
  coalesced chime. A tray dot and launcher badge (or a Cinnamon taskbar
  needs-attention hint) appear only while the window is hidden or unfocused.
  Nothing happens for the open conversation.
- **Network.** The Reticulum page (mesh button in the rail) shows transport
  state, interfaces, the full path table, announce history and a topology map.
  Interfaces — every type Reticulum supports, including RNode radios — are
  added, edited, enabled and removed there and applied live, without restarting
  the engine. Settings only holds a shortcut to that page.
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
reliable trigger. Each conversation with unread messages gets its own
notification, titled with the conversation — the peer's name for a direct chat —
showing a messaging-style thread of what arrived while you were away, newest at
the bottom. A summary groups them and reads `<n> conversations · <m> new
messages`, with a line per conversation, up to five. Posting is gated on the
activity being visible, the display interactive and the keyguard unlocked, so
nothing appears for a message you are already looking at, and the accumulated
threads are forgotten at the same moment.

Message text is private: a locked screen shows the count only, and Android's own
"hide sensitive content" setting can hide even that. The notification decisions —
which conversations appear, in what order, what is suppressed — are unit-tested
in `src-tauri/src/notify.rs`; the rendering itself is verified by hand on a
device, since the app has no Android test harness.

Last hand pass: a Pixel 8 (`39061FDJH0080U`), two desktop daemons on the same LAN
dialling the phone's Reticulum TCP interface. Confirmed on the shade: a burst
stacks in send order with the newest last and the collapsed card ellipsizes; a
second peer adds a second child under the same summary, which reads
`2 conversations · 4 new messages` and lists one line per conversation; a
1 400-character body stays whole and expands to wrapped text; eleven messages
leave eight in the thread with `11` as the number; opening the app leaves only
the ongoing service entry; force-stopping and restarting leaves no stale thread.
Bodies are `VISIBILITY_PRIVATE` with a count-only public version, checked in
`dumpsys notification`; that device had no secure lock screen configured, so the
locked-screen rendering itself was not exercised.

Two messages sent within the same millisecond can carry equal LXMF timestamps and
arrive out of order; the thread orders by timestamp with arrival order breaking
ties, which is the same order the conversation view shows.

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

## See the mesh

```bash
./scripts/dev-mesh.sh
```

Starts a three-node mesh: two rchat daemons either side of a plain `rnsd` relay
with transport enabled. The Mesh button in the rail opens the Reticulum page:
Overview, Interfaces, Paths, Announced and Map. Each leaf sees the other two
hops away behind the relay, which is what the map draws — a peer on its hop
ring, the first-hop transport as a diamond, and the interface on the edge.

The scripted check asserts the topology, path filtering and announce history,
and that a message crosses the relay with no peer address in either leaf's
interface rows:

```bash
node scripts/smoke-mesh.mjs http://127.0.0.1:8791 .dev/mesh/a/token \
  http://127.0.0.1:8793 .dev/mesh/c/token
```

The full rendezvous story runs the relay and both leaves itself:

```bash
node scripts/smoke-rendezvous.mjs
```

It proves relay delivery and a leaf restart with the other side's stored
configuration untouched. Add `--phases=a,b,c` on two devices (or with each leaf
in its own network namespace) to also remove the relay and fall back to local
discovery; RNS AutoInterface uses one fixed data port per host, so two instances
on this machine cannot both run it.

## Transports

`rchat.db` owns interface configuration; `rns/config` is generated from it and
applied hot through Reticulum's interface management API. Add, edit, disable or
delete interfaces on the Network page. The first start after an upgrade imports
an existing hand-written `rns/config` once (including unknown sections, kept
verbatim), so an RNode added by hand keeps working.

A fresh install starts with one enabled `AutoInterface` row, so two devices on
the same network talk with no configuration at all. Adding a relay never turns
discovery off: keep the `AutoInterface` row and add a TCP (or backbone) client
pointing at a transport-enabled relay, preferably by hostname. The stored address
belongs to the relay, and no device ever writes another device's current address
into its own configuration.

An upgrade imports the old discovery flag and relay string once, with origin
`legacy`; a malformed relay is kept disabled with the reason shown instead of
being dropped.

The environment variables below are a development seed only, and only when
`RCHAT_RNS_ENV_SEED=1`. Production never seeds interfaces from the environment.

| Variable | Meaning |
| --- | --- |
| `RCHAT_RNS_ENV_SEED` | `1` — let the environment seed the bridge (development only) |
| `RCHAT_RNS_AUTO` | `1`/`0` — AutoInterface LAN discovery |
| `RCHAT_RNS_TCP_SERVER` | Listen, e.g. `4242` or `0.0.0.0:4242` |
| `RCHAT_RNS_TCP_CLIENT` | Connect, e.g. `host:4242` |
| `RCHAT_RNS_GROUP` | AutoInterface group id |
| `RCHAT_RNS_MODE` | `embedded` (private instance, default) or `shared` |
| `RCHAT_RNS_CONFIGDIR` | Config directory for `shared` mode |
| `RCHAT_RNS_RESET_CONFIG` | `1` — discard the seed config and re-render it |

Other process settings: `RCHAT_PORT`, `RCHAT_DATA_DIR`, `RCHAT_TOKEN`,
`RCHAT_PYTHON`, `RCHAT_SIDECAR`.

## Attachments

The plus next to the message field attaches photos and files to a message.
Choose up to four at once; they appear as chips, the message field becomes the
caption, and sending stays explicit. On a phone the plus also offers the camera.
Removing a chip before sending costs nothing because nothing is uploaded until
the moment you send.

Attachments travel inside the LXMF message in the standard image and
file-attachments fields, so a Sideband user sees rchat's photos and rchat sees
theirs. In the thread, a single photo is shown at up to 320 px tall, several
arrive as a grid, and tapping one opens a full-screen viewer with a save action.
Anything that is not an image is a card with its name, type and size, plus one
save action that hands the file to the platform — desktop asks where to put it,
Android writes it to Downloads.

Because an attachment is part of the message, its cost is part of the message.
The engine refuses a send that is unreasonable for the peer's link, and names
the limit and the actual size:

| Peer's link | Largest total attachment bytes per message |
| --- | --- |
| LAN or relay | 8 MB |
| Unknown path | 1 MB |
| Radio (LoRa) | 64 KB |

The cap is on the sum of a message's attachments, not on each file. Received
attachments are capped the same way: an oversized one still leaves the message
and its caption in place, with a note that the attachment did not arrive.
Attachments share a 500 MB budget on the device. When it fills, the oldest
received files are removed first and only then the oldest sent ones; the message,
its time and its receipt state stay untouched, and its bubble says the attachment
was removed to save space. Search indexes captions, as before.

## What works

- 1:1 conversations over LXMF, direct and opportunistic delivery
- Photos and files from the plus button, in standard LXMF fields so Sideband
  interoperates both ways, with per-link size limits and a device-wide budget
- Zero-config LAN discovery on by default, and a named relay for devices on
  different networks
- Peer discovery from announces, plus manual add by address
- Delivery receipts, live state (`queued → sending → sent → delivered`)
- Persistent encrypted-at-rest-ready SQLite history and full-text search
- Offline queue that flushes when the mesh comes back
- Honest transport state: `ready` only when an interface is online, `degraded`
  when the bridge is up but nothing can move, per-interface outage age, and a
  per-peer path/hop/interface/last-announce answer
- One-shot import of the old discovery toggle, relay string and hand-written
  `rns/config`
- Display-name changes propagate to peers
- Reconnects automatically if the engine restarts
- A Reticulum page with live interface, counter and path state, durable
  announce history, and a topology map of peers and first-hop transports
- Interface management for every Reticulum type, hot-applied and rolled back
  when Reticulum refuses a change
- On Android, per-conversation notification threads while backgrounded, each
  titled with the peer or conversation and grouped under a summary, gated on the
  screen being off or the app not being in front

## Not built yet

- Group conversations exist in the protocol and store, but there is no UI for them
- Voice notes and live voice
- iOS, and notifications without a running engine (Reticulum has no push, so
  Android keeps a foreground service alive instead)
- Tapping a conversation's notification opens the app, not that conversation
- Multi-device sync

## Known limitations

- Attachments have not had a device acceptance pass yet. Camera capture, saving
  through the Android media store and the attachment notification line are
  implemented, and their logic is covered by tests, but they have not been
  exercised on a phone by hand.
- Attachments over a slow link can still hit LXMF's own delivery timeout. There
  is no resumable transfer: a message is whole or it is not there.

## Voice plan (so the current design does not block it)

1. **IP / LAN:** real calls over direct UDP or WebRTC; Reticulum handles
   discovery, identity and signalling. Opus at 6–16 kbps.
2. **Reticulum over TCP:** unreliable single packets (not Links — retransmits
   would stall audio), 60 ms Opus frames bundled under the 500-byte MTU.
3. **LoRa and radio:** live audio is not realistic. Short voice notes carried as
   files are, and that is the honest UX.

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
tests run with `cd apps/mobile/src-tauri && cargo test --lib`. The Python bridge
has its own tests, including the LXMF field format guard:
`python3 -m unittest discover -s sidecar/tests`. Smoke tests:
`node scripts/smoke-offline.mjs` and `node scripts/smoke-rns.mjs` (see the script
headers for arguments).
