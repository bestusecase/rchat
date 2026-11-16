# 10 — Interface schema descriptor and device listing

Status: resolved
Type: task
Blocked by: 09

## Goal

Let the UI build a correct form for every RNS interface type without hardcoding RNS
knowledge in Svelte.

## Work

- Bridge `describe`: return one entry per supported type (`AutoInterface`,
  `TCPClientInterface`, `TCPServerInterface`, `BackboneInterface`,
  `BackboneClientInterface`, `UDPInterface`, `PipeInterface`, `I2PInterface`,
  `KISSInterface`, `AX25KISSInterface`, `RNodeInterface`, `RNodeMultiInterface`,
  `WeaveInterface`) with:
  - `label`, `summary`, `fields[]` (`key`, `label`, `widget`, `required`, `default`,
    `placeholder`, `help`, `common`, optional `show_if`, optional `choices`/`units`);
  - field order is the form order; `common: false` renders under Advanced.
- `list_devices { kind: "serial" }`: enumerate serial ports with
  `serial.tools.list_ports` (already a RNS dependency) and return
  `{ device, description }`; return an empty list with a reason on platforms without it
  (Android for now).
- Engine: pass-through commands `DescribeInterfaces`, `ListDevices` using the same
  token-correlated bridge request path; cache the descriptor per bridge connection.
- Unknown keys present on an imported interface stay editable: include them in Advanced
  with a widget guessed from the stored value (`true/false` → bool, digits → int,
  otherwise text) and never drop them on save.
- Wire types in `rchat-ipc`: `InterfaceType`, `InterfaceField`, `DeviceInfo`.

## Acceptance

- Describing returns every type above with at least one field, valid defaults and no
  duplicate keys inside a type.
- A form generated from the descriptor for `TCPClientInterface` produces a config RNS
  accepts when filled with host and port.
- Editing only an unknown key on an imported interface keeps all known keys intact.
- Serial listing works on desktop Linux and returns an empty list on Android.

## Comments

<!-- outcome -->
- Bridge `interface_catalogue()` + `describe_payload()` cover all 13 types with ordered
  fields (`widget`, `required`, `common`, `show_if`, `choices`). `do_list_devices()` returns
  an empty list with a reason when `serial.tools.list_ports` is unavailable (Android).
- Engine passes through `DescribeInterfaces`/`ListDevices` on the token-correlated bridge
  request path and caches the descriptor per bridge connection.
- Tests: `test_every_type_is_describable`,
  `test_tcp_client_defaults_form_a_valid_config`, `test_only_types_rns_refuses_are_read_only`,
  `test_shared_instance_marks_the_catalogue_read_only`.
