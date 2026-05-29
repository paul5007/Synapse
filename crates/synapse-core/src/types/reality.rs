use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{EntityId, EventRef, EventSource, ProfileId};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub surface: RealitySourceSurface,
    pub path: String,
    pub offset: Option<u64>,
    pub hash: Option<String>,
    pub summary: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealitySourceSurface {
    Window,
    A11yUia,
    PixelFrame,
    Hud,
    GameLog,
    File,
    Process,
    ActionAudit,
    Storage,
    Device,
    Profile,
    Model,
    IssueState,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedactionSummary {
    pub policy: RedactionPolicy,
    pub raw_private_fields_omitted: bool,
    pub redacted_fields: Vec<String>,
    pub forbidden_raw_kinds: Vec<ForbiddenRawDataKind>,
}

impl RedactionSummary {
    #[must_use]
    pub fn default_private() -> Self {
        Self::default()
    }
}

impl Default for RedactionSummary {
    fn default() -> Self {
        Self {
            policy: RedactionPolicy::DefaultPrivate,
            raw_private_fields_omitted: true,
            redacted_fields: Vec::new(),
            forbidden_raw_kinds: vec![
                ForbiddenRawDataKind::RawChatBody,
                ForbiddenRawDataKind::RawLogBody,
                ForbiddenRawDataKind::HighCardinalityPrivateData,
                ForbiddenRawDataKind::Secret,
                ForbiddenRawDataKind::Credential,
                ForbiddenRawDataKind::AccountIdentifier,
            ],
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RedactionPolicy {
    #[default]
    DefaultPrivate,
    PublicOnly,
    ExplicitOperatorApproved,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForbiddenRawDataKind {
    RawChatBody,
    RawLogBody,
    HighCardinalityPrivateData,
    Secret,
    Credential,
    AccountIdentifier,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityBaseline {
    pub epoch_id: String,
    pub baseline_seq: u64,
    pub generated_at: DateTime<Utc>,
    pub profile_id: Option<ProfileId>,
    pub source_surfaces: Vec<RealitySourceSurface>,
    pub source_refs: Vec<SourceRef>,
    pub compact_state_hash: String,
    pub redaction: RedactionSummary,
    pub size_bytes: u32,
    pub size_estimate_tokens: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityDelta {
    pub epoch_id: String,
    pub seq: u64,
    pub previous_seq: u64,
    pub at: DateTime<Utc>,
    pub source: EventSource,
    pub kind: String,
    pub path: String,
    pub target: RealityTargetRef,
    pub before: serde_json::Value,
    pub after: serde_json::Value,
    pub confidence: f32,
    pub expected_previous_hash: Option<String>,
    pub source_refs: Vec<SourceRef>,
    pub correlations: Vec<EventRef>,
    pub conflict: Option<RealityDeltaConflict>,
    pub redaction: RedactionSummary,
}

impl RealityDelta {
    /// Validate invariants that cannot be expressed by serde shape alone.
    ///
    /// # Errors
    ///
    /// Returns an error when the delta is not append-ordered or the confidence
    /// is outside the closed range [0.0, 1.0].
    pub fn validate_append_order(&self) -> Result<(), RealityDeltaValidationError> {
        if self.seq <= self.previous_seq {
            return Err(RealityDeltaValidationError::OutOfOrderSeq {
                seq: self.seq,
                previous_seq: self.previous_seq,
            });
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(RealityDeltaValidationError::ConfidenceOutOfRange {
                confidence: self.confidence,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityTargetRef {
    pub kind: RealityTargetKind,
    pub entity_id: Option<EntityId>,
    pub field: Option<String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealityTargetKind {
    Foreground,
    Focus,
    HudField,
    Entity,
    LogCursor,
    Zone,
    Location,
    Action,
    StorageRow,
    Profile,
    Device,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityDeltaConflict {
    pub expected_previous_hash: Option<String>,
    pub actual_previous_hash: Option<String>,
    pub detail: String,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityAudit {
    pub audit_id: String,
    pub epoch_id: String,
    pub baseline_seq: u64,
    pub compared_seq_start: u64,
    pub compared_seq_end: u64,
    pub ran_at: DateTime<Utc>,
    pub baseline_status: RealityBaselineStatus,
    pub assumption_hash: String,
    pub actual_hash: String,
    pub drift_status: RealityDriftStatus,
    pub drift_items: Vec<RealityDriftItem>,
    pub physical_source_refs: Vec<SourceRef>,
    pub rebase_required: bool,
    pub rebase_reason: Option<String>,
    pub follow_up_refs: Vec<EventRef>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealityBaselineStatus {
    Current,
    Stale,
    SourceUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RealityDriftItem {
    pub path: String,
    pub assumed: serde_json::Value,
    pub actual: serde_json::Value,
    pub severity: RealityDriftStatus,
    pub source_refs: Vec<SourceRef>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RealityDriftStatus {
    InSync,
    MinorDrift,
    MajorDrift,
    RebaseRequired,
    SourceUnavailable,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum RealityDeltaValidationError {
    #[error("reality delta seq {seq} must be greater than previous_seq {previous_seq}")]
    OutOfOrderSeq { seq: u64, previous_seq: u64 },
    #[error("reality delta confidence {confidence} is outside [0.0, 1.0]")]
    ConfidenceOutOfRange { confidence: f32 },
}
