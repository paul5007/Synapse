# Hardware Troubleshooting

Use this page when the M4 Pico HID path does not enumerate, flash, identify, or
stay connected. Every fix must end with a direct source-of-truth readback:
device list, COM port list, registry USB Enum key, `RPI-RP2` volume, firmware
identity response, action error code, or visible hardware state.

Related error-code source:

- [Hardware HID codes](../computergames/06_data_schemas.md#88-hardware-hid)
- [Action codes](../computergames/06_data_schemas.md#82-action)
- [Safety codes](../computergames/06_data_schemas.md#89-safety)

## Common failures

| Symptom | Likely cause | Source-of-truth readback | Fix | Related code |
|---|---|---|---|---|
| Pico does not appear as `RPI-RP2` while holding `BOOTSEL`. | Charge-only cable, `BOOTSEL` not held during plug-in, bad USB port, or damaged board. | `Get-CimInstance Win32_LogicalDisk` has no `VolumeName = RPI-RP2`; `Get-PnpDevice -PresentOnly` has no RP2/Pico boot device. | Use a known data-capable cable, hold `BOOTSEL` before plugging in, try a direct host USB port, then re-read the volume/device list. | [`HID_PORT_NOT_FOUND`](../computergames/06_data_schemas.md#88-hardware-hid) |
| Pico appears as `RPI-RP2` but no COM port appears after copying the UF2. | Wrong UF2, copy did not complete, firmware crashed, or the board is still in bootloader mode. | `RPI-RP2` remains mounted after copy or no matching `Win32_SerialPort`/`Get-PnpDevice` row appears after reboot. | Rebuild `pico-hid.uf2`, copy it again, wait for the volume to dismount, then read `Get-PnpDevice` and `Win32_SerialPort`. | [`HID_PORT_NOT_FOUND`](../computergames/06_data_schemas.md#88-hardware-hid) |
| `hid identify` cannot open the selected COM port. | Wrong COM port, port already open, disconnected cable, or Windows denied the handle. | `Win32_SerialPort` shows no matching port, or the port appears but open fails immediately. | Close other serial monitors, unplug/replug the Pico, select the exact COM port from `Win32_SerialPort`, and retry. | [`HID_PORT_OPEN_FAILED`](../computergames/06_data_schemas.md#88-hardware-hid) |
| `hid identify` opens the port but fails the handshake. | Non-Synapse firmware, stale protocol framing, wrong baud/settings, or a partial serial read. | Port exists, but identify response bytes are absent, malformed, or not `IDENTIFY_RESP`. | Reflash with the bundled Synapse UF2, reconnect, and retry `hid identify`; record the raw response/error in the issue. | [`HID_PROTOCOL_HANDSHAKE_FAILED`](../computergames/06_data_schemas.md#88-hardware-hid) |
| `hid identify` returns firmware version mismatch. | Board has Synapse firmware with a different major protocol version. | `IDENTIFY_RESP` is present but `fw_major` differs from host `EXPECTED_FW_MAJOR`. | Reflash with the bundled `.uf2` from the same Synapse release and verify identify again. | [`HID_FIRMWARE_VERSION_MISMATCH`](../computergames/06_data_schemas.md#88-hardware-hid) |
| Windows shows `Unknown Device` after flash. | USB composite descriptor problem, cable/port instability, or Windows cached a bad device state. | `Get-PnpDevice -PresentOnly` shows `Unknown Device` or problem status for the Pico USB instance. | Unplug/replug once, try a direct port and data cable, reflash, then inspect `HKLM:\SYSTEM\CurrentControlSet\Enum\USB` for the actual VID/PID row. | [`HID_PROTOCOL_HANDSHAKE_FAILED`](../computergames/06_data_schemas.md#88-hardware-hid) |
| Hardware actions report backend unavailable. | Synapse was not started with hardware HID enabled or auto-discovery found no Synapse Pico. | MCP/action error contains `ACTION_BACKEND_UNAVAILABLE`; device/COM readbacks show no active Synapse hardware route. | Start with `--hardware-hid auto` or the exact COM port, verify identify, then rerun the action. | [`ACTION_BACKEND_UNAVAILABLE`](../computergames/06_data_schemas.md#82-action) |
| Hardware actions report disconnected after unplug/replug. | Serial link dropped and reconnect has not completed. | Error contains `ACTION_HID_PORT_DISCONNECTED`; `Get-PnpDevice` or `Win32_SerialPort` changed before/after unplug. | Wait for reconnect, re-read the COM port, then rerun identify and the action. | [`ACTION_HID_PORT_DISCONNECTED`](../computergames/06_data_schemas.md#82-action) |
| Firmware rejects a command. | Frame is well-formed but invalid for current firmware state, range, report kind, or permission gate. | Host receives a command rejection status and maps it to `HID_COMMAND_REJECTED`. | Capture the command payload, compare it to `docs/computergames/09_hardware_hid_gateway.md`, fix the caller or firmware validator, and retry. | [`HID_COMMAND_REJECTED`](../computergames/06_data_schemas.md#88-hardware-hid) |
| Watchdog keeps firing. | Host is not sending `WATCHDOG_KICK`, scheduler is stalled, or the firmware watchdog timeout is too short for the current path. | Firmware telemetry shows watchdog count increasing; action/reflex logs show missing or late kicks. | Verify the host loop sends kicks during idle periods, inspect telemetry before/after, and tune only after proving the trigger path. | [`HID_LINK_TIMEOUT`](../computergames/06_data_schemas.md#88-hardware-hid) |
| HID keyboard/mouse reports interfere with the real keyboard or mouse. | Expected behavior: hardware HID is a real input device once flashed. | Focused app receives physical HID input; action log or hardware observer shows reports emitted. | Hit the operator panic hotkey, run `release_all`, unplug the Pico if needed, then inspect action state before resuming. | [`SAFETY_OPERATOR_HOTKEY_FIRED`](../computergames/06_data_schemas.md#89-safety), [`SAFETY_RELEASE_ALL_FIRED`](../computergames/06_data_schemas.md#82-action) |

## Before/after checklist

For every hardware troubleshooting step, record:

1. Before device state: `Get-PnpDevice -PresentOnly`, `Win32_SerialPort`, and
   `Win32_LogicalDisk` when BOOTSEL is expected.
2. Trigger: cable swap, BOOTSEL flash, UF2 copy, daemon restart, `hid identify`,
   or hardware action call.
3. After device state: the same physical readback command plus any firmware
   identify response, telemetry counter, or action error payload.
4. Expected outcome: name the exact row, port, volume, code, or LED state that
   should change.

Do not close a hardware issue from command success alone. Read the physical
device or firmware state that the command was supposed to create.
