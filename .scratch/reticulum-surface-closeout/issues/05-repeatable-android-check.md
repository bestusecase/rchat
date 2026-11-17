# Make the Android pass repeatable

Status: ready-for-agent
Type: task
Blocked by: 01, 02
Effort: `reticulum-surface-closeout`

## Question

The Pixel 8 acceptance was proven once, on a transient tree, partly through a
host firewall workaround. Turn it into a check that can be run from the repo:
build the debug arm64 APK, install it over the app, exercise import, toggling and
a TCP message over `adb`, and leave the manual steps that cannot be scripted
written down.

## Acceptance

- A shell entry point builds the APK from the current tree, installs it over the
  existing app (keeping data), writes a hand-written `rns/config` into the
  bridge directory with a unique `group_id` and a TCP client row, launches the
  app, forwards the engine port, and runs the assertions over the public API.
- Assertions cover: both rows imported with their original values and
  `origin = imported`; no duplicate seed row; AutoInterface online; toggling it
  off yields the derived `degraded` transport with no online interface, and
  back on yields `ready`; a message to a desktop daemon arrives with a
  `delivered` receipt; `list_devices` returns an empty serial list with a
  reason; `describe` returns every type.
- The phone reaches the desktop over `adb reverse`, so the host firewall is not
  involved. The forward and reverse mappings are removed on exit, and the
  engine port is read from the app's own endpoint file.
- A `--fresh` flag is the only path that clears app data, because it destroys
  the profile; the default run is non-destructive and reuses the existing
  identity and rows.
- When `adb`, the device or the SDK/NDK environment is missing, the script
  prints one reason and exits 0, matching the rendezvous scripts' skip
  behaviour.
- The manual steps still required are documented in the ticket and in the
  script's usage text: open the Network page with the engine stopped (last-known
  data plus offline banner) and background/foreground the app while the graph
  and paths update.
- The README's Android section names the check and the command that runs it.

## Notes

- Prior art: `scripts/smoke-rendezvous.mjs` and its netns companion for API
  assertions and skip behaviour; `scripts/dev-two-peers.sh` for the desktop
  endpoint the phone talks to.
- The import-fidelity and hidden-fragment tickets must land first, because the
  check asserts the imported row set and the client-visible section list.
- Do not uninstall the app in a normal run: the identity carries the address
  the desktop daemon knows.
- The udev rule and the Pixel serial are environment setup, not part of this
  ticket.

## Comments

<!-- outcome -->
