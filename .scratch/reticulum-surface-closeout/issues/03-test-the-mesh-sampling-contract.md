# Test the mesh sampling contract

Status: ready-for-agent
Type: task
Effort: `reticulum-surface-closeout`

## Question

The bridge claims the mesh is live but cheap: it samples every two seconds, emits
only when a fingerprint changes (or on an announce or interface change), and
caps the path table without dropping the peers nearest to us. Every one of those
claims is currently checked by hand or not at all. Add bridge tests at the emit
and payload boundaries so a regression fails the suite.

## Acceptance

- An unchanged payload emitted twice produces no second event; an explicit wake
  after a change produces exactly one. Capture events at the bridge's existing
  emit boundary; do not add a production seam just for the test.
- An announce wake and an interface state change each produce exactly one event;
  byte counters alone never do.
- The sampling loop leaves when readiness drops, asserted without a wall-clock
  sleep. If the thread lifecycle cannot be made deterministic, assert the loop's
  exit condition and document why.
- A path table larger than the 2000-entry cap yields a payload with exactly 2000
  paths, `paths_truncated` true and the untruncated total reported. The kept set
  is ordered nearest hops first and, within a hop count, most recently used
  first.
- A table at or below the cap is not truncated.
- The fingerprint test is extended rather than duplicated, and the new tests use
  synthetic data rather than a live Reticulum instance.
- No production behaviour changes unless a test exposes a real defect; in that
  case the fix lands with the test and the outcome notes it.

## Notes

- Prior art: `test_fingerprint_ignores_byte_counters_but_sees_structure` in
  `sidecar/tests/test_bridge.py`; the smoke scripts already prove the live path,
  this ticket is about the unit-level rules.
- The path cap and the ordering decision come from the mesh-status section of
  `.scratch/reticulum-surface/spec.md` and the closeout spec.
- Keep the test runtime tiny; these run with the rest of the bridge suite.

## Comments

<!-- outcome -->
