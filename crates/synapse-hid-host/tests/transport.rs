use std::time::Duration;

use synapse_core::error_codes;
use synapse_hid_host::{DEFAULT_BAUD_RATE, DEFAULT_READ_TIMEOUT_MS, HidError, HidGateway};

#[test]
fn connect_missing_port_returns_port_not_found() {
    let missing_port = "SYNAPSE_MISSING_PORT_DO_NOT_CREATE";
    let error = match HidGateway::connect(missing_port) {
        Ok(gateway) => panic!("missing port should not connect: {gateway:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HidError::PortNotFound {
            port_name: missing_port.to_owned(),
        }
    );
    assert_eq!(error.code(), error_codes::HID_PORT_NOT_FOUND);
}

#[test]
fn transport_defaults_match_m4_contract() {
    assert_eq!(DEFAULT_BAUD_RATE, 1_000_000);
    assert_eq!(
        Duration::from_millis(DEFAULT_READ_TIMEOUT_MS).as_millis(),
        5
    );
}
