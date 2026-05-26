use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use synapse_core::error_codes;
use synapse_core::{
    EXPECTED_FW_MAJOR, SYNAPSE_PICO_HID_BUILD_HASH_LEN, SYNAPSE_PICO_HID_FW_MINOR,
    SYNAPSE_PICO_HID_FW_PATCH,
};

use crate::error::{HidError, HidResult};
use crate::protocol::{
    DEVICE_COMMAND_IDENTIFY_RESP, EncodeError, MAX_FRAME_LEN, ParseError, encode_identify_frame,
    parse_device_frame,
};

pub const IDENTIFY_RESP_LEN: usize = 20;
pub const IDENTIFY_TIMEOUT_MS: u64 = 200;
pub const IDENTIFY_SEQUENCE: u32 = 0;

/// Parsed firmware identity from the Pico `IDENTIFY_RESP` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirmwareIdentity {
    pub fw_major: u8,
    pub fw_minor: u8,
    pub fw_patch: u8,
    pub build_hash: [u8; SYNAPSE_PICO_HID_BUILD_HASH_LEN],
    pub vid: u16,
    pub pid: u16,
    pub capabilities: u32,
}

impl FirmwareIdentity {
    /// Returns true when the firmware major version matches the host contract.
    #[must_use]
    pub const fn matches_expected_version(&self) -> bool {
        self.fw_major == EXPECTED_FW_MAJOR
    }
}

/// Host-side failures while parsing or validating the firmware handshake.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HandshakeError {
    #[error("identify payload length {actual} did not match expected {expected}")]
    InvalidIdentifyPayloadLength { actual: usize, expected: usize },
    #[error("firmware major version {actual} did not match expected {expected}")]
    FirmwareVersionMismatch { expected: u8, actual: u8 },
}

impl HandshakeError {
    /// Returns the structured Synapse error code for this handshake failure.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidIdentifyPayloadLength { .. } => error_codes::HID_PROTOCOL_HANDSHAKE_FAILED,
            Self::FirmwareVersionMismatch { .. } => error_codes::HID_FIRMWARE_VERSION_MISMATCH,
        }
    }
}

/// Parses the fixed 20-byte Pico `IDENTIFY_RESP` payload.
///
/// # Errors
///
/// Returns [`HandshakeError::InvalidIdentifyPayloadLength`] when the payload
/// length does not exactly match [`IDENTIFY_RESP_LEN`].
pub fn parse_identify_response(payload: &[u8]) -> Result<FirmwareIdentity, HandshakeError> {
    if payload.len() != IDENTIFY_RESP_LEN {
        return Err(HandshakeError::InvalidIdentifyPayloadLength {
            actual: payload.len(),
            expected: IDENTIFY_RESP_LEN,
        });
    }

    let mut build_hash = [0u8; SYNAPSE_PICO_HID_BUILD_HASH_LEN];
    build_hash.copy_from_slice(&payload[4..12]);

    Ok(FirmwareIdentity {
        fw_major: payload[0],
        fw_minor: payload[1],
        fw_patch: payload[2],
        build_hash,
        vid: u16::from_le_bytes([payload[12], payload[13]]),
        pid: u16::from_le_bytes([payload[14], payload[15]]),
        capabilities: u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]),
    })
}

/// Parses and validates the Pico `IDENTIFY_RESP` payload against host constants.
///
/// # Errors
///
/// Returns [`HandshakeError::InvalidIdentifyPayloadLength`] for malformed
/// payload sizes or [`HandshakeError::FirmwareVersionMismatch`] when the
/// firmware major version differs from [`EXPECTED_FW_MAJOR`].
pub fn parse_and_validate_identify_response(
    payload: &[u8],
) -> Result<FirmwareIdentity, HandshakeError> {
    let identity = parse_identify_response(payload)?;
    validate_expected_major(&identity)?;
    Ok(identity)
}

/// Validates the parsed firmware major version against [`EXPECTED_FW_MAJOR`].
///
/// # Errors
///
/// Returns [`HandshakeError::FirmwareVersionMismatch`] when the parsed major
/// version does not match the host contract.
pub const fn validate_expected_major(identity: &FirmwareIdentity) -> Result<(), HandshakeError> {
    if identity.matches_expected_version() {
        Ok(())
    } else {
        Err(HandshakeError::FirmwareVersionMismatch {
            expected: EXPECTED_FW_MAJOR,
            actual: identity.fw_major,
        })
    }
}

/// Returns the host-expected firmware version triplet.
#[must_use]
pub const fn expected_version_triplet() -> [u8; 3] {
    [
        EXPECTED_FW_MAJOR,
        SYNAPSE_PICO_HID_FW_MINOR,
        SYNAPSE_PICO_HID_FW_PATCH,
    ]
}

/// Sends `IDENTIFY` and waits for a validated `IDENTIFY_RESP`.
///
/// # Errors
///
/// Returns [`HidError::ProtocolHandshakeFailed`] when the frame cannot be
/// written, no response arrives before `timeout`, or the response frame is
/// malformed. Returns [`HidError::FirmwareVersionMismatch`] when the firmware
/// major version differs from the host contract.
pub fn perform_identify_handshake<T>(
    transport: &mut T,
    timeout: Duration,
) -> HidResult<FirmwareIdentity>
where
    T: Read + Write + ?Sized,
{
    let mut tx = [0u8; MAX_FRAME_LEN];
    let tx_len = encode_identify_frame(IDENTIFY_SEQUENCE, &mut tx).map_err(encode_error)?;
    transport
        .write_all(&tx[..tx_len])
        .map_err(|error| handshake_failed(format!("failed to write IDENTIFY frame: {error}")))?;
    transport
        .flush()
        .map_err(|error| handshake_failed(format!("failed to flush IDENTIFY frame: {error}")))?;

    read_identify_response(transport, timeout)
}

fn read_identify_response<T>(transport: &mut T, timeout: Duration) -> HidResult<FirmwareIdentity>
where
    T: Read + ?Sized,
{
    let start = Instant::now();
    let mut rx = [0u8; MAX_FRAME_LEN];
    let mut rx_len = 0usize;

    loop {
        if start.elapsed() > timeout {
            return Err(handshake_failed(format!(
                "IDENTIFY_RESP timeout after {} ms",
                timeout.as_millis()
            )));
        }

        if rx_len == rx.len() {
            return Err(handshake_failed("IDENTIFY_RESP frame exceeded buffer"));
        }

        match transport.read(&mut rx[rx_len..]) {
            Ok(0) => {}
            Ok(count) => {
                rx_len += count;
                match parse_device_frame(&rx[..rx_len]) {
                    Ok(frame) => {
                        return validate_identify_frame(frame.seq, frame.command, frame.payload);
                    }
                    Err(ParseError::NeedMore { .. }) => {}
                    Err(error) => {
                        return Err(handshake_failed(format!(
                            "invalid IDENTIFY_RESP frame: {error:?}"
                        )));
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
            Err(error) => {
                return Err(handshake_failed(format!(
                    "failed to read IDENTIFY_RESP frame: {error}"
                )));
            }
        }
    }
}

fn validate_identify_frame(seq: u32, command: u8, payload: &[u8]) -> HidResult<FirmwareIdentity> {
    if seq != IDENTIFY_SEQUENCE {
        return Err(handshake_failed(format!(
            "IDENTIFY_RESP seq {seq} did not match request seq {IDENTIFY_SEQUENCE}"
        )));
    }

    if command != DEVICE_COMMAND_IDENTIFY_RESP {
        return Err(handshake_failed(format!(
            "expected IDENTIFY_RESP 0x{DEVICE_COMMAND_IDENTIFY_RESP:02X}, got 0x{command:02X}"
        )));
    }

    parse_and_validate_identify_response(payload).map_err(HidError::from)
}

fn encode_error(error: EncodeError) -> HidError {
    handshake_failed(format!("failed to encode IDENTIFY frame: {error:?}"))
}

fn handshake_failed(detail: impl Into<String>) -> HidError {
    HidError::ProtocolHandshakeFailed {
        detail: detail.into(),
    }
}
