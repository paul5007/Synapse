# 05 - M4 Retired Hardware HID Plan

This phase file is retained as a historical pointer only. The physical
hardware-HID plan was superseded by the software-only input decision in #588 and
the removal work in #589.

Current implementation direction:

- Live input backends are `software` and `vigem`.
- `Backend::Hardware` remains only as a compatibility token and fails closed
  through `HardwareUnavailableBackend` with `ACTION_BACKEND_UNAVAILABLE`.
- The `synapse-hid-host` crate, RP2040 firmware project, hardware consent
  prompt, `--hardware-hid` / `SYNAPSE_HARDWARE_HID`, and `hid identify` / `hid
  flash` surfaces are not active work.
- M4 compound action surfaces (`act_combo`, `act_run_shell`, `act_launch`) and
  profile/runtime work continue independently of physical HID hardware.

For current implementation work, use `06_m5_production_polish.md`, the M5
registry/audit issue set, and the open GitHub issue queue. Manual FSV remains
mandatory per `AGENTS.md` D1.
