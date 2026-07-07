// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Shared utilities for positioning overlays relative to the viewport.
//!
//! All overlay elements in OgreNotes (remote cursors, comment highlights, selection
//! toolbar, popups) use `position: fixed` with viewport-relative coordinates from
//! `getBoundingClientRect()`. This module centralizes those calculations so they're
//! consistent and easy to maintain.
//!
//! Components using these utilities must re-render on the `scroll_tick` signal to
//! keep viewport-relative positions up to date when the editor scrolls.

use wasm_bindgen::JsCast;

pub const MULTI_LINE_FALLBACK_WIDTH: f64 = 400.0;
const VIEWPORT_MARGIN: f64 = 4.0;

// ─── Viewport rect wrapper ────────────────────────────────────

/// Viewport-relative rectangle (from getBoundingClientRect).
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewportRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl ViewportRect {
    pub fn right(&self) -> f64 {
        self.left + self.width
    }
    pub fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn from_dom_rect(r: &web_sys::DomRect) -> Self {
        Self {
            left: r.left(),
            top: r.top(),
            width: r.width(),
            height: r.height(),
        }
    }
}

// ─── Element + range positioning ───────────────────────────────

/// Get viewport rect of a DOM element.
pub fn element_viewport_rect(el: &web_sys::Element) -> ViewportRect {
    ViewportRect::from_dom_rect(&el.get_bounding_client_rect())
}

/// Get viewport rect from the current browser Selection range.
/// Returns None if nothing is selected or the selection is collapsed.
pub fn selection_viewport_rect() -> Option<ViewportRect> {
    let window = web_sys::window()?;
    let sel = window.get_selection().ok()??;
    if sel.range_count() == 0 {
        return None;
    }
    let range = sel.get_range_at(0).ok()?;
    let rect = range.get_bounding_client_rect();
    if rect.width() < 1.0 {
        return None;
    }
    Some(ViewportRect::from_dom_rect(&rect))
}

/// Convert a model position to viewport coordinates (left, top, height).
/// Used for cursor overlays — maps the editor's internal position index
/// to a pixel location in the viewport.
pub fn dom_position_for_model_pos(model_pos: u32) -> Option<(f64, f64, f64)> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let editor: web_sys::HtmlElement = document
        .query_selector(".editor-content")
        .ok()??
        .dyn_into()
        .ok()?;

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

/// Get a bounding box for a range between two model positions.
/// Returns (left, top, width, height) in viewport coordinates.
pub fn range_rect(from: u32, to: u32) -> Option<(f64, f64, f64, f64)> {
    let (l1, t1, _h1) = dom_position_for_model_pos(from)?;
    let (l2, t2, h2) = dom_position_for_model_pos(to)?;

    if (t1 - t2).abs() < 2.0 {
        let left = l1.min(l2);
        let width = (l2 - l1).abs().max(2.0);
        Some((left, t1, width, h2))
    } else {
        let width = MULTI_LINE_FALLBACK_WIDTH;
        Some((l1, t1, width, (t2 + h2) - t1))
    }
}

// ─── Placement helpers ─────────────────────────────────────────

/// Place a popup above a reference rect, centered horizontally.
/// `popup_height` is the rendered height of the popup; `gap` is the pixel space
/// between the reference rect and the popup.
pub fn place_above(
    ref_rect: &ViewportRect,
    popup_width: f64,
    popup_height: f64,
    gap: f64,
) -> (f64, f64) {
    let x = ref_rect.left + (ref_rect.width / 2.0) - (popup_width / 2.0);
    let y = ref_rect.top - popup_height - gap;
    clamp_to_viewport(x, y, popup_width, popup_height)
}

/// Place a popup below a reference rect, aligned to the left edge.
/// `popup_width` is used for right-edge clamping.
pub fn place_below(ref_rect: &ViewportRect, popup_width: f64, offset_y: f64) -> (f64, f64) {
    let x = ref_rect.left;
    let y = ref_rect.bottom() + offset_y;
    clamp_to_viewport(x, y, popup_width, 300.0)
}

/// Place a popup in the left margin of a reference rect.
pub fn place_left_margin(
    ref_rect: &ViewportRect,
    popup_width: f64,
    margin: f64,
) -> (f64, f64) {
    let x = ref_rect.left - popup_width - margin;
    let y = ref_rect.top;
    // Use a reasonable default popup height for clamping (actual height varies)
    clamp_to_viewport(x, y, popup_width, 500.0)
}

/// Clamp (left, top) so a popup stays within the viewport.
pub fn clamp_to_viewport(left: f64, top: f64, width: f64, height: f64) -> (f64, f64) {
    let (vw, vh) = viewport_size();
    clamp_to_viewport_with_size(left, top, width, height, vw, vh)
}

/// Get the current viewport dimensions.
pub fn viewport_size() -> (f64, f64) {
    let vw = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0);
    let vh = web_sys::window()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0);
    (vw, vh)
}

/// Pure math version of `clamp_to_viewport` — testable without a window.
pub fn clamp_to_viewport_with_size(
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    viewport_width: f64,
    viewport_height: f64,
) -> (f64, f64) {
    // When the popup is wider/taller than the viewport (e.g. a 420px popup
    // on a 375px phone), the naive
    //   left.max(MARGIN).min(viewport_width - width - MARGIN)
    // produces a negative left because the right-edge clamp dominates the
    // left-edge clamp.  Pin the popup to the start margin in that case so
    // the popup stays visible — the CSS `max-width: calc(100vw - …)` rules
    // shrink the rendered width to fit.
    let x = if viewport_width < width + 2.0 * VIEWPORT_MARGIN {
        VIEWPORT_MARGIN
    } else {
        left
            .max(VIEWPORT_MARGIN)
            .min(viewport_width - width - VIEWPORT_MARGIN)
    };
    let y = if viewport_height < height + 2.0 * VIEWPORT_MARGIN {
        VIEWPORT_MARGIN
    } else {
        top
            .max(VIEWPORT_MARGIN)
            .min(viewport_height - height - VIEWPORT_MARGIN)
    };
    (x, y)
}

// ─── Text range utilities ──────────────────────────────────────

/// Find a DOM element by its `data-block-id` attribute.
pub fn find_block_element(block_id: &str) -> Option<web_sys::Element> {
    if !block_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let window = web_sys::window()?;
    let document = window.document()?;
    let selector = format!("[data-block-id=\"{block_id}\"]");
    document.query_selector(&selector).ok()?
}

/// Get viewport rects for a text range within a block element.
/// Returns a list of (left, top, width, height) for each line fragment.
pub fn text_range_rects(
    block_el: &web_sys::Element,
    start: u32,
    end: u32,
) -> Vec<(f64, f64, f64, f64)> {
    let document = web_sys::window().and_then(|w| w.document());
    let Some(document) = document else {
        return vec![];
    };
    let Ok(range) = document.create_range() else {
        return vec![];
    };

    let start_pos = find_text_offset_in_element(block_el, start);
    let end_pos = find_text_offset_in_element(block_el, end);

    let Some((start_node, start_off)) = start_pos else {
        let r = block_el.get_bounding_client_rect();
        return vec![(r.left(), r.top(), r.width(), r.height())];
    };
    let Some((end_node, end_off)) = end_pos else {
        let r = block_el.get_bounding_client_rect();
        return vec![(r.left(), r.top(), r.width(), r.height())];
    };

    if range.set_start(&start_node, start_off).is_err()
        || range.set_end(&end_node, end_off).is_err()
    {
        let r = block_el.get_bounding_client_rect();
        return vec![(r.left(), r.top(), r.width(), r.height())];
    }

    let r = range.get_bounding_client_rect();
    if r.width() > 0.5 && r.height() > 0.5 {
        vec![(r.left(), r.top(), r.width(), r.height())]
    } else {
        vec![]
    }
}

/// Walk text nodes within an element to find the DOM node + offset
/// for a character position.
pub fn find_text_offset_in_element(
    element: &web_sys::Element,
    target: u32,
) -> Option<(web_sys::Node, u32)> {
    let mut pos = 0u32;
    walk_text_nodes(element.as_ref(), &mut pos, target)
}

fn walk_text_nodes(
    node: &web_sys::Node,
    pos: &mut u32,
    target: u32,
) -> Option<(web_sys::Node, u32)> {
    let children = node.child_nodes();
    for i in 0..children.length() {
        let Some(child) = children.item(i) else {
            continue;
        };
        if child.node_type() == web_sys::Node::TEXT_NODE {
            let text = child.text_content().unwrap_or_default();
            let len = text.chars().count() as u32;
            if target >= *pos && target <= *pos + len {
                return Some((child, target - *pos));
            }
            *pos += len;
        } else if child.node_type() == web_sys::Node::ELEMENT_NODE {
            if let Some(el) = child.dyn_ref::<web_sys::Element>() {
                // Leaf inline elements (e.g. <br> for HardBreak) count as 1
                // position in the editor model but have no text content.
                if el.tag_name().eq_ignore_ascii_case("br") {
                    if target == *pos {
                        return Some((child, 0));
                    }
                    *pos += 1;
                } else if let Some(result) = walk_text_nodes(el.as_ref(), pos, target) {
                    return Some(result);
                }
            }
        }
    }
    None
}

// ─── Block scrolling ───────────────────────────────────────────

/// Scroll the live editor to the block identified by `block_id`. Falls
/// back to the `fallback_index`-th element bearing a `data-block-id`
/// attribute (document order) when the id can't be found — useful for
/// legacy blocks that never received a CRDT id, and for diff entries
/// describing a block that was deleted from the live doc but still has
/// an ordinal position in the listing.
///
/// Returns `true` when an element was scrolled into view, `false` when
/// neither lookup succeeded. The caller has already closed any modal
/// that owns this jump action; the bool is informational only.
pub fn scroll_to_block(block_id: Option<&str>, fallback_index: usize) -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Some(doc) = window.document() else { return false };

    let target: Option<web_sys::Element> = block_id
        .and_then(|id| {
            // Escape the id for use inside a CSS attribute selector — the
            // CRDT ids are A–Za–z0–9 (see editor/model.rs::generate_block_id),
            // but be defensive in case a future id format includes quotes
            // or backslashes.
            let escaped = id.replace('\\', "\\\\").replace('"', "\\\"");
            doc.query_selector(&format!("[data-block-id=\"{escaped}\"]"))
                .ok()
                .flatten()
        })
        .or_else(|| nth_top_level_block(&doc, fallback_index));

    if let Some(el) = target {
        let opts = web_sys::ScrollIntoViewOptions::new();
        opts.set_behavior(web_sys::ScrollBehavior::Smooth);
        opts.set_block(web_sys::ScrollLogicalPosition::Center);
        el.scroll_into_view_with_scroll_into_view_options(&opts);
        true
    } else {
        false
    }
}

/// Walk every `[data-block-id]` element in document order and return the
/// `index`-th element whose nearest `[data-block-id]` ancestor is itself
/// (i.e., it isn't nested inside another block). The diff's
/// `block_index` is the top-level block ordinal in the doc; nested
/// blocks (list items inside a list, table cells inside a table) carry
/// their own `data-block-id` but should not be counted here, otherwise
/// the fallback lands on the wrong block in list-heavy or table-heavy
/// docs.
fn nth_top_level_block(doc: &web_sys::Document, index: usize) -> Option<web_sys::Element> {
    let nodes = doc.query_selector_all("[data-block-id]").ok()?;
    let mut seen = 0usize;
    for i in 0..nodes.length() {
        let Some(node) = nodes.item(i) else { continue };
        let Some(el) = node.dyn_ref::<web_sys::Element>() else { continue };
        if !is_top_level_block(el) {
            continue;
        }
        if seen == index {
            return Some(el.clone());
        }
        seen += 1;
    }
    None
}

fn is_top_level_block(el: &web_sys::Element) -> bool {
    let mut ancestor = el.parent_element();
    while let Some(parent) = ancestor {
        if parent.has_attribute("data-block-id") {
            return false;
        }
        ancestor = parent.parent_element();
    }
    true
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ─── clamp_to_viewport_with_size (pure math) ───────────────

    #[test]
    fn clamp_center_of_viewport_unchanged() {
        let (x, y) = clamp_to_viewport_with_size(100.0, 100.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(x, 100.0);
        assert_eq!(y, 100.0);
    }

    #[test]
    fn clamp_negative_left_snaps_to_margin() {
        let (x, _) = clamp_to_viewport_with_size(-50.0, 100.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(x, VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_negative_top_snaps_to_margin() {
        let (_, y) = clamp_to_viewport_with_size(100.0, -20.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(y, VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_right_overflow() {
        // Popup at x=900, width=200 would overflow 1024-wide viewport
        let (x, _) = clamp_to_viewport_with_size(900.0, 100.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(x, 1024.0 - 200.0 - VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_bottom_overflow() {
        // Popup at y=700, height=100 would overflow 768-tall viewport
        let (_, y) = clamp_to_viewport_with_size(100.0, 700.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(y, 768.0 - 100.0 - VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_both_corners_overflow() {
        let (x, y) = clamp_to_viewport_with_size(-100.0, -100.0, 200.0, 100.0, 1024.0, 768.0);
        assert_eq!(x, VIEWPORT_MARGIN);
        assert_eq!(y, VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_popup_wider_than_viewport_pins_to_left_margin() {
        // Viewport smaller than popup — keep the popup visible by pinning
        // it to the start margin instead of letting the right-edge clamp
        // pull it negative. CSS max-width shrinks the rendered popup to
        // fit the viewport.
        let (x, _) = clamp_to_viewport_with_size(0.0, 0.0, 500.0, 400.0, 100.0, 100.0);
        assert_eq!(x, VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_popup_wider_than_viewport_with_negative_input_pins_to_margin() {
        // Same situation but with a negative input left (which is what
        // place_left_margin produces on narrow phones). The fix has to
        // handle that case too — not just left=0.
        let (x, _) = clamp_to_viewport_with_size(-200.0, 50.0, 420.0, 500.0, 375.0, 800.0);
        assert_eq!(x, VIEWPORT_MARGIN);
    }

    #[test]
    fn clamp_popup_taller_than_viewport_pins_to_top_margin() {
        let (_, y) = clamp_to_viewport_with_size(50.0, 200.0, 100.0, 800.0, 1024.0, 600.0);
        assert_eq!(y, VIEWPORT_MARGIN);
    }

    // ─── ViewportRect ──────────────────────────────────────────

    #[test]
    fn viewport_rect_right_bottom() {
        let r = ViewportRect {
            left: 10.0,
            top: 20.0,
            width: 100.0,
            height: 50.0,
        };
        assert_eq!(r.right(), 110.0);
        assert_eq!(r.bottom(), 70.0);
    }

    #[test]
    fn viewport_rect_default_is_zero() {
        let r = ViewportRect::default();
        assert_eq!(r.left, 0.0);
        assert_eq!(r.top, 0.0);
        assert_eq!(r.width, 0.0);
        assert_eq!(r.height, 0.0);
    }

    // ─── Placement math (tested via clamp_to_viewport_with_size) ─

    #[test]
    fn place_above_math() {
        // Simulates place_above logic without calling web_sys
        let rect = ViewportRect {
            left: 200.0, top: 300.0, width: 100.0, height: 20.0,
        };
        let popup_width = 40.0;
        let popup_height = 32.0;
        let gap = 4.0;
        let x = rect.left + (rect.width / 2.0) - (popup_width / 2.0);
        let y = rect.top - popup_height - gap;
        let (cx, cy) = clamp_to_viewport_with_size(x, y, popup_width, popup_height, 1024.0, 768.0);
        assert_eq!(cx, 230.0); // 200 + 50 - 20
        assert_eq!(cy, 264.0); // 300 - 32 - 4
    }

    #[test]
    fn place_above_near_top_clamps() {
        let rect = ViewportRect {
            left: 200.0, top: 10.0, width: 100.0, height: 20.0,
        };
        let y = rect.top - 32.0 - 4.0; // = -26
        let (_, cy) = clamp_to_viewport_with_size(230.0, y, 40.0, 32.0, 1024.0, 768.0);
        assert_eq!(cy, VIEWPORT_MARGIN);
    }

    #[test]
    fn place_below_math() {
        let rect = ViewportRect {
            left: 100.0, top: 200.0, width: 80.0, height: 20.0,
        };
        let x = rect.left;
        let y = rect.bottom() + 4.0;
        let (cx, cy) = clamp_to_viewport_with_size(x, y, 280.0, 300.0, 1024.0, 768.0);
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 224.0); // 200 + 20 + 4
    }

    #[test]
    fn place_left_margin_math() {
        let rect = ViewportRect {
            left: 500.0, top: 200.0, width: 300.0, height: 20.0,
        };
        let x = rect.left - 420.0 - 12.0; // 68
        let y = rect.top;
        let (cx, cy) = clamp_to_viewport_with_size(x, y, 420.0, 500.0, 1024.0, 768.0);
        assert_eq!(cx, 68.0);
        assert_eq!(cy, 200.0);
    }

    #[test]
    fn place_left_margin_clamps_left_edge() {
        let rect = ViewportRect {
            left: 100.0, top: 200.0, width: 300.0, height: 20.0,
        };
        let x = rect.left - 420.0 - 12.0; // -332
        let (cx, _) = clamp_to_viewport_with_size(x, rect.top, 420.0, 500.0, 1024.0, 768.0);
        assert_eq!(cx, VIEWPORT_MARGIN); // clamped
    }
}

// ─── Browser-based DOM tests ───────────────────────────────────
//
// These require a real browser DOM (run via `wasm-pack test --headless --chrome`
// or `cargo test --bin ogrenotes-frontend --target wasm32-unknown-unknown`).

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use super::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn create_positioned_element(left: f64, top: f64, width: f64, height: f64) -> web_sys::HtmlElement {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div = doc.create_element("div").unwrap();
        let style = format!(
            "position:fixed; left:{}px; top:{}px; width:{}px; height:{}px; background:red;",
            left, top, width, height
        );
        div.set_attribute("style", &style).unwrap();
        doc.body().unwrap().append_child(&div).unwrap();
        div.dyn_into::<web_sys::HtmlElement>().unwrap()
    }

    fn cleanup(el: &web_sys::HtmlElement) {
        el.remove();
    }

    #[wasm_bindgen_test]
    fn element_viewport_rect_returns_correct_position() {
        let el = create_positioned_element(50.0, 80.0, 200.0, 30.0);

        let rect = element_viewport_rect(&el);

        // Fixed-positioned elements should report their CSS position
        assert!((rect.left - 50.0).abs() < 1.0, "left: expected ~50, got {}", rect.left);
        assert!((rect.top - 80.0).abs() < 1.0, "top: expected ~80, got {}", rect.top);
        assert!((rect.width - 200.0).abs() < 1.0, "width: expected ~200, got {}", rect.width);
        assert!((rect.height - 30.0).abs() < 1.0, "height: expected ~30, got {}", rect.height);

        cleanup(&el);
    }

    #[wasm_bindgen_test]
    fn element_viewport_rect_right_bottom() {
        let el = create_positioned_element(100.0, 200.0, 150.0, 40.0);

        let rect = element_viewport_rect(&el);
        assert!((rect.right() - 250.0).abs() < 1.0);
        assert!((rect.bottom() - 240.0).abs() < 1.0);

        cleanup(&el);
    }

    #[wasm_bindgen_test]
    fn find_block_element_by_data_attribute() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div = doc.create_element("div").unwrap();
        div.set_attribute("data-block-id", "test-block-123").unwrap();
        doc.body().unwrap().append_child(&div).unwrap();

        let found = find_block_element("test-block-123");
        assert!(found.is_some(), "should find element by data-block-id");

        let not_found = find_block_element("nonexistent");
        assert!(not_found.is_none(), "should return None for missing block");

        div.remove();
    }

    #[wasm_bindgen_test]
    fn find_block_element_rejects_unsafe_ids() {
        // IDs with special chars should be rejected (prevents selector injection)
        assert!(find_block_element("test<script>").is_none());
        assert!(find_block_element("test\"id").is_none());
        assert!(find_block_element("test'id").is_none());
    }

    #[wasm_bindgen_test]
    fn find_text_offset_in_simple_element() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div: web_sys::HtmlElement = doc.create_element("div").unwrap().dyn_into().unwrap();
        div.set_inner_html("Hello World");
        doc.body().unwrap().append_child(&div).unwrap();

        let el: web_sys::Element = div.clone().dyn_into().unwrap();

        // Offset 0 should find the text node at position 0
        let result = find_text_offset_in_element(&el, 0);
        assert!(result.is_some(), "should find offset 0");
        let (node, off) = result.unwrap();
        assert_eq!(node.node_type(), web_sys::Node::TEXT_NODE);
        assert_eq!(off, 0);

        // Offset 5 should find position 5 in "Hello World"
        let result = find_text_offset_in_element(&el, 5);
        assert!(result.is_some(), "should find offset 5");
        let (_, off) = result.unwrap();
        assert_eq!(off, 5);

        // Offset beyond text length should return None
        let result = find_text_offset_in_element(&el, 100);
        assert!(result.is_none(), "should not find offset beyond text length");

        div.remove();
    }

    #[wasm_bindgen_test]
    fn find_text_offset_in_nested_elements() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div: web_sys::HtmlElement = doc.create_element("div").unwrap().dyn_into().unwrap();
        // "Hello" in a span + " World" as text = 11 chars total
        div.set_inner_html("<span>Hello</span> World");
        doc.body().unwrap().append_child(&div).unwrap();

        let el: web_sys::Element = div.clone().dyn_into().unwrap();

        // Offset 3 should be in the span's text node ("Hel|lo")
        let result = find_text_offset_in_element(&el, 3);
        assert!(result.is_some());
        let (_, off) = result.unwrap();
        assert_eq!(off, 3);

        // Offset 7 should be in the outer text node (" Wo|rld")
        let result = find_text_offset_in_element(&el, 7);
        assert!(result.is_some());

        div.remove();
    }

    #[wasm_bindgen_test]
    fn find_text_offset_with_br_hardbreak() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div: web_sys::HtmlElement = doc.create_element("div").unwrap().dyn_into().unwrap();
        // "Hello" + <br> + "World" = 5 text + 1 br + 5 text = 11 model positions
        div.set_inner_html("Hello<br>World");
        doc.body().unwrap().append_child(&div).unwrap();

        let el: web_sys::Element = div.clone().dyn_into().unwrap();

        // Offset 4 should be in "Hello" (position 4 = 'o')
        let result = find_text_offset_in_element(&el, 4);
        assert!(result.is_some(), "should find offset 4 in 'Hello'");
        let (node, off) = result.unwrap();
        assert_eq!(node.node_type(), web_sys::Node::TEXT_NODE);
        assert_eq!(off, 4);

        // Offset 5 should be the <br> element (HardBreak counts as 1 position)
        let result = find_text_offset_in_element(&el, 5);
        assert!(result.is_some(), "should find offset 5 at <br>");

        // Offset 6 should be the start of "World" (first char after the <br>)
        let result = find_text_offset_in_element(&el, 6);
        assert!(result.is_some(), "should find offset 6 in 'World'");
        let (node, off) = result.unwrap();
        assert_eq!(node.node_type(), web_sys::Node::TEXT_NODE);
        assert_eq!(off, 0); // start of "World" text node

        // Offset 10 should be the last char of "World" ('d')
        let result = find_text_offset_in_element(&el, 10);
        assert!(result.is_some(), "should find offset 10 at end of 'World'");
        let (_, off) = result.unwrap();
        assert_eq!(off, 4);

        div.remove();
    }

    #[wasm_bindgen_test]
    fn find_text_offset_with_multiple_br() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let div: web_sys::HtmlElement = doc.create_element("div").unwrap().dyn_into().unwrap();
        // "A" + <br> + <br> + "B" = 1 + 1 + 1 + 1 = 4 model positions
        div.set_inner_html("A<br><br>B");
        doc.body().unwrap().append_child(&div).unwrap();

        let el: web_sys::Element = div.clone().dyn_into().unwrap();

        // Offset 0 = 'A'
        let result = find_text_offset_in_element(&el, 0);
        assert!(result.is_some());
        let (node, off) = result.unwrap();
        assert_eq!(node.node_type(), web_sys::Node::TEXT_NODE);
        assert_eq!(off, 0);

        // Offset 1 = first <br>
        let result = find_text_offset_in_element(&el, 1);
        assert!(result.is_some(), "should find first <br>");

        // Offset 2 = second <br>
        let result = find_text_offset_in_element(&el, 2);
        assert!(result.is_some(), "should find second <br>");

        // Offset 3 = 'B'
        let result = find_text_offset_in_element(&el, 3);
        assert!(result.is_some(), "should find 'B'");
        let (node, off) = result.unwrap();
        assert_eq!(node.node_type(), web_sys::Node::TEXT_NODE);
        assert_eq!(off, 0);

        div.remove();
    }

    #[wasm_bindgen_test]
    fn clamp_to_viewport_uses_real_window_size() {
        // Just verify it doesn't panic and returns reasonable values
        let (x, y) = clamp_to_viewport(100.0, 100.0, 50.0, 50.0);
        assert!(x >= VIEWPORT_MARGIN);
        assert!(y >= VIEWPORT_MARGIN);
    }

    #[wasm_bindgen_test]
    fn viewport_size_returns_positive_values() {
        let (vw, vh) = viewport_size();
        assert!(vw > 0.0, "viewport width should be positive, got {vw}");
        assert!(vh > 0.0, "viewport height should be positive, got {vh}");
    }
}
