use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use rmcp::ErrorData;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use synapse_action::{
    ActionBackend, ActionError, ActionHandle, ArcLengthPath, EmitState, RecordedInput,
    RecordingBackend, StrokeError, StrokePlan, plan_timed_stroke,
};
use synapse_core::{
    Action, Backend, HumanizeParams, MouseButton, PathSpec, StrokeTiming, VelocityProfile,
    error_codes,
};

use crate::m1::mcp_error;

pub const MAX_STROKE_PATH_POINTS: usize = 4096;
pub const MAX_STROKE_SAMPLES: usize = 60_001;
const MAX_STROKE_DURATION_MS: f64 = 60_000.0;
const MODIFIER_RELEASE_SETTLE_MS: u64 = 200;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActStrokeParams {
    pub path: PathSpec,
    #[serde(default)]
    #[schemars(default)]
    pub button: Option<MouseButton>,
    #[serde(default = "default_stroke_velocity_profile")]
    #[schemars(default = "default_stroke_velocity_profile")]
    pub velocity_profile: VelocityProfile,
    pub duration_or_speed: StrokeTiming,
    #[serde(default)]
    #[schemars(default)]
    pub humanize: Option<HumanizeParams>,
    #[serde(default = "default_stroke_backend")]
    #[schemars(default = "default_stroke_backend")]
    pub backend: StrokeBackend,
    #[serde(default)]
    #[schemars(default)]
    pub modifiers: Vec<StrokeModifier>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StrokeBackend {
    Software,
    Hardware,
    Auto,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StrokeModifier {
    Ctrl,
    Shift,
    Alt,
    Super,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActStrokeResponse {
    pub ok: bool,
    pub path_kind: String,
    pub control_point_count: u32,
    pub button_used: Option<MouseButton>,
    pub velocity_profile_used: VelocityProfile,
    pub duration_or_speed_used: StrokeTiming,
    pub humanized: bool,
    pub point_stream_count: u32,
    pub path_length_px: f64,
    pub duration_ms: f64,
    pub modifiers_used: Vec<StrokeModifier>,
    pub backend_used: String,
    pub elapsed_ms: u32,
}

pub async fn act_stroke_with_handle(
    handle: ActionHandle,
    recording: Option<Arc<RecordingBackend>>,
    params: ActStrokeParams,
) -> Result<ActStrokeResponse, ErrorData> {
    let started = Instant::now();
    let backend = params.backend.to_backend();
    let plan = validate_and_plan(&params)?;
    let action = Action::MouseStroke {
        path: params.path.clone(),
        button: params.button,
        profile: params.velocity_profile,
        timing: params.duration_or_speed.clone(),
        humanize: params.humanize,
        backend,
    };
    let modifier_keys: Vec<_> = params
        .modifiers
        .iter()
        .map(|modifier| modifier.to_key())
        .collect();

    if let Some(recording) = recording {
        execute_recording(&recording, &modifier_keys, &action, backend)?;
    } else {
        execute_with_modifiers(&handle, &modifier_keys, action, backend).await?;
    }

    Ok(response(&params, &plan, started, backend))
}

impl StrokeBackend {
    const fn to_backend(self) -> Backend {
        match self {
            Self::Software => Backend::Software,
            Self::Hardware => Backend::Hardware,
            Self::Auto => Backend::Auto,
        }
    }
}

impl StrokeModifier {
    fn to_key(self) -> synapse_core::Key {
        let value = match self {
            Self::Ctrl => "ctrl",
            Self::Shift => "shift",
            Self::Alt => "alt",
            Self::Super => "super",
        };
        synapse_core::Key {
            code: synapse_core::KeyCode::Named {
                value: value.to_owned(),
            },
            use_scancode: false,
        }
    }
}

fn validate_and_plan(params: &ActStrokeParams) -> Result<StrokePlan, ErrorData> {
    validate_control_point_cap(&params.path)?;
    validate_duration_cap(params)?;
    let plan = plan_timed_stroke(
        &params.path,
        params.velocity_profile,
        &params.duration_or_speed,
        params.humanize,
    )
    .map_err(|error| stroke_error_to_mcp(&error))?;
    if plan.samples.len() > MAX_STROKE_SAMPLES {
        return Err(params_invalid(format!(
            "act_stroke planned point stream count {} exceeds max {MAX_STROKE_SAMPLES}",
            plan.samples.len()
        )));
    }
    Ok(plan)
}

fn validate_control_point_cap(path: &PathSpec) -> Result<(), ErrorData> {
    let count = control_point_count(path);
    if count > MAX_STROKE_PATH_POINTS {
        return Err(params_invalid(format!(
            "act_stroke path control point count {count} exceeds max {MAX_STROKE_PATH_POINTS}"
        )));
    }
    Ok(())
}

fn validate_duration_cap(params: &ActStrokeParams) -> Result<(), ErrorData> {
    let path_length_px = ArcLengthPath::new(&params.path)
        .map_err(|error| params_invalid(format!("act_stroke path invalid: {error}")))?
        .length();
    let duration_ms = match &params.duration_or_speed {
        StrokeTiming::DurationMs { duration_ms } => f64::from(*duration_ms),
        StrokeTiming::SpeedPxPerSec { px_per_sec } => {
            if !px_per_sec.is_finite() || *px_per_sec <= 0.0 {
                return Err(params_invalid(format!(
                    "act_stroke speed px_per_sec must be finite and greater than zero, got {px_per_sec}"
                )));
            }
            path_length_px / px_per_sec * 1000.0
        }
    };
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        return Err(params_invalid(format!(
            "act_stroke duration_ms must be finite and greater than zero, got {duration_ms}"
        )));
    }
    if duration_ms > MAX_STROKE_DURATION_MS {
        return Err(params_invalid(format!(
            "act_stroke planned duration_ms {duration_ms:.3} exceeds max {MAX_STROKE_DURATION_MS:.0}"
        )));
    }
    Ok(())
}

async fn execute_with_modifiers(
    handle: &ActionHandle,
    modifier_keys: &[synapse_core::Key],
    stroke_action: Action,
    backend: Backend,
) -> Result<(), ErrorData> {
    let mut pressed = Vec::with_capacity(modifier_keys.len());
    for key in modifier_keys {
        if let Err(error) = handle
            .execute(Action::KeyDown {
                key: key.clone(),
                backend,
            })
            .await
        {
            let _ = release_pressed_modifiers(handle, &pressed, backend).await;
            return Err(action_error_to_mcp(&error));
        }
        pressed.push(key.clone());
    }

    let stroke_result = handle.execute(stroke_action).await;
    if stroke_result.is_ok() && !pressed.is_empty() {
        tokio::time::sleep(Duration::from_millis(MODIFIER_RELEASE_SETTLE_MS)).await;
    }
    let release_result = release_pressed_modifiers(handle, &pressed, backend).await;

    if let Err(error) = stroke_result {
        return Err(action_error_to_mcp(&error));
    }
    if let Err(error) = release_result {
        return Err(action_error_to_mcp(&error));
    }
    Ok(())
}

async fn release_pressed_modifiers(
    handle: &ActionHandle,
    pressed: &[synapse_core::Key],
    backend: Backend,
) -> Result<(), ActionError> {
    let mut release_error = None;
    for key in pressed.iter().rev() {
        if let Err(error) = handle
            .execute(Action::KeyUp {
                key: key.clone(),
                backend,
            })
            .await
            && release_error.is_none()
        {
            release_error = Some(error);
        }
    }
    release_error.map_or(Ok(()), Err)
}

fn execute_recording(
    recording: &RecordingBackend,
    modifier_keys: &[synapse_core::Key],
    stroke_action: &Action,
    backend: Backend,
) -> Result<(), ErrorData> {
    let before_events = recording.events();
    let before_event_count = before_events.len();
    let mut emit_state = EmitState::new();
    for key in modifier_keys {
        recording
            .execute(
                &Action::KeyDown {
                    key: key.clone(),
                    backend,
                },
                &mut emit_state,
            )
            .map_err(|error| action_error_to_mcp(&error))?;
    }
    recording
        .execute(stroke_action, &mut emit_state)
        .map_err(|error| action_error_to_mcp(&error))?;
    for key in modifier_keys.iter().rev() {
        recording
            .execute(
                &Action::KeyUp {
                    key: key.clone(),
                    backend,
                },
                &mut emit_state,
            )
            .map_err(|error| action_error_to_mcp(&error))?;
    }
    let after_events = recording.events();
    let new_events = &after_events[before_event_count..];
    let event_sequence = event_sequence(new_events);
    tracing::info!(
        code = "M2_ACT_STROKE_RECORDING_READBACK",
        kind = "act_stroke",
        before_event_count,
        after_event_count = after_events.len(),
        new_event_count = new_events.len(),
        event_sequence,
        ?new_events,
        "readback=recording_backend tool=act_stroke after_events_readback"
    );
    Ok(())
}

fn response(
    params: &ActStrokeParams,
    plan: &StrokePlan,
    started: Instant,
    backend: Backend,
) -> ActStrokeResponse {
    ActStrokeResponse {
        ok: true,
        path_kind: path_kind(&params.path).to_owned(),
        control_point_count: u32::try_from(control_point_count(&params.path)).unwrap_or(u32::MAX),
        button_used: params.button,
        velocity_profile_used: params.velocity_profile,
        duration_or_speed_used: params.duration_or_speed.clone(),
        humanized: params.humanize.is_some(),
        point_stream_count: u32::try_from(plan.samples.len()).unwrap_or(u32::MAX),
        path_length_px: plan.path_length_px,
        duration_ms: plan.duration_ms,
        modifiers_used: params.modifiers.clone(),
        backend_used: backend_used_name(backend).to_owned(),
        elapsed_ms: u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX),
    }
}

fn event_sequence(events: &[RecordedInput]) -> String {
    events.iter().map(event_label).collect::<Vec<_>>().join(">")
}

fn event_label(event: &RecordedInput) -> String {
    match event {
        RecordedInput::KeyDown { key } => format!("key_down:{}", key_label(key)),
        RecordedInput::KeyUp { key } => format!("key_up:{}", key_label(key)),
        RecordedInput::MouseButtonDown { button } => format!("down:{}", button_label(*button)),
        RecordedInput::MouseButtonUp { button } => format!("up:{}", button_label(*button)),
        RecordedInput::MouseStrokePoint { elapsed_ms, point } => {
            format!(
                "stroke_point:{elapsed_ms:.3}:screen({},{})",
                point.x, point.y
            )
        }
        other => format!("{other:?}"),
    }
}

fn key_label(key: &synapse_core::Key) -> String {
    match &key.code {
        synapse_core::KeyCode::Named { value } => value.clone(),
        synapse_core::KeyCode::Symbol { value } => value.to_string(),
        synapse_core::KeyCode::HidCode { value } => format!("hid:{value}"),
    }
}

const fn button_label(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::X1 => "x1",
        MouseButton::X2 => "x2",
    }
}

fn control_point_count(path: &PathSpec) -> usize {
    match path {
        PathSpec::Line { .. } => 2,
        PathSpec::Arc { .. } | PathSpec::Circle { .. } => 1,
        PathSpec::CubicBezier { .. } => 4,
        PathSpec::Polyline { points, .. } => points.len(),
        PathSpec::CatmullRom { waypoints, .. } => waypoints.len(),
    }
}

fn path_kind(path: &PathSpec) -> &'static str {
    match path {
        PathSpec::Line { .. } => "line",
        PathSpec::Arc { .. } => "arc",
        PathSpec::Circle { .. } => "circle",
        PathSpec::CubicBezier { .. } => "cubic_bezier",
        PathSpec::Polyline { .. } => "polyline",
        PathSpec::CatmullRom { .. } => "catmull_rom",
    }
}

fn stroke_error_to_mcp(error: &StrokeError) -> ErrorData {
    params_invalid(format!("act_stroke params invalid: {error}"))
}

fn action_error_to_mcp(error: &ActionError) -> ErrorData {
    mcp_error(error.code(), error.to_string())
}

fn params_invalid(message: impl Into<String>) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, message)
}

const fn default_stroke_velocity_profile() -> VelocityProfile {
    VelocityProfile::Constant
}

const fn default_stroke_backend() -> StrokeBackend {
    StrokeBackend::Auto
}

const fn backend_used_name(backend: Backend) -> &'static str {
    match backend {
        Backend::Auto | Backend::Software => "software",
        Backend::Hardware => "hardware",
        Backend::Vigem => "vigem",
    }
}
