//! Target-scoped raw-CDP browser emulation helpers (#1173/#1174).

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use serde::{Deserialize, Serialize};

use crate::{A11yError, A11yResult};

pub const CDP_DEVICE_METRICS_MAX_DIMENSION: u32 = 10_000_000;
pub const CDP_DEVICE_SCALE_FACTOR_MAX: f64 = 1000.0;
pub const CDP_DEVICE_MAX_TOUCH_POINTS: u32 = 16;
pub const CDP_DEVICE_MAX_USER_AGENT_CHARS: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpViewportOverride {
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpViewportReadback {
    pub inner_width: i64,
    pub inner_height: i64,
    pub device_pixel_ratio: f64,
    pub screen_width: i64,
    pub screen_height: i64,
    pub outer_width: i64,
    pub outer_height: i64,
    pub visual_viewport_width: Option<f64>,
    pub visual_viewport_height: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpViewportResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub requested: Option<CdpViewportOverride>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpViewportReadback,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpDeviceDescriptor {
    pub user_agent: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    pub is_mobile: bool,
    pub has_touch: bool,
    pub max_touch_points: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CdpDeviceReadback {
    pub viewport: CdpViewportReadback,
    pub user_agent: String,
    pub max_touch_points: i64,
    pub ontouchstart_available: bool,
    pub pointer_coarse: bool,
    pub any_pointer_coarse: bool,
    pub hover_none: bool,
    pub any_hover_none: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CdpDeviceResult {
    pub endpoint: String,
    pub cdp_target_id: String,
    pub operation: String,
    pub descriptor: Option<CdpDeviceDescriptor>,
    pub restored_user_agent: Option<String>,
    pub page_url: String,
    pub page_title: String,
    pub ready_state: String,
    pub readback: CdpDeviceReadback,
}

enum DeviceMetricsCommand {
    Set(CdpViewportOverride),
    Reset,
}

enum DeviceDescriptorCommand {
    Set(CdpDeviceDescriptor),
    Reset { original_user_agent: Option<String> },
}

fn original_user_agents() -> &'static Mutex<HashMap<String, String>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn device_registry_key(endpoint: &str, target_id: &str) -> String {
    format!("{endpoint}\0{target_id}")
}

/// Applies `Emulation.setDeviceMetricsOverride` to one CDP page target, then
/// reads back page-visible viewport metrics from the same target.
pub async fn cdp_set_viewport_size(
    endpoint: &str,
    target_id: &str,
    width: u32,
    height: u32,
    device_scale_factor: f64,
) -> A11yResult<CdpViewportResult> {
    validate_viewport_override(width, height, device_scale_factor)?;
    let requested = CdpViewportOverride {
        width,
        height,
        device_scale_factor,
        mobile: false,
    };
    run_device_metrics_command(
        endpoint,
        target_id,
        DeviceMetricsCommand::Set(requested.clone()),
    )
    .await?;
    let readback = viewport_readback(endpoint, target_id).await?;
    Ok(CdpViewportResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        requested: Some(requested),
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears `Emulation.setDeviceMetricsOverride` for one CDP page target, then
/// reads back the real page-visible viewport metrics from that target.
pub async fn cdp_reset_viewport_size(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpViewportResult> {
    run_device_metrics_command(endpoint, target_id, DeviceMetricsCommand::Reset).await?;
    let readback = viewport_readback(endpoint, target_id).await?;
    Ok(CdpViewportResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        requested: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Applies a Playwright-style device descriptor to one CDP page target: user
/// agent, viewport/DPR/mobile metrics, and touch capability in one operation.
pub async fn cdp_apply_device_descriptor(
    endpoint: &str,
    target_id: &str,
    descriptor: CdpDeviceDescriptor,
) -> A11yResult<CdpDeviceResult> {
    validate_device_descriptor(&descriptor)?;
    let key = device_registry_key(endpoint, target_id);
    let original_readback = device_readback(endpoint, target_id).await?;
    if let Ok(mut originals) = original_user_agents().lock() {
        originals
            .entry(key)
            .or_insert_with(|| original_readback.metrics.user_agent.clone());
    }
    run_device_descriptor_command(
        endpoint,
        target_id,
        DeviceDescriptorCommand::Set(descriptor.clone()),
    )
    .await?;
    let readback = device_readback(endpoint, target_id).await?;
    Ok(CdpDeviceResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "set".to_owned(),
        descriptor: Some(descriptor),
        restored_user_agent: None,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

/// Clears the active device descriptor for one CDP page target. Device metrics
/// and touch emulation are cleared through CDP. The user agent is restored to
/// the value observed before the first descriptor set in this process.
pub async fn cdp_reset_device_descriptor(
    endpoint: &str,
    target_id: &str,
) -> A11yResult<CdpDeviceResult> {
    let key = device_registry_key(endpoint, target_id);
    let original_user_agent = original_user_agents()
        .lock()
        .ok()
        .and_then(|mut originals| originals.remove(&key));
    run_device_descriptor_command(
        endpoint,
        target_id,
        DeviceDescriptorCommand::Reset {
            original_user_agent: original_user_agent.clone(),
        },
    )
    .await?;
    let readback = device_readback(endpoint, target_id).await?;
    Ok(CdpDeviceResult {
        endpoint: endpoint.to_owned(),
        cdp_target_id: readback.target_id,
        operation: "reset".to_owned(),
        descriptor: None,
        restored_user_agent: original_user_agent,
        page_url: readback.url,
        page_title: readback.title,
        ready_state: readback.ready_state,
        readback: readback.metrics,
    })
}

fn validate_viewport_override(width: u32, height: u32, device_scale_factor: f64) -> A11yResult<()> {
    if width == 0 || width > CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "viewport width must be 1..={CDP_DEVICE_METRICS_MAX_DIMENSION}, got {width}"
            ),
        });
    }
    if height == 0 || height > CDP_DEVICE_METRICS_MAX_DIMENSION {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "viewport height must be 1..={CDP_DEVICE_METRICS_MAX_DIMENSION}, got {height}"
            ),
        });
    }
    if !device_scale_factor.is_finite()
        || device_scale_factor <= 0.0
        || device_scale_factor > CDP_DEVICE_SCALE_FACTOR_MAX
    {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "device_scale_factor must be finite and in 0..={CDP_DEVICE_SCALE_FACTOR_MAX}, got {device_scale_factor}"
            ),
        });
    }
    Ok(())
}

fn validate_device_descriptor(descriptor: &CdpDeviceDescriptor) -> A11yResult<()> {
    validate_user_agent(&descriptor.user_agent)?;
    validate_viewport_override(
        descriptor.width,
        descriptor.height,
        descriptor.device_scale_factor,
    )?;
    if descriptor.has_touch {
        if descriptor.max_touch_points == 0
            || descriptor.max_touch_points > CDP_DEVICE_MAX_TOUCH_POINTS
        {
            return Err(A11yError::CdpAxtreeFailed {
                detail: format!(
                    "max_touch_points must be 1..={CDP_DEVICE_MAX_TOUCH_POINTS} when has_touch=true, got {}",
                    descriptor.max_touch_points
                ),
            });
        }
    } else if descriptor.max_touch_points != 0 {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "max_touch_points must be 0 when has_touch=false, got {}",
                descriptor.max_touch_points
            ),
        });
    }
    Ok(())
}

fn validate_user_agent(value: &str) -> A11yResult<()> {
    if value.trim() != value || value.is_empty() {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "device descriptor user_agent must be non-empty without surrounding whitespace"
                .to_owned(),
        });
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(A11yError::CdpAxtreeFailed {
            detail: "device descriptor user_agent must not contain line breaks or NUL".to_owned(),
        });
    }
    if value.chars().count() > CDP_DEVICE_MAX_USER_AGENT_CHARS {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "device descriptor user_agent must be at most {CDP_DEVICE_MAX_USER_AGENT_CHARS} Unicode scalar values"
            ),
        });
    }
    Ok(())
}

async fn run_device_metrics_command(
    endpoint: &str,
    target_id: &str,
    command: DeviceMetricsCommand,
) -> A11yResult<()> {
    use chromiumoxide::Browser;
    use chromiumoxide::cdp::browser_protocol::emulation::{
        ClearDeviceMetricsOverrideParams, SetDeviceMetricsOverrideParams,
    };
    use futures_util::StreamExt as _;

    let (browser, mut handler) =
        Browser::connect(endpoint)
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("connect {endpoint}: {err}"),
            })?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = crate::cdp_action::get_target_page_with_discovery(&browser, target_id).await?;
        match command {
            DeviceMetricsCommand::Set(override_metrics) => {
                let params = SetDeviceMetricsOverrideParams::builder()
                    .width(i64::from(override_metrics.width))
                    .height(i64::from(override_metrics.height))
                    .device_scale_factor(override_metrics.device_scale_factor)
                    .mobile(override_metrics.mobile)
                    .screen_width(i64::from(override_metrics.width))
                    .screen_height(i64::from(override_metrics.height))
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride params: {err}"),
                    })?;
                page.execute(params)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride: {err}"),
                    })?;
            }
            DeviceMetricsCommand::Reset => {
                page.execute(ClearDeviceMetricsOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.clearDeviceMetricsOverride: {err}"),
                    })?;
            }
        }
        Ok(())
    }
    .await;

    handler_task.abort();
    result
}

async fn run_device_descriptor_command(
    endpoint: &str,
    target_id: &str,
    command: DeviceDescriptorCommand,
) -> A11yResult<()> {
    use chromiumoxide::Browser;
    use chromiumoxide::cdp::browser_protocol::emulation::{
        ClearDeviceMetricsOverrideParams, SetDeviceMetricsOverrideParams,
        SetEmitTouchEventsForMouseConfiguration, SetEmitTouchEventsForMouseParams,
        SetTouchEmulationEnabledParams, SetUserAgentOverrideParams,
    };
    use futures_util::StreamExt as _;

    let (browser, mut handler) =
        Browser::connect(endpoint)
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("connect {endpoint}: {err}"),
            })?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = crate::cdp_action::get_target_page_with_discovery(&browser, target_id).await?;
        match command {
            DeviceDescriptorCommand::Set(descriptor) => {
                let user_agent = SetUserAgentOverrideParams::builder()
                    .user_agent(descriptor.user_agent.clone())
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setUserAgentOverride params: {err}"),
                    })?;
                page.execute(user_agent)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setUserAgentOverride: {err}"),
                    })?;

                let metrics = SetDeviceMetricsOverrideParams::builder()
                    .width(i64::from(descriptor.width))
                    .height(i64::from(descriptor.height))
                    .device_scale_factor(descriptor.device_scale_factor)
                    .mobile(descriptor.is_mobile)
                    .screen_width(i64::from(descriptor.width))
                    .screen_height(i64::from(descriptor.height))
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride params: {err}"),
                    })?;
                page.execute(metrics)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setDeviceMetricsOverride: {err}"),
                    })?;

                let mut touch =
                    SetTouchEmulationEnabledParams::builder().enabled(descriptor.has_touch);
                if descriptor.has_touch {
                    touch = touch.max_touch_points(i64::from(descriptor.max_touch_points));
                }
                page.execute(touch.build().map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Emulation.setTouchEmulationEnabled params: {err}"),
                })?)
                .await
                .map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("Emulation.setTouchEmulationEnabled: {err}"),
                })?;

                let touch_config = if descriptor.is_mobile {
                    SetEmitTouchEventsForMouseConfiguration::Mobile
                } else {
                    SetEmitTouchEventsForMouseConfiguration::Desktop
                };
                let emit = SetEmitTouchEventsForMouseParams::builder()
                    .enabled(descriptor.has_touch)
                    .configuration(touch_config)
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse params: {err}"),
                    })?;
                page.execute(emit)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse: {err}"),
                    })?;
            }
            DeviceDescriptorCommand::Reset {
                original_user_agent,
            } => {
                page.execute(ClearDeviceMetricsOverrideParams::default())
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.clearDeviceMetricsOverride: {err}"),
                    })?;
                page.execute(SetTouchEmulationEnabledParams::new(false))
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setTouchEmulationEnabled(false): {err}"),
                    })?;
                let emit = SetEmitTouchEventsForMouseParams::builder()
                    .enabled(false)
                    .configuration(SetEmitTouchEventsForMouseConfiguration::Desktop)
                    .build()
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse params: {err}"),
                    })?;
                page.execute(emit)
                    .await
                    .map_err(|err| A11yError::CdpAxtreeFailed {
                        detail: format!("Emulation.setEmitTouchEventsForMouse(false): {err}"),
                    })?;
                if let Some(original_user_agent) = original_user_agent {
                    let user_agent = SetUserAgentOverrideParams::builder()
                        .user_agent(original_user_agent)
                        .build()
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setUserAgentOverride restore params: {err}"),
                        })?;
                    page.execute(user_agent)
                        .await
                        .map_err(|err| A11yError::CdpAxtreeFailed {
                            detail: format!("Emulation.setUserAgentOverride restore: {err}"),
                        })?;
                }
            }
        }
        Ok(())
    }
    .await;

    handler_task.abort();
    result
}

struct ViewportReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpViewportReadback,
}

struct DeviceReadback {
    target_id: String,
    url: String,
    title: String,
    ready_state: String,
    metrics: CdpDeviceReadback,
}

async fn viewport_readback(endpoint: &str, target_id: &str) -> A11yResult<ViewportReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        VIEWPORT_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpViewportReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("viewport metrics readback decode: {error}"),
            }
        })?;
    Ok(ViewportReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

async fn device_readback(endpoint: &str, target_id: &str) -> A11yResult<DeviceReadback> {
    let evaluated = crate::cdp_action::cdp_evaluate_expression(
        endpoint,
        target_id,
        DEVICE_READBACK_JS,
        false,
        true,
    )
    .await?;
    let metrics =
        serde_json::from_value::<CdpDeviceReadback>(evaluated.value).map_err(|error| {
            A11yError::CdpAxtreeFailed {
                detail: format!("device descriptor readback decode: {error}"),
            }
        })?;
    Ok(DeviceReadback {
        target_id: evaluated.target_id,
        url: evaluated.url,
        title: evaluated.title,
        ready_state: evaluated.ready_state,
        metrics,
    })
}

const VIEWPORT_READBACK_JS: &str = r#"(() => {
  const viewport = globalThis.visualViewport || null;
  return {
    inner_width: Math.round(globalThis.innerWidth || 0),
    inner_height: Math.round(globalThis.innerHeight || 0),
    device_pixel_ratio: Number(globalThis.devicePixelRatio || 0),
    screen_width: Math.round(globalThis.screen ? globalThis.screen.width || 0 : 0),
    screen_height: Math.round(globalThis.screen ? globalThis.screen.height || 0 : 0),
    outer_width: Math.round(globalThis.outerWidth || 0),
    outer_height: Math.round(globalThis.outerHeight || 0),
    visual_viewport_width: viewport ? Number(viewport.width) : null,
    visual_viewport_height: viewport ? Number(viewport.height) : null
  };
})()"#;

const DEVICE_READBACK_JS: &str = r#"(() => {
  const viewport = globalThis.visualViewport || null;
  const media = query => {
    try { return Boolean(globalThis.matchMedia && globalThis.matchMedia(query).matches); }
    catch (_error) { return false; }
  };
  return {
    viewport: {
      inner_width: Math.round(globalThis.innerWidth || 0),
      inner_height: Math.round(globalThis.innerHeight || 0),
      device_pixel_ratio: Number(globalThis.devicePixelRatio || 0),
      screen_width: Math.round(globalThis.screen ? globalThis.screen.width || 0 : 0),
      screen_height: Math.round(globalThis.screen ? globalThis.screen.height || 0 : 0),
      outer_width: Math.round(globalThis.outerWidth || 0),
      outer_height: Math.round(globalThis.outerHeight || 0),
      visual_viewport_width: viewport ? Number(viewport.width) : null,
      visual_viewport_height: viewport ? Number(viewport.height) : null
    },
    user_agent: String(globalThis.navigator ? globalThis.navigator.userAgent || "" : ""),
    max_touch_points: Number(globalThis.navigator ? globalThis.navigator.maxTouchPoints || 0 : 0),
    ontouchstart_available: Boolean("ontouchstart" in globalThis),
    pointer_coarse: media("(pointer: coarse)"),
    any_pointer_coarse: media("(any-pointer: coarse)"),
    hover_none: media("(hover: none)"),
    any_hover_none: media("(any-hover: none)")
  };
})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_override_validation_edges() {
        assert!(validate_viewport_override(1280, 720, 1.25).is_ok());
        assert!(validate_viewport_override(0, 720, 1.0).is_err());
        assert!(validate_viewport_override(1280, 0, 1.0).is_err());
        assert!(
            validate_viewport_override(CDP_DEVICE_METRICS_MAX_DIMENSION + 1, 720, 1.0).is_err()
        );
        assert!(validate_viewport_override(1280, 720, 0.0).is_err());
        assert!(validate_viewport_override(1280, 720, f64::NAN).is_err());
    }

    #[test]
    fn device_descriptor_validation_edges() {
        let mobile = CdpDeviceDescriptor {
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) Mobile/15E148"
                .to_owned(),
            width: 390,
            height: 844,
            device_scale_factor: 3.0,
            is_mobile: true,
            has_touch: true,
            max_touch_points: 5,
        };
        assert!(validate_device_descriptor(&mobile).is_ok());

        let mut bad_ua = mobile.clone();
        bad_ua.user_agent = " bad ".to_owned();
        assert!(validate_device_descriptor(&bad_ua).is_err());

        let mut no_touch_points = mobile.clone();
        no_touch_points.max_touch_points = 0;
        assert!(validate_device_descriptor(&no_touch_points).is_err());

        let desktop = CdpDeviceDescriptor {
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_owned(),
            width: 1280,
            height: 720,
            device_scale_factor: 1.0,
            is_mobile: false,
            has_touch: false,
            max_touch_points: 0,
        };
        assert!(validate_device_descriptor(&desktop).is_ok());

        let mut desktop_with_touch_points = desktop;
        desktop_with_touch_points.max_touch_points = 1;
        assert!(validate_device_descriptor(&desktop_with_touch_points).is_err());
    }
}
