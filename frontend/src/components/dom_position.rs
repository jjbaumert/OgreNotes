//! Shared utilities for converting editor model positions to DOM viewport coordinates.
//! Used by CursorOverlay and CommentHighlights.

use wasm_bindgen::JsCast;

/// Convert a model position to DOM viewport coordinates (left, top, height).
pub fn dom_position_for_model_pos(model_pos: u32) -> Option<(f64, f64, f64)> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let editor: web_sys::HtmlElement = document
        .query_selector(".editor-content")
        .ok()??
        .dyn_into()
        .ok()?;

    // Reuse the same position-mapping logic as EditorView's selection sync
    // to avoid divergence between cursor rendering and selection handling.
    let (node, offset) = crate::editor::view::find_dom_position(&editor, model_pos as usize)?;

    let range = document.create_range().ok()?;
    range.set_start(&node, offset as u32).ok()?;
    range.set_end(&node, offset as u32).ok()?;
    let rect = range.get_bounding_client_rect();

    if rect.height() < 1.0 {
        return None;
    }

    Some((rect.left(), rect.top(), rect.height()))
}

/// Get a simple bounding box for a range between two model positions.
/// Returns (left, top, width, height).
pub fn range_rect(from: u32, to: u32) -> Option<(f64, f64, f64, f64)> {
    let (l1, t1, _h1) = dom_position_for_model_pos(from)?;
    let (l2, t2, h2) = dom_position_for_model_pos(to)?;

    if (t1 - t2).abs() < 2.0 {
        // Same line
        let left = l1.min(l2);
        let width = (l2 - l1).abs().max(2.0);
        Some((left, t1, width, h2))
    } else {
        // Multi-line: approximate rectangle covering the range
        let width = 400.0;
        Some((l1, t1, width, (t2 + h2) - t1))
    }
}
