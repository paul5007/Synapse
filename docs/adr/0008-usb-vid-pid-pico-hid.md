# ADR-0008: Synapse Pico HID USB Identity

Status: Superseded by the software-only input decision in #588 and the
hardware-path removal in #589. This ADR is historical context only; there is no
active physical HID firmware, host driver, USB identity source, or release
artifact in the repository.

## Context

The retired M4 hardware plan introduced an RP2040 firmware image that would
enumerate as a composite USB device: boot mouse, boot keyboard, HID gamepad,
and CDC ACM serial control channel. The device needed stable USB identity
constants so the firmware, host driver, docs, and Windows source-of-truth
checks would all agree on the same VID/PID and USB strings.

The previous PRD text used the pid.codes community VID `0x1209` and a
placeholder PID `0xC0C0`. Issue #369 asked for a decision between that path,
USB-IF/pid.codes purchase or allocation, and the Raspberry Pi RP2040 VID path.

## Decision

Synapse uses Raspberry Pi's RP2040 vendor ID `0x2E8A` for the bundled Pico HID
firmware and internally reserves PID `0x1F50` for the Synapse Pico HID composite
CDC/HID device.

The canonical constants are:

```text
VID:          0x2E8A
PID:          0x1F50
Manufacturer: Synapse
Product:      Synapse Pico HID
Serial prefix: SYN-PICO-HID
```

This source constant has been removed with the retired hardware path.

Before broad public distribution, Synapse should submit the Raspberry Pi USB
PID application/PR for `0x1F50` or the closest available replacement. That
external reservation is an acquisition/setup step under #351: prepare the exact
form/PR and ask only for the approval needed to modify the external account or
upstream repository. The local M4 source decision and configured-host firmware
work use `0x1F50` unless the upstream assignment returns a different PID.

## Rationale

Raspberry Pi's `usb-pid` list is specifically for RP2040 products and states
that Raspberry Pi can sublicense PIDs under VID `0x2E8A` for RP2040-based
customer products. This matched the hardware considered by the retired plan.

The device uses standard CDC ACM and HID class drivers, so Windows should not
need a vendor driver. The product/manufacturer/serial strings still make the
device identifiable in Windows Enum, Device Manager, and Linux `lsusb -v`.
Keeping a unique Synapse PID avoids collisions with generic Pico SDK CDC UART
devices and gives the host driver one stable match tuple once hardware
enumeration lands.

The PID `0x1F50` is inside Raspberry Pi's listed commercial allocation range
`0x1000 - 0x1fff` and was not present in the Raspberry Pi `usb-pid` README on
2026-05-26. It is memorable enough for manual review but does not encode a
class or driver behavior.

## Alternatives Considered

- pid.codes `0x1209:0xC0C0` - rejected because Synapse hardware is RP2040, and
  Raspberry Pi maintains an RP2040-specific VID/PID sublicense path.
- USB-IF vendor ID purchase - rejected for M4 because the official VID-only
  form lists a US$6,000 processing fee and would require an external payment
  approval before it adds local configured-host value.
- Raspberry Pi generic Pico SDK CDC UART PID `0x000A` - rejected because the
  Synapse firmware is a composite CDC/HID device and should not collide with
  generic SDK UART examples.
- Development-only test VID/PID - rejected because Windows Enum evidence and
  host-driver auto-detect need a stable project identity from the start.

## Consequences

- Positive: firmware, host code, docs, and later Windows Enum FSV share one
  identity source.
- Positive: the chosen VID belongs to the RP2040 ecosystem rather than a
  generic community VID.
- Positive: public release can move through the official Raspberry Pi
  allocation workflow without changing architecture.
- Negative: upstream public reservation still needs an external PR/form step
  before release distribution.
- Trade-off accepted: M4 development uses an internal reservation now, and the
  release process must update the constants if Raspberry Pi assigns a different
  PID.

## Supersedes

- `docs/computergames/09_hardware_hid_gateway.md` previous `0x1209:0xC0C0`
  default.

## References

- Issue: #369
- Raspberry Pi USB PID allocation list: https://github.com/raspberrypi/usb-pid
- pid.codes VID 1209 rules: https://pid.codes/1209/
- USB-IF VID-only form: https://www.usb.org/sites/default/files/vid_only_form_070119.pdf
