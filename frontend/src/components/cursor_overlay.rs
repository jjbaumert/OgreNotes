// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::collab::ws_client::RemoteCursor;
use super::dom_position;

/// Resolve a block-relative position (block_id, char_offset) to viewport
/// coordinates (left, top, height) by finding the block element in the DOM
/// and walking its text nodes.
fn block_pos_to_viewport(block_id: &str, char_offset: u32) -> Option<(f64, f64, f64)> {
    let block_el = dom_position::find_block_element(block_id)?;
    let (node, offset) = dom_position::find_text_offset_in_element(&block_el, char_offset)?;

    let window = web_sys::window()?;
    let document = window.document()?;
    let range = document.create_range().ok()?;
    range.set_start(&node, offset).ok()?;
    range.set_end(&node, offset).ok()?;
    let rect = range.get_bounding_client_rect();

    if rect.height() < 1.0 {
        return None;
    }

    Some((rect.left(), rect.top(), rect.height()))
}

/// Resolve a (block_id, char_offset) to the DOM (node, offset) pair needed
/// for Range.setStart/setEnd.
fn resolve_endpoint(block_id: &str, char_offset: u32) -> Option<(web_sys::Node, u32)> {
    let block_el = dom_position::find_block_element(block_id)?;
    dom_position::find_text_offset_in_element(&block_el, char_offset)
}

/// Compute the per-line rectangles that cover a selection from `anchor` to
/// `head`. A single DOM Range spanning anchor→head is created and
/// `getClientRects()` returns one rect per visual line fragment — the same
/// mechanism the native browser selection uses to draw its highlight.
///
/// This replaces a prior `block_range_rect` that fell back to a fixed
/// `MULTI_LINE_FALLBACK_WIDTH` box whenever anchor and head were on
/// different DOM lines, which produced a floating rectangle that didn't
/// follow the text.
fn block_range_rects(
    anchor: &(String, u32),
    head: &(String, u32),
) -> Vec<(f64, f64, f64, f64)> {
    let Some((n1, o1)) = resolve_endpoint(&anchor.0, anchor.1) else { return vec![]; };
    let Some((n2, o2)) = resolve_endpoint(&head.0, head.1) else { return vec![]; };

    let window = match web_sys::window() { Some(w) => w, None => return vec![] };
    let document = match window.document() { Some(d) => d, None => return vec![] };
    let range = match document.create_range() { Ok(r) => r, Err(_) => return vec![] };

    // Range.setEnd before start throws in strict document order, so try the
    // "natural" ordering first and fall back to the swap if it fails.
    // (Selecting right-to-left produces anchor > head in doc order.)
    if range.set_start(&n1, o1).is_ok() && range.set_end(&n2, o2).is_ok() {
        // start ≤ end — ok
    } else {
        let range2 = match document.create_range() { Ok(r) => r, Err(_) => return vec![] };
        if range2.set_start(&n2, o2).is_err() || range2.set_end(&n1, o1).is_err() {
            return vec![];
        }
        return collect_rects(&range2);
    }
    collect_rects(&range)
}

fn collect_rects(range: &web_sys::Range) -> Vec<(f64, f64, f64, f64)> {
    let list = range.get_client_rects();
    let mut out = Vec::new();
    if let Some(list) = list {
        for i in 0..list.length() {
            if let Some(r) = list.item(i) {
                // Skip zero-size rects (empty ranges at block boundaries
                // sometimes emit them).
                if r.width() > 0.5 && r.height() > 0.5 {
                    out.push((r.left(), r.top(), r.width(), r.height()));
                }
            }
        }
    }
    out
}

/// Overlay that renders remote users' cursors on top of the editor.
#[component]
pub fn CursorOverlay(
    cursors: ReadSignal<Vec<RemoteCursor>>,
    /// Scroll tick — forces re-render when the editor container scrolls.
    scroll_tick: ReadSignal<u32>,
    /// When false, remote cursors are not rendered at all (View → Show
    /// Cursors toggle, #99 — for users who find the name tags distracting
    /// or text-obscuring).
    #[prop(into)] enabled: Signal<bool>,
) -> impl IntoView {
    view! {
        {move || {
            if !enabled.get() {
                return Vec::new();
            }
            let _tick = scroll_tick.get();
            cursors.get().into_iter().filter_map(|cursor| {
                let (block_id, char_offset) = cursor.cursor_block.as_ref()?;
                let (left, top, height) = block_pos_to_viewport(block_id, *char_offset)?;

                let color = cursor.color.clone();
                let name = cursor.name.clone();

                // One rect per visual line fragment — native-selection shape.
                let selection_rects: Vec<(f64, f64, f64, f64, String)> = if let (Some(anchor), Some(head)) = (
                    cursor.selection_anchor_block.as_ref(),
                    cursor.selection_head_block.as_ref(),
                ) {
                    let bg = format!("{}33", color);
                    block_range_rects(anchor, head)
                        .into_iter()
                        .map(|(l, t, w, h)| (l, t, w, h, bg.clone()))
                        .collect()
                } else {
                    Vec::new()
                };

                let color_caret = color.clone();

                Some(view! {
                    <For
                        each=move || selection_rects.clone().into_iter().enumerate()
                        key=|(i, _)| *i
                        children=move |(_, (sl, st, sw, sh, bg))| view! {
                            <div
                                class="remote-cursor-selection"
                                style:left=format!("{}px", sl)
                                style:top=format!("{}px", st)
                                style:width=format!("{}px", sw)
                                style:height=format!("{}px", sh)
                                style:background-color=bg
                            ></div>
                        }
                    />
                    <div
                        class="remote-cursor-caret"
                        style:left=format!("{}px", left)
                        style:top=format!("{}px", top)
                        style:height=format!("{}px", height)
                        style:border-left-color=color_caret
                    >
                        <span
                            class="remote-cursor-label"
                            style:background-color=color
                        >{name}</span>
                    </div>
                })
            }).collect::<Vec<_>>()
        }}
    }
}

// ─── Browser tests ─────────────────────────────────────────────
//
// `Range.getClientRects()` requires a real DOM, so these only run under a
// browser. Invoke via `cd frontend && wasm-pack test --headless --chrome`.
//
// The bug being guarded: anchor and head landing on different visual lines
// previously fell back to a fixed `MULTI_LINE_FALLBACK_WIDTH = 400.0`-wide
// rectangle that didn't follow the text. Tests below assert (1) we get one
// rect per visual line, (2) widths track the text content, and (3) no rect
// is the literal fallback width.

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use super::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Build a paragraph element with the given inner HTML and a known
    /// `data-block-id`, append to body, return it.
    fn make_block(block_id: &str, inner_html: &str, width_px: u32) -> web_sys::HtmlElement {
        let doc = web_sys::window().unwrap().document().unwrap();
        let p: web_sys::HtmlElement = doc.create_element("p").unwrap().dyn_into().unwrap();
        p.set_attribute("data-block-id", block_id).unwrap();
        // Constrain width so wrap-tests have a predictable wrap point.
        // Use a monospace font so character widths are stable across
        // browsers and the wrap line count is reliable.
        let style = format!(
            "position:absolute; top:50px; left:50px; width:{width_px}px; \
             font-family:monospace; font-size:14px; line-height:18px; \
             white-space:normal;"
        );
        p.set_attribute("style", &style).unwrap();
        p.set_inner_html(inner_html);
        doc.body().unwrap().append_child(&p).unwrap();
        p
    }

    fn cleanup(els: &[&web_sys::HtmlElement]) {
        for el in els {
            el.remove();
        }
    }

    #[wasm_bindgen_test]
    fn same_line_selection_produces_one_rect() {
        let p = make_block("test-same-line", "Hello world!", 800);
        let anchor = ("test-same-line".to_string(), 0u32);
        let head = ("test-same-line".to_string(), 5u32);

        let rects = block_range_rects(&anchor, &head);

        assert_eq!(rects.len(), 1, "expected exactly one rect on a single line, got {}", rects.len());
        let (_, _, w, _) = rects[0];
        assert!(w > 1.0, "rect width should be > 1px, got {w}");
        assert!(
            (w - dom_position::MULTI_LINE_FALLBACK_WIDTH).abs() > 0.5,
            "rect width must not be the legacy 400px fallback (was {w})",
        );

        cleanup(&[&p]);
    }

    #[wasm_bindgen_test]
    fn wrapped_selection_produces_per_line_rects() {
        // Width forces "AAAAAAAA BBBBBBBB CCCCCCCC DDDDDDDD" to wrap into
        // multiple visual lines. Each visible line should get its own rect.
        let p = make_block(
            "test-wrap",
            "AAAAAAAA BBBBBBBB CCCCCCCC DDDDDDDD EEEEEEEE FFFFFFFF",
            120,
        );
        let total_chars = 53u32; // length of inner_html above
        let anchor = ("test-wrap".to_string(), 0);
        let head = ("test-wrap".to_string(), total_chars);

        let rects = block_range_rects(&anchor, &head);

        assert!(
            rects.len() >= 2,
            "wrapped selection must produce ≥2 rects (one per visual line), got {}",
            rects.len(),
        );
        for (i, (_, _, w, _)) in rects.iter().enumerate() {
            assert!(
                (w - dom_position::MULTI_LINE_FALLBACK_WIDTH).abs() > 0.5,
                "rect {i} width is the legacy 400px fallback ({w}) — \
                getClientRects() should not produce that value",
            );
        }
        // Verify the rects descend the page (top values strictly increase).
        for w in rects.windows(2) {
            assert!(
                w[1].1 >= w[0].1,
                "rect tops should be in document order: {} then {}", w[0].1, w[1].1,
            );
        }

        cleanup(&[&p]);
    }

    #[wasm_bindgen_test]
    fn cross_block_selection_produces_multiple_rects() {
        let p1 = make_block("test-cross-a", "Block A content here.", 800);
        let p2 = make_block("test-cross-b", "Block B content here.", 800);
        let anchor = ("test-cross-a".to_string(), 6); // mid of "Block A"
        let head = ("test-cross-b".to_string(), 5); // mid of "Block B"

        let rects = block_range_rects(&anchor, &head);

        assert!(
            rects.len() >= 2,
            "cross-block selection must produce ≥2 rects, got {}",
            rects.len(),
        );
        for (_, _, w, _) in &rects {
            assert!(
                (w - dom_position::MULTI_LINE_FALLBACK_WIDTH).abs() > 0.5,
                "rect width must not be the legacy 400px fallback (was {w})",
            );
        }

        cleanup(&[&p1, &p2]);
    }

    #[wasm_bindgen_test]
    fn reverse_order_endpoints_still_produce_rects() {
        // User selected right-to-left: head precedes anchor in document order.
        let p = make_block("test-reverse", "Hello world!", 800);
        let anchor = ("test-reverse".to_string(), 11u32);
        let head = ("test-reverse".to_string(), 2u32);

        let rects = block_range_rects(&anchor, &head);

        assert!(
            !rects.is_empty(),
            "reverse-ordered endpoints must still produce at least one rect",
        );

        cleanup(&[&p]);
    }

    #[wasm_bindgen_test]
    fn missing_block_returns_empty() {
        let anchor = ("nonexistent-block-xyz".to_string(), 0u32);
        let head = ("nonexistent-block-xyz".to_string(), 5u32);

        let rects = block_range_rects(&anchor, &head);

        assert!(rects.is_empty(), "missing block must return empty Vec");
    }

    #[wasm_bindgen_test]
    fn collapsed_range_produces_no_rect() {
        // When anchor == head, we shouldn't render a selection box at all
        // (caller would render only the caret instead). collect_rects
        // filters zero-size rects.
        let p = make_block("test-collapsed", "Hello world!", 800);
        let pos = ("test-collapsed".to_string(), 5u32);

        let rects = block_range_rects(&pos, &pos);

        assert!(
            rects.is_empty(),
            "collapsed range must not produce any rect, got {}",
            rects.len(),
        );

        cleanup(&[&p]);
    }
}
