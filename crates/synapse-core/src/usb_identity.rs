/// Raspberry Pi RP2040 USB vendor ID used by the Synapse Pico HID firmware.
pub const SYNAPSE_PICO_HID_USB_VID: u16 = 0x2E8A;

/// Synapse internal PID reservation for the pico-hid composite CDC/HID device.
pub const SYNAPSE_PICO_HID_USB_PID: u16 = 0x1F50;

/// USB manufacturer string for the bundled Synapse Pico HID firmware.
pub const SYNAPSE_PICO_HID_MANUFACTURER: &str = "Synapse";

/// USB product string for the bundled Synapse Pico HID firmware.
pub const SYNAPSE_PICO_HID_PRODUCT: &str = "Synapse Pico HID";

/// Prefix for stable per-board USB serial numbers.
pub const SYNAPSE_PICO_HID_SERIAL_PREFIX: &str = "SYN-PICO-HID";
