#![allow(unsafe_code)]

pub mod error;
pub mod handshake;
pub mod protocol;
pub mod transport;

pub use error::{HidError, HidResult};
pub use handshake::{
    FirmwareIdentity, HandshakeError, IDENTIFY_RESP_LEN, IDENTIFY_TIMEOUT_MS,
    expected_version_triplet, parse_and_validate_identify_response, parse_identify_response,
    perform_identify_handshake, validate_expected_major,
};
pub use protocol::{
    DEVICE_COMMAND_IDENTIFY_RESP, HOST_COMMAND_IDENTIFY, HOST_MAGIC, MAX_FRAME_LEN,
    MAX_PAYLOAD_LEN, encode_device_frame, encode_host_frame, encode_identify_frame,
    parse_device_frame,
};
pub use transport::{DEFAULT_BAUD_RATE, DEFAULT_READ_TIMEOUT_MS, HidGateway};
