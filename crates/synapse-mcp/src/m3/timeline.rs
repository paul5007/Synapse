//! `timeline_search` (#841) and `timeline_purge` (#843) MCP tools
//! (ADR 2026-06-11-timeline-data-model).
//!
//! Searches `CF_TIMELINE` rows by time range, app, record kind, actor, and
//! case-insensitive text over the record's app and payload string values
//! (titles, paths, URLs, clipboard snippets). Results page via an opaque
//! cursor; per-call scan work is budgeted so one query can never pin the
//! runtime lock on an arbitrarily large timeline. Undecodable rows are
//! counted and logged, never silently skipped.
//!
//! Purge shares the same filter machinery (what you can find is exactly what
//! you can delete), hard-deletes via `delete_batch`, compacts the purged key
//! range (tombstone reclamation per the ADR §6 / RocksDB guidance), and
//! writes a `kind = purge` audit row carrying counts and the filters — never
//! deleted content. Blanket purges skip `purge` audit rows so a purge can
//! never consume its own audit trail; deleting audit rows requires naming
//! `kinds: ["purge"]` explicitly.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU32, Ordering},
};

use rmcp::{ErrorData, schemars::JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use synapse_core::error_codes;
use synapse_core::types::{TimelineActor, TimelineKind, TimelineRecord};
use synapse_reflex::ReflexRuntime;
use synapse_storage::{cf, decode_json, timeline as timeline_codec};

use crate::m1::mcp_error;

use super::{
    M3ToolStub,
    permissions::{Permission, RequiredPermissions, required},
};

/// Default number of matches returned when `limit` is omitted.
pub const DEFAULT_LIMIT: u32 = 100;
/// Hard upper bound for `limit`.
pub const MAX_LIMIT: u32 = 500;
/// Maximum rows scanned per call before the search pauses with a cursor.
pub const MAX_SCAN_ROWS_PER_CALL: usize = 100_000;
/// Chunk size for bounded storage reads inside one call.
const SCAN_CHUNK_ROWS: usize = 4_096;
/// Maximum accepted `text` filter length in bytes.
const MAX_TEXT_FILTER_BYTES: usize = 512;

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineSearchParams {
    /// Inclusive lower bound on the record `ts_ns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ts_ns: Option<u64>,
    /// Inclusive upper bound on the record `ts_ns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ts_ns: Option<u64>,
    /// Case-insensitive exact matches on the record `app` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    /// Case-insensitive substring over app + payload string values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Snake-case record kinds (e.g. `focus_change`, `browser_nav`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// `human` or `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Maximum matches to return (default 100, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineSearchResponse {
    pub matches: Vec<TimelineSearchMatch>,
    /// Rows examined this call (matching or not).
    pub scanned_rows: u64,
    /// Rows whose value failed to decode as a `TimelineRecord`; details are
    /// in daemon logs under code `TIMELINE_ROW_DECODE_FAILED`.
    pub invalid_rows: u64,
    /// Present when more rows may match; pass back as `cursor` to continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// Why the call stopped: `limit_reached`, `scan_budget_exhausted`,
    /// `end_ts_reached`, or `end_of_timeline`.
    pub stopped_because: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineSearchMatch {
    /// Hex-encoded storage key (stable row identity).
    pub key_hex: String,
    pub ts_ns: u64,
    /// Key sequence component; absent for rows with non-codec keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    pub kind: String,
    /// `human` or `agent:<session_id>`.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub payload: Value,
}

#[must_use]
pub const fn timeline_search() -> M3ToolStub {
    M3ToolStub::new("timeline_search")
}

#[must_use]
pub const fn timeline_purge() -> M3ToolStub {
    M3ToolStub::new("timeline_purge")
}

#[must_use]
pub fn required_permissions(_params: &TimelineSearchParams) -> RequiredPermissions {
    required([Permission::ReadStorage])
}

#[derive(Debug)]
struct Filters {
    start_ts_ns: u64,
    end_ts_ns: u64,
    apps_lower: Vec<String>,
    text_lower: Option<String>,
    kinds: Vec<TimelineKind>,
    actor: Option<ActorFilter>,
    limit: usize,
    start_key: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActorFilter {
    Human,
    Agent,
}

pub fn search_timeline(
    runtime: &Arc<Mutex<ReflexRuntime>>,
    params: &TimelineSearchParams,
) -> Result<TimelineSearchResponse, ErrorData> {
    let filters = validate(params)?;
    let runtime = lock_runtime(runtime)?;

    let mut matches = Vec::new();
    let mut scanned_rows = 0_u64;
    let mut invalid_rows = 0_u64;
    let mut next_start = filters.start_key.clone();
    let mut last_key: Option<Vec<u8>> = None;
    let mut stopped_because = "end_of_timeline";
    let mut storage_has_more = false;

    'scan: loop {
        let remaining_budget = MAX_SCAN_ROWS_PER_CALL - usize::try_from(scanned_rows).unwrap_or(0);
        if remaining_budget == 0 {
            stopped_because = "scan_budget_exhausted";
            break;
        }
        let chunk_rows = SCAN_CHUNK_ROWS.min(remaining_budget);
        let (rows, more) = runtime
            .storage_cf_rows_from(cf::CF_TIMELINE, &next_start, chunk_rows)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        storage_has_more = more;
        if rows.is_empty() {
            break;
        }
        for (key, value) in rows {
            scanned_rows += 1;
            last_key = Some(key.clone());
            let codec_ts = timeline_codec::decode_timeline_key(&key).ok();
            // Codec keys iterate in ts order, so the first codec key past the
            // end bound proves no later codec row can match (ADR key scheme).
            if let Some((key_ts, _seq)) = codec_ts
                && key_ts > filters.end_ts_ns
            {
                stopped_because = "end_ts_reached";
                storage_has_more = false;
                break 'scan;
            }
            match decode_json::<TimelineRecord>(&value) {
                Ok(record) => {
                    if record_matches(&record, &filters) {
                        matches.push(to_match(&key, codec_ts.map(|(_ts, seq)| seq), record));
                        if matches.len() >= filters.limit {
                            stopped_because = "limit_reached";
                            break 'scan;
                        }
                    }
                }
                Err(error) => {
                    invalid_rows += 1;
                    tracing::warn!(
                        code = "TIMELINE_ROW_DECODE_FAILED",
                        key_hex = %hex_encode(&key),
                        %error,
                        "timeline_search skipped undecodable CF_TIMELINE row"
                    );
                }
            }
        }
        if !more {
            break;
        }
        let Some(last) = last_key.as_ref() else { break };
        next_start = key_after(last);
    }
    drop(runtime);

    let resume_possible = matches!(stopped_because, "limit_reached" | "scan_budget_exhausted")
        && (storage_has_more || stopped_because == "limit_reached");
    let next_cursor = if resume_possible {
        last_key.as_deref().map(hex_encode)
    } else {
        None
    };
    Ok(TimelineSearchResponse {
        matches,
        scanned_rows,
        invalid_rows,
        next_cursor,
        stopped_because: stopped_because.to_owned(),
    })
}

/// Monotonic per-process sequence for purge-audit keys, offset away from the
/// recorder's own sequence space so a same-nanosecond collision is
/// unrepresentable in practice.
static PURGE_AUDIT_SEQ: AtomicU32 = AtomicU32::new(0xFFFF_0000);

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelinePurgeParams {
    /// Inclusive lower bound on the record `ts_ns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ts_ns: Option<u64>,
    /// Inclusive upper bound on the record `ts_ns`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ts_ns: Option<u64>,
    /// Case-insensitive exact matches on the record `app` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<String>>,
    /// Case-insensitive substring over app + payload string values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Snake-case record kinds. `purge` audit rows are only deleted when
    /// this explicitly contains `"purge"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// `human` or `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Explicit full-timeline purge. Mutually exclusive with every filter;
    /// without it, at least one filter is required.
    #[serde(default)]
    pub all: bool,
    /// Count matches without deleting anything.
    #[serde(default)]
    pub dry_run: bool,
    /// Opaque continuation cursor from a previous purge response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelinePurgeResponse {
    /// Rows that matched the filters this call.
    pub matched_rows: u64,
    /// Rows physically deleted (0 on `dry_run`).
    pub deleted_rows: u64,
    /// Rows examined this call (matching or not).
    pub scanned_rows: u64,
    /// Undecodable rows: counted, logged, and never deleted (a row that
    /// cannot be decoded cannot be proven to match the filters).
    pub invalid_rows: u64,
    /// Matching `purge` audit rows protected because `kinds` did not
    /// explicitly include `"purge"`.
    pub protected_audit_rows: u64,
    pub dry_run: bool,
    /// Hex storage key of the audit row written for this purge; absent on
    /// `dry_run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_key_hex: Option<String>,
    /// Whether the purged key range was compacted (tombstone reclamation).
    pub compacted: bool,
    /// Present when the scan budget paused the purge; pass back as `cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// `scan_budget_exhausted`, `end_ts_reached`, or `end_of_timeline`.
    pub stopped_because: String,
}

#[must_use]
pub fn required_permissions_purge(_params: &TimelinePurgeParams) -> RequiredPermissions {
    required([Permission::ReadStorage, Permission::WriteStorage])
}

pub fn purge_timeline(
    runtime: &Arc<Mutex<ReflexRuntime>>,
    params: &TimelinePurgeParams,
    by_session: &str,
) -> Result<TimelinePurgeResponse, ErrorData> {
    let has_filter = params.start_ts_ns.is_some()
        || params.end_ts_ns.is_some()
        || params.apps.is_some()
        || params.text.is_some()
        || params.kinds.is_some()
        || params.actor.is_some();
    if params.all && has_filter {
        return Err(invalid(
            "timeline_purge all=true is mutually exclusive with filters; drop the filters or drop all",
        ));
    }
    if !params.all && !has_filter {
        return Err(invalid(
            "timeline_purge requires at least one filter (start_ts_ns/end_ts_ns/apps/text/kinds/actor) or an explicit all=true",
        ));
    }
    let search_equivalent = TimelineSearchParams {
        start_ts_ns: params.start_ts_ns,
        end_ts_ns: params.end_ts_ns,
        apps: params.apps.clone(),
        text: params.text.clone(),
        kinds: params.kinds.clone(),
        actor: params.actor.clone(),
        limit: None,
        cursor: params.cursor.clone(),
    };
    let mut filters = validate(&search_equivalent)?;
    // Purge has no match cap: everything matched inside the scan budget is
    // deleted; the budget plus cursor bound one call's work.
    filters.limit = usize::MAX;
    let purge_kind_explicit = filters.kinds.contains(&TimelineKind::Purge);

    let runtime_guard = lock_runtime(runtime)?;
    let mut keys_to_delete: Vec<Vec<u8>> = Vec::new();
    let mut scanned_rows = 0_u64;
    let mut invalid_rows = 0_u64;
    let mut protected_audit_rows = 0_u64;
    let mut next_start = filters.start_key.clone();
    let mut last_key: Option<Vec<u8>> = None;
    let mut stopped_because = "end_of_timeline";
    let mut storage_has_more = false;

    'scan: loop {
        let remaining_budget = MAX_SCAN_ROWS_PER_CALL - usize::try_from(scanned_rows).unwrap_or(0);
        if remaining_budget == 0 {
            stopped_because = "scan_budget_exhausted";
            break;
        }
        let chunk_rows = SCAN_CHUNK_ROWS.min(remaining_budget);
        let (rows, more) = runtime_guard
            .storage_cf_rows_from(cf::CF_TIMELINE, &next_start, chunk_rows)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        storage_has_more = more;
        if rows.is_empty() {
            break;
        }
        for (key, value) in rows {
            scanned_rows += 1;
            last_key = Some(key.clone());
            let codec_ts = timeline_codec::decode_timeline_key(&key).ok();
            if let Some((key_ts, _seq)) = codec_ts
                && key_ts > filters.end_ts_ns
            {
                stopped_because = "end_ts_reached";
                storage_has_more = false;
                break 'scan;
            }
            match decode_json::<TimelineRecord>(&value) {
                Ok(record) => {
                    if record_matches(&record, &filters) {
                        if record.kind == TimelineKind::Purge && !purge_kind_explicit {
                            // A purge must never consume its own audit trail:
                            // audit rows are deleted only by naming the kind.
                            protected_audit_rows += 1;
                        } else {
                            keys_to_delete.push(key);
                        }
                    }
                }
                Err(error) => {
                    invalid_rows += 1;
                    tracing::warn!(
                        code = "TIMELINE_ROW_DECODE_FAILED",
                        key_hex = %hex_encode(&key),
                        %error,
                        "timeline_purge left undecodable CF_TIMELINE row in place"
                    );
                }
            }
        }
        if !more {
            break;
        }
        let Some(last) = last_key.as_ref() else { break };
        next_start = key_after(last);
    }

    let matched_rows = u64::try_from(keys_to_delete.len()).unwrap_or(u64::MAX);
    let mut deleted_rows = 0_u64;
    let mut compacted = false;
    let mut audit_key_hex = None;
    if !params.dry_run {
        if let (Some(first), Some(last)) = (keys_to_delete.first(), keys_to_delete.last()) {
            let compact_start = first.clone();
            let compact_end = key_after(last);
            deleted_rows = matched_rows;
            runtime_guard
                .storage_delete_rows(cf::CF_TIMELINE, keys_to_delete)
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!(
                            "timeline_purge delete_batch failed; no audit row was written: {error}"
                        ),
                    )
                })?;
            runtime_guard
                .storage_compact_cf_range(cf::CF_TIMELINE, &compact_start, &compact_end)
                .map_err(|error| {
                    mcp_error(
                        error.code(),
                        format!(
                            "timeline_purge deleted {deleted_rows} rows but compacting the purged range failed: {error}"
                        ),
                    )
                })?;
            compacted = true;
        }
        let resume_cursor_pending = matches!(stopped_because, "scan_budget_exhausted");
        let audit_payload = json!({
            "deleted_rows": deleted_rows,
            "matched_rows": matched_rows,
            "scanned_rows": scanned_rows,
            "invalid_rows": invalid_rows,
            "protected_audit_rows": protected_audit_rows,
            "by_session": by_session,
            "continued_from_cursor": params.cursor.is_some(),
            "more_pending": resume_cursor_pending,
            "filters": {
                "start_ts_ns": params.start_ts_ns,
                "end_ts_ns": params.end_ts_ns,
                "apps": params.apps,
                "text": params.text,
                "kinds": params.kinds,
                "actor": params.actor,
                "all": params.all,
            },
        });
        audit_key_hex = Some(write_purge_audit_row(&runtime_guard, audit_payload)?);
    }
    drop(runtime_guard);

    let next_cursor = if stopped_because == "scan_budget_exhausted" && storage_has_more {
        last_key.as_deref().map(hex_encode)
    } else {
        None
    };
    tracing::info!(
        code = "TIMELINE_PURGE_COMPLETED",
        deleted_rows,
        matched_rows,
        scanned_rows,
        invalid_rows,
        protected_audit_rows,
        dry_run = params.dry_run,
        by_session,
        stopped_because,
        "timeline purge completed"
    );
    Ok(TimelinePurgeResponse {
        matched_rows,
        deleted_rows,
        scanned_rows,
        invalid_rows,
        protected_audit_rows,
        dry_run: params.dry_run,
        audit_key_hex,
        compacted,
        next_cursor,
        stopped_because: stopped_because.to_owned(),
    })
}

/// Writes the purge audit row with the pressure bypass (an audit obligation
/// must not shed), flushes it, and proves it by reading the exact key back.
fn write_purge_audit_row(
    runtime: &MutexGuard<'_, ReflexRuntime>,
    payload: Value,
) -> Result<String, ErrorData> {
    let ts_ns = now_ts_ns();
    let seq = PURGE_AUDIT_SEQ.fetch_add(1, Ordering::Relaxed);
    let key = timeline_codec::timeline_key(ts_ns, seq);
    let mut record = TimelineRecord::new(ts_ns, TimelineKind::Purge, TimelineActor::Human);
    record.payload = payload;
    let value = serde_json::to_vec(&record).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("encode timeline purge audit row: {error}"),
        )
    })?;
    runtime
        .storage_put_rows_pressure_bypass(cf::CF_TIMELINE, vec![(key.clone(), value)])
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("rows were purged but writing the purge audit row failed: {error}"),
            )
        })?;
    runtime.storage_flush().map_err(|error| {
        mcp_error(
            error.code(),
            format!("rows were purged but flushing the purge audit row failed: {error}"),
        )
    })?;
    // Full-state verification inside the tool: the audit row must be
    // physically present, not just acked.
    let (rows, _more) = runtime
        .storage_cf_rows_from(cf::CF_TIMELINE, &key, 1)
        .map_err(|error| mcp_error(error.code(), error.to_string()))?;
    if rows.first().map(|(row_key, _value)| row_key.as_slice()) != Some(key.as_slice()) {
        return Err(mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "rows were purged but the purge audit row is absent on readback",
        ));
    }
    Ok(hex_encode(&key))
}

fn now_ts_ns() -> u64 {
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
    u64::try_from(nanos).unwrap_or(0)
}

fn validate(params: &TimelineSearchParams) -> Result<Filters, ErrorData> {
    let start_ts_ns = params.start_ts_ns.unwrap_or(0);
    let end_ts_ns = params.end_ts_ns.unwrap_or(u64::MAX);
    if start_ts_ns > end_ts_ns {
        return Err(invalid(format!(
            "timeline_search start_ts_ns {start_ts_ns} must be <= end_ts_ns {end_ts_ns}"
        )));
    }
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(invalid(format!(
            "timeline_search limit must be between 1 and {MAX_LIMIT}; got {limit}"
        )));
    }
    let apps_lower = params
        .apps
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|app| {
            let trimmed = app.trim();
            if trimmed.is_empty() {
                Err(invalid("timeline_search apps entries must not be empty"))
            } else {
                Ok(trimmed.to_lowercase())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let text_lower = params
        .text
        .as_deref()
        .map(|text| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Err(invalid("timeline_search text must not be empty"));
            }
            if trimmed.len() > MAX_TEXT_FILTER_BYTES {
                return Err(invalid(format!(
                    "timeline_search text must be <= {MAX_TEXT_FILTER_BYTES} bytes"
                )));
            }
            Ok(trimmed.to_lowercase())
        })
        .transpose()?;
    let kinds = params
        .kinds
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|kind| parse_kind(kind))
        .collect::<Result<Vec<_>, _>>()?;
    let actor = params
        .actor
        .as_deref()
        .map(|actor| match actor.trim().to_lowercase().as_str() {
            "human" => Ok(ActorFilter::Human),
            "agent" => Ok(ActorFilter::Agent),
            other => Err(invalid(format!(
                "timeline_search actor must be \"human\" or \"agent\"; got {other:?}"
            ))),
        })
        .transpose()?;
    let start_key = match params.cursor.as_deref() {
        Some(cursor) => {
            let decoded = hex_decode(cursor).ok_or_else(|| {
                invalid("timeline_search cursor is not a valid hex key from a prior response")
            })?;
            key_after(&decoded)
        }
        None => timeline_codec::timeline_scan_start(start_ts_ns),
    };
    Ok(Filters {
        start_ts_ns,
        end_ts_ns,
        apps_lower,
        text_lower,
        kinds,
        actor,
        limit: limit as usize,
        start_key,
    })
}

fn parse_kind(raw: &str) -> Result<TimelineKind, ErrorData> {
    serde_json::from_value::<TimelineKind>(Value::String(raw.trim().to_owned())).map_err(|_error| {
        invalid(format!(
            "timeline_search kinds entry {raw:?} is not a known timeline kind"
        ))
    })
}

fn record_matches(record: &TimelineRecord, filters: &Filters) -> bool {
    if record.ts_ns < filters.start_ts_ns || record.ts_ns > filters.end_ts_ns {
        return false;
    }
    if !filters.kinds.is_empty() && !filters.kinds.contains(&record.kind) {
        return false;
    }
    if let Some(actor) = filters.actor {
        let is_human = matches!(record.actor, TimelineActor::Human);
        if (actor == ActorFilter::Human) != is_human {
            return false;
        }
    }
    if !filters.apps_lower.is_empty() {
        let Some(app) = record.app.as_deref() else {
            return false;
        };
        if !filters.apps_lower.contains(&app.to_lowercase()) {
            return false;
        }
    }
    if let Some(needle) = filters.text_lower.as_deref() {
        let in_app = record
            .app
            .as_deref()
            .is_some_and(|app| app.to_lowercase().contains(needle));
        if !in_app && !value_contains(&record.payload, needle) {
            return false;
        }
    }
    true
}

/// Case-insensitive substring search over every string value in a JSON tree.
fn value_contains(value: &Value, needle_lower: &str) -> bool {
    match value {
        Value::String(text) => text.to_lowercase().contains(needle_lower),
        Value::Array(items) => items.iter().any(|item| value_contains(item, needle_lower)),
        Value::Object(map) => map
            .values()
            .any(|entry| value_contains(entry, needle_lower)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn to_match(key: &[u8], seq: Option<u32>, record: TimelineRecord) -> TimelineSearchMatch {
    TimelineSearchMatch {
        key_hex: hex_encode(key),
        ts_ns: record.ts_ns,
        seq,
        kind: kind_name(record.kind),
        actor: match &record.actor {
            TimelineActor::Human => "human".to_owned(),
            TimelineActor::Agent { session_id } => format!("agent:{session_id}"),
        },
        app: record.app,
        payload: record.payload,
    }
}

fn kind_name(kind: TimelineKind) -> String {
    serde_json::to_value(kind).map_or_else(
        |_error| format!("{kind:?}"),
        |value| value.as_str().unwrap_or_default().to_owned(),
    )
}

fn key_after(key: &[u8]) -> Vec<u8> {
    let mut next = key.to_vec();
    next.push(0);
    next
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

fn invalid(detail: impl Into<String>) -> ErrorData {
    mcp_error(error_codes::TOOL_PARAMS_INVALID, detail.into())
}

fn lock_runtime(
    runtime: &Arc<Mutex<ReflexRuntime>>,
) -> Result<MutexGuard<'_, ReflexRuntime>, ErrorData> {
    runtime.lock().map_err(|_err| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            "reflex runtime lock poisoned",
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use synapse_core::types::{TimelineActor, TimelineKind, TimelineRecord};

    use super::*;

    fn record(ts_ns: u64, kind: TimelineKind, app: &str, payload: Value) -> TimelineRecord {
        let mut record = TimelineRecord::new(ts_ns, kind, TimelineActor::Human);
        record.app = Some(app.to_owned());
        record.payload = payload;
        record
    }

    fn filters() -> Filters {
        Filters {
            start_ts_ns: 0,
            end_ts_ns: u64::MAX,
            apps_lower: Vec::new(),
            text_lower: None,
            kinds: Vec::new(),
            actor: None,
            limit: 10,
            start_key: Vec::new(),
        }
    }

    #[test]
    fn text_filter_searches_nested_payload_strings_case_insensitively() {
        let row = record(
            5,
            TimelineKind::BrowserNav,
            "chrome.exe",
            json!({ "nav": { "url": "https://example.test/Quarterly-Report" } }),
        );
        let mut with_text = filters();
        with_text.text_lower = Some("quarterly-report".to_owned());
        assert!(record_matches(&row, &with_text));
        with_text.text_lower = Some("missing".to_owned());
        assert!(!record_matches(&row, &with_text));
    }

    #[test]
    fn app_kind_actor_and_time_filters_apply() {
        let row = record(50, TimelineKind::FocusChange, "Excel.EXE", Value::Null);
        let mut all = filters();
        all.apps_lower = vec!["excel.exe".to_owned()];
        all.kinds = vec![TimelineKind::FocusChange];
        all.actor = Some(ActorFilter::Human);
        all.start_ts_ns = 50;
        all.end_ts_ns = 50;
        assert!(record_matches(&row, &all));
        all.kinds = vec![TimelineKind::Clipboard];
        assert!(!record_matches(&row, &all));
        all.kinds = vec![TimelineKind::FocusChange];
        all.actor = Some(ActorFilter::Agent);
        assert!(!record_matches(&row, &all));
        all.actor = None;
        all.end_ts_ns = 49;
        assert!(!record_matches(&row, &all));
    }

    #[test]
    fn validate_rejects_bad_ranges_limits_kinds_actor_and_cursor() {
        let reject = |params: TimelineSearchParams, fragment: &str| {
            let error = validate(&params).expect_err(fragment);
            assert!(
                error.message.contains(fragment),
                "expected {fragment:?} in {:?}",
                error.message
            );
        };
        reject(
            TimelineSearchParams {
                start_ts_ns: Some(10),
                end_ts_ns: Some(5),
                ..TimelineSearchParams::default()
            },
            "must be <=",
        );
        reject(
            TimelineSearchParams {
                limit: Some(0),
                ..TimelineSearchParams::default()
            },
            "limit",
        );
        reject(
            TimelineSearchParams {
                kinds: Some(vec!["keylogger_dump".to_owned()]),
                ..TimelineSearchParams::default()
            },
            "not a known timeline kind",
        );
        reject(
            TimelineSearchParams {
                actor: Some("alien".to_owned()),
                ..TimelineSearchParams::default()
            },
            "actor",
        );
        reject(
            TimelineSearchParams {
                cursor: Some("zz-not-hex".to_owned()),
                ..TimelineSearchParams::default()
            },
            "cursor",
        );
    }

    #[test]
    fn cursor_roundtrips_and_resumes_after_key() {
        let key = synapse_storage::timeline::timeline_key(42, 7);
        let cursor = hex_encode(&key);
        let decoded = hex_decode(&cursor).expect("hex roundtrip");
        assert_eq!(decoded, key);
        let params = TimelineSearchParams {
            cursor: Some(cursor),
            ..TimelineSearchParams::default()
        };
        let filters = validate(&params).expect("cursor accepted");
        assert_eq!(filters.start_key, key_after(&key));
    }
}
