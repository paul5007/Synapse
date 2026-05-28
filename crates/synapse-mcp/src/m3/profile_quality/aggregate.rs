use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use synapse_core::error_codes;
use synapse_profiles::ProfileStatus;
use synapse_storage::{cf, decode_json};

use super::{
    ProfileCompatibilitySummary, ProfileQualityContribution, ProfileQualityCounts,
    ProfileQualityRates, ProfileQualityRedaction, ProfileQualityRefreshParams,
    ProfileQualityRuntimeEvidence, ProfileQualityScore, ProfileQualitySnapshot,
    ProfileQualitySource, ProfileQualityVersionSummary, hex_encode,
};

const QUALITY_SCHEMA_VERSION: u32 = 2;
const FUTURE_SKEW_NS: u64 = 60 * 1_000_000_000;
const WILSON_Z_95: f64 = 1.959_963_984_540_054;
const CONFIDENCE_FULL_SAMPLE: f64 = 20.0;

pub(super) struct ProfileQualityInputRows {
    pub action: Vec<(Vec<u8>, Vec<u8>)>,
    pub observations: Vec<(Vec<u8>, Vec<u8>)>,
    pub events: Vec<(Vec<u8>, Vec<u8>)>,
}

#[derive(Debug)]
struct ParsedAuditRow {
    audit_id: Option<String>,
    ts_ns: u64,
    tool: String,
    status: String,
    error_code: Option<String>,
    active_profile_id: Option<String>,
    active_profile_schema_version: Option<u32>,
    foreground_profile_id: Option<String>,
    foreground_profile_schema_version: Option<u32>,
    foreground_process_name: Option<String>,
    response_backend: Option<String>,
}

#[derive(Debug)]
struct ParsedObservationRow {
    observation_id: Option<String>,
    ts_ns: u64,
    foreground_profile_id: Option<String>,
    foreground_process_name: Option<String>,
    target_id: Option<String>,
    recent_event_kinds: Vec<String>,
}

#[derive(Debug)]
struct ParsedEventRow {
    event_id: Option<String>,
    ts_ns: u64,
    profile_id: Option<String>,
    kind: String,
    process_name: Option<String>,
    target_id: Option<String>,
}

pub(super) fn build_snapshot(
    profile: &ProfileStatus,
    rows: ProfileQualityInputRows,
    params: &ProfileQualityRefreshParams,
    generated_at_ns: u64,
) -> ProfileQualitySnapshot {
    let mut builder = SnapshotBuilder::new(
        profile,
        params,
        rows.action.len() as u64,
        rows.observations.len() as u64,
        rows.events.len() as u64,
        generated_at_ns,
    );
    for (_key, value) in rows.action {
        match parse_audit_row(&value) {
            Ok(row) => builder.record_action_row(&row),
            Err(()) => builder.source.audit_rows_decode_failed += 1,
        }
    }
    for (_key, value) in rows.observations {
        match parse_observation_row(&value) {
            Ok(row) => builder.record_observation_row(&row),
            Err(()) => builder.runtime_evidence.observation_rows_decode_failed += 1,
        }
    }
    for (_key, value) in rows.events {
        match parse_event_row(&value) {
            Ok(row) => builder.record_event_row(&row),
            Err(()) => builder.runtime_evidence.event_rows_decode_failed += 1,
        }
    }
    builder.finish()
}

fn parse_audit_row(value: &[u8]) -> Result<ParsedAuditRow, ()> {
    let row = decode_json::<Value>(value).map_err(|_error| ())?;
    let ts_ns = row.get("ts_ns").and_then(Value::as_u64).ok_or(())?;
    let tool = row
        .get("tool")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(())?
        .to_owned();
    let status = row
        .get("status")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(())?
        .to_owned();
    Ok(ParsedAuditRow {
        audit_id: optional_string(&row, "audit_id"),
        ts_ns,
        tool,
        status,
        error_code: optional_string(&row, "error_code"),
        active_profile_id: optional_string(&row, "active_profile_id"),
        active_profile_schema_version: optional_u32(&row, "active_profile_schema_version")
            .or_else(|| optional_u32(&row, "profile_schema_version")),
        foreground_profile_id: pointer_string(&row, "/foreground/profile_id"),
        foreground_profile_schema_version: row
            .pointer("/foreground/profile_schema_version")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        foreground_process_name: pointer_string(&row, "/foreground/process_name"),
        response_backend: pointer_string(&row, "/details/response/backend")
            .or_else(|| pointer_string(&row, "/details/response/backend_used")),
    })
}

fn parse_observation_row(value: &[u8]) -> Result<ParsedObservationRow, ()> {
    let row = decode_json::<Value>(value).map_err(|_error| ())?;
    let recent_event_kinds = row
        .get("recent_events")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| pointer_string(item, "/kind"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ParsedObservationRow {
        observation_id: optional_string(&row, "observation_id"),
        ts_ns: row.get("ts_ns").and_then(Value::as_u64).ok_or(())?,
        foreground_profile_id: pointer_string(&row, "/foreground/profile_id"),
        foreground_process_name: pointer_string(&row, "/foreground/process_name"),
        target_id: pointer_identifier_string(&row, "/foreground/steam_appid")
            .or_else(|| pointer_string(&row, "/foreground/target_id")),
        recent_event_kinds,
    })
}

fn parse_event_row(value: &[u8]) -> Result<ParsedEventRow, ()> {
    let row = decode_json::<Value>(value).map_err(|_error| ())?;
    let kind = optional_string(&row, "kind").ok_or(())?;
    Ok(ParsedEventRow {
        event_id: optional_string(&row, "event_id"),
        ts_ns: row.get("ts_ns").and_then(Value::as_u64).ok_or(())?,
        profile_id: optional_string(&row, "profile_id")
            .or_else(|| pointer_string(&row, "/audit_context/profile_id"))
            .or_else(|| pointer_string(&row, "/data/profile_id"))
            .or_else(|| pointer_string(&row, "/foreground/profile_id")),
        kind,
        process_name: pointer_string(&row, "/data/process_name")
            .or_else(|| pointer_string(&row, "/audit_context/app_context/process_name"))
            .or_else(|| pointer_string(&row, "/foreground/process_name")),
        target_id: pointer_string(&row, "/audit_context/app_context/target_id")
            .or_else(|| pointer_string(&row, "/data/target_id"))
            .or_else(|| pointer_identifier_string(&row, "/foreground/steam_appid")),
    })
}

fn optional_string(row: &Value, field: &str) -> Option<String> {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn optional_u32(row: &Value, field: &str) -> Option<u32> {
    row.get(field)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn pointer_string(row: &Value, pointer: &str) -> Option<String> {
    row.pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn pointer_identifier_string(row: &Value, pointer: &str) -> Option<String> {
    let value = row.pointer(pointer)?;
    if let Some(value) = value.as_str().filter(|value| !value.is_empty()) {
        return Some(value.to_owned());
    }
    value
        .as_u64()
        .map(|value| value.to_string())
        .or_else(|| value.as_i64().map(|value| value.to_string()))
}

struct SnapshotBuilder {
    profile_id: String,
    source: ProfileQualitySource,
    counts: ProfileQualityCounts,
    compatibility: ProfileCompatibilitySummary,
    versioning: ProfileQualityVersionSummary,
    runtime_evidence: ProfileQualityRuntimeEvidence,
    generated_at_ns: u64,
    evidence_parts: Vec<String>,
    profile_label: String,
    profile_schema_version: u32,
    quality_signal: Option<String>,
    manual_fsv_evidence_ref: Option<String>,
    profile_target_id: Option<String>,
}

impl SnapshotBuilder {
    fn new(
        profile: &ProfileStatus,
        params: &ProfileQualityRefreshParams,
        audit_rows_scanned: u64,
        observation_rows_scanned: u64,
        event_rows_scanned: u64,
        generated_at_ns: u64,
    ) -> Self {
        Self {
            profile_id: profile.id.clone(),
            source: ProfileQualitySource {
                audit_cf_name: cf::CF_ACTION_LOG.to_owned(),
                profile_cf_name: cf::CF_PROFILES.to_owned(),
                audit_rows_scanned,
                audit_rows_decode_failed: 0,
                audit_rows_stale: 0,
                audit_rows_future: 0,
                audit_rows_other_profile: 0,
                audit_rows_profile_relevant: 0,
                first_relevant_audit_id: None,
                last_relevant_audit_id: None,
                first_relevant_ts_ns: None,
                last_relevant_ts_ns: None,
                max_audit_rows: params.max_audit_rows,
                stale_after_ns: params.stale_after_ns,
            },
            counts: ProfileQualityCounts {
                started_rows: 0,
                ok_rows: 0,
                error_rows: 0,
                denied_rows: 0,
                unknown_status_rows: 0,
                quality_eligible_ok_rows: 0,
                quality_eligible_error_rows: 0,
                backend_unavailable_rows: 0,
                release_all_rows: 0,
                launch_ok_rows: 0,
                launch_error_rows: 0,
                tool_counts: BTreeMap::new(),
                error_code_counts: BTreeMap::new(),
            },
            compatibility: ProfileCompatibilitySummary {
                foreground_match_rows: 0,
                active_profile_only_rows: 0,
                profile_mismatch_rows: 0,
                target_denied_rows: 0,
                observed_process_names: BTreeMap::new(),
                observed_backends: BTreeMap::new(),
            },
            versioning: ProfileQualityVersionSummary {
                current_profile_schema_version: profile.schema_version,
                rows_with_profile_schema_version: 0,
                current_version_rows: 0,
                older_version_rows: 0,
                newer_version_rows: 0,
                unknown_version_rows: 0,
                mixed_profile_schema_versions: false,
                observed_profile_schema_versions: BTreeMap::new(),
            },
            runtime_evidence: ProfileQualityRuntimeEvidence {
                observation_cf_name: cf::CF_OBSERVATIONS.to_owned(),
                event_cf_name: cf::CF_EVENTS.to_owned(),
                observation_rows_scanned,
                event_rows_scanned,
                ..ProfileQualityRuntimeEvidence::default()
            },
            generated_at_ns,
            evidence_parts: Vec::new(),
            profile_label: profile.label.clone(),
            profile_schema_version: profile.schema_version,
            quality_signal: profile.metadata.get("registry.quality_signal").cloned(),
            manual_fsv_evidence_ref: params
                .manual_fsv_evidence_ref
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            profile_target_id: profile
                .metadata
                .get("registry.compatibility_target")
                .or_else(|| profile.metadata.get("benchmark_id"))
                .cloned(),
        }
    }

    fn record_action_row(&mut self, row: &ParsedAuditRow) {
        if self.is_action_stale_or_future(row.ts_ns) {
            return;
        }
        let foreground_match = row.foreground_profile_id.as_deref() == Some(&self.profile_id);
        let active_match = row.active_profile_id.as_deref() == Some(&self.profile_id);
        if !(foreground_match || active_match) {
            self.source.audit_rows_other_profile += 1;
            return;
        }

        self.source.audit_rows_profile_relevant += 1;
        self.record_action_range(row);
        self.record_compatibility(row, foreground_match, active_match);
        *self.counts.tool_counts.entry(row.tool.clone()).or_default() += 1;
        if let Some(error_code) = &row.error_code {
            *self
                .counts
                .error_code_counts
                .entry(error_code.clone())
                .or_default() += 1;
            if error_code == error_codes::ACTION_BACKEND_UNAVAILABLE {
                self.counts.backend_unavailable_rows += 1;
            }
        }
        if let Some(backend) = &row.response_backend {
            *self
                .compatibility
                .observed_backends
                .entry(backend.clone())
                .or_default() += 1;
        }
        self.record_status(row, foreground_match);
        self.record_version(row, foreground_match);
        self.evidence_parts.push(action_evidence_part(row));
    }

    fn record_observation_row(&mut self, row: &ParsedObservationRow) {
        if self.is_runtime_stale_or_future(row.ts_ns, RuntimeRowKind::Observation) {
            return;
        }
        if row.foreground_profile_id.as_deref() != Some(&self.profile_id) {
            self.runtime_evidence.observation_rows_other_profile += 1;
            return;
        }

        self.runtime_evidence.observation_rows_profile_relevant += 1;
        if self
            .runtime_evidence
            .first_relevant_observation_id
            .is_none()
        {
            self.runtime_evidence
                .first_relevant_observation_id
                .clone_from(&row.observation_id);
        }
        self.runtime_evidence
            .last_relevant_observation_id
            .clone_from(&row.observation_id);
        self.record_runtime_ts(row.ts_ns);
        if let Some(process_name) = &row.foreground_process_name {
            increment(
                &mut self.runtime_evidence.observed_process_names,
                process_name.clone(),
            );
        }
        let target_id = row.target_id.as_ref().or(self.profile_target_id.as_ref());
        if let Some(target_id) = target_id {
            increment(
                &mut self.runtime_evidence.observed_target_ids,
                target_id.clone(),
            );
        }
        for kind in &row.recent_event_kinds {
            increment(
                &mut self.runtime_evidence.observed_event_kinds,
                kind.clone(),
            );
            if is_log_event_kind(kind) {
                increment(
                    &mut self.runtime_evidence.observed_log_event_kinds,
                    kind.clone(),
                );
            }
        }
        self.evidence_parts.push(observation_evidence_part(row));
    }

    fn record_event_row(&mut self, row: &ParsedEventRow) {
        if self.is_runtime_stale_or_future(row.ts_ns, RuntimeRowKind::Event) {
            return;
        }
        if row.profile_id.as_deref() != Some(&self.profile_id) {
            self.runtime_evidence.event_rows_other_profile += 1;
            return;
        }

        self.runtime_evidence.event_rows_profile_relevant += 1;
        if self.runtime_evidence.first_relevant_event_id.is_none() {
            self.runtime_evidence
                .first_relevant_event_id
                .clone_from(&row.event_id);
        }
        self.runtime_evidence
            .last_relevant_event_id
            .clone_from(&row.event_id);
        self.record_runtime_ts(row.ts_ns);
        if let Some(process_name) = &row.process_name {
            increment(
                &mut self.runtime_evidence.observed_process_names,
                process_name.clone(),
            );
        }
        let target_id = row.target_id.as_ref().or(self.profile_target_id.as_ref());
        if let Some(target_id) = target_id {
            increment(
                &mut self.runtime_evidence.observed_target_ids,
                target_id.clone(),
            );
        }
        increment(
            &mut self.runtime_evidence.observed_event_kinds,
            row.kind.clone(),
        );
        if is_log_event_kind(&row.kind) {
            increment(
                &mut self.runtime_evidence.observed_log_event_kinds,
                row.kind.clone(),
            );
        }
        self.evidence_parts.push(event_evidence_part(row));
    }

    const fn is_action_stale_or_future(&mut self, ts_ns: u64) -> bool {
        if ts_ns > self.generated_at_ns.saturating_add(FUTURE_SKEW_NS) {
            self.source.audit_rows_future += 1;
            return true;
        }
        if self.generated_at_ns.saturating_sub(ts_ns) > self.source.stale_after_ns {
            self.source.audit_rows_stale += 1;
            return true;
        }
        false
    }

    const fn is_runtime_stale_or_future(&mut self, ts_ns: u64, kind: RuntimeRowKind) -> bool {
        if ts_ns > self.generated_at_ns.saturating_add(FUTURE_SKEW_NS) {
            match kind {
                RuntimeRowKind::Observation => self.runtime_evidence.observation_rows_future += 1,
                RuntimeRowKind::Event => self.runtime_evidence.event_rows_future += 1,
            }
            return true;
        }
        if self.generated_at_ns.saturating_sub(ts_ns) > self.source.stale_after_ns {
            match kind {
                RuntimeRowKind::Observation => self.runtime_evidence.observation_rows_stale += 1,
                RuntimeRowKind::Event => self.runtime_evidence.event_rows_stale += 1,
            }
            return true;
        }
        false
    }

    fn record_action_range(&mut self, row: &ParsedAuditRow) {
        if self.source.first_relevant_ts_ns.is_none() {
            self.source.first_relevant_ts_ns = Some(row.ts_ns);
            self.source
                .first_relevant_audit_id
                .clone_from(&row.audit_id);
        }
        self.source.last_relevant_ts_ns = Some(row.ts_ns);
        self.source.last_relevant_audit_id.clone_from(&row.audit_id);
    }

    fn record_runtime_ts(&mut self, ts_ns: u64) {
        self.runtime_evidence.last_relevant_ts_ns = Some(
            self.runtime_evidence
                .last_relevant_ts_ns
                .map_or(ts_ns, |previous| previous.max(ts_ns)),
        );
    }

    fn record_compatibility(
        &mut self,
        row: &ParsedAuditRow,
        foreground_match: bool,
        active_match: bool,
    ) {
        if foreground_match {
            self.compatibility.foreground_match_rows += 1;
        } else if active_match {
            self.compatibility.active_profile_only_rows += 1;
            self.compatibility.profile_mismatch_rows += 1;
        }
        if let Some(process_name) = &row.foreground_process_name {
            *self
                .compatibility
                .observed_process_names
                .entry(process_name.clone())
                .or_default() += 1;
        }
    }

    fn record_status(&mut self, row: &ParsedAuditRow, foreground_match: bool) {
        match row.status.as_str() {
            "started" => self.counts.started_rows += 1,
            "ok" => {
                self.counts.ok_rows += 1;
                if foreground_match {
                    self.counts.quality_eligible_ok_rows += 1;
                }
                if row.tool == "act_launch" {
                    self.counts.launch_ok_rows += 1;
                }
                if row.tool == "release_all" {
                    self.counts.release_all_rows += 1;
                }
            }
            "error" => {
                self.counts.error_rows += 1;
                if foreground_match {
                    self.counts.quality_eligible_error_rows += 1;
                }
                if row.tool == "act_launch" {
                    self.counts.launch_error_rows += 1;
                }
            }
            "denied" => {
                self.counts.denied_rows += 1;
                self.compatibility.target_denied_rows += 1;
            }
            _ => self.counts.unknown_status_rows += 1,
        }
    }

    fn record_version(&mut self, row: &ParsedAuditRow, foreground_match: bool) {
        let row_version = if foreground_match {
            row.foreground_profile_schema_version
        } else {
            row.active_profile_schema_version
        };
        let Some(row_version) = row_version else {
            self.versioning.unknown_version_rows += 1;
            return;
        };

        self.versioning.rows_with_profile_schema_version += 1;
        *self
            .versioning
            .observed_profile_schema_versions
            .entry(row_version.to_string())
            .or_default() += 1;

        match row_version.cmp(&self.profile_schema_version) {
            std::cmp::Ordering::Equal => self.versioning.current_version_rows += 1,
            std::cmp::Ordering::Less => self.versioning.older_version_rows += 1,
            std::cmp::Ordering::Greater => self.versioning.newer_version_rows += 1,
        }
    }

    fn finish(self) -> ProfileQualitySnapshot {
        let score = score_from_counts(&self.counts);
        let rates = rates_from_counts(&self.counts);
        let evidence_hash = evidence_hash(
            &self.profile_id,
            &self.evidence_parts,
            &self.source,
            &self.runtime_evidence,
            self.manual_fsv_evidence_ref.as_deref(),
        );
        let mut versioning = self.versioning;
        versioning.mixed_profile_schema_versions =
            versioning.observed_profile_schema_versions.len() > 1;
        ProfileQualitySnapshot {
            schema_version: QUALITY_SCHEMA_VERSION,
            profile_id: self.profile_id,
            profile_label: self.profile_label,
            profile_schema_version: self.profile_schema_version,
            quality_signal: self.quality_signal,
            manual_fsv_evidence_ref: self.manual_fsv_evidence_ref,
            generated_at_ns: self.generated_at_ns,
            evidence_hash,
            source: self.source,
            counts: self.counts,
            rates,
            score,
            compatibility: self.compatibility,
            versioning,
            runtime_evidence: self.runtime_evidence,
            redaction: ProfileQualityRedaction {
                local_only: true,
                snapshot_redacts_process_path: true,
                snapshot_redacts_window_title: true,
                retained_identifiers: vec![
                    "profile_id".to_owned(),
                    "manual_fsv_evidence_ref".to_owned(),
                    "tool".to_owned(),
                    "status".to_owned(),
                    "error_code".to_owned(),
                    "process_name".to_owned(),
                    "target_id".to_owned(),
                    "observation_id".to_owned(),
                    "event_id".to_owned(),
                    "event_kind".to_owned(),
                ],
            },
            contribution: ProfileQualityContribution {
                export_allowed: false,
                operator_consent_required: true,
                future_bundle_shape: "operator_approved_profile_quality_v2".to_owned(),
            },
        }
    }
}

#[derive(Clone, Copy)]
enum RuntimeRowKind {
    Observation,
    Event,
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn score_from_counts(counts: &ProfileQualityCounts) -> ProfileQualityScore {
    let successes = counts.quality_eligible_ok_rows;
    let failures = counts.quality_eligible_error_rows;
    let sample_size = successes.saturating_add(failures);
    let wilson = wilson_lower_bound(successes, sample_size);
    ProfileQualityScore {
        score_0_100: (wilson * 100.0).round() as u32,
        confidence_0_1: (sample_size as f64 / CONFIDENCE_FULL_SAMPLE).min(1.0),
        wilson_success_lower_95: wilson,
        sample_size,
        method: "wilson_lower_95_on_foreground_ok_vs_error; denied rows tracked separately"
            .to_owned(),
    }
}

fn rates_from_counts(counts: &ProfileQualityCounts) -> ProfileQualityRates {
    let terminal = counts
        .ok_rows
        .saturating_add(counts.error_rows)
        .saturating_add(counts.denied_rows);
    ProfileQualityRates {
        success_rate: ratio(counts.ok_rows, terminal),
        error_rate: ratio(counts.error_rows, terminal),
        denied_rate: ratio(counts.denied_rows, terminal),
        backend_unavailable_rate: ratio(counts.backend_unavailable_rows, terminal),
    }
}

#[allow(clippy::cast_precision_loss)]
fn wilson_lower_bound(successes: u64, sample_size: u64) -> f64 {
    if sample_size == 0 {
        return 0.0;
    }
    let n = sample_size as f64;
    let p = successes as f64 / n;
    let z2 = WILSON_Z_95 * WILSON_Z_95;
    let center = p + z2 / (2.0 * n);
    let margin = WILSON_Z_95 * (p.mul_add(1.0 - p, z2 / (4.0 * n)) / n).sqrt();
    ((center - margin) / (1.0 + z2 / n)).clamp(0.0, 1.0)
}

#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    numerator as f64 / denominator as f64
}

fn action_evidence_part(row: &ParsedAuditRow) -> String {
    format!(
        "action|{}|{}|{}|{}|{}|{}|{}|{}",
        row.audit_id.as_deref().unwrap_or(""),
        row.ts_ns,
        row.tool,
        row.status,
        row.error_code.as_deref().unwrap_or(""),
        row.foreground_profile_id.as_deref().unwrap_or(""),
        row.foreground_profile_schema_version
            .map(|value| value.to_string())
            .unwrap_or_default(),
        row.active_profile_schema_version
            .map(|value| value.to_string())
            .unwrap_or_default()
    )
}

fn observation_evidence_part(row: &ParsedObservationRow) -> String {
    format!(
        "observation|{}|{}|{}|{}|{}",
        row.observation_id.as_deref().unwrap_or(""),
        row.ts_ns,
        row.foreground_profile_id.as_deref().unwrap_or(""),
        row.foreground_process_name.as_deref().unwrap_or(""),
        row.recent_event_kinds.join(",")
    )
}

fn event_evidence_part(row: &ParsedEventRow) -> String {
    format!(
        "event|{}|{}|{}|{}|{}|{}",
        row.event_id.as_deref().unwrap_or(""),
        row.ts_ns,
        row.profile_id.as_deref().unwrap_or(""),
        row.kind.as_str(),
        row.process_name.as_deref().unwrap_or(""),
        row.target_id.as_deref().unwrap_or("")
    )
}

fn evidence_hash(
    profile_id: &str,
    parts: &[String],
    source: &ProfileQualitySource,
    runtime: &ProfileQualityRuntimeEvidence,
    manual_fsv_evidence_ref: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(profile_id.as_bytes());
    hasher.update(source.stale_after_ns.to_be_bytes());
    hasher.update(source.audit_rows_decode_failed.to_be_bytes());
    hasher.update(source.audit_rows_stale.to_be_bytes());
    hasher.update(source.audit_rows_future.to_be_bytes());
    hasher.update(runtime.observation_rows_decode_failed.to_be_bytes());
    hasher.update(runtime.observation_rows_stale.to_be_bytes());
    hasher.update(runtime.observation_rows_future.to_be_bytes());
    hasher.update(runtime.event_rows_decode_failed.to_be_bytes());
    hasher.update(runtime.event_rows_stale.to_be_bytes());
    hasher.update(runtime.event_rows_future.to_be_bytes());
    if let Some(value) = manual_fsv_evidence_ref {
        hasher.update(value.as_bytes());
    }
    hasher.update([0]);
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("sha256:{}", hex_encode(&hasher.finalize()))
}

fn increment(counts: &mut BTreeMap<String, u64>, key: String) {
    *counts.entry(key).or_default() += 1;
}

fn is_log_event_kind(kind: &str) -> bool {
    kind.starts_with("everquest.log.")
}
