# Script the hot-apply interface check

Status: ready-for-agent
Type: task
Effort: `reticulum-surface-closeout`

## Question

The phase-2 contract — attach an interface, see it online, delete it, see it
gone, and watch a refused apply roll back — has only ever been checked by hand
and in one smoke run. Make it a script that runs against a daemon the dev
scripts already start, so it can be repeated before a release or after a bridge
change.

## Acceptance

- A script in the style of the existing smoke checks takes a base URL and token,
  and against a running daemon:
  - creates a TCP server interface on a free port, waits until it is online in
    `mesh_status` and present in `rns/config`;
  - deletes it, waits until it is absent from `mesh_status` and the config file,
    and confirms no row remains;
  - attempts an apply that must fail (for example binding a port already in
    use), asserts the refusal names the interface, and asserts the previously
    working interface stays online and the stored row is unchanged.
- The script exits non-zero on any failed assertion and closes its client on
  every path.
- It reuses `scripts/lib.mjs` and the public command vocabulary the UI uses; no
  engine or bridge change is required.
- It can run against the leaves of `scripts/dev-mesh.sh` or
  `scripts/dev-two-peers.sh` without extra setup, and the README's mesh section
  names it.
- The existing `smoke-mesh.mjs` and `smoke-rns.mjs` keep their current scope;
  this script covers interface management only.

## Notes

- Prior art: `scripts/mesh-config.mjs` (create/update/delete through the API),
  `scripts/smoke-mesh.mjs` (waiting on status), and the manual phase-2 check.
- Pick the failure case so it fails for the right reason: a port already bound
  by the script's own server, not an unreachable host, is dependable.
- Keep the script idempotent: it should clean up rows it created even when an
  assertion fails.

## Comments

<!-- outcome -->
