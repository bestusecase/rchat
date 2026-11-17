# Preserve the whole hand-written config on import

Status: ready-for-agent
Type: task
Effort: `reticulum-surface-closeout`

## Question

Make the bridge keep the parts of `rns/config` it does not generate: the file
preamble, and every unknown key and comment inside `[reticulum]` and `[logging]`.
The preserved content must render back in its original place, and the generated
keys must remain the only authority for transport mode, sharing, ports and log
level.

## Acceptance

- Parsing a config returns, alongside the interface rows, three new preserved
  fragments: the preamble, the unknown lines of `[reticulum]`, and the unknown
  lines of `[logging]`. They are carried under reserved placement keys beginning
  with `@`, so they cannot be mistaken for a real section or for an
  `interfaces/<name>` block.
- A preserved line whose key is one the renderer generates for that section is
  not preserved: the generated line is written once, and the original is not
  duplicated. Comments and blank lines inside those sections are preserved as
  free lines.
- Rendering writes the preamble first, then `[reticulum]` with its preserved
  lines directly after the generated keys, then `[logging]` the same way, then
  `[interfaces]` and the unmanaged sections exactly as today.
- Parsing and rendering the same input twice produces byte-identical output
  (idempotence), including when the input has duplicate generated sections.
- A missing config still imports as the existing empty shape; malformed input
  still never loses content silently.
- The import reply shape is unchanged: interface rows, `[section, raw]` pairs,
  and `existed`. The engine and the client contract need no change for this
  ticket; `configure` already forwards every pair it receives.
- Bridge unit tests cover: preamble with comments, unknown keys in both
  generated sections, a key colliding with a generated one, idempotence, and a
  config with only generated keys producing no fragments.
- The README's config-import paragraph says the header and unknown keys inside
  the generated sections are kept.

## Notes

- Decision D9 in `.scratch/reticulum-surface/spec.md` and the fidelity decision
  in `.scratch/reticulum-surface-closeout/spec.md` are the authority.
- "Byte-for-byte" covers preserved fragment lines and their order. The spacing
  rchat owns, the generated keys, and the key order inside managed interface
  blocks are not promised.
- Prior art: `test_unknown_interface_and_sections_are_preserved`,
  `test_render_round_trips_managed_rows` and `test_render_order_and_disabled_flag`
  in `sidecar/tests/test_bridge.py`.
- The import marker, the config-wins notice and the one-shot model do not change.

## Comments

<!-- outcome -->
