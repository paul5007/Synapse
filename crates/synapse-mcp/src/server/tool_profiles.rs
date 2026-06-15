use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use rmcp::{ErrorData, RoleServer, model::ErrorCode, model::Tool, service::RequestContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use synapse_action::lease;
use synapse_core::error_codes;
use synapse_storage::cf;

use super::{Json, Parameters, SynapseService, empty_input_schema, mcp_error, tool, tool_router};

const TOOL_PROFILE_PREFIX: &str = "mcp/tool-profile/v1/";
const TOOL_PROFILE_SOURCE_OF_TRUTH: &str = "CF_SESSIONS mcp/tool-profile/v1/<session_id>";
const TOOL_PROFILE_ROW_KIND: &str = "mcp_tool_profile";
const TOOL_PROFILE_SCHEMA_VERSION: u32 = 1;
const MAX_PROFILE_REASON_CHARS: usize = 1024;

const NORMAL_ALLOWED_EXACT: &[&str] = &[
    "act_run_shell",
    "act_run_shell_cancel",
    "act_run_shell_start",
    "act_run_shell_status",
    "act_spawn_agent",
    "agent_cost",
    "agent_inbox",
    "agent_interrupt",
    "agent_kill",
    "agent_query",
    "agent_receipts",
    "agent_send",
    "agent_send_broadcast",
    "agent_stats",
    "agent_wait",
    "approval_decide",
    "approval_gate",
    "approval_list",
    "approval_request",
    "audit_intelligence_query",
    "capture_screenshot",
    "cdp_bridge_reload",
    "cdp_close_tab",
    "cdp_navigate_tab",
    "cdp_open_tab",
    "cdp_target_info",
    "clear_target",
    "control_lease_acquire",
    "control_lease_handoff",
    "control_lease_release",
    "control_lease_status",
    "escalation_ack",
    "escalation_list",
    "find",
    "fleet_stop",
    "get_target",
    "health",
    "hygiene_flags",
    "hygiene_scan_storage",
    "hygiene_scan_text",
    "local_model_list",
    "local_model_probe",
    "local_model_register",
    "local_model_remove",
    "local_model_update",
    "observe",
    "observe_delta",
    "profile_list",
    "read_text",
    "reality_audit",
    "reality_baseline",
    "session_end",
    "session_list",
    "session_status",
    "set_capture_target",
    "set_perception_mode",
    "set_target",
    "storage_inspect",
    "target_claim",
    "target_claim_adopt",
    "target_claim_status",
    "target_release",
    "timeline_digest",
    "timeline_get",
    "timeline_search",
    "timeline_stats",
    "tool_profile_set",
    "tool_profile_status",
    "workspace_get",
    "workspace_list",
    "workspace_put",
    "workspace_subscribe",
];

const NORMAL_ALLOWED_PREFIXES: &[&str] = &["agent_template_", "task_"];

const BROWSER_CONTROL_ALLOWED_EXACT: &[&str] = &[
    "approval_list",
    "capture_screenshot",
    "cdp_bridge_reload",
    "cdp_close_tab",
    "cdp_navigate_tab",
    "cdp_open_tab",
    "cdp_target_info",
    "clear_target",
    "control_lease_status",
    "escalation_list",
    "find",
    "get_target",
    "health",
    "observe",
    "observe_delta",
    "read_text",
    "reality_audit",
    "reality_baseline",
    "session_end",
    "session_list",
    "session_status",
    "set_capture_target",
    "set_perception_mode",
    "set_target",
    "storage_inspect",
    "target_claim",
    "target_claim_adopt",
    "target_claim_status",
    "target_release",
    "tool_profile_set",
    "tool_profile_status",
    "workspace_get",
    "workspace_list",
    "workspace_put",
    "workspace_subscribe",
];

const BREAK_GLASS_HAZARDOUS_TOOLS: &[&str] = &[
    "act_click",
    "act_clipboard",
    "act_combo",
    "act_focus_window",
    "act_keymap",
    "act_launch",
    "act_pad",
    "act_press",
    "act_scroll",
    "act_set_field_text",
    "act_set_value",
    "act_stroke",
    "act_type",
    "action_diagnostic_queue_full_setup",
    "action_diagnostic_rate_limit_override",
    "hidden_desktop_pip_frame",
    "release_all",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolProfileKind {
    NormalAgent,
    BrowserControl,
    BreakGlass,
}

impl ToolProfileKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::NormalAgent => "normal_agent",
            Self::BrowserControl => "browser_control",
            Self::BreakGlass => "break_glass",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::NormalAgent => "normal_agent",
            Self::BrowserControl => "dashboard/browser-control task",
            Self::BreakGlass => "break-glass/admin",
        }
    }

    fn is_visible(self, tool_name: &str) -> bool {
        match self {
            Self::BreakGlass => true,
            Self::NormalAgent => {
                NORMAL_ALLOWED_EXACT.contains(&tool_name)
                    || NORMAL_ALLOWED_PREFIXES
                        .iter()
                        .any(|prefix| tool_name.starts_with(prefix))
            }
            Self::BrowserControl => BROWSER_CONTROL_ALLOWED_EXACT.contains(&tool_name),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct PersistedToolProfile {
    schema_version: u32,
    row_kind: String,
    session_id: String,
    profile: ToolProfileKind,
    source: String,
    reason: Option<String>,
    set_by_session_id: Option<String>,
    stored_at_unix_ms: u64,
    allowed_tool_count: usize,
    allowed_tool_sha256: String,
    denied_break_glass_tools: Vec<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileAssignment {
    pub schema_version: u32,
    pub row_kind: String,
    pub session_id: String,
    pub profile: ToolProfileKind,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_by_session_id: Option<String>,
    pub stored_at_unix_ms: u64,
    pub allowed_tool_count: usize,
    pub allowed_tool_sha256: String,
    pub denied_break_glass_tools: Vec<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileRowReadback {
    pub cf_name: &'static str,
    pub key_hex: String,
    pub value_len_bytes: u64,
    pub value_sha256: String,
    pub record: ToolProfileAssignment,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileAuditReadback {
    pub cf_name: &'static str,
    pub key_hex: String,
    pub value_len_bytes: u64,
    pub value_sha256: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileSnapshot {
    pub source_of_truth: &'static str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub profile: ToolProfileKind,
    pub profile_label: &'static str,
    pub source: String,
    pub implementation_tool_count: usize,
    pub visible_tool_count: usize,
    pub visible_tool_sha256: String,
    pub visible_tool_names: Vec<String>,
    pub denied_break_glass_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_row: Option<ToolProfileRowReadback>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileStatusResponse {
    pub snapshot: ToolProfileSnapshot,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileSetParams {
    pub profile: ToolProfileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    #[schemars(default)]
    pub confirm_break_glass: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileLeaseProof {
    pub required: bool,
    pub held: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
    pub caller_is_owner: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolProfileSetResponse {
    pub before: ToolProfileSnapshot,
    pub after: ToolProfileSnapshot,
    pub row_readback: ToolProfileRowReadback,
    pub intent_audit: ToolProfileAuditReadback,
    pub final_audit: ToolProfileAuditReadback,
    pub lease_proof: ToolProfileLeaseProof,
}

#[tool_router(router = tool_profile_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Read this MCP session's effective tool profile, visible tools/list names, and durable CF_SESSIONS policy row. This is the Source of Truth for why foreground-prone break-glass tools are visible or hidden.",
        input_schema = empty_input_schema()
    )]
    pub async fn tool_profile_status(
        &self,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolProfileStatusResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "tool_profile_status",
            "tool.invocation kind=tool_profile_status"
        );
        let session_id = super::context::mcp_session_id_from_request_context(&request_context)?;
        Ok(Json(ToolProfileStatusResponse {
            snapshot: self.tool_profile_snapshot(session_id.as_deref())?,
        }))
    }

    #[tool(
        description = "Set this MCP session's durable tool profile. normal_agent and browser_control keep raw foreground primitives hidden; break_glass exposes the full raw surface only when confirm_break_glass=true, reason is non-empty, and this session currently owns the foreground input lease."
    )]
    pub async fn tool_profile_set(
        &self,
        params: Parameters<ToolProfileSetParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<ToolProfileSetResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "tool_profile_set",
            "tool.invocation kind=tool_profile_set"
        );
        let session_id = super::context::mcp_session_id_from_request_context(&request_context)?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::HTTP_SESSION_INVALID,
                    "tool_profile_set requires an MCP session id so the policy decision can be persisted",
                )
            })?;
        let params = params.0;
        let reason = normalize_reason(params.reason.as_deref())?;
        let before = self.tool_profile_snapshot(Some(&session_id))?;
        let lease_proof = break_glass_lease_proof(&session_id, params.profile);
        let command_payload = json!({
            "requested_profile": params.profile.as_str(),
            "reason": reason,
            "confirm_break_glass": params.confirm_break_glass,
        });
        let command_before = json!({
            "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
            "before_profile": before.profile.as_str(),
            "before_visible_tool_count": before.visible_tool_count,
            "lease_proof": lease_proof,
        });
        let intent_audit = audit_readback(self.command_audit_intent(
            super::command_audit::CommandAuditInput::mcp(
                "tool_profile_set",
                "profile_set",
                Some(session_id.clone()),
                Some(session_id.clone()),
                command_payload.clone(),
                command_before.clone(),
                Value::Null,
                "pending",
            ),
        )?);

        if let Err(error) = validate_profile_set_policy(
            &session_id,
            params.profile,
            reason.as_deref(),
            params.confirm_break_glass,
            &lease_proof,
        ) {
            let final_audit = self.command_audit_final(
                super::command_audit::CommandAuditInput::mcp(
                    "tool_profile_set",
                    "profile_set",
                    Some(session_id.clone()),
                    Some(session_id.clone()),
                    command_payload,
                    command_before,
                    json!({
                        "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
                        "after_profile": before.profile.as_str(),
                        "lease_proof": lease_proof,
                    }),
                    "error",
                )
                .with_error(
                    super::command_audit::command_audit_error_from_error_data(&error),
                ),
            )?;
            let _final_audit = audit_readback(final_audit);
            return Err(error);
        }

        let row_readback = match self.write_tool_profile_assignment(
            &session_id,
            params.profile,
            "tool_profile_set",
            reason.clone(),
            Some(session_id.clone()),
        ) {
            Ok(row) => row,
            Err(error) => {
                let final_audit = self.command_audit_final(
                    super::command_audit::CommandAuditInput::mcp(
                        "tool_profile_set",
                        "profile_set",
                        Some(session_id.clone()),
                        Some(session_id.clone()),
                        command_payload,
                        command_before,
                        json!({
                            "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
                            "after_profile": before.profile.as_str(),
                            "lease_proof": lease_proof,
                        }),
                        "error",
                    )
                    .with_error(
                        super::command_audit::command_audit_error_from_error_data(&error),
                    ),
                )?;
                let _final_audit = audit_readback(final_audit);
                return Err(error);
            }
        };
        let after = self.tool_profile_snapshot(Some(&session_id))?;
        let final_audit = audit_readback(self.command_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "tool_profile_set",
                "profile_set",
                Some(session_id.clone()),
                Some(session_id),
                command_payload,
                command_before,
                json!({
                    "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
                    "after_profile": after.profile.as_str(),
                    "after_visible_tool_count": after.visible_tool_count,
                    "row_readback": row_readback,
                    "lease_proof": lease_proof,
                }),
                "ok",
            ),
        )?);

        Ok(Json(ToolProfileSetResponse {
            before,
            after,
            row_readback,
            intent_audit,
            final_audit,
            lease_proof,
        }))
    }
}

impl SynapseService {
    pub(crate) fn tools_for_session_profile(
        &self,
        session_id: Option<&str>,
    ) -> Result<Vec<Tool>, ErrorData> {
        let snapshot = self.tool_profile_snapshot(session_id)?;
        let mut tools = self.full_sanitized_tools();
        if session_id.is_some() {
            tools.retain(|tool| snapshot.profile.is_visible(tool.name.as_ref()));
        }
        sort_tools_for_profile(&mut tools, snapshot.profile);
        Ok(tools)
    }

    pub(crate) fn tool_profile_snapshot(
        &self,
        session_id: Option<&str>,
    ) -> Result<ToolProfileSnapshot, ErrorData> {
        let full_tool_names = self.full_tool_names();
        let implementation_tool_count = full_tool_names.len();
        let (profile, source, policy_row) = match session_id {
            Some(session_id) => {
                let row = self.ensure_tool_profile_assignment(session_id)?;
                (row.record.profile, row.record.source.clone(), Some(row))
            }
            None => (
                ToolProfileKind::BreakGlass,
                "unscoped_stdio_admin".to_owned(),
                None,
            ),
        };
        let visible_tool_names = if session_id.is_some() {
            visible_tool_names_for_profile(profile, &full_tool_names)
        } else {
            full_tool_names
        };
        let visible_tool_sha256 = sha256_json_hex(&visible_tool_names)?;
        let denied_break_glass_tools = denied_break_glass_tools(&visible_tool_names);
        Ok(ToolProfileSnapshot {
            source_of_truth: TOOL_PROFILE_SOURCE_OF_TRUTH,
            session_id: session_id.map(ToOwned::to_owned),
            profile,
            profile_label: profile.label(),
            source,
            implementation_tool_count,
            visible_tool_count: visible_tool_names.len(),
            visible_tool_sha256,
            visible_tool_names,
            denied_break_glass_tools,
            policy_row,
        })
    }

    pub(crate) fn admit_tool_call_for_profile(
        &self,
        tool_name: &str,
        session_id: Option<&str>,
    ) -> Result<(), ErrorData> {
        let Some(session_id) = session_id else {
            return Ok(());
        };
        let full_tool_names = self.full_tool_names();
        if !full_tool_names.iter().any(|name| name == tool_name) {
            return Ok(());
        }
        let row = self.ensure_tool_profile_assignment(session_id)?;
        if row.record.profile.is_visible(tool_name) {
            return Ok(());
        }
        let visible_tool_names =
            visible_tool_names_for_profile(row.record.profile, &full_tool_names);
        let error = ErrorData::new(
            ErrorCode(-32099),
            format!(
                "tool {tool_name:?} is hidden by MCP tool profile {} for session {session_id}",
                row.record.profile.as_str()
            ),
            Some(json!({
                "code": error_codes::TOOL_PROFILE_POLICY_DENIED,
                "tool": tool_name,
                "session_id": session_id,
                "profile": row.record.profile.as_str(),
                "profile_label": row.record.profile.label(),
                "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
                "policy_row": row,
                "visible_tool_count": visible_tool_names.len(),
                "resolution": "use a background-safe/session-targeted tool, or explicitly acquire the foreground input lease and set profile=break_glass with a non-empty reason",
            })),
        );
        let command_payload = json!({
            "requested_tool": tool_name,
            "profile": row.record.profile.as_str(),
        });
        let command_before = json!({
            "source_of_truth": TOOL_PROFILE_SOURCE_OF_TRUTH,
            "policy_row": row,
            "visible_tool_count": visible_tool_names.len(),
        });
        self.command_audit_final(
            super::command_audit::CommandAuditInput::mcp(
                "tool_profile_policy",
                "tool_call_denied",
                Some(session_id.to_owned()),
                Some(session_id.to_owned()),
                command_payload,
                command_before,
                json!({
                    "source_of_truth": "CF_ACTION_LOG command_audit row",
                    "denied_tool": tool_name,
                }),
                "error",
            )
            .with_error(super::command_audit::command_audit_error_from_error_data(
                &error,
            )),
        )?;
        Err(error)
    }

    fn ensure_tool_profile_assignment(
        &self,
        session_id: &str,
    ) -> Result<ToolProfileRowReadback, ErrorData> {
        match self.read_tool_profile_assignment(session_id)? {
            Some(row) => Ok(row),
            None => self.write_tool_profile_assignment(
                session_id,
                ToolProfileKind::NormalAgent,
                "default_normal_agent",
                None,
                None,
            ),
        }
    }

    fn read_tool_profile_assignment(
        &self,
        session_id: &str,
    ) -> Result<Option<ToolProfileRowReadback>, ErrorData> {
        let db = self.m3_storage()?;
        let key = tool_profile_key(session_id);
        let rows = db
            .scan_cf_prefix(cf::CF_SESSIONS, &key)
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let Some((read_key, value)) = rows.into_iter().find(|(row_key, _)| row_key == &key) else {
            return Ok(None);
        };
        let persisted =
            synapse_storage::decode_json::<PersistedToolProfile>(&value).map_err(|error| {
                mcp_error(
                    error.code(),
                    format!("decode tool profile row failed for {session_id}: {error}"),
                )
            })?;
        if persisted.schema_version != TOOL_PROFILE_SCHEMA_VERSION
            || persisted.row_kind != TOOL_PROFILE_ROW_KIND
            || persisted.session_id != session_id
        {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!(
                    "tool profile row mismatch for {session_id}: schema_version={} row_kind={} row_session_id={}",
                    persisted.schema_version, persisted.row_kind, persisted.session_id
                ),
            ));
        }
        let record = ToolProfileAssignment {
            schema_version: persisted.schema_version,
            row_kind: persisted.row_kind,
            session_id: persisted.session_id,
            profile: persisted.profile,
            source: persisted.source,
            reason: persisted.reason,
            set_by_session_id: persisted.set_by_session_id,
            stored_at_unix_ms: persisted.stored_at_unix_ms,
            allowed_tool_count: persisted.allowed_tool_count,
            allowed_tool_sha256: persisted.allowed_tool_sha256,
            denied_break_glass_tools: persisted.denied_break_glass_tools,
        };
        Ok(Some(ToolProfileRowReadback {
            cf_name: cf::CF_SESSIONS,
            key_hex: hex_lower(&read_key),
            value_len_bytes: value.len() as u64,
            value_sha256: sha256_hex(&value),
            record,
        }))
    }

    fn write_tool_profile_assignment(
        &self,
        session_id: &str,
        profile: ToolProfileKind,
        source: impl Into<String>,
        reason: Option<String>,
        set_by_session_id: Option<String>,
    ) -> Result<ToolProfileRowReadback, ErrorData> {
        let full_tool_names = self.full_tool_names();
        let allowed_tool_names = visible_tool_names_for_profile(profile, &full_tool_names);
        let allowed_tool_sha256 = sha256_json_hex(&allowed_tool_names)?;
        let record = ToolProfileAssignment {
            schema_version: TOOL_PROFILE_SCHEMA_VERSION,
            row_kind: TOOL_PROFILE_ROW_KIND.to_owned(),
            session_id: session_id.to_owned(),
            profile,
            source: source.into(),
            reason,
            set_by_session_id,
            stored_at_unix_ms: unix_ms_now(),
            allowed_tool_count: allowed_tool_names.len(),
            allowed_tool_sha256,
            denied_break_glass_tools: denied_break_glass_tools(&allowed_tool_names),
        };
        let encoded = synapse_storage::encode_json(&record).map_err(|error| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("encode tool profile row failed for {session_id}: {error}"),
            )
        })?;
        let db = self.m3_storage()?;
        let key = tool_profile_key(session_id);
        db.put_batch_pressure_bypass(cf::CF_SESSIONS, [(key.clone(), encoded.clone())])
            .map_err(|error| mcp_error(error.code(), error.to_string()))?;
        let readback = self
            .read_tool_profile_assignment(session_id)?
            .ok_or_else(|| {
                mcp_error(
                    error_codes::STORAGE_CORRUPTED,
                    format!("tool profile row missing after write for {session_id}"),
                )
            })?;
        if readback.value_sha256 != sha256_hex(&encoded) {
            return Err(mcp_error(
                error_codes::STORAGE_CORRUPTED,
                format!("tool profile row readback hash mismatch for {session_id}"),
            ));
        }
        tracing::info!(
            code = "MCP_TOOL_PROFILE_PERSISTED",
            session_id,
            profile = profile.as_str(),
            visible_tool_count = readback.record.allowed_tool_count,
            key_hex = %readback.key_hex,
            "persisted MCP tool profile to CF_SESSIONS"
        );
        Ok(readback)
    }

    fn full_sanitized_tools(&self) -> Vec<Tool> {
        super::schema_sanitize::sanitize_tools(self.tool_router.list_all())
    }

    fn full_tool_names(&self) -> Vec<String> {
        let mut names = self
            .full_sanitized_tools()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

fn validate_profile_set_policy(
    session_id: &str,
    profile: ToolProfileKind,
    reason: Option<&str>,
    confirm_break_glass: bool,
    lease_proof: &ToolProfileLeaseProof,
) -> Result<(), ErrorData> {
    if profile != ToolProfileKind::BreakGlass {
        return Ok(());
    }
    if !confirm_break_glass {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "profile=break_glass requires confirm_break_glass=true",
        ));
    }
    if reason.is_none_or(str::is_empty) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            "profile=break_glass requires a non-empty reason",
        ));
    }
    if !lease_proof.caller_is_owner {
        return Err(ErrorData::new(
            ErrorCode(-32099),
            format!(
                "profile=break_glass requires this MCP session to own the foreground input lease; current owner={:?}",
                lease_proof.owner_session_id
            ),
            Some(json!({
                "code": error_codes::ACTION_FOREGROUND_LEASE_NOT_HELD,
                "session_id": session_id,
                "profile": profile.as_str(),
                "lease_proof": lease_proof,
                "resolution": "call control_lease_acquire first, then retry tool_profile_set with confirm_break_glass=true and a reason",
            })),
        ));
    }
    Ok(())
}

fn break_glass_lease_proof(session_id: &str, profile: ToolProfileKind) -> ToolProfileLeaseProof {
    let status = lease::status();
    ToolProfileLeaseProof {
        required: profile == ToolProfileKind::BreakGlass,
        held: status.held,
        owner_session_id: status.owner_session_id.clone(),
        caller_is_owner: status.owner_session_id.as_deref() == Some(session_id),
        expires_in_ms: status.expires_in_ms,
    }
}

fn normalize_reason(raw: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.chars().count() > MAX_PROFILE_REASON_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("tool profile reason must be at most {MAX_PROFILE_REASON_CHARS} characters"),
        ));
    }
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

fn visible_tool_names_for_profile(
    profile: ToolProfileKind,
    full_tool_names: &[String],
) -> Vec<String> {
    let mut names = full_tool_names
        .iter()
        .filter(|name| profile.is_visible(name))
        .cloned()
        .collect::<Vec<_>>();
    names.sort_by(|left, right| {
        tool_rank(profile, left)
            .cmp(&tool_rank(profile, right))
            .then(left.cmp(right))
    });
    names
}

fn denied_break_glass_tools(visible_tool_names: &[String]) -> Vec<String> {
    let visible = visible_tool_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    BREAK_GLASS_HAZARDOUS_TOOLS
        .iter()
        .copied()
        .filter(|name| !visible.contains(name))
        .map(str::to_owned)
        .collect()
}

fn sort_tools_for_profile(tools: &mut [Tool], profile: ToolProfileKind) {
    tools.sort_by(|left, right| {
        let left_name = left.name.as_ref();
        let right_name = right.name.as_ref();
        tool_rank(profile, left_name)
            .cmp(&tool_rank(profile, right_name))
            .then(left_name.cmp(right_name))
    });
}

fn tool_rank(profile: ToolProfileKind, tool_name: &str) -> usize {
    match profile {
        ToolProfileKind::NormalAgent => NORMAL_ALLOWED_EXACT
            .iter()
            .position(|name| *name == tool_name)
            .unwrap_or(usize::MAX),
        ToolProfileKind::BrowserControl => BROWSER_CONTROL_ALLOWED_EXACT
            .iter()
            .position(|name| *name == tool_name)
            .unwrap_or(usize::MAX),
        ToolProfileKind::BreakGlass => usize::MAX,
    }
}

fn tool_profile_key(session_id: &str) -> Vec<u8> {
    format!("{TOOL_PROFILE_PREFIX}{session_id}").into_bytes()
}

fn audit_readback(
    readback: super::command_audit::CommandAuditRowReadback,
) -> ToolProfileAuditReadback {
    ToolProfileAuditReadback {
        cf_name: readback.cf_name,
        key_hex: readback.key_hex,
        value_len_bytes: readback.value_len_bytes,
        value_sha256: readback.value_sha256,
    }
}

fn sha256_json_hex<T: Serialize>(value: &T) -> Result<String, ErrorData> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("serialize tool profile digest payload failed: {error}"),
        )
    })?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{num::NonZeroUsize, path::Path};

    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use crate::{m2::M2ServiceConfig, m3::M3ServiceConfig, m4::M4ServiceConfig};

    fn service_with_db(path: &Path) -> SynapseService {
        SynapseService::try_with_m2_shutdown_reason_and_m3_config(
            CancellationToken::new(),
            "test",
            CancellationToken::new(),
            &M2ServiceConfig::default(),
            M3ServiceConfig::from_cli_parts(
                Some(path.join("db")),
                Some(path.to_path_buf()),
                false,
                "127.0.0.1:0".to_owned(),
                NonZeroUsize::new(4).expect("nonzero"),
                false,
                true,
                None,
                false,
                None,
            ),
            M4ServiceConfig::default(),
        )
        .expect("construct service")
    }

    fn tool_names(tools: Vec<Tool>) -> Vec<String> {
        tools
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    fn names() -> Vec<String> {
        let mut names = BREAK_GLASS_HAZARDOUS_TOOLS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        names.extend(
            [
                "act_run_shell",
                "cdp_open_tab",
                "health",
                "session_list",
                "tool_profile_set",
                "tool_profile_status",
            ]
            .iter()
            .map(|name| (*name).to_owned()),
        );
        names
    }

    #[test]
    fn normal_profile_hides_foreground_primitives() {
        let visible = visible_tool_names_for_profile(ToolProfileKind::NormalAgent, &names());
        assert!(visible.contains(&"act_run_shell".to_owned()));
        assert!(visible.contains(&"cdp_open_tab".to_owned()));
        assert!(visible.contains(&"tool_profile_set".to_owned()));
        assert!(!visible.contains(&"act_click".to_owned()));
        assert!(!visible.contains(&"act_type".to_owned()));
        assert!(!visible.contains(&"release_all".to_owned()));
    }

    #[test]
    fn browser_profile_is_narrower_than_normal_agent() {
        let visible = visible_tool_names_for_profile(ToolProfileKind::BrowserControl, &names());
        assert!(visible.contains(&"cdp_open_tab".to_owned()));
        assert!(visible.contains(&"session_list".to_owned()));
        assert!(!visible.contains(&"act_run_shell".to_owned()));
        assert!(!visible.contains(&"act_click".to_owned()));
    }

    #[test]
    fn break_glass_profile_exposes_full_surface() {
        let mut expected = names();
        expected.sort();
        let visible = visible_tool_names_for_profile(ToolProfileKind::BreakGlass, &names());
        assert_eq!(visible, expected);
        assert!(denied_break_glass_tools(&visible).is_empty());
    }

    #[test]
    fn break_glass_requires_confirm_reason_and_lease() {
        let proof = ToolProfileLeaseProof {
            required: true,
            held: false,
            owner_session_id: None,
            caller_is_owner: false,
            expires_in_ms: None,
        };
        assert!(
            validate_profile_set_policy(
                "s1",
                ToolProfileKind::BreakGlass,
                Some("need raw foreground click"),
                false,
                &proof,
            )
            .is_err()
        );
        assert!(
            validate_profile_set_policy("s1", ToolProfileKind::BreakGlass, None, true, &proof,)
                .is_err()
        );
        assert!(
            validate_profile_set_policy(
                "s1",
                ToolProfileKind::BreakGlass,
                Some("need raw foreground click"),
                true,
                &proof,
            )
            .is_err()
        );
    }

    #[test]
    fn break_glass_policy_accepts_owned_lease_proof() {
        let proof = ToolProfileLeaseProof {
            required: true,
            held: true,
            owner_session_id: Some("s1".to_owned()),
            caller_is_owner: true,
            expires_in_ms: Some(10_000),
        };
        validate_profile_set_policy(
            "s1",
            ToolProfileKind::BreakGlass,
            Some("need raw foreground click"),
            true,
            &proof,
        )
        .expect("owned lease proof should allow break-glass");
    }

    #[test]
    fn default_normal_profile_persists_policy_row_and_filters_tools() {
        let dir = TempDir::new().expect("tmp");
        let service = service_with_db(dir.path());
        let session_id = "issue1008-default-session";
        let before = service
            .read_tool_profile_assignment(session_id)
            .expect("read before");
        assert!(before.is_none());

        let tools = tool_names(
            service
                .tools_for_session_profile(Some(session_id))
                .expect("profile tools"),
        );
        assert!(tools.contains(&"health".to_owned()));
        assert!(tools.contains(&"cdp_open_tab".to_owned()));
        assert!(tools.contains(&"tool_profile_status".to_owned()));
        assert!(!tools.contains(&"act_click".to_owned()));
        assert!(!tools.contains(&"act_type".to_owned()));
        assert!(!tools.contains(&"release_all".to_owned()));

        let row = service
            .read_tool_profile_assignment(session_id)
            .expect("read after")
            .expect("row after tools/list profile resolution");
        assert_eq!(row.cf_name, cf::CF_SESSIONS);
        assert_eq!(row.record.profile, ToolProfileKind::NormalAgent);
        assert_eq!(row.record.source, "default_normal_agent");
        assert_eq!(row.record.allowed_tool_count, tools.len());
        assert!(row.value_sha256.starts_with("sha256:"));

        let db = service.m3_storage().expect("storage");
        let stored = db
            .scan_cf_prefix(cf::CF_SESSIONS, &tool_profile_key(session_id))
            .expect("scan policy rows");
        assert_eq!(stored.len(), 1);
        assert_eq!(hex_lower(&stored[0].0), row.key_hex);
        assert_eq!(sha256_hex(&stored[0].1), row.value_sha256);
    }

    #[test]
    fn browser_control_profile_excludes_shell_and_foreground_primitives() {
        let dir = TempDir::new().expect("tmp");
        let service = service_with_db(dir.path());
        let session_id = "issue1008-browser-session";
        let row = service
            .write_tool_profile_assignment(
                session_id,
                ToolProfileKind::BrowserControl,
                "test_browser_control",
                Some("dashboard inactive tab verification".to_owned()),
                Some(session_id.to_owned()),
            )
            .expect("write browser profile");
        assert_eq!(row.record.profile, ToolProfileKind::BrowserControl);

        let tools = tool_names(
            service
                .tools_for_session_profile(Some(session_id))
                .expect("browser profile tools"),
        );
        assert!(tools.contains(&"cdp_open_tab".to_owned()));
        assert!(tools.contains(&"cdp_target_info".to_owned()));
        assert!(tools.contains(&"tool_profile_set".to_owned()));
        assert!(!tools.contains(&"act_run_shell".to_owned()));
        assert!(!tools.contains(&"act_spawn_agent".to_owned()));
        assert!(!tools.contains(&"act_type".to_owned()));
    }

    #[test]
    fn hidden_tool_call_denial_writes_policy_audit_row() {
        let dir = TempDir::new().expect("tmp");
        let service = service_with_db(dir.path());
        let session_id = "issue1008-denied-session";
        let error = service
            .admit_tool_call_for_profile("act_type", Some(session_id))
            .expect_err("normal profile must deny hidden foreground typing tool");
        let code = error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str);
        assert_eq!(code, Some(error_codes::TOOL_PROFILE_POLICY_DENIED));

        let db = service.m3_storage().expect("storage");
        let audit_rows = db
            .scan_cf_prefix(cf::CF_ACTION_LOG, b"")
            .expect("scan command audit");
        let matching = audit_rows
            .iter()
            .filter(|(_, value)| {
                let text = String::from_utf8_lossy(value);
                text.contains("tool_profile_policy")
                    && text.contains("tool_call_denied")
                    && text.contains("act_type")
                    && text.contains(error_codes::TOOL_PROFILE_POLICY_DENIED)
            })
            .count();
        assert_eq!(matching, 1);
    }
}
