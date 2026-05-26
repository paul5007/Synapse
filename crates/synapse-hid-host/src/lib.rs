#![allow(unsafe_code)]

pub mod error;
pub mod handshake;
pub mod transport;

pub use error::{HidError, HidResult};
pub use handshake::{
    FirmwareIdentity, HandshakeError, IDENTIFY_RESP_LEN, expected_version_triplet,
    parse_and_validate_identify_response, parse_identify_response, validate_expected_major,
};
pub use transport::{DEFAULT_BAUD_RATE, DEFAULT_READ_TIMEOUT_MS, HidGateway};
