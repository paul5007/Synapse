use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use crate::error::{HidError, HidResult};
use crate::pipeline::{
    NAK_REASON_LEN_INVALID, NAK_REASON_PAYLOAD_INVALID, NAK_REASON_UNKNOWN_COMMAND,
};
use crate::protocol::{
    DEVICE_COMMAND_NAK, DEVICE_COMMAND_TELEMETRY_RESP, EncodeError, HOST_COMMAND_GET_TELEMETRY,
    MAX_FRAME_LEN, ParseError, encode_host_frame, parse_device_frame_prefix,
};

pub const TELEMETRY_PAYLOAD_LEN: usize = 28;
const READ_CHUNK_LEN: usize = 64;
const MAX_RX_BUFFER_LEN: usize = MAX_FRAME_LEN * 2;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct HidTelemetrySnapshot {
    pub uptime_ms: u32,
    pub frames_received: u32,
    pub frames_dropped: u32,
    pub link_errors: u32,
    pub commands_executed: u32,
    pub watchdog_fires: u32,
    pub crc_errors: u32,
}

impl HidTelemetrySnapshot {
    #[must_use]
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != TELEMETRY_PAYLOAD_LEN {
            return None;
        }

        Some(Self {
            uptime_ms: u32_at(payload, 0),
            frames_received: u32_at(payload, 4),
            frames_dropped: u32_at(payload, 8),
            link_errors: u32_at(payload, 12),
            commands_executed: u32_at(payload, 16),
            watchdog_fires: u32_at(payload, 20),
            crc_errors: u32_at(payload, 24),
        })
    }
}

fn u32_at(payload: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ])
}

pub(crate) fn request_telemetry<T>(
    transport: &mut T,
    rx: &mut Vec<u8>,
    seq: u32,
    timeout_ms: u64,
) -> HidResult<HidTelemetrySnapshot>
where
    T: Read + Write + ?Sized,
{
    let mut frame = vec![0u8; MAX_FRAME_LEN];
    let len = encode_host_frame(seq, HOST_COMMAND_GET_TELEMETRY, &[], &mut frame)
        .map_err(|error| telemetry_encode_error(seq, error))?;
    frame.truncate(len);
    transport
        .write_all(&frame)
        .map_err(|_error| link_timeout("writing telemetry request", timeout_ms))?;
    transport
        .flush()
        .map_err(|_error| link_timeout("flushing telemetry request", timeout_ms))?;
    read_telemetry_response(transport, rx, seq, timeout_ms)
}

fn read_telemetry_response<T>(
    transport: &mut T,
    rx: &mut Vec<u8>,
    expected_seq: u32,
    timeout_ms: u64,
) -> HidResult<HidTelemetrySnapshot>
where
    T: Read + ?Sized,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(snapshot) = try_parse_telemetry_response(rx, expected_seq, timeout_ms)? {
            return Ok(snapshot);
        }

        if Instant::now() >= deadline {
            return Err(link_timeout("reading telemetry", timeout_ms));
        }

        let mut chunk = [0u8; READ_CHUNK_LEN];
        match transport.read(&mut chunk) {
            Ok(0) => {}
            Ok(count) => {
                if rx.len() + count > MAX_RX_BUFFER_LEN {
                    rx.clear();
                    return Err(link_timeout("reading telemetry", timeout_ms));
                }
                rx.extend_from_slice(&chunk[..count]);
            }
            Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                if timeout_ms == 0 {
                    return Err(link_timeout("reading telemetry", timeout_ms));
                }
            }
            Err(_error) => return Err(link_timeout("reading telemetry", timeout_ms)),
        }
    }
}

fn try_parse_telemetry_response(
    rx: &mut Vec<u8>,
    expected_seq: u32,
    timeout_ms: u64,
) -> HidResult<Option<HidTelemetrySnapshot>> {
    loop {
        match parse_device_frame_prefix(rx) {
            Ok((frame, consumed)) => {
                let snapshot = decode_telemetry_response(
                    expected_seq,
                    frame.seq,
                    frame.command,
                    frame.payload,
                )?;
                rx.drain(..consumed);
                return Ok(Some(snapshot));
            }
            Err(ParseError::NeedMore { .. }) => return Ok(None),
            Err(
                ParseError::BadMagic { .. }
                | ParseError::LenTooShort { .. }
                | ParseError::LenOverflow { .. },
            ) => {
                if rx.is_empty() {
                    return Ok(None);
                }
                rx.remove(0);
            }
            Err(ParseError::CrcInvalid { .. }) => {
                rx.clear();
                return Err(link_timeout("reading telemetry", timeout_ms));
            }
        }
    }
}

fn decode_telemetry_response(
    expected_seq: u32,
    frame_seq: u32,
    command: u8,
    payload: &[u8],
) -> HidResult<HidTelemetrySnapshot> {
    if frame_seq != expected_seq {
        return Err(HidError::CommandRejected {
            seq: frame_seq,
            command,
            reason: NAK_REASON_PAYLOAD_INVALID,
        });
    }

    if command == DEVICE_COMMAND_NAK {
        if payload.len() == 5 {
            let payload_seq = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            if payload_seq == frame_seq {
                return Err(HidError::CommandRejected {
                    seq: frame_seq,
                    command: HOST_COMMAND_GET_TELEMETRY,
                    reason: payload[4],
                });
            }
        }
        return Err(HidError::CommandRejected {
            seq: frame_seq,
            command,
            reason: NAK_REASON_PAYLOAD_INVALID,
        });
    }

    if command != DEVICE_COMMAND_TELEMETRY_RESP {
        return Err(HidError::CommandRejected {
            seq: frame_seq,
            command,
            reason: NAK_REASON_UNKNOWN_COMMAND,
        });
    }

    HidTelemetrySnapshot::from_payload(payload).ok_or(HidError::CommandRejected {
        seq: frame_seq,
        command,
        reason: NAK_REASON_PAYLOAD_INVALID,
    })
}

const fn telemetry_encode_error(seq: u32, error: EncodeError) -> HidError {
    let reason = match error {
        EncodeError::PayloadTooLarge => NAK_REASON_PAYLOAD_INVALID,
        EncodeError::OutputTooSmall { .. } => NAK_REASON_LEN_INVALID,
    };
    HidError::CommandRejected {
        seq,
        command: HOST_COMMAND_GET_TELEMETRY,
        reason,
    }
}

const fn link_timeout(operation: &'static str, timeout_ms: u64) -> HidError {
    HidError::LinkTimeout {
        operation,
        timeout_ms,
    }
}
