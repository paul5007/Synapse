//! Network capture listing tools (#1081) backed by the a11y CDP Network buffer.

use super::{
    ErrorData, Json, Parameters, SynapseService,
    m1_tools::{
        browser_raw_cdp_required_error, cdp_target_id_audit_ref, require_target_session_id,
        validate_cdp_target_id,
    },
    tool, tool_router,
};
use crate::m1::{BrowserNetworkWaitEntry, mcp_error};
use rmcp::{RoleServer, schemars::JsonSchema, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::json;
use synapse_core::error_codes;

const TOOL: &str = "browser_network_requests";
const DEFAULT_NETWORK_REQUEST_LIMIT: usize = 100;
const MAX_NETWORK_REQUEST_LIMIT: usize = 1000;
const MAX_NETWORK_FILTER_CHARS: usize = 8192;
const MAX_NETWORK_RESOURCE_TYPE_CHARS: usize = 128;

/// Parameters for `browser_network_requests` (#1081): return captured Network
/// request records for the calling session's owned CDP target.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestsParams {
    /// CDP TargetID to read. Defaults to the active session CDP target. Must be
    /// owned by this session; the human foreground tab is never an implicit fallback.
    #[serde(default)]
    pub cdp_target_id: Option<String>,
    /// Browser HWND that owns the target. Required only with an explicit
    /// `cdp_target_id` and no active session target.
    #[serde(default)]
    pub window_hwnd: Option<i64>,
    /// Return only records whose latest update sequence is >= this cursor.
    #[serde(default)]
    pub since_seq: Option<u64>,
    /// Maximum records to return after filtering. Defaults to 100, max 1000.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Case-insensitive substring filter against request URL.
    #[serde(default)]
    pub url_contains: Option<String>,
    /// Regular expression filter against request URL.
    #[serde(default)]
    pub url_regex: Option<String>,
    /// Case-insensitive CDP Network resource type filter.
    #[serde(default)]
    pub resource_type: Option<String>,
    /// Minimum HTTP status, inclusive.
    #[serde(default)]
    pub status_min: Option<i64>,
    /// Maximum HTTP status, inclusive.
    #[serde(default)]
    pub status_max: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_seq: Option<u64>,
    pub limit: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_max: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserNetworkRequestsResponse {
    pub session_id: String,
    pub window_hwnd: i64,
    pub transport: String,
    pub endpoint: String,
    pub cdp_target_id: String,
    pub capture_newly_armed: bool,
    pub next_cursor: u64,
    pub returned: usize,
    pub total_buffered: usize,
    pub dropped: u64,
    pub filters: BrowserNetworkRequestFilters,
    pub entries: Vec<BrowserNetworkWaitEntry>,
    pub readback_backend: String,
    pub backend_tier_used: String,
    pub required_foreground: bool,
}

#[derive(Debug)]
struct NormalizedBrowserNetworkRequestsParams {
    since_seq: Option<u64>,
    limit: usize,
    url_contains: Option<String>,
    url_regex_pattern: Option<String>,
    url_regex: Option<regex::Regex>,
    resource_type: Option<String>,
    status_min: Option<i64>,
    status_max: Option<i64>,
}

#[tool_router(router = browser_network_tool_router, vis = "pub(super)")]
impl SynapseService {
    #[tool(
        description = "List captured Network request records for the calling session's owned browser tab. Arms/reuses the target-scoped raw CDP Network buffer, returns cursor-delimited entries, and supports filters for url_contains, url_regex, resource_type, status_min/status_max, and since_seq. Target-scoped and background-safe: never activates the tab, never uses OS foreground input, and never falls back to the human foreground tab. Raw CDP only; the popup-safe normal Chrome extension bridge fails closed."
    )]
    pub async fn browser_network_requests(
        &self,
        params: Parameters<BrowserNetworkRequestsParams>,
        request_context: RequestContext<RoleServer>,
    ) -> Result<Json<BrowserNetworkRequestsResponse>, ErrorData> {
        tracing::info!(
            code = "MCP_TOOL_INVOCATION",
            kind = TOOL,
            "tool.invocation kind=browser_network_requests"
        );
        let session_id = require_target_session_id(&request_context)?;
        let filters = validate_browser_network_requests_params(&params.0)?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": params.0.window_hwnd,
            "requested_cdp_target": cdp_target_id_audit_ref(params.0.cdp_target_id.as_deref()),
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "url_regex_len": filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": filters.resource_type.as_deref(),
            "status_min": filters.status_min,
            "status_max": filters.status_max,
            "required_foreground": false,
            "phase": "target_resolution",
        });
        let resolution = self.resolve_cdp_tab_mutation_target(
            TOOL,
            &session_id,
            params.0.window_hwnd,
            params.0.cdp_target_id.as_deref(),
        );
        let (window_hwnd, cdp_target_id) = self.audit_cdp_target_resolution_result(
            TOOL,
            &session_id,
            &request_details,
            resolution,
        )?;
        let request_details = json!({
            "session_id": &session_id,
            "window_hwnd": window_hwnd,
            "cdp_target_id": &cdp_target_id,
            "since_seq": filters.since_seq,
            "limit": filters.limit,
            "url_contains_len": filters.url_contains.as_deref().map(str::len),
            "url_regex_len": filters.url_regex_pattern.as_deref().map(str::len),
            "resource_type": filters.resource_type.as_deref(),
            "status_min": filters.status_min,
            "status_max": filters.status_max,
            "required_foreground": false,
        });
        self.audit_action_started_with_details_for_session(TOOL, &request_details, &session_id)?;
        let result = self
            .browser_network_requests_impl(&session_id, window_hwnd, &cdp_target_id, &filters)
            .await;
        self.audit_action_result_for_session(TOOL, &result, &session_id)?;
        result.map(Json)
    }

    #[cfg(windows)]
    async fn browser_network_requests_impl(
        &self,
        session_id: &str,
        window_hwnd: i64,
        cdp_target_id: &str,
        filters: &NormalizedBrowserNetworkRequestsParams,
    ) -> Result<BrowserNetworkRequestsResponse, ErrorData> {
        let Some(endpoint) = synapse_a11y::endpoint_for_window(window_hwnd) else {
            return Err(browser_raw_cdp_required_error(TOOL, window_hwnd));
        };
        let capture = synapse_a11y::network_capture_ensure(
            &endpoint,
            cdp_target_id,
            synapse_a11y::DEFAULT_NETWORK_BUFFER_CAPACITY,
        )
        .await
        .map_err(|error| {
            mcp_error(
                error.code(),
                format!("{TOOL} raw CDP network capture failed: {error}"),
            )
        })?;
        let read = synapse_a11y::network_capture_read(
            cdp_target_id,
            &synapse_a11y::CdpNetworkReadFilter {
                since_seq: filters.since_seq,
                max: 0,
                ..Default::default()
            },
        )
        .ok_or_else(|| {
            mcp_error(
                error_codes::TOOL_INTERNAL_ERROR,
                format!("{TOOL} network capture was not armed for target {cdp_target_id}"),
            )
        })?;
        let entries = filter_network_entries(read.entries.into_iter(), filters)
            .into_iter()
            .take(filters.limit)
            .map(|entry| browser_network_entry_to_wire(&entry))
            .collect::<Vec<_>>();
        tracing::info!(
            code = "CDP_BACKGROUND_NETWORK_REQUESTS",
            session_id = %session_id,
            hwnd = window_hwnd,
            endpoint = %endpoint,
            cdp_target_id,
            returned = entries.len(),
            total_buffered = read.total_buffered,
            next_cursor = read.next_cursor,
            "readback=Network.event_buffer(browser_network_requests) outcome=list_returned"
        );
        Ok(BrowserNetworkRequestsResponse {
            session_id: session_id.to_owned(),
            window_hwnd,
            transport: "raw_cdp".to_owned(),
            endpoint,
            cdp_target_id: cdp_target_id.to_owned(),
            capture_newly_armed: capture.newly_armed,
            next_cursor: read.next_cursor,
            returned: entries.len(),
            total_buffered: read.total_buffered,
            dropped: read.dropped,
            filters: filters.to_wire(),
            entries,
            readback_backend: "Network event buffer(browser_network_requests)".to_owned(),
            backend_tier_used: "cdp".to_owned(),
            required_foreground: false,
        })
    }

    #[cfg(not(windows))]
    async fn browser_network_requests_impl(
        &self,
        _session_id: &str,
        _window_hwnd: i64,
        _cdp_target_id: &str,
        _filters: &NormalizedBrowserNetworkRequestsParams,
    ) -> Result<BrowserNetworkRequestsResponse, ErrorData> {
        Err(mcp_error(
            error_codes::A11Y_NOT_AVAILABLE,
            "browser_network_requests is only available on Windows in this build",
        ))
    }
}

impl NormalizedBrowserNetworkRequestsParams {
    fn to_wire(&self) -> BrowserNetworkRequestFilters {
        BrowserNetworkRequestFilters {
            since_seq: self.since_seq,
            limit: self.limit,
            url_contains: self.url_contains.clone(),
            url_regex: self.url_regex_pattern.clone(),
            resource_type: self.resource_type.clone(),
            status_min: self.status_min,
            status_max: self.status_max,
        }
    }
}

fn validate_browser_network_requests_params(
    params: &BrowserNetworkRequestsParams,
) -> Result<NormalizedBrowserNetworkRequestsParams, ErrorData> {
    if let Some(target_id) = params.cdp_target_id.as_deref() {
        validate_cdp_target_id(target_id)?;
    }
    let limit = params.limit.unwrap_or(DEFAULT_NETWORK_REQUEST_LIMIT);
    if !(1..=MAX_NETWORK_REQUEST_LIMIT).contains(&limit) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} limit must be 1..={MAX_NETWORK_REQUEST_LIMIT}"),
        ));
    }
    if params.url_contains.is_some() && params.url_regex.is_some() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} accepts url_contains or url_regex, not both"),
        ));
    }
    let url_contains = validate_text_filter("url_contains", params.url_contains.as_deref())?;
    let url_regex_pattern = validate_text_filter("url_regex", params.url_regex.as_deref())?;
    let url_regex = url_regex_pattern
        .as_deref()
        .map(|pattern| {
            regex::Regex::new(pattern).map_err(|error| {
                mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    format!("{TOOL} url_regex is invalid: {error}"),
                )
            })
        })
        .transpose()?;
    let resource_type = validate_resource_type(params.resource_type.as_deref())?;
    validate_status_bound("status_min", params.status_min)?;
    validate_status_bound("status_max", params.status_max)?;
    if let (Some(min), Some(max)) = (params.status_min, params.status_max)
        && min > max
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} status_min must be <= status_max"),
        ));
    }
    Ok(NormalizedBrowserNetworkRequestsParams {
        since_seq: params.since_seq,
        limit,
        url_contains,
        url_regex_pattern,
        url_regex,
        resource_type,
        status_min: params.status_min,
        status_max: params.status_max,
    })
}

fn validate_text_filter(field: &str, value: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} {field} must not be empty"),
        ));
    }
    if value.contains('\0') {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} {field} must not contain NUL"),
        ));
    }
    if value.chars().count() > MAX_NETWORK_FILTER_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{TOOL} {field} must be at most {MAX_NETWORK_FILTER_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn validate_resource_type(value: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} resource_type must not be empty"),
        ));
    }
    if value.trim() != value {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} resource_type must not contain leading or trailing whitespace"),
        ));
    }
    if value.contains('\0') || value.chars().any(char::is_control) {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} resource_type must not contain control characters"),
        ));
    }
    if value.chars().count() > MAX_NETWORK_RESOURCE_TYPE_CHARS {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!(
                "{TOOL} resource_type must be at most {MAX_NETWORK_RESOURCE_TYPE_CHARS} Unicode scalar values"
            ),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn validate_status_bound(field: &str, value: Option<i64>) -> Result<(), ErrorData> {
    if let Some(value) = value
        && !(0..=999).contains(&value)
    {
        return Err(mcp_error(
            error_codes::TOOL_PARAMS_INVALID,
            format!("{TOOL} {field} must be 0..=999"),
        ));
    }
    Ok(())
}

fn filter_network_entries(
    entries: impl Iterator<Item = synapse_a11y::CdpNetworkEntry>,
    filters: &NormalizedBrowserNetworkRequestsParams,
) -> Vec<synapse_a11y::CdpNetworkEntry> {
    entries
        .filter(|entry| network_entry_matches(entry, filters))
        .collect()
}

fn network_entry_matches(
    entry: &synapse_a11y::CdpNetworkEntry,
    filters: &NormalizedBrowserNetworkRequestsParams,
) -> bool {
    if let Some(resource_type) = filters.resource_type.as_deref()
        && !entry
            .resource_type
            .as_deref()
            .is_some_and(|entry_type| entry_type.eq_ignore_ascii_case(resource_type))
    {
        return false;
    }
    let status = entry.response.as_ref().map(|response| response.status);
    if let Some(min) = filters.status_min
        && !status.is_some_and(|status| status >= min)
    {
        return false;
    }
    if let Some(max) = filters.status_max
        && !status.is_some_and(|status| status <= max)
    {
        return false;
    }
    if let Some(needle) = filters.url_contains.as_deref()
        && !entry
            .url
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(&needle.to_lowercase())
    {
        return false;
    }
    if let Some(regex) = filters.url_regex.as_ref()
        && !entry.url.as_deref().is_some_and(|url| regex.is_match(url))
    {
        return false;
    }
    true
}

fn browser_network_entry_to_wire(entry: &synapse_a11y::CdpNetworkEntry) -> BrowserNetworkWaitEntry {
    let response = entry.response.as_ref();
    BrowserNetworkWaitEntry {
        seq: entry.seq,
        request_id: entry.request_id.clone(),
        url: entry.url.clone(),
        method: entry.method.clone(),
        resource_type: entry.resource_type.clone(),
        request_headers: entry.request_headers.clone(),
        response_received: entry.response_received,
        response_url: response.map(|response| response.url.clone()),
        status: response.map(|response| response.status),
        status_text: response.map(|response| response.status_text.clone()),
        response_headers: response.map(|response| response.headers.clone()),
        response_timing: response.and_then(|response| response.timing.clone()),
        protocol: response.and_then(|response| response.protocol.clone()),
        remote_ip_address: response.and_then(|response| response.remote_ip_address.clone()),
        remote_port: response.and_then(|response| response.remote_port),
        encoded_data_length: entry
            .encoded_data_length
            .or_else(|| response.map(|response| response.encoded_data_length)),
        loading_finished: entry.loading_finished,
        loading_failed: entry.loading_failed,
        failure_error_text: entry.failure_error_text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        seq: u64,
        request_id: &str,
        url: &str,
        resource_type: &str,
        status: Option<i64>,
    ) -> synapse_a11y::CdpNetworkEntry {
        let response = status.map(|status| synapse_a11y::CdpNetworkResponseSnapshot {
            url: url.to_owned(),
            status,
            status_text: "OK".to_owned(),
            headers: json!({"content-type": "application/json"}),
            request_headers: None,
            mime_type: "application/json".to_owned(),
            protocol: Some("h2".to_owned()),
            remote_ip_address: Some("127.0.0.1".to_owned()),
            remote_port: Some(443),
            encoded_data_length: 42.0,
            timing: Some(json!({"requestTime": 1.0})),
            response_time_ms: None,
            from_disk_cache: None,
            from_service_worker: None,
            from_prefetch_cache: None,
            from_early_hints: None,
            timestamp_s: Some(2.0),
            resource_type: Some(resource_type.to_owned()),
        });
        synapse_a11y::CdpNetworkEntry {
            seq,
            first_seq: seq,
            request_id: request_id.to_owned(),
            loader_id: Some("loader".to_owned()),
            frame_id: Some("frame".to_owned()),
            document_url: Some("https://example.test/".to_owned()),
            url: Some(url.to_owned()),
            method: Some("GET".to_owned()),
            resource_type: Some(resource_type.to_owned()),
            request_headers: Some(json!({"accept": "*/*"})),
            request_has_post_data: None,
            request_timestamp_s: Some(1.0),
            request_wall_time_ms: Some(1_710_000_000_000.0),
            initiator: None,
            redirects: Vec::new(),
            response_timestamp_s: response.as_ref().and_then(|r| r.timestamp_s),
            response_received: response.is_some(),
            response,
            loading_finished: true,
            loading_failed: false,
            finished_timestamp_s: Some(3.0),
            failed_timestamp_s: None,
            encoded_data_length: Some(84.0),
            failure_error_text: None,
            failure_canceled: None,
            failure_blocked_reason: None,
            failure_cors_error_status: None,
        }
    }

    #[test]
    fn browser_network_requests_validation_edges() {
        let ok = validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            cdp_target_id: Some("target-123".to_owned()),
            since_seq: Some(7),
            limit: Some(MAX_NETWORK_REQUEST_LIMIT),
            url_regex: Some(r"^https://example\.test/api".to_owned()),
            resource_type: Some("XHR".to_owned()),
            status_min: Some(200),
            status_max: Some(299),
            ..Default::default()
        })
        .expect("valid params pass");
        assert_eq!(ok.since_seq, Some(7));
        assert_eq!(ok.limit, MAX_NETWORK_REQUEST_LIMIT);
        assert!(
            ok.url_regex
                .as_ref()
                .unwrap()
                .is_match("https://example.test/api")
        );
        assert_eq!(ok.resource_type.as_deref(), Some("XHR"));

        for error in [
            validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
                limit: Some(0),
                ..Default::default()
            })
            .expect_err("zero limit must be rejected"),
            validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
                url_contains: Some("api".to_owned()),
                url_regex: Some("api".to_owned()),
                ..Default::default()
            })
            .expect_err("ambiguous URL filters must be rejected"),
            validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
                url_regex: Some("(".to_owned()),
                ..Default::default()
            })
            .expect_err("invalid URL regex must be rejected"),
            validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
                resource_type: Some(" XHR".to_owned()),
                ..Default::default()
            })
            .expect_err("resource type whitespace must be rejected"),
            validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
                status_min: Some(500),
                status_max: Some(200),
                ..Default::default()
            })
            .expect_err("inverted status range must be rejected"),
        ] {
            let code = error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(serde_json::Value::as_str);
            assert_eq!(code, Some(error_codes::TOOL_PARAMS_INVALID));
        }
    }

    #[test]
    fn browser_network_requests_filters_entries_after_cursor_read() {
        let filters = validate_browser_network_requests_params(&BrowserNetworkRequestsParams {
            url_contains: Some("/api/".to_owned()),
            resource_type: Some("XHR".to_owned()),
            status_min: Some(200),
            status_max: Some(299),
            ..Default::default()
        })
        .expect("filters validate");
        let filtered = filter_network_entries(
            vec![
                entry(1, "doc", "https://example.test/", "Document", Some(200)),
                entry(
                    2,
                    "api-ok",
                    "https://example.test/api/users",
                    "XHR",
                    Some(204),
                ),
                entry(
                    3,
                    "api-err",
                    "https://example.test/api/fail",
                    "XHR",
                    Some(500),
                ),
            ]
            .into_iter(),
            &filters,
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].request_id, "api-ok");
        let wire = browser_network_entry_to_wire(&filtered[0]);
        assert_eq!(wire.status, Some(204));
        assert_eq!(
            wire.response_headers,
            Some(json!({"content-type": "application/json"}))
        );
        assert_eq!(wire.encoded_data_length, Some(84.0));
    }
}
