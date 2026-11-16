# 08 — Three-node dev mesh and scripted checks

Status: resolved
Type: task
Blocked by: 01, 02, 06

## Goal

Prove the graph and status data against a real multi-hop mesh, and keep it proven.

## Work

- `scripts/dev-mesh.sh`:
  - starts a plain `rnsd` in `.dev/relay` with `enable_transport = Yes`, a TCP server,
    and a TCP client per leaf (or one server both leaves connect to);
  - starts rchat daemons A and C in `.dev/mesh-a`, `.dev/mesh-c`, each with
    `RCHAT_RNS_AUTO=0` and a TCP client to the relay;
  - prints each leaf's API URL, token and address, and the relay's config path;
  - cleans up all three processes on exit.
- `scripts/smoke-rns.mjs` additions (or a sibling `smoke-mesh.mjs`):
  - assert `mesh_status` on A lists both TCP interfaces, `peer_paths` contains C with
    `hops == 2` and a `next_hop` that is not C, and that the announced peer is present;
  - assert `mesh_paths` filtering by C's address prefix returns one row;
  - assert `mesh_announces` returns C after its announce.
- Rust tests for the engine-side assembly (peer_paths intersection, bandwidth_class
  mapping, path paging).
- `sidecar/tests/` with stdlib `unittest` to cover the mesh payload whitelist and
  fingerprint (no info leak from raw stats objects).
- README: a short "See the mesh" section pointing at the script and the Network page.

## Acceptance

- `./scripts/dev-mesh.sh` comes up reliably from a clean `.dev/` and exits cleanly.
- The scripted checks pass on a machine with only `rnspath`/`rnsd` from `pip install rns`.
- The Network page against that mesh shows a 2-hop peer behind the relay, a 1-hop peer,
  and both entries in Announced.
- Stopping the relay makes both peers drop off the graph on the next refresh without
  breaking the page.

## Comments

<!-- outcome -->
- `scripts/dev-mesh.sh` starts `rnsd` as the relay plus two `rchatd` nodes (A and C) with
  `RCHAT_RNS_AUTO=0`; `scripts/smoke-mesh.mjs` asserts A sees C at 2 hops behind the relay
  and that the path table and announce history agree.
- Ran 2026-11-15 16:34–16:43; the relay's `storage/cache/announces.db-wal` shows the mesh
  was live. Start `rchatd` with the repo root as the working directory, or it cannot find
  the bridge script.
- Two rchat instances with AutoInterface on one host collide on the UDP discovery port,
  which is why the script disables AutoInterface.
