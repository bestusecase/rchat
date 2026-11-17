# Record the final hardware pass

Status: ready-for-agent
Type: task
Blocked by: 05
Effort: `reticulum-surface-closeout`

## Question

The branch was rebased several times while the surface landed, so the last
recorded Pixel run does not necessarily match the commit that closes this
effort. Rebuild from the landed HEAD, run the new checks, and record the exact
result, or fix what they expose.

## Acceptance

- Confirm HEAD, rebuild the debug arm64 APK, install it, and run the Android
  check from ticket 05 against a desktop daemon.
- The non-destructive run passes: imported rows keep their values, toggle
  off/on follows the transport word, and a message crosses the TCP link with a
  delivered receipt.
- Record the commit hash, the phone serial and the date in the outcome of ticket
  05 and append a pointer to
  `.scratch/reticulum-surface/issues/13-android-parity.md`, so the older
  acceptance does not claim a tree it did not run on.
- Complete the manual steps and record the observed wording: the page with the
  engine stopped shows last-known data and the offline banner; a
  background/foreground cycle refreshes the graph and paths without a reload.
- If any check fails, fix the defect in a separate ticket-sized change, link it
  here, and rerun; do not record a partial pass as a pass.
- Confirm HEAD is unchanged after the run (no concurrent rebase landed), and
  note it in the outcome.

## Notes

- The concurrent session has been rewriting `main`; check `git log` before and
  after the hardware step rather than trusting the hash from an earlier run.
- The host `ufw` blocks inbound LAN TCP, so the check uses `adb reverse`; that
  is an environment fact, not a defect.
- Prior recorded pass: ticket 13, covering import, toggle, messages and the page
  during startup, before the rebased closeout commits.
- Adding a new hardware ticket for RNode/BLE is ticket 14's territory and stays
  deferred.

## Comments

<!-- outcome -->
