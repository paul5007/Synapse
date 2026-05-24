use rmcp::ErrorData;
use synapse_action::{ActionBackend, EmitState, RecordedInput, RecordingBackend};
use synapse_core::{Action, Key, KeyCode};

use super::action_error_to_mcp;

pub(in crate::m2::press) fn execute_recording(
    recording: &RecordingBackend,
    action: &Action,
) -> Result<(), ErrorData> {
    let before_events = recording.events();
    let before_event_count = before_events.len();
    let mut emit_state = EmitState::new();
    recording
        .execute(action, &mut emit_state)
        .map_err(|error| action_error_to_mcp(&error))?;
    let after_events = recording.events();
    let new_events = &after_events[before_event_count..];
    let event_sequence = event_sequence(new_events);
    tracing::info!(
        code = "M2_ACT_PRESS_RECORDING_READBACK",
        kind = "act_press",
        before_event_count,
        after_event_count = after_events.len(),
        new_event_count = new_events.len(),
        event_sequence,
        ?new_events,
        "source_of_truth=recording_backend tool=act_press after_events_readback"
    );
    Ok(())
}

pub(in crate::m2::press) fn event_sequence(events: &[RecordedInput]) -> String {
    events.iter().map(event_label).collect::<Vec<_>>().join(">")
}

fn event_label(event: &RecordedInput) -> String {
    match event {
        RecordedInput::KeyDown { key } => format!("down:{}", key_label(key)),
        RecordedInput::KeyUp { key } => format!("up:{}", key_label(key)),
        RecordedInput::DelayMs { ms } => format!("delay:{ms}"),
        other => format!("{other:?}"),
    }
}

fn key_label(key: &Key) -> String {
    match &key.code {
        KeyCode::Named { value } => value.clone(),
        KeyCode::Symbol { value } => value.to_string(),
        KeyCode::HidCode { value } => format!("hid:{value}"),
    }
}
