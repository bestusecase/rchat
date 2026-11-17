# 04 — Documentation and device validation

Status: ready-for-agent

## Scope

- Correct the repository documentation that asserts notifications carry no
  message content, that the shell has no previews, and that the notification is
  a count only.
- Keep the missing conversation deep link listed as a known gap, and record the
  new privacy posture — private visibility, count on a locked screen — where the
  Android shell is described.
- Record the testing shape: the state-machine tests are the automated coverage,
  and the rendering is verified by hand.

## Acceptance

- No remaining sentence in the repository claims notification content never
  leaves the app.
- The Android section describes per-conversation threads and the locked-screen
  behaviour accurately.
- The device acceptance pass from the spec is recorded, with the device and the
  peer used noted, so the next person can repeat it.
