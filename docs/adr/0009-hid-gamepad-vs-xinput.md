# ADR-0009: Pico HID Gamepad vs XInput Emulation

## Context

M4 adds a Raspberry Pi Pico firmware image that exposes mouse, keyboard,
gamepad, and CDC ACM interfaces to Windows. The gamepad interface needs a clear
contract before the composite HID descriptor lands in `firmware/pico-hid`.

The PRD used "XInput-like" wording for the 14-byte pad report. That wording is
too loose: XInput is the Windows API for XUSB controllers, while ordinary USB
HID gamepads are consumed through DirectInput, RawInput, HID APIs, or newer
game-input APIs depending on the application.

## Decision

`pico-hid` emits a standard USB HID gamepad interface, not a raw XInput/XUSB
device.

The firmware pad report remains the 14-byte Synapse report:

```text
buttons: u16        // A/B/X/Y, bumpers, Back/Start, stick buttons, d-pad bits, Guide, reserved
left_trigger: u8
right_trigger: u8
thumb_lx: i16
thumb_ly: i16
thumb_rx: i16
thumb_ry: i16
reserved: u16       // zero; keeps the M4 14-byte report ABI explicit
```

The descriptor must be HID class, Generic Desktop/Game Pad usage, with standard
button and axis usages. It should enumerate as a HID/DirectInput-visible
gamepad in Windows, and `joy.cpl` is the manual source-of-truth check for the
pad surface.

Synapse keeps ViGEm as the software XInput/X360 path. Profiles or callers that
require an XInput-visible Xbox controller should use the ViGEm backend, not the
Pico HID backend.

## Rationale

Microsoft documents XInput as the API used to interact with XUSB controllers
on Windows. Microsoft also documents that the Windows XUSB driver implements
the kernel-mode interface used by the XInput DLL and separately exposes a HID
class interface so DirectInput can see those devices. That is not the same as a
generic HID descriptor becoming an XInput device.

Standard HID is the portable, class-driver path for open RP2040 firmware. A
custom XUSB/XInput driver path would move Synapse into kernel-driver territory.
Microsoft's Windows driver-signing documentation says kernel-mode drivers must
be signed to load on 64-bit Windows, and Windows 10+ kernel-mode drivers go
through the Hardware Developer Center signing flow. That does not fit the M4
configured-host firmware goal.

ViGEm already covers the XInput use case as a Windows kernel-mode virtual
gamepad bus that emulates Xbox 360 and DualShock 4 controllers. Synapse already
uses that backend for software-only gamepad output, so duplicating that work in
RP2040 firmware would add signing and compatibility burden without improving
the hardware-HID value proposition.

## Consequences

- Positive: the Pico firmware stays on standard USB class drivers and does not
  require a custom Windows kernel driver.
- Positive: Windows Device Manager, `Get-PnpDevice -Class HIDClass`, HID APIs,
  DirectInput, and `joy.cpl` can all serve as real SoT surfaces for manual FSV.
- Positive: ViGEm remains the explicit, well-understood XInput compatibility
  path for games that require Xbox-controller semantics.
- Negative: XInput-only games may not recognize the hardware gamepad interface.
- Trade-off accepted: M4 hardware HID optimizes for real peripheral input; the
  software ViGEm backend optimizes for XInput compatibility.

## Verification Plan

When #371 has a flashed Pico attached, manual FSV must read:

1. Device Manager composite parent and HID child interfaces.
2. `Get-PnpDevice -Class HIDClass` rows for the Synapse Pico HID gamepad.
3. `joy.cpl` showing the gamepad and live axis/button state after known
   synthetic reports.

Before the real device exists, documentation SoT is the committed ADR and
patched PRD text only; that is not a substitute for the hardware FSV.

## Supersedes

- `docs/computergames/09_hardware_hid_gateway.md` references to a
  "Vendor-defined gamepad (X-input-compatible report)".
- `docs/impplan/05_m4_hardware_hid_first_game.md` references to an
  "XInput-like" Pico pad.

## References

- Issue: #372
- Microsoft XInput programming guide: https://learn.microsoft.com/en-us/windows/win32/xinput/programming-guide
- Microsoft DirectInput and XUSB devices: https://learn.microsoft.com/en-us/windows/win32/xinput/directinput-and-xusb-devices
- Microsoft XInput and DirectInput comparison: https://learn.microsoft.com/en-us/windows/win32/xinput/xinput-and-directinput
- Microsoft Windows driver signing: https://learn.microsoft.com/en-us/windows-hardware/drivers/install/driver-signing
- ViGEmBus repository: https://github.com/nefarius/ViGEmBus
