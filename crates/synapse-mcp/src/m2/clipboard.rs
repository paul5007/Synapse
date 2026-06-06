use std::time::Instant;

use rmcp::ErrorData;
use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use synapse_action::{ActionError, ClipboardFormat};
use synapse_core::error_codes;

use crate::m1::mcp_error;
use crate::m2::postcondition::{
    ActPostcondition, default_verify_timeout_ms, no_observed_delta_error,
    postcondition_failed_error, postcondition_not_requested, postcondition_observed_delta,
    text_signature,
};

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActClipboardParams {
    pub verb: ActClipboardVerb,
    pub text: Option<String>,
    #[serde(default = "default_clipboard_format")]
    #[schemars(default = "default_clipboard_format")]
    pub format: ActClipboardFormat,
    #[serde(default)]
    #[schemars(default)]
    pub verify_delta: bool,
    #[serde(default = "default_verify_timeout_ms")]
    #[schemars(default = "default_verify_timeout_ms", range(min = 50, max = 5000))]
    pub verify_timeout_ms: u32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActClipboardVerb {
    Read,
    Write,
    Clear,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActClipboardFormat {
    Text,
    Unicode,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActClipboardResponse {
    pub ok: bool,
    pub verb: ActClipboardVerb,
    pub format: ActClipboardFormat,
    pub written: bool,
    pub cleared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_len: Option<usize>,
    pub elapsed_ms: u32,
    pub postcondition: ActPostcondition,
}

pub async fn act_clipboard(params: ActClipboardParams) -> Result<ActClipboardResponse, ErrorData> {
    validate_params(&params)?;
    let started = Instant::now();
    let format = params.format.to_clipboard_format();
    let before_text = if params.verify_delta && !matches!(params.verb, ActClipboardVerb::Read) {
        Some(
            synapse_action::read_clipboard_text(format)
                .map_err(|error| action_error_to_mcp(&error))?,
        )
    } else {
        None
    };
    let response = match params.verb {
        ActClipboardVerb::Read => {
            let text = synapse_action::read_clipboard_text(format)
                .map_err(|error| action_error_to_mcp(&error))?;
            ActClipboardResponse {
                ok: true,
                verb: params.verb,
                format: params.format,
                written: false,
                cleared: false,
                text_len: Some(text.chars().count()),
                text: Some(text),
                elapsed_ms: elapsed_ms(started),
                postcondition: postcondition_not_requested("act_clipboard", "clipboard_text"),
            }
        }
        ActClipboardVerb::Write => {
            let text = params
                .text
                .as_deref()
                .ok_or_else(missing_write_text_error)?;
            synapse_action::write_clipboard_text(format, text)
                .map_err(|error| action_error_to_mcp(&error))?;
            ActClipboardResponse {
                ok: true,
                verb: params.verb,
                format: params.format,
                written: true,
                cleared: false,
                text: None,
                text_len: Some(text.chars().count()),
                elapsed_ms: elapsed_ms(started),
                postcondition: postcondition_not_requested("act_clipboard", "clipboard_text"),
            }
        }
        ActClipboardVerb::Clear => {
            synapse_action::clear_clipboard().map_err(|error| action_error_to_mcp(&error))?;
            ActClipboardResponse {
                ok: true,
                verb: params.verb,
                format: params.format,
                written: false,
                cleared: true,
                text: None,
                text_len: None,
                elapsed_ms: elapsed_ms(started),
                postcondition: postcondition_not_requested("act_clipboard", "clipboard_text"),
            }
        }
    };
    let response = if let Some(before) = before_text {
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(
            params.verify_timeout_ms,
        )))
        .await;
        let after = synapse_action::read_clipboard_text(format)
            .map_err(|error| action_error_to_mcp(&error))?;
        verify_clipboard_delta(response, &params, before, after)?
    } else {
        response
    };
    tracing::info!(
        code = "M2_ACT_CLIPBOARD_READBACK",
        kind = "act_clipboard",
        verb = response.verb.as_str(),
        format = response.format.as_str(),
        written = response.written,
        cleared = response.cleared,
        text_len = response.text_len,
        "readback=clipboard_backend tool=act_clipboard after_operation_readback"
    );
    Ok(response)
}

impl ActClipboardFormat {
    const fn to_clipboard_format(self) -> ClipboardFormat {
        match self {
            Self::Text => ClipboardFormat::Text,
            Self::Unicode => ClipboardFormat::Unicode,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Unicode => "unicode",
        }
    }
}

impl ActClipboardVerb {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Clear => "clear",
        }
    }
}

fn validate_params(params: &ActClipboardParams) -> Result<(), ErrorData> {
    match params.verb {
        ActClipboardVerb::Write => {
            if params.text.is_none() {
                return Err(missing_write_text_error());
            }
        }
        ActClipboardVerb::Read | ActClipboardVerb::Clear => {
            if params.text.is_some() {
                return Err(mcp_error(
                    error_codes::TOOL_PARAMS_INVALID,
                    "act_clipboard text is only valid with verb=write",
                ));
            }
        }
    }
    Ok(())
}

fn missing_write_text_error() -> ErrorData {
    mcp_error(
        error_codes::TOOL_PARAMS_INVALID,
        "act_clipboard verb=write requires text",
    )
}

fn action_error_to_mcp(error: &ActionError) -> ErrorData {
    mcp_error(error.code(), error.to_string())
}

fn verify_clipboard_delta(
    mut response: ActClipboardResponse,
    params: &ActClipboardParams,
    before: String,
    after: String,
) -> Result<ActClipboardResponse, ErrorData> {
    let before_signature = text_signature(&before);
    let after_signature = text_signature(&after);
    if before == after {
        return Err(no_observed_delta_error(
            "act_clipboard",
            "clipboard_text",
            params.verify_timeout_ms,
            before_signature,
            after_signature,
            serde_json::json!({
                "verb": params.verb,
                "format": params.format,
                "before_len": before.chars().count(),
                "after_len": after.chars().count(),
            }),
        ));
    }
    if matches!(params.verb, ActClipboardVerb::Write)
        && after != params.text.as_deref().unwrap_or_default()
    {
        return Err(postcondition_failed_error(
            "act_clipboard",
            "clipboard_text",
            "clipboard text changed but did not equal requested write text",
            before_signature,
            after_signature,
            serde_json::json!({
                "verb": params.verb,
                "format": params.format,
                "expected_len": params.text.as_ref().map(|text| text.chars().count()),
                "after_len": after.chars().count(),
            }),
        ));
    }
    if matches!(params.verb, ActClipboardVerb::Clear) && !after.is_empty() {
        return Err(postcondition_failed_error(
            "act_clipboard",
            "clipboard_text",
            "clipboard text changed but was not empty after clear",
            before_signature,
            after_signature,
            serde_json::json!({
                "verb": params.verb,
                "format": params.format,
                "after_len": after.chars().count(),
            }),
        ));
    }
    response.postcondition = postcondition_observed_delta(
        "act_clipboard",
        "clipboard_text",
        before_signature,
        after_signature,
        "observed clipboard text Source-of-Truth change",
    );
    Ok(response)
}

const fn default_clipboard_format() -> ActClipboardFormat {
    ActClipboardFormat::Unicode
}

fn elapsed_ms(started: Instant) -> u32 {
    u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_format_non_ascii_reaches_backend_validation() {
        let params = ActClipboardParams {
            verb: ActClipboardVerb::Write,
            text: Some("unicode-clipboard-edge-雪".to_owned()),
            format: ActClipboardFormat::Text,
            verify_delta: false,
            verify_timeout_ms: default_verify_timeout_ms(),
        };

        assert!(validate_params(&params).is_ok());
    }
}
