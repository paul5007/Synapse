use std::{borrow::Cow, path::Path};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::Deserialize;
use synapse_core::{
    Action, Backend, ButtonAction, ComboInput, ComboStep, DataPredicate, EventFilter, EventSource,
    ReflexButtonTarget, ReflexLifetime, ReflexThen,
};
use synapse_reflex::ReflexError;

use crate::{
    m2::{ActPressParams, ActTypeParams, action_from_press_params, action_from_type_params},
    m3::a11y_events,
};

pub(super) fn reflex_kind_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "enum": ["aim_track", "hold_move", "hold_button", "combo", "on_event", "path_follow"]
    })
}

pub(super) const fn default_reflex_priority() -> u32 {
    synapse_reflex::DEFAULT_REFLEX_PRIORITY
}

pub(super) const fn default_lifetime() -> ReflexLifetime {
    ReflexLifetime::UntilCancelled
}

pub(super) const fn default_backend() -> Backend {
    Backend::Auto
}

pub(super) const FILE_JSONL_TAIL_EVENT_KIND: &str = "file_jsonl_tail";
pub(super) const AUDIT_READBACK_ACTION: &str = "audit/readback";
const DEFAULT_FILE_JSONL_TAIL_POLL_INTERVAL_MS: u64 = 1000;
const MIN_FILE_JSONL_TAIL_POLL_INTERVAL_MS: u64 = 50;
const MAX_FILE_JSONL_TAIL_POLL_INTERVAL_MS: u64 = 600_000;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ReflexWhenParam {
    FileJsonlTail(FileJsonlTailWhen),
    Filter(EventFilter),
    WindowEvent(WindowEventWhen),
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileJsonlTailWhen {
    pub kind: FileJsonlTailKind,
    pub host: String,
    pub path: String,
    pub predicate: FileJsonlTailPredicate,
    #[schemars(range(min = 1))]
    pub min_lines: u64,
    #[serde(default = "default_file_jsonl_tail_poll_interval_ms")]
    #[schemars(
        default = "default_file_jsonl_tail_poll_interval_ms",
        range(min = 50, max = 600000)
    )]
    pub poll_interval_ms: u64,
}

#[derive(Copy, Clone, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileJsonlTailKind {
    FileJsonlTail,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileJsonlTailPredicate {
    pub json_path: String,
    pub equals: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ValidatedFileJsonlTailWhen {
    pub host: String,
    pub path: String,
    pub json_path: String,
    pub json_pointer: String,
    pub event_data_pointer: String,
    pub equals: serde_json::Value,
    pub min_lines: u64,
    pub poll_interval_ms: u64,
    pub local_host: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowEventWhen {
    pub kind: String,
    #[serde(default, rename = "match")]
    pub match_clause: WindowEventMatch,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowEventMatch {
    #[serde(default)]
    pub window_title_regex: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ReflexThenParam {
    Core(ReflexThen),
    AuditAction(ReflexThenAuditActionParam),
    Steps { steps: Vec<ReflexThenStep> },
}

#[derive(JsonSchema)]
#[serde(untagged)]
#[allow(
    dead_code,
    reason = "schema-only wrapper; runtime uses ReflexThenParam"
)]
enum ReflexThenPublicSchema {
    Core(ReflexThen),
    AuditAction(ReflexThenAuditActionParam),
    Steps(ReflexThenStepsPublicSchema),
}

#[derive(JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "schema-only wrapper; runtime uses ReflexThenParam"
)]
struct ReflexThenStepsPublicSchema {
    steps: Vec<ReflexThenStep>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReflexThenAuditActionParam {
    #[schemars(schema_with = "audit_readback_action_schema")]
    pub action: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReflexThenStep {
    pub action: String,
    #[serde(default = "empty_params")]
    pub params: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ReflexComboStepParam {
    Core(ComboStep),
    Tool(ReflexTimedThenStep),
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReflexTimedThenStep {
    #[serde(default)]
    pub at_ms: u32,
    pub action: String,
    #[serde(default = "empty_params")]
    pub params: serde_json::Value,
}

impl JsonSchema for ReflexThenParam {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ReflexThenParam")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        ReflexThenPublicSchema::json_schema(generator)
    }
}

impl JsonSchema for ReflexComboStepParam {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ReflexComboStepParam")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        ReflexTimedThenStep::json_schema(generator)
    }
}

pub(super) fn required_then(
    then: Option<ReflexThenParam>,
    kind: &'static str,
) -> Result<ReflexThenParam, ReflexError> {
    then.ok_or_else(|| ReflexError::ParamsInvalid {
        detail: format!("{kind} reflex requires then"),
    })
}

pub(super) fn actions_from_then(
    then: ReflexThenParam,
    backend: Backend,
) -> Result<Vec<Action>, ReflexError> {
    let mut actions = match then {
        ReflexThenParam::Core(ReflexThen::Action { action }) => vec![action],
        ReflexThenParam::Core(ReflexThen::Actions { actions }) => actions,
        ReflexThenParam::Core(ReflexThen::Combo {
            steps,
            backend: combo_backend,
        }) => vec![Action::Combo {
            steps,
            backend: combo_backend,
        }],
        ReflexThenParam::AuditAction(action) => actions_from_audit_action(action)?,
        ReflexThenParam::Steps { steps } => actions_from_demo_steps(steps)?,
    };
    for action in &mut actions {
        apply_backend_default(action, backend);
    }
    Ok(actions)
}

pub(super) const fn button_down_action(button: &ReflexButtonTarget, backend: Backend) -> Action {
    match *button {
        ReflexButtonTarget::Mouse { button } => Action::MouseButton {
            button,
            action: ButtonAction::Down,
            hold_ms: 0,
            backend,
        },
        ReflexButtonTarget::Pad { pad, button } => Action::PadButton {
            pad,
            button,
            action: ButtonAction::Down,
            hold_ms: 0,
        },
    }
}

pub(super) fn combo_steps_from_params(
    steps: Option<Vec<ReflexComboStepParam>>,
    then: Option<ReflexThenParam>,
) -> Result<Vec<ComboStep>, ReflexError> {
    if let Some(steps) = steps {
        if steps.is_empty() {
            return Err(ReflexError::ParamsInvalid {
                detail: "combo steps must contain at least one step".to_owned(),
            });
        }
        return steps
            .into_iter()
            .enumerate()
            .map(|(index, step)| combo_step_from_param(index, step))
            .collect();
    }

    match then {
        Some(ReflexThenParam::Core(ReflexThen::Combo { steps, .. })) if !steps.is_empty() => {
            Ok(steps)
        }
        Some(ReflexThenParam::Core(ReflexThen::Combo { .. })) => Err(ReflexError::ParamsInvalid {
            detail: "combo steps must contain at least one step".to_owned(),
        }),
        Some(ReflexThenParam::Steps { steps }) => steps
            .into_iter()
            .enumerate()
            .map(|(index, step)| timed_demo_step_to_combo_step(index, 0, step))
            .collect(),
        Some(ReflexThenParam::AuditAction(_)) => Err(ReflexError::ParamsInvalid {
            detail: "combo reflex cannot use then.action=audit/readback".to_owned(),
        }),
        Some(ReflexThenParam::Core(_)) | None => Err(ReflexError::ParamsInvalid {
            detail: "combo reflex requires steps or then.kind=combo".to_owned(),
        }),
    }
}

impl ReflexWhenParam {
    pub(super) fn requires_a11y_event_bridge(&self) -> bool {
        match self {
            Self::FileJsonlTail(_) => false,
            Self::Filter(filter) => a11y_events::event_filter_requires_a11y_bridge(filter),
            Self::WindowEvent(_) => true,
        }
    }

    pub(super) fn into_event_filter(self) -> Result<EventFilter, ReflexError> {
        match self {
            Self::FileJsonlTail(when) => when.into_event_filter(),
            Self::Filter(filter) => Ok(filter),
            Self::WindowEvent(when) => when.into_event_filter(),
        }
    }

    pub(super) fn file_jsonl_tail(&self) -> Option<&FileJsonlTailWhen> {
        match self {
            Self::FileJsonlTail(when) => Some(when),
            Self::Filter(_) | Self::WindowEvent(_) => None,
        }
    }
}

impl FileJsonlTailWhen {
    pub(super) fn validate(&self) -> Result<ValidatedFileJsonlTailWhen, ReflexError> {
        match self.kind {
            FileJsonlTailKind::FileJsonlTail => {}
        }
        let host = validate_file_jsonl_tail_host(&self.host)?;
        let local_host = is_local_file_jsonl_tail_host(&host);
        let path = validate_file_jsonl_tail_path(&self.path, local_host)?;
        if self.min_lines == 0 {
            return Err(ReflexError::ParamsInvalid {
                detail: "file_jsonl_tail min_lines must be at least 1".to_owned(),
            });
        }
        let poll_interval_ms = self.poll_interval_ms;
        if !(MIN_FILE_JSONL_TAIL_POLL_INTERVAL_MS..=MAX_FILE_JSONL_TAIL_POLL_INTERVAL_MS)
            .contains(&poll_interval_ms)
        {
            return Err(ReflexError::ParamsInvalid {
                detail: format!(
                    "file_jsonl_tail poll_interval_ms must be between {MIN_FILE_JSONL_TAIL_POLL_INTERVAL_MS} and {MAX_FILE_JSONL_TAIL_POLL_INTERVAL_MS}"
                ),
            });
        }
        let json_path = self.predicate.json_path.trim().to_owned();
        let json_pointer = json_path_to_pointer(&json_path)?;
        let event_data_pointer = format!("/last_json{json_pointer}");
        Ok(ValidatedFileJsonlTailWhen {
            host,
            path,
            json_path,
            json_pointer,
            event_data_pointer,
            equals: self.predicate.equals.clone(),
            min_lines: self.min_lines,
            poll_interval_ms,
            local_host,
        })
    }

    fn into_event_filter(self) -> Result<EventFilter, ReflexError> {
        let validated = self.validate()?;
        Ok(EventFilter::And {
            args: vec![
                EventFilter::Source {
                    source: EventSource::Filesystem,
                },
                EventFilter::Kind {
                    kind: FILE_JSONL_TAIL_EVENT_KIND.to_owned(),
                },
                EventFilter::Data {
                    path: "/host".to_owned(),
                    predicate: DataPredicate::Eq {
                        value: serde_json::json!(validated.host),
                    },
                },
                EventFilter::Data {
                    path: "/path".to_owned(),
                    predicate: DataPredicate::Eq {
                        value: serde_json::json!(validated.path),
                    },
                },
                EventFilter::Data {
                    path: "/line_count".to_owned(),
                    predicate: DataPredicate::Ge {
                        value: serde_json::json!(validated.min_lines),
                    },
                },
                EventFilter::Data {
                    path: validated.event_data_pointer,
                    predicate: DataPredicate::Eq {
                        value: validated.equals,
                    },
                },
            ],
        })
    }
}

impl WindowEventWhen {
    fn into_event_filter(self) -> Result<EventFilter, ReflexError> {
        let kind = normalize_window_event_kind(&self.kind)?;
        let mut filters = vec![EventFilter::Kind { kind }];
        if let Some(pattern) = self.match_clause.window_title_regex {
            validate_regex(&pattern)?;
            filters.push(EventFilter::Data {
                path: "/window_title".to_owned(),
                predicate: DataPredicate::Regex { pattern },
            });
        }
        if filters.len() == 1 {
            Ok(filters.remove(0))
        } else {
            Ok(EventFilter::And { args: filters })
        }
    }
}

fn combo_step_from_param(
    index: usize,
    step: ReflexComboStepParam,
) -> Result<ComboStep, ReflexError> {
    match step {
        ReflexComboStepParam::Core(step) => Ok(step),
        ReflexComboStepParam::Tool(step) => {
            let at_ms = step.at_ms;
            let demo_step = ReflexThenStep {
                action: step.action,
                params: step.params,
            };
            timed_demo_step_to_combo_step(index, at_ms, demo_step)
        }
    }
}

fn timed_demo_step_to_combo_step(
    index: usize,
    at_ms: u32,
    step: ReflexThenStep,
) -> Result<ComboStep, ReflexError> {
    let action = action_from_demo_step(index, step)?;
    match action {
        Action::KeyPress { key, hold_ms, .. } => {
            let hold_ms = u16::try_from(hold_ms).map_err(|_err| ReflexError::ParamsInvalid {
                detail: format!("combo steps[{index}] hold_ms exceeds u16::MAX"),
            })?;
            Ok(ComboStep {
                at_ms,
                input: ComboInput::KeyPress { key, hold_ms },
            })
        }
        Action::MouseButton { button, action, .. } => Ok(ComboStep {
            at_ms,
            input: ComboInput::MouseButton { button, action },
        }),
        Action::MouseMoveRelative { dx, dy, .. } => Ok(ComboStep {
            at_ms,
            input: ComboInput::MouseMoveRel { dx, dy },
        }),
        other => Err(ReflexError::ParamsInvalid {
            detail: format!(
                "combo steps[{index}] action {other:?} cannot be used as one timed combo input"
            ),
        }),
    }
}

fn normalize_window_event_kind(raw: &str) -> Result<String, ReflexError> {
    let kind = raw.trim().replace('_', "-").to_ascii_lowercase();
    if kind.is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "window event kind must not be empty".to_owned(),
        });
    }
    Ok(kind)
}

fn validate_file_jsonl_tail_host(raw: &str) -> Result<String, ReflexError> {
    let host = raw.trim();
    if host.is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail host must not be empty".to_owned(),
        });
    }
    if host.chars().any(|ch| {
        ch.is_ascii_control()
            || ch.is_ascii_whitespace()
            || matches!(
                ch,
                '\'' | '"' | '`' | '$' | ';' | '|' | '&' | '<' | '>' | '\\' | '/'
            )
    }) {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail host contains unsupported characters".to_owned(),
        });
    }
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail host must contain only letters, digits, '.', '_' or '-'"
                .to_owned(),
        });
    }
    Ok(host.to_owned())
}

fn validate_file_jsonl_tail_path(raw: &str, local_host: bool) -> Result<String, ReflexError> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail path must not be empty".to_owned(),
        });
    }
    if path
        .chars()
        .any(|ch| ch == '\0' || ch == '\n' || ch == '\r' || ch.is_ascii_control())
    {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail path contains unsupported control characters".to_owned(),
        });
    }
    if local_host {
        if path.starts_with('/') || Path::new(path).is_absolute() || is_windows_absolute_path(path)
        {
            return Ok(path.to_owned());
        }
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail local path must be absolute".to_owned(),
        });
    }
    if path.starts_with('/') {
        return Ok(path.to_owned());
    }
    Err(ReflexError::ParamsInvalid {
        detail: "file_jsonl_tail remote path must be POSIX absolute".to_owned(),
    })
}

fn is_windows_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || path.starts_with("\\\\")
}

pub(super) fn is_local_file_jsonl_tail_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "127.0.0.1") {
        return true;
    }
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .is_some_and(|name| name.eq_ignore_ascii_case(&host))
}

fn json_path_to_pointer(json_path: &str) -> Result<String, ReflexError> {
    if json_path == "$" {
        return Ok(String::new());
    }
    let Some(path) = json_path.strip_prefix("$.") else {
        return Err(ReflexError::ParamsInvalid {
            detail:
                "file_jsonl_tail predicate.json_path must be '$' or a simple '$.field[.field]' path"
                    .to_owned(),
        });
    };
    if path.is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "file_jsonl_tail predicate.json_path must not end at '$.'".to_owned(),
        });
    }
    let mut pointer = String::new();
    for segment in path.split('.') {
        if segment.is_empty() {
            return Err(ReflexError::ParamsInvalid {
                detail: "file_jsonl_tail predicate.json_path has an empty segment".to_owned(),
            });
        }
        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(ReflexError::ParamsInvalid {
                detail: "file_jsonl_tail predicate.json_path supports only simple object fields"
                    .to_owned(),
            });
        }
        pointer.push('/');
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
    }
    Ok(pointer)
}

fn actions_from_audit_action(
    action: ReflexThenAuditActionParam,
) -> Result<Vec<Action>, ReflexError> {
    if action.action == AUDIT_READBACK_ACTION {
        return Ok(Vec::new());
    }
    Err(ReflexError::ParamsInvalid {
        detail: format!(
            "then.action {:?} is unsupported; supported actions: {AUDIT_READBACK_ACTION}",
            action.action
        ),
    })
}

fn validate_regex(pattern: &str) -> Result<(), ReflexError> {
    if pattern.trim().is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "window_title_regex must not be empty".to_owned(),
        });
    }
    regex::Regex::new(pattern).map_err(|error| ReflexError::ParamsInvalid {
        detail: format!("window_title_regex is invalid: {error}"),
    })?;
    Ok(())
}

fn actions_from_demo_steps(steps: Vec<ReflexThenStep>) -> Result<Vec<Action>, ReflexError> {
    if steps.is_empty() {
        return Err(ReflexError::ParamsInvalid {
            detail: "then.steps must contain at least one action".to_owned(),
        });
    }
    steps
        .into_iter()
        .enumerate()
        .map(|(index, step)| action_from_demo_step(index, step))
        .collect()
}

fn action_from_demo_step(index: usize, step: ReflexThenStep) -> Result<Action, ReflexError> {
    match step.action.trim() {
        "act_type" => {
            let params = serde_json::from_value::<ActTypeParams>(step.params).map_err(|error| {
                ReflexError::ParamsInvalid {
                    detail: format!("then.steps[{index}].act_type params invalid: {error}"),
                }
            })?;
            action_from_type_params(&params).map_err(|error| ReflexError::ParamsInvalid {
                detail: format!("then.steps[{index}].act_type params invalid: {error}"),
            })
        }
        "act_press" => {
            let params =
                serde_json::from_value::<ActPressParams>(step.params).map_err(|error| {
                    ReflexError::ParamsInvalid {
                        detail: format!("then.steps[{index}].act_press params invalid: {error}"),
                    }
                })?;
            action_from_press_params(&params).map_err(|error| ReflexError::ParamsInvalid {
                detail: format!("then.steps[{index}].act_press params invalid: {error}"),
            })
        }
        other => Err(ReflexError::ParamsInvalid {
            detail: format!(
                "then.steps[{index}].action {other:?} is unsupported; supported actions: act_type, act_press"
            ),
        }),
    }
}

fn empty_params() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

const fn default_file_jsonl_tail_poll_interval_ms() -> u64 {
    DEFAULT_FILE_JSONL_TAIL_POLL_INTERVAL_MS
}

fn audit_readback_action_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "enum": [AUDIT_READBACK_ACTION]
    })
}

fn apply_backend_default(action: &mut Action, fallback: Backend) {
    if fallback == Backend::Auto {
        return;
    }
    match action {
        Action::KeyPress { backend, .. }
        | Action::KeyDown { backend, .. }
        | Action::KeyUp { backend, .. }
        | Action::KeyChord { backend, .. }
        | Action::TypeText { backend, .. }
        | Action::MouseMove { backend, .. }
        | Action::MouseMoveRelative { backend, .. }
        | Action::MouseButton { backend, .. }
        | Action::MouseDrag { backend, .. }
        | Action::MouseStroke { backend, .. }
        | Action::MouseScroll { backend, .. }
        | Action::AimAt { backend, .. }
        | Action::Combo { backend, .. }
            if *backend == Backend::Auto =>
        {
            *backend = fallback;
        }
        Action::KeyPress { .. }
        | Action::KeyDown { .. }
        | Action::KeyUp { .. }
        | Action::KeyChord { .. }
        | Action::TypeText { .. }
        | Action::MouseMove { .. }
        | Action::MouseMoveRelative { .. }
        | Action::MouseButton { .. }
        | Action::MouseDrag { .. }
        | Action::MouseStroke { .. }
        | Action::MouseScroll { .. }
        | Action::AimAt { .. }
        | Action::Combo { .. }
        | Action::PadButton { .. }
        | Action::PadStick { .. }
        | Action::PadTrigger { .. }
        | Action::PadReport { .. }
        | Action::ReleaseAll => {}
    }
}
