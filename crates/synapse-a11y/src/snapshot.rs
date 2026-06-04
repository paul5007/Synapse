use synapse_core::{AccessibleNode, AccessibleSubtree, ElementId, Point, UiaPattern};

use crate::{A11yResult, UIElement, platform};

/// Captures a UIA subtree rooted at `root`.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the root no longer produces a node, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn snapshot(root: &UIElement, depth: u32) -> A11yResult<AccessibleSubtree> {
    platform::snapshot(root, depth)
}

/// Captures the current foreground UIA subtree without returning COM elements.
///
/// # Errors
///
/// Returns `A11Y_NO_FOREGROUND` when Windows has no foreground HWND, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn snapshot_focused_window(depth: u32) -> A11yResult<AccessibleSubtree> {
    platform::snapshot_focused_window(depth)
}

/// Captures a UIA subtree rooted at a native HWND without returning COM
/// elements.
///
/// # Errors
///
/// Returns `A11Y_NO_FOREGROUND` when the HWND is invalid, a structured UIA
/// error for OS failures, or `A11Y_NOT_AVAILABLE` on non-Windows platforms.
pub fn snapshot_window_from_hwnd(hwnd: i64, depth: u32) -> A11yResult<AccessibleSubtree> {
    platform::snapshot_window_from_hwnd(hwnd, depth)
}

/// Captures a UIA subtree rooted at the visible top-level window for `pid`.
///
/// # Errors
///
/// Returns `A11Y_NO_FOREGROUND` when no visible window exists for the pid, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn snapshot_window_for_process(pid: u32, depth: u32) -> A11yResult<AccessibleSubtree> {
    platform::snapshot_window_for_process(pid, depth)
}

/// Re-resolves an element id and snapshots it without returning the COM
/// element.
///
/// # Errors
///
/// Returns `A11Y_ELEMENT_STALE` when the element id cannot be re-resolved, a
/// structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn snapshot_element(id: &ElementId, depth: u32) -> A11yResult<AccessibleSubtree> {
    platform::snapshot_element(id, depth)
}

/// Returns the currently focused UIA element as a plain `AccessibleNode`.
///
/// # Errors
///
/// Returns a structured UIA error when the focused element cannot be resolved,
/// or `A11Y_NOT_AVAILABLE` on non-Windows platforms.
pub fn focused_element_node() -> A11yResult<AccessibleNode> {
    platform::focused_element_node()
}

/// Returns the UIA element at a screen-space point as a plain `AccessibleNode`.
///
/// # Errors
///
/// Returns a structured UIA error when hit testing fails, or
/// `A11Y_NOT_AVAILABLE` on non-Windows platforms.
pub fn element_node_from_point(point: Point) -> A11yResult<AccessibleNode> {
    platform::element_node_from_point(point)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementSearchScope {
    Children,
    Descendants,
    Subtree,
}

/// Finds the first enabled element under `root` with the requested UIA name and
/// pattern availability. This uses direct UIA search with a `RawView` cache.
///
/// # Errors
///
/// Returns a structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn find_by_name_and_pattern(
    root: &UIElement,
    name: &str,
    pattern: UiaPattern,
    scope: ElementSearchScope,
) -> A11yResult<Option<AccessibleNode>> {
    platform::find_by_name_and_pattern(root, name, pattern, scope)
}

/// Finds the first enabled element under an HWND with the requested UIA name
/// and pattern availability, returning plain data only.
///
/// # Errors
///
/// Returns a structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn find_by_name_and_pattern_in_window(
    hwnd: i64,
    name: impl Into<String>,
    pattern: UiaPattern,
    scope: ElementSearchScope,
) -> A11yResult<Option<AccessibleNode>> {
    platform::find_by_name_and_pattern_in_window(hwnd, name.into(), pattern, scope)
}

/// Returns Chromium renderer UIA nodes that UIA raw child walking can omit even
/// after `--force-renderer-accessibility` activates the renderer tree.
///
/// # Errors
///
/// Returns a structured UIA error for OS failures, or `A11Y_NOT_AVAILABLE` on
/// non-Windows platforms.
pub fn chromium_renderer_accessibility_nodes_from_window(
    hwnd: i64,
    depth: u32,
    max_nodes: usize,
) -> A11yResult<Vec<AccessibleNode>> {
    platform::chromium_renderer_accessibility_nodes_from_window(hwnd, depth, max_nodes)
}
