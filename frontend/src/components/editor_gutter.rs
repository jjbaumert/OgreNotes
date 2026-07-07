// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #139: editor gutter overlay — Show Line Numbers + Show Page Breaks.
//!
//! Both are presentation-only (no model/CRDT change) and opt-in via the View
//! menu. They are measured from the live DOM rather than approximated in CSS:
//!
//! - **Line numbers** number each *visual* line (wrapped continuation lines
//!   included), in a single left-aligned column with one uniform font/size —
//!   not per-block with the block's own font (which made headings render giant
//!   numbers). A table — like any atomic block — gets a *single* number to the
//!   left of its first line.
//! - **Page breaks** mark roughly where pages would break with an unobtrusive
//!   right-margin dashed tick + "Page N" label, instead of a full-width rule
//!   drawn *through* the text.
//!
//! Measurement uses `Range.getClientRects()` (one rect per visual line
//! fragment — the same mechanism the native selection highlight uses), so it
//! tracks wrapping and zoom. The overlay is `position: fixed` and recomputes on
//! scroll + content change, matching `CursorOverlay`/`CommentHighlights`.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::state::EditorState;

/// Approximate printable page height in CSS px (~US-Letter content area). A
/// visual aid, not a real print-flow measurement.
const PAGE_HEIGHT_PX: f64 = 960.0;
/// Right edge of the line-number column, measured from the page's left edge.
const LINE_GUTTER_RIGHT_INSET: f64 = 40.0;
/// Width of each number box (right-aligned within it).
const LINE_NUM_WIDTH: f64 = 32.0;
/// Width reserved for the page-break marker (dashed tick + label), measured
/// back from the page's right edge.
const PAGE_MARKER_WIDTH: f64 = 150.0;

struct LineNumber {
    top: f64,
    left: f64,
    n: u32,
}

struct PageMark {
    top: f64,
    left: f64,
    n: u32,
}

fn editor_content() -> Option<web_sys::Element> {
    web_sys::window()?
        .document()?
        .query_selector(".editor-content")
        .ok()
        .flatten()
}

/// Collect the distinct visual-line tops of a block's text content. Rects on
/// the same visual line share (near-)identical tops; a new line is recognized
/// when the top jumps by more than ~60% of the line-box height.
fn block_line_tops(document: &web_sys::Document, block: &web_sys::Node) -> Vec<f64> {
    let Ok(range) = document.create_range() else { return Vec::new() };
    if range.select_node_contents(block).is_err() {
        return Vec::new();
    }
    let Some(list) = range.get_client_rects() else { return Vec::new() };

    let mut rects: Vec<(f64, f64)> = Vec::new(); // (top, height)
    for i in 0..list.length() {
        if let Some(r) = list.item(i) {
            if r.height() > 0.5 && r.width() > 0.5 {
                rects.push((r.top(), r.height()));
            }
        }
    }
    rects.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut tops: Vec<f64> = Vec::new();
    for (top, h) in rects {
        match tops.last() {
            Some(&last) if top - last < h * 0.6 => {} // same visual line
            _ => tops.push(top),
        }
    }
    tops
}

fn measure_line_numbers() -> Vec<LineNumber> {
    let Some(window) = web_sys::window() else { return Vec::new() };
    let Some(document) = window.document() else { return Vec::new() };
    let Some(content) = editor_content() else { return Vec::new() };
    let content_rect = content.get_bounding_client_rect();
    // Single aligned column, independent of per-block indentation.
    let col_left = content_rect.left() + LINE_GUTTER_RIGHT_INSET - LINE_NUM_WIDTH;

    // Direct children only — the top-level editor blocks.
    let Ok(children) = content.query_selector_all(":scope > *") else { return Vec::new() };

    let mut out = Vec::new();
    let mut n: u32 = 1;
    for i in 0..children.length() {
        let Some(node) = children.item(i) else { continue };
        let Some(el) = node.dyn_ref::<web_sys::Element>() else { continue };

        // A table (or any block containing one) is referenced by a single
        // number at its first line, never one-per-row.
        let is_atomic = {
            let tag = el.tag_name().to_uppercase();
            tag == "TABLE"
                || tag == "HR"
                || tag == "FIGURE"
                || el.query_selector("table").ok().flatten().is_some()
        };

        if is_atomic {
            let r = el.get_bounding_client_rect();
            out.push(LineNumber { top: r.top(), left: col_left, n });
            n += 1;
            continue;
        }

        let mut tops = block_line_tops(&document, &node);
        if tops.is_empty() {
            // Empty block (e.g. a blank paragraph) still occupies one line.
            tops.push(el.get_bounding_client_rect().top());
        }
        for top in tops {
            out.push(LineNumber { top, left: col_left, n });
            n += 1;
        }
    }
    out
}

fn measure_page_breaks() -> Vec<PageMark> {
    let Some(content) = editor_content() else { return Vec::new() };
    let rect = content.get_bounding_client_rect();
    let height = rect.height();
    if height <= PAGE_HEIGHT_PX {
        return Vec::new();
    }
    let breaks = (height / PAGE_HEIGHT_PX).floor() as u32;
    let left = rect.right() - PAGE_MARKER_WIDTH;
    (1..=breaks)
        .map(|k| PageMark {
            top: rect.top() + (k as f64) * PAGE_HEIGHT_PX,
            left,
            // The page that *begins* at this break.
            n: k + 1,
        })
        .collect()
}

/// Fixed-position overlay drawing line numbers and/or page-break markers over
/// the editor. Renders nothing unless at least one toggle is enabled.
#[component]
pub fn EditorGutterOverlay(
    /// Re-measure trigger: content edits flow through here.
    #[prop(into)] editor_state: Signal<Option<EditorState>>,
    /// Scroll tick — forces re-measure when the editor container scrolls.
    scroll_tick: ReadSignal<u32>,
    #[prop(into)] line_numbers: Signal<bool>,
    #[prop(into)] page_breaks: Signal<bool>,
) -> impl IntoView {
    view! {
        {move || {
            // Subscribe to the triggers so the overlay tracks edits + scroll.
            let _tick = scroll_tick.get();
            let _state = editor_state.get();

            let numbers = if line_numbers.get() { measure_line_numbers() } else { Vec::new() };
            let marks = if page_breaks.get() { measure_page_breaks() } else { Vec::new() };

            let number_views = numbers
                .into_iter()
                .map(|ln| view! {
                    <div
                        class="editor-line-number"
                        style:top=format!("{}px", ln.top)
                        style:left=format!("{}px", ln.left)
                        style:width=format!("{}px", LINE_NUM_WIDTH)
                    >{ln.n}</div>
                })
                .collect_view();

            let mark_views = marks
                .into_iter()
                .map(|pm| {
                    let label = crate::t!("editor-page-break", n = pm.n);
                    view! {
                        <div
                            class="editor-page-break"
                            style:top=format!("{}px", pm.top)
                            style:left=format!("{}px", pm.left)
                            style:width=format!("{}px", PAGE_MARKER_WIDTH)
                        >
                            <span class="editor-page-break-line"></span>
                            <span class="editor-page-break-label">{label}</span>
                        </div>
                    }
                })
                .collect_view();

            view! { {number_views} {mark_views} }
        }}
    }
}
