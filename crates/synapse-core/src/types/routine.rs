use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Envelope schema version for [`RoutineRecord`] rows.
pub const ROUTINE_RECORD_VERSION: u32 = 1;

/// Envelope schema version for [`RoutineStateRecord`] rows.
pub const ROUTINE_STATE_RECORD_VERSION: u32 = 1;

/// Newest-last cap on [`RoutineStateRecord::transitions`]. Overflow drops the
/// oldest entry and increments `transitions_truncated` so the loss is visible.
pub const ROUTINE_STATE_MAX_TRANSITIONS: usize = 64;

/// Newest-last cap on [`RoutineStateRecord::confidence_history`].
pub const ROUTINE_STATE_MAX_CONFIDENCE_POINTS: usize = 180;

/// Identity granularity a routine was mined at (#848).
///
/// `App` patterns generalize across documents ("opens Excel every morning");
/// `AppDocument` patterns are document-specific ("opens report.xlsx every
/// morning"). Both passes run; closed-pattern suppression removes an `App`
/// routine that carries no information beyond an `AppDocument` one.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutineGranularity {
    App,
    AppDocument,
}

/// Day-of-week classification of a routine's schedule signature (#848).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutineDowClass {
    /// Seen on at least six distinct weekdays.
    Daily,
    /// Seen on two or more distinct weekdays, Monday–Friday only.
    Weekdays,
    /// Seen on Saturday/Sunday only.
    Weekend,
    /// Explicit weekday list (0 = Monday … 6 = Sunday), sorted ascending.
    Days { days: Vec<u8> },
}

/// One ordered step of a routine's episode template.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineStep {
    /// Lowercased process executable name.
    pub app: String,
    /// Lowercased document identity (URL host for browser episodes,
    /// normalized window title otherwise). `None` for `App`-granularity
    /// steps and episodes without a document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<String>,
}

/// One occurrence of the routine, kept as inspectable support evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineEvidence {
    /// Local-midnight start of the day the occurrence happened on.
    pub day_start_ns: u64,
    /// Minute of that local day the first step started at.
    pub minute_of_day: u32,
    /// Stable episode ids (`ep1-…`) of the steps, in template order.
    pub episode_ids: Vec<String>,
}

/// One mined routine persisted in `CF_ROUTINES` (#848).
///
/// Routines are derived state: a pure, deterministic function of the
/// episode store and the mining config. Re-mining replaces all rows
/// atomically, so the store always reflects exactly one mining run.
/// `ts_ns` is the mining instant (the one engine input that varies between
/// runs); `routine_id` deliberately excludes it so re-mining the same
/// episodes reproduces the same ids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineRecord {
    pub record_version: u32,
    /// Mining instant (ns since epoch). `CF_ROUTINES` has no TTL; this is
    /// provenance, not a retention contract.
    pub ts_ns: u64,
    /// Stable deterministic id: `rt1-` + first 16 hex chars of SHA-256 over
    /// granularity, step keys, day-of-week class, and time-cluster ordinal.
    pub routine_id: String,
    pub granularity: RoutineGranularity,
    /// Ordered episode template (collapsed: consecutive identical
    /// identities merge into one step).
    pub steps: Vec<RoutineStep>,
    pub dow_class: RoutineDowClass,
    /// Circular mean start minute of the local day (0..1440).
    pub mean_minute_of_day: u32,
    /// Maximum circular deviation from the mean across occurrences.
    pub tolerance_minutes: u32,
    /// Human-readable schedule signature, e.g. `weekdays 08:45±20m`.
    pub schedule_label: String,
    /// Distinct local days the routine occurred on (the support count).
    pub support_days: u32,
    /// Total occurrences inside the time cluster (a day can hold several).
    pub occurrence_count: u32,
    /// Active days in the window matching `dow_class` — the denominator
    /// the confidence is computed against.
    pub opportunity_days: u32,
    /// Wilson 95% lower bound of `support_days / opportunity_days`;
    /// honest at low support by construction.
    pub confidence: f64,
    /// Day-snapped mining window this record was derived from.
    pub window_start_ns: u64,
    pub window_end_ns: u64,
    /// Days in the window with at least one eligible episode.
    pub active_days_in_window: u32,
    pub first_seen_day_start_ns: u64,
    pub last_seen_day_start_ns: u64,
    /// Most recent occurrences (capped), newest last.
    pub evidence: Vec<RoutineEvidence>,
}

/// Operator-owned lifecycle of a routine (#849).
///
/// `CF_ROUTINES` rows are disposable derived state; lifecycle decisions are
/// not. They live in `CF_ROUTINE_STATE`, anchored on the same deterministic
/// `routine_id`, so re-mining can replace every derived row without touching
/// a single operator decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutineLifecycle {
    /// Mined but not yet reviewed by the operator.
    Candidate,
    /// Operator confirmed the routine as real and useful.
    Confirmed,
    /// Operator disabled it: the miner keeps re-deriving the record, but
    /// intent matching and suggestion surfaces must ignore it, and nothing
    /// may re-promote it automatically.
    Disabled,
    /// Operator archived it: hidden from default listings.
    Archived,
}

/// Operation kinds recorded in a routine's transition audit trail.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutineStateAction {
    /// The miner materialized the state row for a newly mined routine.
    Discovered,
    Confirm,
    Disable,
    Enable,
    Archive,
    Rename,
}

/// One audit entry in a routine's lifecycle history: what happened, when,
/// by whom, and the before/after states (append-only, newest last).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineTransition {
    pub ts_ns: u64,
    pub action: RoutineStateAction,
    /// `None` only for the creation entry ([`RoutineStateAction::Discovered`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<RoutineLifecycle>,
    pub to: RoutineLifecycle,
    /// Who performed it: an MCP session id, `"stdio"`, or `"miner"`.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One confidence observation appended by a mining run (only when the value
/// actually changed, so the history records change-points, not heartbeats).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineConfidencePoint {
    /// Mining instant that produced this observation.
    pub ts_ns: u64,
    pub confidence: f64,
    pub support_days: u32,
    pub opportunity_days: u32,
}

/// Durable operator state for one routine, persisted in `CF_ROUTINE_STATE`
/// (#849), keyed by the routine's stable deterministic id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RoutineStateRecord {
    pub record_version: u32,
    pub routine_id: String,
    pub lifecycle: RoutineLifecycle,
    /// Operator-assigned display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub created_ts_ns: u64,
    /// Last write of any kind to this row.
    pub updated_ts_ns: u64,
    /// Mining instant of the last run that produced this routine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_mined_ts_ns: Option<u64>,
    /// Whether the most recent mining run still derived this routine.
    pub present_in_last_mine: bool,
    /// Lifecycle audit trail, newest last, capped at
    /// [`ROUTINE_STATE_MAX_TRANSITIONS`].
    pub transitions: Vec<RoutineTransition>,
    /// Oldest entries dropped from `transitions` after the cap.
    pub transitions_truncated: u64,
    /// Confidence change-points, newest last, capped at
    /// [`ROUTINE_STATE_MAX_CONFIDENCE_POINTS`].
    pub confidence_history: Vec<RoutineConfidencePoint>,
    /// Oldest entries dropped from `confidence_history` after the cap.
    pub confidence_history_truncated: u64,
}
