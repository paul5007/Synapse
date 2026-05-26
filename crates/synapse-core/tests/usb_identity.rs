use synapse_core::{
    SYNAPSE_PICO_HID_MANUFACTURER, SYNAPSE_PICO_HID_PRODUCT, SYNAPSE_PICO_HID_SERIAL_PREFIX,
    SYNAPSE_PICO_HID_USB_PID, SYNAPSE_PICO_HID_USB_VID,
};

#[test]
fn pico_hid_usb_identity_matches_adr_0008() {
    assert_eq!(SYNAPSE_PICO_HID_USB_VID, 0x2E8A);
    assert_eq!(SYNAPSE_PICO_HID_USB_PID, 0x1F50);
    assert_eq!(SYNAPSE_PICO_HID_MANUFACTURER, "Synapse");
    assert_eq!(SYNAPSE_PICO_HID_PRODUCT, "Synapse Pico HID");
    assert_eq!(SYNAPSE_PICO_HID_SERIAL_PREFIX, "SYN-PICO-HID");
}
