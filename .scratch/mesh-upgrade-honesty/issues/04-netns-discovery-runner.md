# Add a Linux namespace runner for the discovery case

Status: ready-for-agent
Type: task
Effort: `mesh-upgrade-honesty`

## Question

Make the relay-free discovery case runnable on one machine by putting one leaf
in its own network namespace, so the zero-config path is scripted rather than
only manual.

## Acceptance

- A companion runner (`scripts/smoke-rendezvous-netns.mjs`) starts leaf A on the
  host and leaf C in a netns over a veth pair, with no relay configured.
- A forwarder inside the namespace exposes leaf C's loopback HTTP/WS port on the
  veth address so the orchestrator can drive both leaves identically.
- Both leaves use a per-run AutoInterface `group_id`, start with one
  AutoInterface row each, exchange a message, and finish with neither leaf
  storing the other's Reticulum address.
- The runner reuses `scripts/lib.mjs` and `scripts/mesh-config.mjs` rather than
  duplicating the client.
- When `ip netns` access or IPv6 on the veth is unavailable, the runner prints
  the unmet requirement and exits 0.
- Namespace, veth and forwarder are removed on success, failure, SIGINT and
  SIGTERM.
- `node scripts/smoke-rendezvous.mjs` remains unchanged for unprivileged hosts,
  and the skip path is tested without namespaces.

## Notes

- Linux only; Windows and macOS skip.
- See `.scratch/mesh-upgrade-honesty/spec.md`, the decisions on the namespace
  runner, topology, LAN isolation, skipping and cleanup.
