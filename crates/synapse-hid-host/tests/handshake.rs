use std::io::{self, ErrorKind, Read, Write};
use std::time::Duration;

use synapse_core::error_codes;
use synapse_core::{
    EXPECTED_FW_MAJOR, SYNAPSE_PICO_HID_FW_MINOR, SYNAPSE_PICO_HID_FW_PATCH,
    SYNAPSE_PICO_HID_USB_PID, SYNAPSE_PICO_HID_USB_VID,
};
use synapse_hid_host::handshake::{
    HandshakeError, IDENTIFY_RESP_LEN, expected_version_triplet,
    parse_and_validate_identify_response, parse_identify_response, perform_identify_handshake,
};
use synapse_hid_host::protocol::{
    DEVICE_COMMAND_IDENTIFY_RESP, HOST_COMMAND_IDENTIFY, MAX_FRAME_LEN, encode_device_frame,
};
use synapse_hid_host::{HOST_MAGIC, HidError};

#[test]
fn parse_identify_response_reads_current_wire_layout() {
    let payload = identify_payload(EXPECTED_FW_MAJOR);
    let identity = match parse_and_validate_identify_response(&payload) {
        Ok(identity) => identity,
        Err(error) => panic!("matching identify payload should parse: {error}"),
    };

    assert_eq!(identity.fw_major, EXPECTED_FW_MAJOR);
    assert_eq!(identity.fw_minor, SYNAPSE_PICO_HID_FW_MINOR);
    assert_eq!(identity.fw_patch, SYNAPSE_PICO_HID_FW_PATCH);
    assert_eq!(&identity.build_hash, b"TESTHASH");
    assert_eq!(identity.vid, SYNAPSE_PICO_HID_USB_VID);
    assert_eq!(identity.pid, SYNAPSE_PICO_HID_USB_PID);
    assert_eq!(identity.capabilities, 0x1F);
    assert_eq!(
        expected_version_triplet(),
        [
            EXPECTED_FW_MAJOR,
            SYNAPSE_PICO_HID_FW_MINOR,
            SYNAPSE_PICO_HID_FW_PATCH
        ]
    );
}

#[test]
fn parse_identify_response_rejects_major_version_mismatch() {
    let mismatched_major = EXPECTED_FW_MAJOR.wrapping_add(1);
    let payload = identify_payload(mismatched_major);
    let error = match parse_and_validate_identify_response(&payload) {
        Ok(identity) => panic!("mismatched identify payload should fail: {identity:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HandshakeError::FirmwareVersionMismatch {
            expected: EXPECTED_FW_MAJOR,
            actual: mismatched_major,
        }
    );
    assert_eq!(error.code(), error_codes::HID_FIRMWARE_VERSION_MISMATCH);
}

#[test]
fn parse_identify_response_rejects_malformed_lengths() {
    let short = [0u8; IDENTIFY_RESP_LEN - 1];
    let short_error = match parse_identify_response(&short) {
        Ok(identity) => panic!("short identify payload should fail: {identity:?}"),
        Err(error) => error,
    };
    assert_eq!(
        short_error,
        HandshakeError::InvalidIdentifyPayloadLength {
            actual: IDENTIFY_RESP_LEN - 1,
            expected: IDENTIFY_RESP_LEN,
        }
    );
    assert_eq!(
        short_error.code(),
        error_codes::HID_PROTOCOL_HANDSHAKE_FAILED
    );

    let long = [0u8; IDENTIFY_RESP_LEN + 1];
    let long_error = match parse_identify_response(&long) {
        Ok(identity) => panic!("long identify payload should fail: {identity:?}"),
        Err(error) => error,
    };
    assert_eq!(
        long_error,
        HandshakeError::InvalidIdentifyPayloadLength {
            actual: IDENTIFY_RESP_LEN + 1,
            expected: IDENTIFY_RESP_LEN,
        }
    );
    assert_eq!(
        long_error.code(),
        error_codes::HID_PROTOCOL_HANDSHAKE_FAILED
    );
}

#[test]
fn perform_identify_handshake_writes_identify_and_validates_response() {
    let response = identify_response_frame(EXPECTED_FW_MAJOR, DEVICE_COMMAND_IDENTIFY_RESP);
    let mut transport = ScriptedTransport::new(response);

    let identity = match perform_identify_handshake(&mut transport, Duration::from_millis(200)) {
        Ok(identity) => identity,
        Err(error) => panic!("matching identify response should pass: {error}"),
    };

    assert_eq!(identity.fw_major, EXPECTED_FW_MAJOR);
    assert_eq!(transport.written[0], HOST_MAGIC);
    assert_eq!(transport.written[7], HOST_COMMAND_IDENTIFY);
    assert_eq!(
        u32::from_le_bytes([
            transport.written[3],
            transport.written[4],
            transport.written[5],
            transport.written[6],
        ]),
        0
    );
}

#[test]
fn perform_identify_handshake_rejects_mismatched_major() {
    let mismatched_major = EXPECTED_FW_MAJOR.wrapping_add(1);
    let response = identify_response_frame(mismatched_major, DEVICE_COMMAND_IDENTIFY_RESP);
    let mut transport = ScriptedTransport::new(response);

    let error = match perform_identify_handshake(&mut transport, Duration::from_millis(200)) {
        Ok(identity) => panic!("mismatched identify response should fail: {identity:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HidError::FirmwareVersionMismatch {
            expected: EXPECTED_FW_MAJOR,
            actual: mismatched_major,
        }
    );
    assert_eq!(error.code(), error_codes::HID_FIRMWARE_VERSION_MISMATCH);
}

#[test]
fn perform_identify_handshake_rejects_missing_or_wrong_response() {
    let mut silent = ScriptedTransport::new(Vec::new());
    let timeout_error = match perform_identify_handshake(&mut silent, Duration::ZERO) {
        Ok(identity) => panic!("silent transport should fail: {identity:?}"),
        Err(error) => error,
    };
    assert_eq!(
        timeout_error.code(),
        error_codes::HID_PROTOCOL_HANDSHAKE_FAILED
    );

    let wrong_response = identify_response_frame(EXPECTED_FW_MAJOR, 0x84);
    let mut wrong_transport = ScriptedTransport::new(wrong_response);
    let wrong_error =
        match perform_identify_handshake(&mut wrong_transport, Duration::from_millis(200)) {
            Ok(identity) => panic!("wrong response command should fail: {identity:?}"),
            Err(error) => error,
        };
    assert_eq!(
        wrong_error.code(),
        error_codes::HID_PROTOCOL_HANDSHAKE_FAILED
    );
}

fn identify_payload(major: u8) -> [u8; IDENTIFY_RESP_LEN] {
    let mut payload = [0u8; IDENTIFY_RESP_LEN];
    payload[0] = major;
    payload[1] = SYNAPSE_PICO_HID_FW_MINOR;
    payload[2] = SYNAPSE_PICO_HID_FW_PATCH;
    payload[4..12].copy_from_slice(b"TESTHASH");
    payload[12..14].copy_from_slice(&SYNAPSE_PICO_HID_USB_VID.to_le_bytes());
    payload[14..16].copy_from_slice(&SYNAPSE_PICO_HID_USB_PID.to_le_bytes());
    payload[16..20].copy_from_slice(&0x1Fu32.to_le_bytes());
    payload
}

fn identify_response_frame(major: u8, command: u8) -> Vec<u8> {
    let payload = identify_payload(major);
    let mut frame = [0u8; MAX_FRAME_LEN];
    let len = match encode_device_frame(0, command, &payload, &mut frame) {
        Ok(len) => len,
        Err(error) => panic!("test identify response should encode: {error:?}"),
    };
    frame[..len].to_vec()
}

struct ScriptedTransport {
    read_data: Vec<u8>,
    read_offset: usize,
    written: Vec<u8>,
}

impl ScriptedTransport {
    const fn new(read_data: Vec<u8>) -> Self {
        Self {
            read_data,
            read_offset: 0,
            written: Vec::new(),
        }
    }
}

impl Read for ScriptedTransport {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.read_offset >= self.read_data.len() {
            return Err(io::Error::new(ErrorKind::TimedOut, "scripted timeout"));
        }

        let remaining = self.read_data.len() - self.read_offset;
        let count = remaining.min(buffer.len());
        buffer[..count]
            .copy_from_slice(&self.read_data[self.read_offset..self.read_offset + count]);
        self.read_offset += count;
        Ok(count)
    }
}

impl Write for ScriptedTransport {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
