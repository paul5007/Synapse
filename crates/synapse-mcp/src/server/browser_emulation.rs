//! Browser emulation tools (#1173).

use super::{
    ErrorData, Json, Parameters, SynapseService,
    m1_tools::{
        browser_raw_cdp_required_error, cdp_target_id_audit_ref, require_target_session_id,
        validate_cdp_target_id,
    },
    tool, tool_router,
};
use crate::m1::mcp_error;
use rmcp::{RoleServer, schemars::JsonSchema, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::error_codes;

const RESIZE_TOOL: &str = "browser_resize";
const DEVICE_TOOL: &str = "browser_device";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserResizeOperation {
    Set,
    Reset,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowserDeviceOperation {
    Set,
    Reset,
}

impl Default for BrowserDeviceOperation {
    fn default() -> Self {
        Self::Set
    }
}

impl Default for BrowserResizeOperation {
    fn default() -> Self {
        Self::Set
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserResizeParams {
    /// CDP TargetID to resize. Defaults to the active session CDP target. Must
    /// be owned by this session; the human foreground tab is never a fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Rendered viewport width in CSS pixels for operation=set.
    #[serde(default)]
    pub width: Option<u32>,
    /// Rendered viewport height in CSS pixels for operation=set.
    #[serde(default)]
    pub height: Option<u32>,
    /// Device pixel ratio override for operation=set. Defaults to 1.0.
    #[serde(default)]
    pub device_scale_factor: Option<f64>,
    /// `set` applies a viewport/DPR override; `reset` clears it.
    #[serde(default)]
    pub operation: BrowserResizeOperation,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserDeviceParams {
    /// CDP TargetID to emulate. Defaults to the active session CDP target. Must
    /// be owned by this session; the human foreground tab is never a fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// `set` applies the descriptor; `reset` clears metrics/touch and restores
    /// the user agent observed before the first set in this process.
    #[serde(default)]
    pub operation: BrowserDeviceOperation,
    /// User agent string for operation=set.
    #[serde(default)]
    pub user_agent: Option<String>,
    /// Rendered viewport width in CSS pixels for operation=set.
    #[serde(default)]
    pub width: Option<u32>,
    /// Rendered viewport height in CSS pixels for operation=set.
    #[serde(default)]
    pub height: Option<u32>,
    /// Device pixel ratio override for operation=set. Defaults to 1.0.
    #[serde(default)]
    pub device_scale_factor: Option<f64>,
    /// Whether Chromium should apply mobile viewport semantics. Defaults false.
    #[serde(default)]
    pub is_mobile: Option<bool>,
    /// Whether to enable touch emulation. Defaults false.
    #[serde(default)]
    pub has_touch: Option<bool>,
    /// Maximum emulated touch points. Defaults to 5 when has_touch=true and 0
    /// when has_touch=false.
    #[serde(default)]
    pub max_touch_points: Option<u32>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserViewportOverride {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserDeviceDescriptor {
    pub user_agent: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
    pub max_touch_points: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserViewportReadback {
    pub inner_width: i64,
    pub inner_height: i64,
    pub device_pixel_ratio: f64,
    pub screen_width: i64,
    pub screen_height: i64,
    pub outer_width: i64,
    pub outer_height: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual_viewport_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual_viewport_height: Option<f64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserResizeResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: BrowserResizeOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<BrowserViewportOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub viewport: BrowserViewportReadback,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserDeviceReadback {
    pub viewport: BrowserViewportReadback,
    pub user_agent: String,
    pub max_touch_points: i64,
    pub ontouchstart_available: bool,
    pub pointer_coarse: bool,
    pub any_pointer_coarse: bool,
    pub hover_none: bool,
    pub any_hover_none: bool,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserDeviceResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: BrowserDeviceOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor: Option<BrowserDeviceDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_user_agent: Option<String>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub device: BrowserDeviceReadback,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
    pub source_of_truth: String,
}

#[derive(Clone, Debug, PartialEq)]
struct NormalizedBrowserResizeParams {
    operation: BrowserResizeOperation,
    width: Option<u32>,
    height: Option<u32>,
    device_scale_factor: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct NormalizedBrowserDeviceParams {
    operation: BrowserDeviceOperation,
    descriptor: Option<BrowserDeviceDescriptor>,
}

#[tool_router(router = browser_emulation_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "Set or reset the rendered viewport size and device pixel ratio for the calling session's owned raw-CDP browser tab. operation=set uses Emulation.setDeviceMetricsOverride with mobile=false and page-visible readback via Runtime.evaluate; pass width, height, and optional device_scale_factor. operation=reset uses Emulation.clearDeviceMetricsOverride, then reads back the real metrics. Target-scoped and background-safe: never activates the tab, never uses OS foreground input, and never falls back to the human foreground tab. Raw CDP only; use browser_evaluate as an independent FSV readback for window.innerWidth/window.innerHeight/devicePixelRatio."
    )]
    pub async fn browser_resize(
        &self,
        params: Parameters<BrowserResizeParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserResizeResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = RESIZE_TOOL,
            "tool.invocation kind=browser_resize"
        );
        let session_id = require_target_session_id(&request_context)?;
        let resize = validate_browser_resize_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "operation": resize.operation,
            "width": resize.width,
            "height": resize.height,
            "device_scale_factor": resize.device_scale_factor,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            RESIZE_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            RESIZE_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "operation": resize.operation,
            "width": resize.width,
            "height": resize.height,
            "device_scale_factor": resize.device_scale_factor,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            RESIZE_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_resize_impl(&session_id, window_hwnd, &cdp_target_id, &resize)
            .await;
        self.audit_action_result_for_session(RESIZE_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[tool(
        description = "Apply or reset a Playwright-style device descriptor for the calling session's owned raw-CDP browser tab. operation=set applies user_agent, width, height, device_scale_factor, is_mobile, has_touch, and max_touch_points in one target-scoped command sequence using Emulation.setUserAgentOverride, Emulation.setDeviceMetricsOverride, Emulation.setTouchEmulationEnabled, and Emulation.setEmitTouchEventsForMouse, then reads back navigator/user-agent/viewport/touch media state via Runtime.evaluate. operation=reset clears metrics and touch emulation and restores the user agent observed before the first set in this Synapse process. Background-safe: never activates the tab, never uses OS foreground input, and never falls back to the human foreground tab. Raw CDP only; use browser_evaluate as an independent FSV readback for navigator.userAgent, innerWidth/innerHeight, devicePixelRatio, maxTouchPoints, and matchMedia('(pointer: coarse)')."
    )]
    pub async fn browser_device(
        &self,
        params: Parameters<BrowserDeviceParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserDeviceResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = DEVICE_TOOL,
            "tool.invocation kind=browser_device"
        );
        let session_id = require_target_session_id(&request_context)?;
        let device = validate_browser_device_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "operation": device.operation,
            "descriptor": &device.descriptor,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            DEVICE_TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            DEVICE_TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "operation": device.operation,
            "descriptor": &device.descriptor,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(
            DEVICE_TOOL,
            &request_details,
            &session_id,
        )?;
        let result = self
            .browser_device_impl(&session_id, window_hwnd, &cdp_target_id, &device)
            .await;
        self.audit_action_result_for_session(DEVICE_TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[cfg(windows)]
    async fn browser_resize_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        params: &NormalizedBrowserResizeParams,
    ) -> Result<BrowserResizeResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(RESIZE_TOOL, window_hwnd));
        };
        let result = match params.operation {
            BrowserResizeOperation::Set => {
                let width = params.width.expect("validated set width");
                let height = params.height.expect("validated set height");
                let device_scale_factor = params
                    .device_scale_factor
                    .expect("validated set device_scale_factor");
                synapse_a11y::cdp_set_viewport_size(
                    &endpoint,
                    cdp_target_id,
                    width,
                    height,
                    device_scale_factor,
                )
                .await
            }
            BrowserResizeOperation::Reset => {
                synapse_a11y::cdp_reset_viewport_size(&endpoint, cdp_target_id).await
            }
        }
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{RESIZE_TOOL} raw CDP viewport emulation failed: {error}"),
            )
        })?;
        tracing::info!(
            code = "CDP_BACKGROUND_VIEWPORT_RESIZE",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            operation = ?params.operation,
            inner_width = result.readback.inner_width,
            inner_height = result.readback.inner_height,
            device_pixel_ratio = result.readback.device_pixel_ratio,
            "readback=Emulation.setDeviceMetricsOverride+Runtime.evaluate outcome=viewport_metrics"
        );
        Ok(browser_resize_response(session_id, window_hwnd, result))
    }

    #[cfg(windows)]
    async fn browser_device_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        params: &NormalizedBrowserDeviceParams,
    ) -> Result<BrowserDeviceResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(DEVICE_TOOL, window_hwnd));
        };
        let result = match params.operation {
            BrowserDeviceOperation::Set => {
                let descriptor = params
                    .descriptor
                    .as_ref()
                    .expect("validated device descriptor");
                synapse_a11y::cdp_apply_device_descriptor(
                    &endpoint,
                    cdp_target_id,
                    synapse_a11y::CdpDeviceDescriptor {
                        user_agent: descriptor.user_agent.clone(),
                        width: descriptor.width,
                        height: descriptor.height,
                        device_scale_factor: descriptor.device_scale_factor,
                        is_mobile: descriptor.is_mobile,
                        has_touch: descriptor.has_touch,
                        max_touch_points: descriptor.max_touch_points,
                    },
                )
                .await
            }
            BrowserDeviceOperation::Reset => {
                synapse_a11y::cdp_reset_device_descriptor(&endpoint, cdp_target_id).await
            }
        }
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{DEVICE_TOOL} raw CDP device emulation failed: {error}"),
            )
        })?;
        tracing::info!(
            code = "CDP_BACKGROUND_DEVICE_EMULATION",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            operation = ?params.operation,
            inner_width = result.readback.viewport.inner_width,
            inner_height = result.readback.viewport.inner_height,
            device_pixel_ratio = result.readback.viewport.device_pixel_ratio,
            max_touch_points = result.readback.max_touch_points,
            pointer_coarse = result.readback.pointer_coarse,
            "readback=Emulation.device_descriptor+Runtime.evaluate outcome=device_metrics"
        );
        Ok(browser_device_response(session_id, window_hwnd, result))
    }

    #[cfg(not(windows))]
    async fn browser_resize_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _params: &NormalizedBrowserResizeParams,
    ) -> Result<BrowserResizeResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_resize is only available on Windows in this build",
        ))
    }

    #[cfg(not(windows))]
    async fn browser_device_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _params: &NormalizedBrowserDeviceParams,
    ) -> Result<BrowserDeviceResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_device is only available on Windows in this build",
        ))
    }
}

fn validate_browser_resize_params(
    params: &BrowserResizeParams,
) -> Result<NormalizedBrowserResizeParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    if params.operation == BrowserResizeOperation::Reset {
        reject_resize_field(params.width, "width", "reset")?;
        reject_resize_field(params.height, "height", "reset")?;
        reject_resize_field(params.device_scale_factor, "device_scale_factor", "reset")?;
        return Ok(NormalizedBrowserResizeParams {
            operation: BrowserResizeOperation::Reset,
            width: None,
            height: None,
            device_scale_factor: None,
        });
    }

    let width = params.width.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{RESIZE_TOOL} operation=set requires width"),
        )
    })?;
    let height = params.height.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{RESIZE_TOOL} operation=set requires height"),
        )
    })?;
    validate_dimension("width", width)?;
    validate_dimension("height", height)?;
    let device_scale_factor = params.device_scale_factor.unwrap_or(1.0);
    if !device_scale_factor.is_finite()
        || device_scale_factor <= 0.0
        || device_scale_factor > synapse_a11y::CDP_DEVICE_SCALE_FACTOR_MAX
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{RESIZE_TOOL} device_scale_factor must be finite and in 0..={}",
                synapse_a11y::CDP_DEVICE_SCALE_FACTOR_MAX
            ),
        ));
    }
    Ok(NormalizedBrowserResizeParams {
        operation: BrowserResizeOperation::Set,
        width: Some(width),
        height: Some(height),
        device_scale_factor: Some(device_scale_factor),
    })
}

fn validate_browser_device_params(
    params: &BrowserDeviceParams,
) -> Result<NormalizedBrowserDeviceParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    if params.operation == BrowserDeviceOperation::Reset {
        reject_device_field(params.user_agent.as_ref(), "user_agent", "reset")?;
        reject_device_field(params.width, "width", "reset")?;
        reject_device_field(params.height, "height", "reset")?;
        reject_device_field(params.device_scale_factor, "device_scale_factor", "reset")?;
        reject_device_field(params.is_mobile, "is_mobile", "reset")?;
        reject_device_field(params.has_touch, "has_touch", "reset")?;
        reject_device_field(params.max_touch_points, "max_touch_points", "reset")?;
        return Ok(NormalizedBrowserDeviceParams {
            operation: BrowserDeviceOperation::Reset,
            descriptor: None,
        });
    }

    let user_agent = validate_device_user_agent(params.user_agent.as_deref())?;
    let width = params.width.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} operation=set requires width"),
        )
    })?;
    let height = params.height.ok_or_else(|| {
        mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} operation=set requires height"),
        )
    })?;
    validate_device_dimension("width", width)?;
    validate_device_dimension("height", height)?;
    let device_scale_factor = params.device_scale_factor.unwrap_or(1.0);
    if !device_scale_factor.is_finite()
        || device_scale_factor <= 0.0
        || device_scale_factor > synapse_a11y::CDP_DEVICE_SCALE_FACTOR_MAX
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{DEVICE_TOOL} device_scale_factor must be finite and in 0..={}",
                synapse_a11y::CDP_DEVICE_SCALE_FACTOR_MAX
            ),
        ));
    }
    let is_mobile = params.is_mobile.unwrap_or(false);
    let has_touch = params.has_touch.unwrap_or(false);
    let max_touch_points = params
        .max_touch_points
        .unwrap_or(if has_touch { 5 } else { 0 });
    if has_touch {
        if max_touch_points == 0 || max_touch_points > synapse_a11y::CDP_DEVICE_MAX_TOUCH_POINTS {
            return Err(mcp_error(
                error_codes::TOOL_PARAMS_INVALID,
                format!(
                    "{DEVICE_TOOL} max_touch_points must be 1..={} when has_touch=true",
                    synapse_a11y::CDP_DEVICE_MAX_TOUCH_POINTS
                ),
            ));
        }
    } else if max_touch_points != 0 {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} max_touch_points must be 0 when has_touch=false"),
        ));
    }

    Ok(NormalizedBrowserDeviceParams {
        operation: BrowserDeviceOperation::Set,
        descriptor: Some(BrowserDeviceDescriptor {
            user_agent,
            width,
            height,
            device_scale_factor,
            is_mobile,
            has_touch,
            max_touch_points,
        }),
    })
}

fn validate_dimension(field: &str, value: u32) -> Result<(), ErrorData> {
    if value == 0 || value > synapse_a11y::CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{RESIZE_TOOL} {field} must be 1..={}",
                synapse_a11y::CDP_DEVICE_METRICS_MAX_DIMENSION
            ),
        ));
    }
    Ok(())
}

fn validate_device_dimension(field: &str, value: u32) -> Result<(), ErrorData> {
    if value == 0 || value > synapse_a11y::CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{DEVICE_TOOL} {field} must be 1..={}",
                synapse_a11y::CDP_DEVICE_METRICS_MAX_DIMENSION
            ),
        ));
    }
    Ok(())
}

fn validate_device_user_agent(value: Option<&str>) -> Result<String, ErrorData> {
    let Some(value) = value else {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} operation=set requires user_agent"),
        ));
    };
    if value.trim() != value || value.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} user_agent must be non-empty without surrounding whitespace"),
        ));
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} user_agent must not contain line breaks or NUL"),
        ));
    }
    if value.chars().count() > synapse_a11y::CDP_DEVICE_MAX_USER_AGENT_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{DEVICE_TOOL} user_agent must be at most {} Unicode scalar values",
                synapse_a11y::CDP_DEVICE_MAX_USER_AGENT_CHARS
            ),
        ));
    }
    Ok(value.to_owned())
}

fn reject_resize_field<T>(value: Option<T>, field: &str, operation: &str) -> Result<(), ErrorData> {
    if value.is_none() {
        Ok(())
    } else {
        Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{RESIZE_TOOL} {field} is not valid for operation={operation}"),
        ))
    }
}

fn reject_device_field<T>(value: Option<T>, field: &str, operation: &str) -> Result<(), ErrorData> {
    if value.is_none() {
        Ok(())
    } else {
        Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{DEVICE_TOOL} {field} is not valid for operation={operation}"),
        ))
    }
}

fn browser_resize_response(
    session_id: &str,
    window_hwnd: i64,
    result: synapse_a11y::CdpViewportResult,
) -> BrowserResizeResponse {
    BrowserResizeResponse {
        session_id: session_id.to_owned(),
        window_hwnd,
        transport: "raw_cdp".to_owned(),
        endpoint: result.endpoint,
        cdp_target_id: result.cdp_target_id,
        operation: match result.operation.as_str() {
            "reset" => BrowserResizeOperation::Reset,
            _ => BrowserResizeOperation::Set,
        },
        requested: result.requested.map(|requested| BrowserViewportOverride {
            width: requested.width,
            height: requested.height,
            device_scale_factor: requested.device_scale_factor,
            mobile: requested.mobile,
        }),
        page_url: result.page_url,
        page_title: result.page_title,
        ready_state: result.ready_state,
        viewport: BrowserViewportReadback {
            inner_width: result.readback.inner_width,
            inner_height: result.readback.inner_height,
            device_pixel_ratio: result.readback.device_pixel_ratio,
            screen_width: result.readback.screen_width,
            screen_height: result.readback.screen_height,
            outer_width: result.readback.outer_width,
            outer_height: result.readback.outer_height,
            visual_viewport_width: result.readback.visual_viewport_width,
            visual_viewport_height: result.readback.visual_viewport_height,
        },
        readback_backend: "Emulation.setDeviceMetricsOverride / Emulation.clearDeviceMetricsOverride + Runtime.evaluate".to_owned(),
        backend_tier_used: "cdp".to_owned(),
        required_foreground: false,
        source_of_truth: "raw CDP Runtime.evaluate window.innerWidth/window.innerHeight/devicePixelRatio".to_owned(),
    }
}

fn browser_device_response(
    session_id: &str,
    window_hwnd: i64,
    result: synapse_a11y::CdpDeviceResult,
) -> BrowserDeviceResponse {
    BrowserDeviceResponse {
        session_id: session_id.to_owned(),
        window_hwnd,
        transport: "raw_cdp".to_owned(),
        endpoint: result.endpoint,
        cdp_target_id: result.cdp_target_id,
        operation: match result.operation.as_str() {
            "reset" => BrowserDeviceOperation::Reset,
            _ => BrowserDeviceOperation::Set,
        },
        descriptor: result.descriptor.map(|descriptor| BrowserDeviceDescriptor {
            user_agent: descriptor.user_agent,
            width: descriptor.width,
            height: descriptor.height,
            device_scale_factor: descriptor.device_scale_factor,
            is_mobile: descriptor.is_mobile,
            has_touch: descriptor.has_touch,
            max_touch_points: descriptor.max_touch_points,
        }),
        restored_user_agent: result.restored_user_agent,
        page_url: result.page_url,
        page_title: result.page_title,
        ready_state: result.ready_state,
        device: BrowserDeviceReadback {
            viewport: BrowserViewportReadback {
                inner_width: result.readback.viewport.inner_width,
                inner_height: result.readback.viewport.inner_height,
                device_pixel_ratio: result.readback.viewport.device_pixel_ratio,
                screen_width: result.readback.viewport.screen_width,
                screen_height: result.readback.viewport.screen_height,
                outer_width: result.readback.viewport.outer_width,
                outer_height: result.readback.viewport.outer_height,
                visual_viewport_width: result.readback.viewport.visual_viewport_width,
                visual_viewport_height: result.readback.viewport.visual_viewport_height,
            },
            user_agent: result.readback.user_agent,
            max_touch_points: result.readback.max_touch_points,
            ontouchstart_available: result.readback.ontouchstart_available,
            pointer_coarse: result.readback.pointer_coarse,
            any_pointer_coarse: result.readback.any_pointer_coarse,
            hover_none: result.readback.hover_none,
            any_hover_none: result.readback.any_hover_none,
        },
        readback_backend: "Emulation.setUserAgentOverride + Emulation.setDeviceMetricsOverride + Emulation.setTouchEmulationEnabled + Runtime.evaluate".to_owned(),
        backend_tier_used: "cdp".to_owned(),
        required_foreground: false,
        source_of_truth:
            "raw CDP Runtime.evaluate navigator/userAgent/viewport/touch media queries".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_resize_validation_edges() {
        let set = validate_browser_resize_params(&BrowserResizeParams {
            width: Some(1280),
            height: Some(720),
            device_scale_factor: None,
            ..BrowserResizeParams::default()
        })
        .expect("valid set params");
        assert_eq!(set.operation, BrowserResizeOperation::Set);
        assert_eq!(set.device_scale_factor, Some(1.0));

        assert!(
            validate_browser_resize_params(&BrowserResizeParams {
                width: Some(1280),
                operation: BrowserResizeOperation::Reset,
                ..BrowserResizeParams::default()
            })
            .is_err()
        );
        assert!(
            validate_browser_resize_params(&BrowserResizeParams {
                width: Some(0),
                height: Some(720),
                ..BrowserResizeParams::default()
            })
            .is_err()
        );
        assert!(
            validate_browser_resize_params(&BrowserResizeParams {
                width: Some(1280),
                height: Some(720),
                device_scale_factor: Some(f64::INFINITY),
                ..BrowserResizeParams::default()
            })
            .is_err()
        );

        let reset = validate_browser_resize_params(&BrowserResizeParams {
            operation: BrowserResizeOperation::Reset,
            ..BrowserResizeParams::default()
        })
        .expect("valid reset params");
        assert_eq!(reset.operation, BrowserResizeOperation::Reset);
    }

    #[test]
    fn browser_resize_response_maps_viewport_readback() {
        let response = browser_resize_response(
            "session-1",
            0x2200,
            synapse_a11y::CdpViewportResult {
                endpoint: "ws://127.0.0.1/devtools/browser/1".to_owned(),
                cdp_target_id: "target-1".to_owned(),
                operation: "set".to_owned(),
                requested: Some(synapse_a11y::CdpViewportOverride {
                    width: 390,
                    height: 844,
                    device_scale_factor: 3.0,
                    mobile: false,
                }),
                page_url: "https://example.test/".to_owned(),
                page_title: "Example".to_owned(),
                ready_state: "complete".to_owned(),
                readback: synapse_a11y::CdpViewportReadback {
                    inner_width: 390,
                    inner_height: 844,
                    device_pixel_ratio: 3.0,
                    screen_width: 390,
                    screen_height: 844,
                    outer_width: 390,
                    outer_height: 844,
                    visual_viewport_width: Some(390.0),
                    visual_viewport_height: Some(844.0),
                },
            },
        );

        assert_eq!(response.operation, BrowserResizeOperation::Set);
        assert_eq!(response.viewport.inner_width, 390);
        assert_eq!(response.viewport.device_pixel_ratio, 3.0);
        assert_eq!(
            response.requested.as_ref().map(|requested| requested.width),
            Some(390)
        );
        assert!(!response.required_foreground);
    }

    #[test]
    fn browser_device_validation_edges() {
        let mobile = validate_browser_device_params(&BrowserDeviceParams {
            user_agent: Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Mobile/15E148".to_owned(),
            ),
            width: Some(390),
            height: Some(844),
            device_scale_factor: Some(3.0),
            is_mobile: Some(true),
            has_touch: Some(true),
            max_touch_points: None,
            ..BrowserDeviceParams::default()
        })
        .expect("valid mobile descriptor");
        let descriptor = mobile.descriptor.expect("descriptor");
        assert_eq!(descriptor.max_touch_points, 5);
        assert!(descriptor.is_mobile);
        assert!(descriptor.has_touch);

        assert!(
            validate_browser_device_params(&BrowserDeviceParams {
                operation: BrowserDeviceOperation::Reset,
                width: Some(390),
                ..BrowserDeviceParams::default()
            })
            .is_err()
        );
        assert!(
            validate_browser_device_params(&BrowserDeviceParams {
                user_agent: Some(" bad ".to_owned()),
                width: Some(390),
                height: Some(844),
                ..BrowserDeviceParams::default()
            })
            .is_err()
        );
        assert!(
            validate_browser_device_params(&BrowserDeviceParams {
                user_agent: Some("Desktop UA".to_owned()),
                width: Some(1280),
                height: Some(720),
                has_touch: Some(false),
                max_touch_points: Some(1),
                ..BrowserDeviceParams::default()
            })
            .is_err()
        );

        let reset = validate_browser_device_params(&BrowserDeviceParams {
            operation: BrowserDeviceOperation::Reset,
            ..BrowserDeviceParams::default()
        })
        .expect("valid reset");
        assert_eq!(reset.operation, BrowserDeviceOperation::Reset);
        assert!(reset.descriptor.is_none());
    }

    #[test]
    fn browser_device_response_maps_readback() {
        let response = browser_device_response(
            "session-1",
            0x2200,
            synapse_a11y::CdpDeviceResult {
                endpoint: "ws://127.0.0.1/devtools/browser/1".to_owned(),
                cdp_target_id: "target-1".to_owned(),
                operation: "set".to_owned(),
                descriptor: Some(synapse_a11y::CdpDeviceDescriptor {
                    user_agent: "Mobile UA".to_owned(),
                    width: 390,
                    height: 844,
                    device_scale_factor: 3.0,
                    is_mobile: true,
                    has_touch: true,
                    max_touch_points: 5,
                }),
                restored_user_agent: None,
                page_url: "https://example.test/".to_owned(),
                page_title: "Example".to_owned(),
                ready_state: "complete".to_owned(),
                readback: synapse_a11y::CdpDeviceReadback {
                    viewport: synapse_a11y::CdpViewportReadback {
                        inner_width: 390,
                        inner_height: 844,
                        device_pixel_ratio: 3.0,
                        screen_width: 390,
                        screen_height: 844,
                        outer_width: 390,
                        outer_height: 844,
                        visual_viewport_width: Some(390.0),
                        visual_viewport_height: Some(844.0),
                    },
                    user_agent: "Mobile UA".to_owned(),
                    max_touch_points: 5,
                    ontouchstart_available: true,
                    pointer_coarse: true,
                    any_pointer_coarse: true,
                    hover_none: true,
                    any_hover_none: true,
                },
            },
        );

        assert_eq!(response.operation, BrowserDeviceOperation::Set);
        assert_eq!(response.device.viewport.inner_width, 390);
        assert_eq!(response.device.user_agent, "Mobile UA");
        assert!(response.device.pointer_coarse);
        assert_eq!(
            response
                .descriptor
                .as_ref()
                .map(|descriptor| descriptor.max_touch_points),
            Some(5)
        );
        assert!(!response.required_foreground);
    }
}
