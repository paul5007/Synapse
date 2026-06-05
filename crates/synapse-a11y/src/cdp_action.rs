//! CDP-routed actions on web DOM nodes (#686).
//!
//! When an action targets a web node (an element id carrying the
//! [`crate::CDP_RUNTIME_PREFIX`] sentinel), the action layer routes it here
//! instead of UIA/`SendInput`. We attach CDP, locate the page that owns the
//! node, scroll it into view, resolve its live box model, and dispatch via
//! `Input.dispatchMouseEvent` / `Input.insertText` in **viewport CSS
//! coordinates** — which sidesteps the DPI / scroll / window-content-origin
//! mapping that screen-coordinate clicking would need, and works regardless of
//! the node's initial scroll position.
//!
//! Everything here is `cfg(windows)` because it depends on `chromiumoxide`.

#![cfg(windows)]

use chromiumoxide::Browser;
use chromiumoxide::cdp::browser_protocol::dom::{
    BackendNodeId, GetBoxModelParams, ScrollIntoViewIfNeededParams,
};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchMouseEventParams, DispatchMouseEventType, InsertTextParams, MouseButton,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, GetLayoutMetricsParams, Viewport,
};
use chromiumoxide::page::ScreenshotParams;
use futures_util::StreamExt as _;

use crate::{A11yError, A11yResult, cdp_dom::rect_from_quad};

/// Where a CDP action landed, in viewport CSS coordinates (diagnostics).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CdpActionPoint {
    pub x: f64,
    pub y: f64,
}

/// Which pointer button a CDP click uses.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CdpMouseButton {
    Left,
    Right,
    Middle,
}

impl CdpMouseButton {
    const fn to_cdp(self) -> MouseButton {
        match self {
            Self::Left => MouseButton::Left,
            Self::Right => MouseButton::Right,
            Self::Middle => MouseButton::Middle,
        }
    }
}

/// Clicks a web node `click_count` times with `button`, after scrolling it into
/// view. Returns the viewport point clicked.
///
/// # Errors
///
/// `A11Y_CDP_ATTACH_FAILED` if the endpoint/node cannot be reached;
/// `A11Y_CDP_AXTREE_FAILED` if box-model resolution or dispatch fails.
pub async fn cdp_click_node(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
    button: CdpMouseButton,
    click_count: i64,
) -> A11yResult<CdpActionPoint> {
    with_node_center(
        endpoint,
        page_title_hint,
        backend_node_id,
        |page, center| async move {
            page.execute(mouse_event(
                DispatchMouseEventType::MouseMoved,
                center,
                button.to_cdp(),
                0,
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            page.execute(mouse_event(
                DispatchMouseEventType::MousePressed,
                center,
                button.to_cdp(),
                click_count.max(1),
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            page.execute(mouse_event(
                DispatchMouseEventType::MouseReleased,
                center,
                button.to_cdp(),
                click_count.max(1),
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            Ok(center)
        },
    )
    .await
}

/// Focuses a web input node and inserts `text` (as if typed).
///
/// # Errors
///
/// As [`cdp_click_node`].
pub async fn cdp_type_node(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
    text: &str,
) -> A11yResult<()> {
    use chromiumoxide::cdp::browser_protocol::dom::FocusParams;

    let text = text.to_owned();
    with_node_center(
        endpoint,
        page_title_hint,
        backend_node_id,
        |page, center| async move {
            // Click to place the caret, then focus and insert text.
            page.execute(mouse_event(
                DispatchMouseEventType::MousePressed,
                center,
                MouseButton::Left,
                1,
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            page.execute(mouse_event(
                DispatchMouseEventType::MouseReleased,
                center,
                MouseButton::Left,
                1,
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            // The click above already places focus/caret in the field. DOM.focus is
            // a best-effort reinforcement — some nodes (e.g. the AX node maps to a
            // non-focusable wrapper) report "not focusable", which must not abort the
            // insert when the click already focused the input.
            let focus = FocusParams::builder()
                .backend_node_id(BackendNodeId::new(backend_node_id))
                .build();
            let _ = page.execute(focus).await;
            page.execute(InsertTextParams::new(text))
                .await
                .map_err(|err| dispatch_err(&err))?;
            Ok(center)
        },
    )
    .await
    .map(|_point| ())
}

/// Resolves the viewport-CSS centre of a web node (for `act_stroke` target
/// aiming), scrolling it into view first.
///
/// # Errors
///
/// As [`cdp_click_node`].
pub async fn cdp_node_viewport_center(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
) -> A11yResult<CdpActionPoint> {
    with_node_center(
        endpoint,
        page_title_hint,
        backend_node_id,
        |_page, center| async move { Ok(center) },
    )
    .await
}

/// Moves the in-page CDP pointer over a web node after scrolling it into view.
///
/// # Errors
///
/// As [`cdp_click_node`].
pub async fn cdp_aim_node(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
) -> A11yResult<CdpActionPoint> {
    with_node_center(
        endpoint,
        page_title_hint,
        backend_node_id,
        |page, center| async move {
            page.execute(mouse_event(
                DispatchMouseEventType::MouseMoved,
                center,
                MouseButton::None,
                0,
            ))
            .await
            .map_err(|err| dispatch_err(&err))?;
            Ok(center)
        },
    )
    .await
}

/// A decoded, top-down BGRA8 bitmap captured from a web node via CDP (#703).
///
/// `bgra` is 4 bytes per pixel with no row padding, sized `width * height * 4`,
/// ready for the `WinRT` OCR `read_text_from_bgra_bitmap` path.
#[derive(Clone, Debug)]
pub struct CdpNodeBitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Captures just a web node's rendered pixels and returns them as a BGRA8 bitmap
/// for OCR (#703).
///
/// Mirrors how clicks resolve a node — attach, find the owning page, scroll the
/// node into view, resolve its live box model — then converts the viewport-CSS
/// box to document coordinates (using `Page.getLayoutMetrics` scroll offset) and
/// captures exactly that element via `Page.captureScreenshot { clip,
/// captureBeyondViewport:true }`. This is DPI-/scroll-/occlusion-robust and
/// needs no CSS→screen mapping (which the click path also deliberately avoids).
///
/// # Errors
///
/// `A11Y_CDP_ATTACH_FAILED` if the endpoint/node cannot be reached;
/// `A11Y_CDP_AXTREE_FAILED` if box-model resolution, layout metrics, capture, or
/// PNG decode fails.
pub async fn cdp_capture_node_bgra(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
) -> A11yResult<CdpNodeBitmap> {
    let (browser, mut handler) =
        Browser::connect(endpoint)
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("connect {endpoint}: {err}"),
            })?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = resolve_owning_page(&browser, page_title_hint, backend_node_id).await?;
        let rect = node_content_rect(&page, backend_node_id).await?;
        // getBoxModel is viewport-relative (the click path dispatches at its
        // centre as viewport coords and lands correctly); captureScreenshot with
        // captureBeyondViewport=true expects document coords, so add the scroll
        // offset. Chrome's own "Capture node screenshot" uses the same shape.
        let metrics = page
            .execute(GetLayoutMetricsParams::default())
            .await
            .map_err(|err| A11yError::CdpAxtreeFailed {
                detail: format!("getLayoutMetrics: {err}"),
            })?;
        let scroll_x = i32::try_from(metrics.result.css_layout_viewport.page_x).unwrap_or(0);
        let scroll_y = i32::try_from(metrics.result.css_layout_viewport.page_y).unwrap_or(0);
        let clip = Viewport {
            x: f64::from(rect.x) + f64::from(scroll_x),
            y: f64::from(rect.y) + f64::from(scroll_y),
            width: f64::from(rect.w),
            height: f64::from(rect.h),
            scale: 1.0,
        };
        let params = ScreenshotParams::builder()
            .format(CaptureScreenshotFormat::Png)
            .clip(clip)
            .from_surface(true)
            .capture_beyond_viewport(true)
            .build();
        let png_bytes =
            page.screenshot(params)
                .await
                .map_err(|err| A11yError::CdpAxtreeFailed {
                    detail: format!("captureScreenshot: {err}"),
                })?;
        decode_png_to_bgra(&png_bytes)
    }
    .await;

    handler_task.abort();
    result
}

fn mouse_event(
    kind: DispatchMouseEventType,
    point: CdpActionPoint,
    button: MouseButton,
    click_count: i64,
) -> DispatchMouseEventParams {
    // `buttons` is the bitmask of buttons CURRENTLY held: the button's bit while
    // pressed, 0 once moved or released. Getting this wrong (e.g. leaving the
    // bit set on release) makes Chrome think the button is still down and it
    // never synthesises a `click` event.
    let is_pressed = matches!(kind, DispatchMouseEventType::MousePressed);
    let bit = match button {
        MouseButton::Left => 1,
        MouseButton::Right => 2,
        MouseButton::Middle => 4,
        _ => 0,
    };
    let mut params = DispatchMouseEventParams::new(kind, point.x, point.y);
    params.click_count = Some(click_count);
    params.buttons = Some(if is_pressed { bit } else { 0 });
    params.button = Some(button);
    params
}

fn dispatch_err(err: &chromiumoxide::error::CdpError) -> A11yError {
    A11yError::CdpAxtreeFailed {
        detail: format!("CDP input dispatch failed: {err}"),
    }
}

/// Polls `browser.pages()` until target discovery surfaces at least one page
/// (fresh connections discover targets asynchronously), up to ~3s.
async fn wait_for_pages(browser: &chromiumoxide::Browser) -> A11yResult<Vec<chromiumoxide::Page>> {
    for _ in 0..30 {
        match browser.pages().await {
            Ok(pages) if !pages.is_empty() => return Ok(pages),
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            Err(err) => {
                return Err(A11yError::CdpAttachFailed {
                    detail: format!("list pages: {err}"),
                });
            }
        }
    }
    Err(A11yError::CdpAttachFailed {
        detail: "no page targets became available within 3s".to_owned(),
    })
}

/// Attaches, finds the page owning `backend_node_id`, scrolls it into view,
/// resolves its box-model centre, runs `action(page, center)`, and tears down.
async fn with_node_center<A, Fut, T>(
    endpoint: &str,
    page_title_hint: &str,
    backend_node_id: i64,
    action: A,
) -> A11yResult<T>
where
    A: FnOnce(chromiumoxide::Page, CdpActionPoint) -> Fut,
    Fut: std::future::Future<Output = A11yResult<T>>,
{
    let (browser, mut handler) =
        Browser::connect(endpoint)
            .await
            .map_err(|err| A11yError::CdpAttachFailed {
                detail: format!("connect {endpoint}: {err}"),
            })?;
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let result = async {
        let page = resolve_owning_page(&browser, page_title_hint, backend_node_id).await?;
        let rect = node_content_rect(&page, backend_node_id).await?;
        let center = CdpActionPoint {
            x: f64::from(rect.x) + f64::from(rect.w) / 2.0,
            y: f64::from(rect.y) + f64::from(rect.h) / 2.0,
        };
        action(page, center).await
    }
    .await;

    handler_task.abort();
    result
}

/// Finds the attached page that owns `backend_node_id`, priming each candidate's
/// DOM and confirming ownership by scrolling the node into view.
///
/// Backend node ids are per-DOCUMENT, so the same numeric id can exist in
/// several tabs. Candidate pages whose title matches the foreground window (the
/// tab `observe` read) are tried first. A fresh CDP connection discovers targets
/// asynchronously and has not been pushed each page's DOM, so we poll for pages
/// and prime with `DOM.getDocument` before resolving (required, not optional).
async fn resolve_owning_page(
    browser: &chromiumoxide::Browser,
    page_title_hint: &str,
    backend_node_id: i64,
) -> A11yResult<chromiumoxide::Page> {
    use chromiumoxide::cdp::browser_protocol::dom::GetDocumentParams;

    let pages = wait_for_pages(browser).await?;
    let mut ordered = Vec::with_capacity(pages.len());
    let mut tail = Vec::new();
    for page in pages {
        let matches_hint = matches!(
            page.get_title().await,
            Ok(Some(title)) if !title.is_empty() && page_title_hint.contains(title.as_str())
        );
        if matches_hint {
            ordered.push(page);
        } else {
            tail.push(page);
        }
    }
    ordered.extend(tail);

    for page in ordered {
        let prime = GetDocumentParams::builder().depth(-1).pierce(true).build();
        let _ = page.execute(prime).await;
        let scroll = ScrollIntoViewIfNeededParams::builder()
            .backend_node_id(BackendNodeId::new(backend_node_id))
            .build();
        if page.execute(scroll).await.is_ok() {
            return Ok(page);
        }
    }
    Err(A11yError::CdpAxtreeFailed {
        detail: format!("no attached page owns backendNodeId {backend_node_id}"),
    })
}

/// Resolves a web node's live content-box rectangle in viewport-CSS pixels.
async fn node_content_rect(
    page: &chromiumoxide::Page,
    backend_node_id: i64,
) -> A11yResult<synapse_core::Rect> {
    let box_params = GetBoxModelParams::builder()
        .backend_node_id(BackendNodeId::new(backend_node_id))
        .build();
    let model = page
        .execute(box_params)
        .await
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("getBoxModel: {err}"),
        })?;
    rect_from_quad(model.result.model.content.inner()).ok_or_else(|| A11yError::CdpAxtreeFailed {
        detail: "node has no resolvable box model (not rendered)".to_owned(),
    })
}

/// Decodes a Chrome PNG screenshot into a top-down BGRA8 bitmap for OCR (#703).
fn decode_png_to_bgra(png_bytes: &[u8]) -> A11yResult<CdpNodeBitmap> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes))
        .read_info()
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("screenshot PNG header decode failed: {err}"),
        })?;
    let buf_size = reader
        .output_buffer_size()
        .ok_or_else(|| A11yError::CdpAxtreeFailed {
            detail: "screenshot PNG output buffer size overflowed usize".to_owned(),
        })?;
    let mut buf = vec![0u8; buf_size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|err| A11yError::CdpAxtreeFailed {
            detail: format!("screenshot PNG frame decode failed: {err}"),
        })?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(A11yError::CdpAxtreeFailed {
            detail: format!(
                "unexpected screenshot PNG bit depth {:?}; expected 8-bit",
                info.bit_depth
            ),
        });
    }
    let pixels = buf
        .get(..info.buffer_size())
        .ok_or_else(|| A11yError::CdpAxtreeFailed {
            detail: "screenshot PNG buffer shorter than reported frame size".to_owned(),
        })?;
    let bgra = match info.color_type {
        png::ColorType::Rgba => rgba8_to_bgra(pixels),
        png::ColorType::Rgb => rgb8_to_bgra(pixels),
        other => {
            return Err(A11yError::CdpAxtreeFailed {
                detail: format!(
                    "unexpected screenshot PNG color type {other:?}; expected RGB/RGBA"
                ),
            });
        }
    };
    Ok(CdpNodeBitmap {
        width: info.width,
        height: info.height,
        bgra,
    })
}

/// Swaps RGBA8 → BGRA8 (Chrome screenshots are RGBA; `WinRT` OCR wants BGRA).
fn rgba8_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

/// Expands RGB8 → BGRA8 with an opaque alpha channel.
fn rgb8_to_bgra(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for px in rgb.chunks_exact(3) {
        out.extend_from_slice(&[px[2], px[1], px[0], 0xFF]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Locks the FSV-discovered bug: leaving the `buttons` bit set on release
    // makes Chrome think the button is still held and never fires a `click`
    // event. Pressed → button bit; moved/released → 0.
    #[test]
    fn mouse_event_buttons_bitmask_is_set_only_while_pressed() {
        let point = CdpActionPoint { x: 10.0, y: 20.0 };
        let pressed = mouse_event(
            DispatchMouseEventType::MousePressed,
            point,
            MouseButton::Left,
            1,
        );
        let released = mouse_event(
            DispatchMouseEventType::MouseReleased,
            point,
            MouseButton::Left,
            1,
        );
        let moved = mouse_event(
            DispatchMouseEventType::MouseMoved,
            point,
            MouseButton::Left,
            0,
        );
        let hover = mouse_event(
            DispatchMouseEventType::MouseMoved,
            point,
            MouseButton::None,
            0,
        );
        println!(
            "readback=mouse_event buttons pressed:{:?} released:{:?} moved:{:?} hover_button:{:?}",
            pressed.buttons, released.buttons, moved.buttons, hover.button
        );
        assert_eq!(pressed.buttons, Some(1), "left press must hold bit 1");
        assert_eq!(released.buttons, Some(0), "release must clear the bitmask");
        assert_eq!(moved.buttons, Some(0), "move must not hold any button");
        assert_eq!(hover.button, Some(MouseButton::None));
        assert_eq!(pressed.click_count, Some(1));

        let right = mouse_event(
            DispatchMouseEventType::MousePressed,
            point,
            MouseButton::Right,
            1,
        );
        assert_eq!(right.buttons, Some(2), "right press must hold bit 2");
    }

    #[test]
    fn cdp_mouse_button_maps_to_cdp_enum() {
        assert_eq!(CdpMouseButton::Left.to_cdp(), MouseButton::Left);
        assert_eq!(CdpMouseButton::Right.to_cdp(), MouseButton::Right);
        assert_eq!(CdpMouseButton::Middle.to_cdp(), MouseButton::Middle);
    }
}
