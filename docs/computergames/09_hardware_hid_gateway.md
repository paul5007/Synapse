# 09 - Retired Hardware HID Gateway

The physical hardware-HID strategy is retired by issues #588 and #589.

Current state:

- `software` is the live keyboard/mouse backend through Win32 `SendInput`.
- `vigem` is the live software-only virtual controller backend.
- The public `hardware` backend token is retained only for profile/package
  compatibility and always fails closed with `ACTION_BACKEND_UNAVAILABLE`.
- There is no `synapse-hid-host` crate, RP2040 firmware project, hardware
  consent prompt, `--hardware-hid` flag, `SYNAPSE_HARDWARE_HID` env var, `hid
  identify`, or `hid flash` runtime path.

Historical references to this file should be read as design context only. New
input work belongs in the software backend and ViGEm paths, with manual FSV
against the real MCP runtime and physical source-of-truth readbacks.
