# 14 — Android RNode over BLE/USB

Status: deferred
Type: task
Blocked by: 13
Deferred because: no radio hardware available to test

## Goal

Manage an RNode from the phone, so a pocket mesh can be set up without a laptop.

## Work

- Add the `able` package to the Chaquopy bundle and the Android Bluetooth permissions to
  the manifest; handle the runtime permission request from the Kotlin side.
- Use `RNS.Interfaces.Android.RNodeInterface` (BLE) and the USB serial path where
  available; enumerate paired BLE devices through a small bridge command, since
  `serial.tools.list_ports` does not see them.
- Extend the `serial` widget to a device picker that can switch between USB serial and
  paired BLE devices, with a clear "pair in Android settings first" state.
- Surface radio telemetry (RSSI, SNR, airtime, battery) from the existing `radio` block
  in `get_interface_stats`.
- Reuse the desktop RNode form; only the device picker differs.

## Acceptance (once hardware exists)

- Pair an RNode over BLE, add it from the Interfaces tab, and see it online with radio
  stats.
- Failing to pair or connect surfaces RNS's error without persisting the row.
- A message sent over the RNode path is delivered to a second node.
- Battery use while the radio interface is up is measured and recorded in the ticket.
