//! Shared utilities for converting editor model positions to DOM viewport coordinates.
//! Used by CursorOverlay and CommentHighlights.

use wasm_bindgen::JsCast;

/// Convert a model position to DOM viewport coordinates (left, top, height).
pub fn dom_position_for_model_pos(model_pos: u32) -> Option<(f64, f64, f64)> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let editor = document.query_selector(".editor-content").ok()??;

    let (node, offset) = walk_to_position(&editor, model_pos as usize)?;

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

/// Walk the editor DOM to find the text node and UTF-16 offset for a model position.
pub fn walk_to_position(
    root: &web_sys::Element,
    mut remaining: usize,
) -> Option<(web_sys::Node, usize)> {
    let children = root.child_nodes();
    for i in 0..children.length() {
        let child = children.item(i)?;
        let node_type = child.node_type();

        if node_type == web_sys::Node::TEXT_NODE {
            let text = child.text_content().unwrap_or_default();
            let char_count = text.chars().count();
            if remaining <= char_count {
                let utf16_offset: usize = text
                    .chars()
                    .take(remaining)
                    .map(|c| c.len_utf16())
                    .sum();
                return Some((child, utf16_offset));
            }
            remaining -= char_count;
        } else if node_type == web_sys::Node::ELEMENT_NODE {
            let el: &web_sys::Element = child.unchecked_ref();
            let tag = el.tag_name().to_lowercase();

            let is_block = matches!(
                tag.as_str(),
                "p" | "h1" | "h2" | "h3" | "blockquote" | "ul" | "ol" | "li" | "pre" | "hr"
            );

            if is_block {
                if remaining == 0 {
                    return Some((child, 0));
                }
                remaining -= 1; // opening boundary

                if let Some(result) = walk_to_position(el, remaining) {
                    return Some(result);
                }

                let content_size = count_content_size(el);
                if remaining <= content_size {
                    return None;
                }
                remaining -= content_size;
                if remaining == 0 {
                    return None;
                }
                remaining -= 1; // closing boundary
            } else {
                // Inline element — recurse without boundaries.
                if let Some(result) = walk_to_position(el, remaining) {
                    return Some(result);
                }
                let inline_size = el.text_content().unwrap_or_default().chars().count();
                if remaining < inline_size {
                    return None;
                }
                remaining -= inline_size;
            }
        }
    }
    None
}

/// Count the model content size of a block element (excluding boundaries).
pub fn count_content_size(el: &web_sys::Element) -> usize {
    let children = el.child_nodes();
    let mut size = 0;
    for i in 0..children.length() {
        if let Some(child) = children.item(i) {
            if child.node_type() == web_sys::Node::TEXT_NODE {
                size += child.text_content().unwrap_or_default().chars().count();
            } else if child.node_type() == web_sys::Node::ELEMENT_NODE {
                let child_el: &web_sys::Element = child.unchecked_ref();
                let tag = child_el.tag_name().to_lowercase();
                let is_block = matches!(
                    tag.as_str(),
                    "p" | "h1" | "h2" | "h3" | "blockquote" | "ul" | "ol" | "li" | "pre" | "hr"
                );
                if is_block {
                    size += 2 + count_content_size(child_el);
                } else {
                    size += child_el.text_content().unwrap_or_default().chars().count();
                }
            }
        }
    }
    size
}
