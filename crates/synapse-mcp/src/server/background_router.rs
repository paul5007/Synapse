//! `target_act` (#1005): a compact, high-level background-first computer-use
//! router.
//!
//! The raw tool surface is large, and model priors make low-level primitive
//! selection brittle and foreground-prone. `target_act` gives agents one
//! intent-named verb that routes to the correct *background-capable*,
//! session-targeted primitive and never to the human OS foreground. It is a thin
//! dispatcher: each verb delegates to the existing tool method, inheriting that
//! tool's target resolution, background routing, action audit (#1006), and
//! lease/foreground guards (#999/#1004) — so a normal (leaseless) session can
//! drive a background target through this router but cannot escalate to the
//! human foreground, which the delegate refuses before any mutation.

use super::{ErrorData, Json, Parameters, SynapseService, tool, tool_router};
use crate::m1::{
    CaptureScreenshotParams, CdpNavigateAction, CdpNavigateTabParams, ObserveParams, mcp_error,
};
use crate::m2::{ActSetFieldTextParams, default_verify_timeout_ms};
use crate::m4::{ActRunShellExecutionMode, ActRunShellParams};
use rmcp::schemars::JsonSchema;
use rmcp::{RoleServer, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use synapse_core::{ElementId, error_codes};

const DEFAULT_TARGET_ACT_SHELL_TIMEOUT_MS: u64 = 30_000;

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetActVerb {
    /// Observe the session target (UIA/WGC/CDP perception) — background.
    Read,
    /// Capture the session target to a file — background.
    Screenshot,
    /// Navigate the session's owned browser target — background (Chrome bridge
    /// / CDP), never the human foreground tab.
    Navigate,
    /// Replace a target web/UIA field's text by element id — background CDP/UIA
    /// tiers; the delegate refuses before input if only a foreground route
    /// exists and no break-glass lease is held.
    SetField,
    /// Run a shell command in the session workspace — background.
    RunShell,
}

impl TargetActVerb {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Screenshot => "screenshot",
            Self::Navigate => "navigate",
            Self::SetField => "set_field",
            Self::RunShell => "run_shell",
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetActParams {
    /// The high-level operation to perform on the session target.
    pub verb: TargetActVerb,
    /// `navigate`: destination URL.
    #[serde(default)]
    pub url: Option<String>,
    /// `screenshot`: output file path.
    #[serde(default)]
    pub path: Option<String>,
    /// `set_field`: target element id (from observe/find).
    #[serde(default)]
    pub element_id: Option<String>,
    /// `set_field`: full replacement text (empty clears the field).
    #[serde(default)]
    pub text: Option<String>,
    /// `run_shell`: executable/program name (arguments go in `args`).
    #[serde(default)]
    pub command: Option<String>,
    /// `run_shell`: literal arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// `run_shell`: working directory.
    #[serde(default)]
    pub working_dir: Option<String>,
    /// `run_shell`: inline wait budget (ms). Defaults to 30000.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TargetActResponse {
    pub verb: String,
    /// The background primitive this verb routed to.
    pub delegated_tool: String,
    pub routing: String,
    /// The delegated tool's full response.
    pub result: Value,
}

#[tool_router(router = background_router_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "High-level background-first computer-use router (#1005). One verb, routed to the correct background-capable, session-targeted primitive — never the human OS foreground. verb=read observes the target; verb=screenshot captures it; verb=navigate drives the owned browser target (Chrome bridge/CDP); verb=set_field replaces a web/UIA field's text by element id via background tiers; verb=run_shell runs a command in the session workspace. Prefer this over raw act_* primitives: it inherits each delegate's target resolution, action audit, and lease/foreground guards, so a normal (leaseless) session can drive a background target but cannot seize the human foreground (the delegate fails closed before input). Bind a target first with set_target (discover one with window_list)."
    )]
    pub async fn target_act(
        &self,
        params: Parameters<TargetActParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<TargetActResponse>, ErrorData> {
        let params = params.0;
        let verb = params.verb;
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = "target_act",
            verb = verb.as_str(),
            "tool.invocation kind=target_act"
        );

        let (delegated_tool, result) = match verb {
            TargetActVerb::Read => {
                let response = self
                    .observe(Parameters(ObserveParams::default()), request_context)
                    .await?;
                ("observe", target_act_result(&response.0)?)
            }
            TargetActVerb::Screenshot => {
                let path = require_param(params.path, "screenshot", "path")?;
                let response = self
                    .capture_screenshot(
                        Parameters(CaptureScreenshotParams {
                            path,
                            region: None,
                            window_hwnd: None,
                            overwrite: true,
                        }),
                        request_context,
                    )
                    .await?;
                ("capture_screenshot", target_act_result(&response.0)?)
            }
            TargetActVerb::Navigate => {
                let url = require_param(params.url, "navigate", "url")?;
                let response = self
                    .cdp_navigate_tab(
                        Parameters(CdpNavigateTabParams {
                            window_hwnd: None,
                            cdp_target_id: None,
                            action: CdpNavigateAction::Navigate,
                            url: Some(url),
                            wait_timeout_ms: None,
                            ignore_cache: None,
                        }),
                        request_context,
                    )
                    .await?;
                ("cdp_navigate_tab", target_act_result(&response.0)?)
            }
            TargetActVerb::SetField => {
                let element_id = require_param(params.element_id, "set_field", "element_id")?;
                let element_id = ElementId::parse(&element_id).map_err(|error| {
                    mcp_error(
                        error_codes::TOOL_PARAMS_INVALID,
                        format!("target_act verb=set_field element_id is invalid: {error}"),
                    )
                })?;
                let response = self
                    .act_set_field_text(
                        Parameters(ActSetFieldTextParams {
                            element_id,
                            text: params.text.unwrap_or_default(),
                            verify_timeout_ms: default_verify_timeout_ms(),
                        }),
                        request_context,
                    )
                    .await?;
                ("act_set_field_text", target_act_result(&response.0)?)
            }
            TargetActVerb::RunShell => {
                let command = require_param(params.command, "run_shell", "command")?;
                let response = self
                    .act_run_shell(
                        Parameters(ActRunShellParams {
                            command,
                            args: params.args,
                            working_dir: params.working_dir,
                            env: BTreeMap::new(),
                            timeout_ms: params
                                .timeout_ms
                                .unwrap_or(DEFAULT_TARGET_ACT_SHELL_TIMEOUT_MS),
                            execution_mode: ActRunShellExecutionMode::Inline,
                            durable_timeout_ms: None,
                            idempotency_key: None,
                        }),
                        request_context,
                    )
                    .await?;
                ("act_run_shell", target_act_result(&response.0)?)
            }
        };

        Ok(Json(TargetActResponse {
            verb: verb.as_str().to_owned(),
            delegated_tool: delegated_tool.to_owned(),
            routing: "background-first; delegated to the session-targeted primitive, which inherits the action audit and lease/foreground guards and refuses the human foreground before input".to_owned(),
            result,
        }))
    }
}

fn require_param(value: Option<String>, verb: &str, field: &str) -> Result<String, ErrorData> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("target_act verb={verb} requires a non-empty `{field}`"),
        )
    })
}

fn target_act_result<T: Serialize>(value: &T) -> Result<Value, ErrorData> {
    serde_json::to_value(value).map_err(|error| {
        mcp_error(
            error_codes::TOOL_INTERNAL_ERROR,
            format!("target_act failed to encode delegated tool result: {error}"),
        )
    })
}
