//! Server-side autonomous combat loop for the level-1 `EverQuest` wizard (#550).
//!
//! One MCP call runs many bounded game ticks (target -> consider -> cast ->
//! confirm kill -> recover -> retarget) so the agent does not pay a stdio
//! round-trip per keystroke. Every emitted key still flows through the audited
//! `act_keymap` action path and the #517 foreground/profile/scope/UI gates.
//! Looting is intentionally out of scope for the L1->L2 MVP; XP comes from
//! kills + sit-recover. The loop emits keymap aliases (`target_nearest_npc`,
//! `con`, `hotbar4`, `sit`) rather than free text.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rmcp::{ErrorData, schemars::JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::{Profile, error_codes};
use synapse_everquest::tail_log;
use tokio::time::sleep;

use super::{
    Json, Parameters, SynapseService, act_keymap_with_handle, everquest_log::EVERQUEST_PROFILE_ID,
    release_all_with_handles, tool, tool_router,
};
use crate::{
    m1::mcp_error,
    m2::{ActKeymapParams, PressBackend, ReleaseAllParams},
};

const TOOL: &str = "everquest_autocombat";
const SCHEMA_VERSION: u32 = 1;
const RUN_ROW_PREFIX: &str = "everquest/autocombat/v1/everquest.live";
const MAX_LOG_BYTES: usize = 64 * 1024;
const MAX_LOG_EVENTS: usize = 128;
const KEY_HOLD_MS: u32 = 33;
const CONSIDER_TIMEOUT: Duration = Duration::from_millis(1800);
const CAST_TIMEOUT: Duration = Duration::from_secs(8);
const RECOVER_TIMEOUT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(120);
const INTER_KEY_DELAY: Duration = Duration::from_millis(250);
const MAX_TARGET_CYCLES: u32 = 3;
const DEFAULT_HOTBAR_ALIAS: &str = "hotbar4";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActAutocombatParams {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_max_duration_s")]
    pub max_duration_s: u32,
    #[serde(default = "default_hp_floor")]
    pub hp_floor_percent: u32,
    #[serde(default = "default_mana_floor")]
    pub mana_floor_percent: u32,
    #[serde(default = "default_target_level_max")]
    pub target_level_max: u32,
    #[serde(default = "default_stop_at_level")]
    pub stop_at_level: u32,
    #[serde(default = "default_hotbar_alias")]
    pub hotbar_alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

const fn default_max_iterations() -> u32 {
    8
}
const fn default_max_duration_s() -> u32 {
    120
}
const fn default_hp_floor() -> u32 {
    50
}
const fn default_mana_floor() -> u32 {
    30
}
const fn default_target_level_max() -> u32 {
    2
}
const fn default_stop_at_level() -> u32 {
    2
}
fn default_hotbar_alias() -> String {
    DEFAULT_HOTBAR_ALIAS.to_owned()
}

/// Validated, clamped loop policy derived from `ActAutocombatParams`.
#[derive(Clone, Debug)]
struct Policy {
    max_iterations: u32,
    max_duration: Duration,
    hp_floor: u32,
    mana_floor: u32,
    target_level_max: u32,
    stop_at_level: u32,
    hotbar_alias: String,
    run_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ActAutocombatIteration {
    pub index: u32,
    pub target_summary: Option<String>,
    pub target_level: Option<u32>,
    pub con_decision: String,
    pub cast: bool,
    pub outcome: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ActAutocombatResponse {
    pub ok: bool,
    pub iterations: u32,
    pub kills: u32,
    pub casts: u32,
    pub casts_resisted: u32,
    pub casts_fizzled: u32,
    pub started_level: Option<u32>,
    pub final_level: Option<u32>,
    pub final_xp_percent: Option<u32>,
    pub stop_reason: String,
    pub run_row_key: String,
    pub looting_note: String,
    pub per_iteration: Vec<ActAutocombatIteration>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AutocombatRunRow {
    schema_version: u32,
    row_kind: String,
    profile_id: String,
    run_id: String,
    generated_at: DateTime<Utc>,
    response: ActAutocombatResponse,
}

/// Distinct, machine-readable reasons the loop halts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopReason {
    ReachedTargetLevel,
    MaxIterations,
    MaxDuration,
    OperatorPanic,
    ForegroundLost,
    ChatUnsafe,
    HpFloor,
    NoSafeTarget,
}

impl StopReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReachedTargetLevel => "reached_target_level",
            Self::MaxIterations => "max_iterations",
            Self::MaxDuration => "max_duration",
            Self::OperatorPanic => "operator_panic",
            Self::ForegroundLost => "foreground_lost",
            Self::ChatUnsafe => "chat_unsafe",
            Self::HpFloor => "hp_floor",
            Self::NoSafeTarget => "no_safe_target",
        }
    }
    const fn is_success(self) -> bool {
        matches!(
            self,
            Self::ReachedTargetLevel | Self::MaxIterations | Self::MaxDuration
        )
    }
}

/// Consider-line classification for a level-1 wizard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConDecision {
    Safe,
    TooHigh,
    NonNpc,
    Unknown,
}

impl ConDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::TooHigh => "too_high",
            Self::NonNpc => "non_npc",
            Self::Unknown => "unknown",
        }
    }
}

/// Parsed result of polling the EQ log after a cast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CastOutcome {
    Slain,
    Resisted,
    Fizzled,
    OutOfRange,
    NoOutcome,
}

impl CastOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Slain => "slain",
            Self::Resisted => "resisted",
            Self::Fizzled => "fizzled",
            Self::OutOfRange => "out_of_range",
            Self::NoOutcome => "no_outcome",
        }
    }
}

#[tool_router(router = everquest_autocombat_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Run a bounded, operator-attended, server-side EverQuest combat loop for the level-1 wizard (target -> consider -> cast -> confirm -> recover)"
    )]
    pub async fn everquest_autocombat(
        &self,
        params: Parameters<ActAutocombatParams>,
    ) -> Result<Json<ActAutocombatResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = TOOL,
            "tool.invocation kind=everquest_autocombat"
        );
        let policy = normalize_policy(params.0);
        let request_details = json!({ "run_id": policy.run_id, "policy": policy_details(&policy) });
        let profile = match self.autocombat_preflight() {
            Ok(profile) => profile,
            Err(error) => {
                self.audit_action_denied_with_details(TOOL, &error, &request_details);
                return Err(error);
            }
        };
        self.audit_action_started_with_details(TOOL, &request_details)?;
        let result = self.run_autocombat_loop(&policy, &profile).await;
        if let Ok(response) = &result {
            let _ = self.persist_autocombat_run(&policy, response);
        }
        self.audit_action_result(TOOL, &result)?;
        result.map(Json)
    }
}

impl SynapseService {
    fn autocombat_preflight(&self) -> Result<Profile, ErrorData> {
        self.ensure_supported_use_allows_action(TOOL)?;
        self.ensure_active_everquest_profile(TOOL)?;
        self.ensure_literal_command_chat_guard(TOOL, "/autocombat")?;
        self.resolve_active_everquest_log()
            .map_err(|detail| mcp_error(error_codes::ACTION_TARGET_INVALID, detail))?;
        let runtime = self.profile_runtime()?;
        runtime
            .profile(EVERQUEST_PROFILE_ID)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::PROFILE_NOT_FOUND,
                    format!("active profile {EVERQUEST_PROFILE_ID} was not found"),
                )
            })
    }

    async fn run_autocombat_loop(
        &self,
        policy: &Policy,
        profile: &Profile,
    ) -> Result<ActAutocombatResponse, ErrorData> {
        let started = Instant::now();
        let panic_epoch = synapse_action::operator_release_epoch();
        let started_level = self.read_level();
        let mut state = LoopState::default();
        let mut stop = StopReason::MaxIterations;
        for index in 0..policy.max_iterations {
            if let Some(reason) = self.evaluate_stop(policy, panic_epoch, started) {
                stop = reason;
                self.handle_stop_recovery(reason, profile).await;
                break;
            }
            let iteration = self.run_iteration(index, policy, profile, &mut state).await?;
            let is_kill = iteration.outcome == CastOutcome::Slain.as_str();
            state.iterations.push(iteration);
            if is_kill && self.read_level().is_some_and(|lvl| lvl >= policy.stop_at_level) {
                stop = StopReason::ReachedTargetLevel;
                break;
            }
            if state.consecutive_no_target >= MAX_TARGET_CYCLES {
                stop = StopReason::NoSafeTarget;
                break;
            }
        }
        if started.elapsed() >= policy.max_duration && stop == StopReason::MaxIterations {
            stop = StopReason::MaxDuration;
        }
        Ok(self.finalize(policy, started_level, stop, state))
    }

    /// Stop conditions checked BEFORE emitting any input each iteration.
    fn evaluate_stop(
        &self,
        policy: &Policy,
        panic_epoch: u64,
        started: Instant,
    ) -> Option<StopReason> {
        if synapse_action::operator_release_requested_since(panic_epoch) {
            return Some(StopReason::OperatorPanic);
        }
        if started.elapsed() >= policy.max_duration {
            return Some(StopReason::MaxDuration);
        }
        let row = self.build_survival_readiness_row().ok()?;
        if !row.foreground.is_everquest_foreground {
            return Some(StopReason::ForegroundLost);
        }
        if row.ui_context.login_screen_visible || !chat_safe(&row.chat_input_state.decision) {
            return Some(StopReason::ChatUnsafe);
        }
        if row.hud.hp_percent.is_some_and(|hp| hp < policy.hp_floor) {
            return Some(StopReason::HpFloor);
        }
        None
    }

    async fn handle_stop_recovery(&self, reason: StopReason, profile: &Profile) {
        if matches!(reason, StopReason::OperatorPanic) {
            let _ = self.release_all_best_effort().await;
        } else if matches!(reason, StopReason::HpFloor) {
            let _ = self.press_alias("sit", profile).await;
        }
    }

    async fn run_iteration(
        &self,
        index: u32,
        policy: &Policy,
        profile: &Profile,
        state: &mut LoopState,
    ) -> Result<ActAutocombatIteration, ErrorData> {
        self.press_alias("target_nearest_npc", profile).await?;
        sleep(INTER_KEY_DELAY).await;
        let log_path = self.autocombat_log_path()?;
        let offset = file_len(&log_path);
        self.press_alias("con", profile).await?;
        let consider = self.poll_consider(&log_path, offset).await;
        let decision = classify_con(consider.as_deref(), policy.target_level_max);
        let target_level = parse_target_level(consider.as_deref());
        let mut iteration = ActAutocombatIteration {
            index,
            target_summary: consider.clone(),
            target_level,
            con_decision: decision.as_str().to_owned(),
            cast: false,
            outcome: CastOutcome::NoOutcome.as_str().to_owned(),
        };
        if decision != ConDecision::Safe {
            state.consecutive_no_target += 1;
            return Ok(iteration);
        }
        state.consecutive_no_target = 0;
        iteration.cast = true;
        state.casts += 1;
        let cast_offset = file_len(&log_path);
        self.press_alias(&policy.hotbar_alias, profile).await?;
        let outcome = self.poll_cast_outcome(&log_path, cast_offset).await;
        outcome.as_str().clone_into(&mut iteration.outcome);
        self.apply_outcome(outcome, policy, profile, state).await;
        Ok(iteration)
    }

    async fn apply_outcome(
        &self,
        outcome: CastOutcome,
        policy: &Policy,
        profile: &Profile,
        state: &mut LoopState,
    ) {
        match outcome {
            CastOutcome::Slain => {
                state.kills += 1;
                self.recover_mana(policy, profile).await;
            }
            CastOutcome::Resisted => state.resisted += 1,
            CastOutcome::Fizzled => state.fizzled += 1,
            CastOutcome::OutOfRange | CastOutcome::NoOutcome => {}
        }
    }

    /// Sit to recover mana to the floor, bounded by `RECOVER_TIMEOUT`, then stand.
    async fn recover_mana(&self, policy: &Policy, profile: &Profile) {
        if self.press_alias("sit", profile).await.is_err() {
            return;
        }
        let started = Instant::now();
        let panic_epoch = synapse_action::operator_release_epoch();
        while started.elapsed() < RECOVER_TIMEOUT {
            if synapse_action::operator_release_requested_since(panic_epoch) {
                let _ = self.release_all_best_effort().await;
                return;
            }
            let mana = self
                .build_survival_readiness_row()
                .ok()
                .and_then(|row| row.hud.mana_percent);
            if mana.is_some_and(|value| value >= policy.mana_floor) {
                break;
            }
            sleep(POLL_INTERVAL).await;
        }
        let _ = self.press_alias("sit", profile).await;
    }

    async fn poll_consider(&self, log_path: &std::path::Path, offset: u64) -> Option<String> {
        let started = Instant::now();
        while started.elapsed() < CONSIDER_TIMEOUT {
            if let Ok(batch) = tail_log(log_path, offset, MAX_LOG_BYTES, MAX_LOG_EVENTS)
                && let Some(summary) = consider_summary(&batch)
            {
                return Some(summary);
            }
            sleep(POLL_INTERVAL).await;
        }
        None
    }

    async fn poll_cast_outcome(&self, log_path: &std::path::Path, offset: u64) -> CastOutcome {
        let started = Instant::now();
        let mut last = CastOutcome::NoOutcome;
        while started.elapsed() < CAST_TIMEOUT {
            if let Ok(batch) = tail_log(log_path, offset, MAX_LOG_BYTES, MAX_LOG_EVENTS) {
                let summaries: Vec<&str> =
                    batch.events.iter().map(|event| event.summary.as_str()).collect();
                last = classify_cast_outcome(&summaries);
                if matches!(last, CastOutcome::Slain | CastOutcome::Resisted | CastOutcome::Fizzled)
                {
                    return last;
                }
            }
            sleep(POLL_INTERVAL).await;
        }
        last
    }

    async fn press_alias(&self, alias: &str, profile: &Profile) -> Result<(), ErrorData> {
        let (handle, recording, cancel) = self.m2_action_context()?;
        let params = ActKeymapParams {
            alias: alias.to_owned(),
            hold_ms: KEY_HOLD_MS,
            backend: PressBackend::Auto,
        };
        act_keymap_with_handle(handle, recording, cancel, profile, params)
            .await
            .map(|_response| ())
    }

    async fn release_all_best_effort(&self) -> Result<(), ErrorData> {
        let (handle, snapshot) = self.m2_release_all_context()?;
        release_all_with_handles(handle, snapshot, ReleaseAllParams {})
            .await
            .map(|_response| ())
    }

    fn read_level(&self) -> Option<u32> {
        self.build_survival_readiness_row()
            .ok()
            .and_then(|row| parse_level(row.hud.level_raw.as_deref()))
    }

    fn autocombat_log_path(&self) -> Result<std::path::PathBuf, ErrorData> {
        self.resolve_active_everquest_log()
            .map(|active| active.log.path)
            .map_err(|detail| mcp_error(error_codes::ACTION_TARGET_INVALID, detail))
    }

    fn finalize(
        &self,
        policy: &Policy,
        started_level: Option<u32>,
        stop: StopReason,
        state: LoopState,
    ) -> ActAutocombatResponse {
        let final_row = self.build_survival_readiness_row().ok();
        let final_level = final_row
            .as_ref()
            .and_then(|row| parse_level(row.hud.level_raw.as_deref()));
        ActAutocombatResponse {
            ok: stop.is_success(),
            iterations: u32::try_from(state.iterations.len()).unwrap_or(u32::MAX),
            kills: state.kills,
            casts: state.casts,
            casts_resisted: state.resisted,
            casts_fizzled: state.fizzled,
            started_level,
            final_level,
            final_xp_percent: None,
            stop_reason: stop.as_str().to_owned(),
            run_row_key: format!("{RUN_ROW_PREFIX}/{}", policy.run_id),
            looting_note: "Looting is out of scope for the L1->L2 MVP; XP comes from kills + sit-recover.".to_owned(),
            per_iteration: state.iterations,
        }
    }

    fn persist_autocombat_run(
        &self,
        policy: &Policy,
        response: &ActAutocombatResponse,
    ) -> Result<(), ErrorData> {
        let row = AutocombatRunRow {
            schema_version: SCHEMA_VERSION,
            row_kind: "everquest_autocombat_run".to_owned(),
            profile_id: EVERQUEST_PROFILE_ID.to_owned(),
            run_id: policy.run_id.clone(),
            generated_at: Utc::now(),
            response: response.clone(),
        };
        let key = response.run_row_key.clone();
        let encoded = serde_json::to_vec(&row).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode autocombat run row: {error}"),
            )
        })?;
        let runtime = self.reflex_runtime()?;
        let runtime = runtime.lock().map_err(|_error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                "reflex runtime lock poisoned while writing autocombat run row",
            )
        })?;
        let result = runtime
            .storage_put_kv_rows(vec![(key.into_bytes(), encoded)])
            .map_err(|error| mcp_error(error_codes::STORAGE_WRITE_FAILED, error.to_string()));
        drop(runtime);
        result
    }
}

#[derive(Debug, Default)]
struct LoopState {
    iterations: Vec<ActAutocombatIteration>,
    kills: u32,
    casts: u32,
    resisted: u32,
    fizzled: u32,
    consecutive_no_target: u32,
}

fn normalize_policy(params: ActAutocombatParams) -> Policy {
    Policy {
        max_iterations: params.max_iterations.clamp(1, 50),
        max_duration: Duration::from_secs(u64::from(params.max_duration_s.clamp(1, 600))),
        hp_floor: params.hp_floor_percent.min(100),
        mana_floor: params.mana_floor_percent.min(100),
        target_level_max: params.target_level_max,
        stop_at_level: params.stop_at_level.max(1),
        hotbar_alias: normalize_alias(&params.hotbar_alias),
        run_id: params.idempotency_key.map_or_else(default_run_id, |value| {
            sanitize_run_id(&value)
        }),
    }
}

fn normalize_alias(alias: &str) -> String {
    let trimmed = alias.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        DEFAULT_HOTBAR_ALIAS.to_owned()
    } else {
        trimmed
    }
}

fn default_run_id() -> String {
    format!("run-{}", Utc::now().format("%Y%m%dT%H%M%S%3fZ"))
}

fn sanitize_run_id(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .take(64)
        .collect();
    if cleaned.is_empty() {
        default_run_id()
    } else {
        cleaned
    }
}

fn policy_details(policy: &Policy) -> serde_json::Value {
    json!({
        "max_iterations": policy.max_iterations,
        "max_duration_s": policy.max_duration.as_secs(),
        "hp_floor_percent": policy.hp_floor,
        "mana_floor_percent": policy.mana_floor,
        "target_level_max": policy.target_level_max,
        "stop_at_level": policy.stop_at_level,
        "hotbar_alias": policy.hotbar_alias,
    })
}

fn chat_safe(decision: &str) -> bool {
    decision == "allow_empty_chat_input"
}

fn file_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

fn consider_summary(batch: &synapse_everquest::EverQuestLogTailBatch) -> Option<String> {
    batch
        .events
        .iter()
        .rev()
        .find(|event| {
            event.kind == synapse_everquest::EverQuestLogKind::Consider
                || event.summary.to_ascii_lowercase().contains("regards you")
        })
        .map(|event| event.summary.clone())
}

/// Parse the target level from a consider summary (`(Lvl: N)` or `... level N`).
fn parse_target_level(summary: Option<&str>) -> Option<u32> {
    let text = summary?.to_ascii_lowercase();
    if let Some(rest) = text.split("lvl:").nth(1) {
        return rest.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    }
    if let Some(rest) = text.split("level ").nth(1) {
        return rest.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok();
    }
    None
}

/// Parse the character level from the HUD level-raw OCR string.
fn parse_level(level_raw: Option<&str>) -> Option<u32> {
    level_raw?
        .split_ascii_whitespace()
        .find_map(|token| token.parse::<u32>().ok())
}

/// Classify a consider line for a level-1 wizard. Safe = NPC, level within cap,
/// and the con phrase is not in the high-danger set.
fn classify_con(summary: Option<&str>, target_level_max: u32) -> ConDecision {
    let Some(text) = summary else {
        return ConDecision::Unknown;
    };
    let lower = text.to_ascii_lowercase();
    if lower.contains("merchant")
        || lower.contains(" player")
        || lower.contains("a player")
        || lower.contains("guard")
    {
        return ConDecision::NonNpc;
    }
    // "Red" cons mean the target is far above a level-1 wizard regardless of any
    // parsed level (and cover the no-level-parsed case); reject outright.
    if con_phrase_red(&lower) {
        return ConDecision::TooHigh;
    }
    // The absolute level is the primary safety gate. The con difficulty phrase
    // ("gamble" = yellow/even, "even fight" = white, "easy prey" = green) only
    // reflects RELATIVE level, which the absolute cap already bounds — so a
    // level-<=cap NPC is huntable for a ranged nuker even at a yellow ("gamble")
    // con. HP-floor flee + the operator panic hotkey remain the lethality guard.
    match parse_target_level(summary) {
        Some(level) if level > target_level_max => ConDecision::TooHigh,
        Some(_) => ConDecision::Safe,
        None => {
            if con_phrase_safe(&lower) {
                ConDecision::Safe
            } else {
                ConDecision::Unknown
            }
        }
    }
}

/// True only for cons that mean the target is much higher level than a level-1
/// wizard. "gamble" (yellow/even) and faction-hostility phrases are intentionally
/// NOT here — the absolute `target_level_max` gate handles level safety.
fn con_phrase_red(lower: &str) -> bool {
    lower.contains("crazy to attack")
        || lower.contains("kill you")
        || lower.contains("rip you")
        || lower.contains("deadly")
}

fn con_phrase_safe(lower: &str) -> bool {
    lower.contains("regards you indifferently")
        || lower.contains("looks upon you warmly")
        || lower.contains("even fight")
        || lower.contains("gamble")
        || lower.contains("easy prey")
        || lower.contains("afraid")
        || lower.contains("worthy opponent")
}

/// Classify the cast outcome from a window of log summaries (newest wins).
fn classify_cast_outcome(summaries: &[&str]) -> CastOutcome {
    for summary in summaries.iter().rev() {
        let lower = summary.to_ascii_lowercase();
        if lower.contains("has been slain by") {
            return CastOutcome::Slain;
        }
        if lower.contains("resist") {
            return CastOutcome::Resisted;
        }
        if lower.contains("fizzle") {
            return CastOutcome::Fizzled;
        }
        if lower.contains("too far away") || lower.contains("out of range") {
            return CastOutcome::OutOfRange;
        }
    }
    CastOutcome::NoOutcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_safe_indifferent_low_level() {
        let line = "An araneidae spiderling regards you indifferently -- looks like an even fight. (Lvl: 1)";
        assert_eq!(classify_con(Some(line), 2), ConDecision::Safe);
    }

    #[test]
    fn classifies_gamble_level_two_within_cap_as_safe() {
        // Yellow ("gamble") con on a neutral-faction Lvl-2 NPC, cap 2: huntable
        // for a ranged nuker — the absolute level cap is the gate, not the phrase.
        let line = "A garter snake regards you indifferently -- looks like quite a gamble. (Lvl: 2)";
        assert_eq!(classify_con(Some(line), 2), ConDecision::Safe);
    }

    #[test]
    fn classifies_gamble_level_three_over_cap_as_too_high() {
        // Same yellow con but Lvl 3 > cap 2 -> rejected by the absolute level gate.
        let line = "An araneidae spiderling regards you indifferently -- looks like quite a gamble. (Lvl: 3)";
        assert_eq!(classify_con(Some(line), 2), ConDecision::TooHigh);
    }

    #[test]
    fn classifies_red_con_as_too_high_regardless_of_level() {
        let line = "An ancient wurm glares at you, ready to attack -- you would have to be crazy to attack it!";
        assert_eq!(classify_con(Some(line), 2), ConDecision::TooHigh);
    }

    #[test]
    fn classifies_level_over_cap_as_too_high() {
        let line = "consider a decaying skeleton level 5";
        assert_eq!(classify_con(Some(line), 2), ConDecision::TooHigh);
    }

    #[test]
    fn classifies_merchant_and_player_as_non_npc() {
        assert_eq!(
            classify_con(Some("Merchant Kinliat regards you indifferently. (Lvl: 1)"), 2),
            ConDecision::NonNpc
        );
        assert_eq!(
            classify_con(Some("Thenumberone a player regards you. (Lvl: 1)"), 2),
            ConDecision::NonNpc
        );
    }

    #[test]
    fn unknown_when_no_summary() {
        assert_eq!(classify_con(None, 2), ConDecision::Unknown);
    }

    #[test]
    fn parses_consider_levels() {
        assert_eq!(parse_target_level(Some("looks like a gamble. (Lvl: 3)")), Some(3));
        assert_eq!(parse_target_level(Some("consider a skeleton level 5")), Some(5));
        assert_eq!(parse_target_level(Some("no level here")), None);
    }

    #[test]
    fn parses_hud_level() {
        assert_eq!(parse_level(Some("Inventory Thenumberone 1 Wizard")), Some(1));
        assert_eq!(parse_level(Some("Thenumberone 2 Wizard")), Some(2));
        assert_eq!(parse_level(None), None);
    }

    #[test]
    fn classifies_cast_outcomes_from_log_summaries() {
        assert_eq!(
            classify_cast_outcome(&["a decaying skeleton has been slain by Thenumberone!"]),
            CastOutcome::Slain
        );
        assert_eq!(
            classify_cast_outcome(&["Your target resisted the Blast of Cold spell!"]),
            CastOutcome::Resisted
        );
        assert_eq!(
            classify_cast_outcome(&["Your Blast of Cold spell fizzles!"]),
            CastOutcome::Fizzled
        );
        assert_eq!(
            classify_cast_outcome(&["Your target is too far away."]),
            CastOutcome::OutOfRange
        );
        assert_eq!(classify_cast_outcome(&["You begin casting Blast of Cold."]), CastOutcome::NoOutcome);
    }

    #[test]
    fn newest_slain_wins_over_earlier_resist() {
        let summaries = [
            "Your target resisted the Blast of Cold spell!",
            "a decaying skeleton has been slain by Thenumberone!",
        ];
        assert_eq!(classify_cast_outcome(&summaries), CastOutcome::Slain);
    }

    #[test]
    fn stop_reason_success_classification() {
        assert!(StopReason::ReachedTargetLevel.is_success());
        assert!(StopReason::MaxIterations.is_success());
        assert!(!StopReason::HpFloor.is_success());
        assert!(!StopReason::OperatorPanic.is_success());
        assert!(!StopReason::ForegroundLost.is_success());
    }

    #[test]
    fn policy_clamps_bounds() {
        let policy = normalize_policy(ActAutocombatParams {
            max_iterations: 999,
            max_duration_s: 9999,
            hp_floor_percent: 200,
            mana_floor_percent: 200,
            target_level_max: 2,
            stop_at_level: 0,
            hotbar_alias: "  HOTBAR4 ".to_owned(),
            idempotency_key: Some("run/with:bad chars!".to_owned()),
        });
        assert_eq!(policy.max_iterations, 50);
        assert_eq!(policy.max_duration.as_secs(), 600);
        assert_eq!(policy.hp_floor, 100);
        assert_eq!(policy.mana_floor, 100);
        assert_eq!(policy.stop_at_level, 1);
        assert_eq!(policy.hotbar_alias, "hotbar4");
        assert_eq!(policy.run_id, "runwithbadchars");
    }
}
