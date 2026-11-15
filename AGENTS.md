# rchat

A fast, offline-first chat client for [Reticulum](https://reticulum.network), on
desktop and Android. Discord-style interface, peer-to-peer messaging, no central
server.

Layout: `crates/rchatd` is the engine (also linked into the Android app),
`sidecar/bridge.py` is the Reticulum stack, `apps/desktop` and `apps/mobile` are
the shells. The mobile app is a separate Cargo workspace.

## Agent skills

### Issue tracker

Issues and specs live as markdown files under `.scratch/<feature>/` in this repo. See `docs/agents/issue-tracker.md`.

### Domain docs

Single-context: one `GLOSSARY.md` at the repo root, plus `docs/adr/`. See `docs/agents/domain.md`.
