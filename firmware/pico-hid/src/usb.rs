#[path = "../../../crates/synapse-core/src/usb_identity.rs"]
mod usb_identity;

pub use usb_identity::{
    SYNAPSE_PICO_HID_MANUFACTURER, SYNAPSE_PICO_HID_PRODUCT, SYNAPSE_PICO_HID_SERIAL_PREFIX,
    SYNAPSE_PICO_HID_USB_PID, SYNAPSE_PICO_HID_USB_VID,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbIdentity {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: &'static str,
    pub product: &'static str,
    pub serial_prefix: &'static str,
}

pub const fn identity() -> UsbIdentity {
    UsbIdentity {
        vid: SYNAPSE_PICO_HID_USB_VID,
        pid: SYNAPSE_PICO_HID_USB_PID,
        manufacturer: SYNAPSE_PICO_HID_MANUFACTURER,
        product: SYNAPSE_PICO_HID_PRODUCT,
        serial_prefix: SYNAPSE_PICO_HID_SERIAL_PREFIX,
    }
}
