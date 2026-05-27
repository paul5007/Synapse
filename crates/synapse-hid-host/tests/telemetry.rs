use std::io::{self, ErrorKind, Read, Write};

use synapse_hid_host::{
    DEVICE_COMMAND_TELEMETRY_RESP, HOST_COMMAND_MOUSE_MOVE_REL, HidError, HidPipeline,
    HidTelemetrySnapshot, HostCommandRequest, MAX_FRAME_LEN, NAK_REASON_PAYLOAD_INVALID,
    TELEMETRY_PAYLOAD_LEN, encode_device_frame,
};
use synapse_test_utils::hid_loopback::MockPicoFirmware;

#[test]
fn telemetry_snapshot_parses_28_byte_payload_fields() {
    let mut payload = [0u8; TELEMETRY_PAYLOAD_LEN];
    for (index, value) in [1u32, 2, 3, 4, 5, 6, 7].iter().copied().enumerate() {
        payload[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }

    let snapshot = HidTelemetrySnapshot::from_payload(&payload)
        .unwrap_or_else(|| panic!("known-valid telemetry payload should parse"));

    assert_eq!(
        snapshot,
        HidTelemetrySnapshot {
            uptime_ms: 1,
            frames_received: 2,
            frames_dropped: 3,
            link_errors: 4,
            commands_executed: 5,
            watchdog_fires: 6,
            crc_errors: 7,
        }
    );
    assert!(HidTelemetrySnapshot::from_payload(&payload[..TELEMETRY_PAYLOAD_LEN - 1]).is_none());
}

#[test]
fn pipeline_reads_telemetry_after_ten_thousand_commands() {
    let mut firmware = MockPicoFirmware::new();
    let mut pipeline = HidPipeline::new();
    let commands =
        vec![HostCommandRequest::new(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]); 10_000];

    let seqs = match pipeline.send_commands(&mut firmware, &commands) {
        Ok(seqs) => seqs,
        Err(error) => panic!("ten thousand mock Pico commands should ACK: {error}"),
    };
    let before = firmware.telemetry();
    let snapshot = match pipeline.get_telemetry(&mut firmware) {
        Ok(snapshot) => snapshot,
        Err(error) => panic!("mock Pico telemetry request should pass: {error}"),
    };
    let after = firmware.telemetry();

    assert_eq!(seqs.len(), 10_000);
    assert_eq!(seqs.first().copied(), Some(1));
    assert_eq!(seqs.last().copied(), Some(10_000));
    assert_eq!(before.frames_received, 10_000);
    assert_eq!(before.commands_executed, 10_000);
    assert_eq!(snapshot.frames_received, 10_001);
    assert_eq!(snapshot.commands_executed, 10_000);
    assert_eq!(snapshot.link_errors, 0);
    assert_eq!(snapshot.crc_errors, 0);
    assert_eq!(after.frames_received, 10_001);
    assert_eq!(after.commands_executed, 10_001);
    assert_eq!(pipeline.next_sequence(), 10_002);
}

#[test]
fn telemetry_request_requires_empty_pipeline_window() {
    let mut firmware = MockPicoFirmware::new();
    let mut pipeline = HidPipeline::new();
    let seq = match pipeline.try_send_command(
        &mut firmware,
        HOST_COMMAND_MOUSE_MOVE_REL,
        &[1, 0, 2, 0],
    ) {
        Ok(seq) => seq,
        Err(error) => panic!("mock Pico command should enqueue before ACK drain: {error}"),
    };

    let error = match pipeline.get_telemetry(&mut firmware) {
        Ok(snapshot) => panic!("in-flight command should block telemetry, got {snapshot:?}"),
        Err(error) => error,
    };

    assert_eq!(seq, 1);
    assert_eq!(
        error,
        HidError::QueueFull {
            outstanding: 1,
            capacity: 16,
        }
    );
}

#[test]
fn malformed_telemetry_payload_is_rejected() {
    let mut payload = [0u8; 4];
    payload.copy_from_slice(&123u32.to_le_bytes());
    let response = telemetry_frame(1, &payload);
    let mut transport = ScriptedTransport::new(response);
    let mut pipeline = HidPipeline::new();

    let error = match pipeline.get_telemetry(&mut transport) {
        Ok(snapshot) => panic!("malformed telemetry payload should fail, got {snapshot:?}"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        HidError::CommandRejected {
            seq: 1,
            command: DEVICE_COMMAND_TELEMETRY_RESP,
            reason: NAK_REASON_PAYLOAD_INVALID,
        }
    );
    assert_eq!(transport.writes, 1);
}

fn telemetry_frame(seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut frame = [0u8; MAX_FRAME_LEN];
    let len = match encode_device_frame(seq, DEVICE_COMMAND_TELEMETRY_RESP, payload, &mut frame) {
        Ok(len) => len,
        Err(error) => panic!("telemetry frame should encode: {error:?}"),
    };
    frame[..len].to_vec()
}

struct ScriptedTransport {
    read_data: Vec<u8>,
    read_offset: usize,
    writes: usize,
}

impl ScriptedTransport {
    const fn new(read_data: Vec<u8>) -> Self {
        Self {
            read_data,
            read_offset: 0,
            writes: 0,
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
        self.writes += 1;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
