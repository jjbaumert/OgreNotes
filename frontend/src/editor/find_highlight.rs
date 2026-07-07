// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #147: highlight *all* find matches at once via the CSS Custom Highlight
//! API. Each match's model `(from, to)` is mapped to a DOM `Range` (reusing the
//! editor's model→DOM mapping, [`crate::editor::view::find_dom_position`]) and
//! registered under two named highlights — `ogre-find` for every match and
//! `ogre-find-active` for the current one — styled by `::highlight()` rules in
//! `main.css`.
//!
//! The API is reached via `js_sys` reflection (`window.CSS.highlights`,
//! `new Highlight(...ranges)`) rather than `web_sys`, whose `Highlight`
//! bindings are gated behind the `web_sys_unstable_apis` cfg. On a browser
//! without the API every helper here no-ops, so the feature degrades to the
//! existing active-match-via-selection behavior instead of erroring.

use wasm_bindgen::{JsCast, JsValue};

const ALL_NAME: &str = "ogre-find";
const ACTIVE_NAME: &str = "ogre-find-active";

/// The live `CSS.highlights` registry, or `None` when the API is unavailable.
fn highlight_registry() -> Option<JsValue> {
    let window = web_sys::window()?;
    let css = js_sys::Reflect::get(&window, &JsValue::from_str("CSS")).ok()?;
    if css.is_undefined() || css.is_null() {
        return None;
    }
    let registry = js_sys::Reflect::get(&css, &JsValue::from_str("highlights")).ok()?;
    if registry.is_undefined() || registry.is_null() {
        return None;
    }
    Some(registry)
}

/// A DOM `Range` spanning model positions `[from, to)` in the live editor.
fn range_for(
    document: &web_sys::Document,
    container: &web_sys::HtmlElement,
    from: usize,
    to: usize,
) -> Option<web_sys::Range> {
    let (start_node, start_off) = crate::editor::view::find_dom_position(container, from)?;
    let (end_node, end_off) = crate::editor::view::find_dom_position(container, to)?;
    let range = document.create_range().ok()?;
    range.set_start(&start_node, start_off as u32).ok()?;
    range.set_end(&end_node, end_off as u32).ok()?;
    Some(range)
}

/// `new Highlight(...ranges)`.
fn make_highlight(window: &web_sys::Window, ranges: &[web_sys::Range]) -> Option<JsValue> {
    let ctor: js_sys::Function = js_sys::Reflect::get(window, &JsValue::from_str("Highlight"))
        .ok()?
        .dyn_into()
        .ok()?;
    let args = js_sys::Array::new();
    for r in ranges {
        args.push(r.as_ref());
    }
    js_sys::Reflect::construct(&ctor, &args).ok()
}

/// `registry.set(name, highlight)`.
fn registry_set(registry: &JsValue, name: &str, highlight: &JsValue) {
    if let Ok(set_fn) = js_sys::Reflect::get(registry, &JsValue::from_str("set")) {
        if let Ok(set_fn) = set_fn.dyn_into::<js_sys::Function>() {
            let _ = set_fn.call2(registry, &JsValue::from_str(name), highlight);
        }
    }
}

/// `registry.delete(name)`.
fn registry_delete(registry: &JsValue, name: &str) {
    if let Ok(del) = js_sys::Reflect::get(registry, &JsValue::from_str("delete")) {
        if let Ok(del) = del.dyn_into::<js_sys::Function>() {
            let _ = del.call1(registry, &JsValue::from_str(name));
        }
    }
}

/// Register highlights for every match in `matches`, with the `active`-th shown
/// distinctly. A no-op (after clearing) when the API is unavailable, the editor
/// isn't mounted, or no match maps to a DOM range.
pub fn apply(matches: &[(usize, usize)], active: usize) {
    let Some(registry) = highlight_registry() else { return };
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let Ok(Some(el)) = document.query_selector(".editor-content") else {
        clear();
        return;
    };
    let Ok(container) = el.dyn_into::<web_sys::HtmlElement>() else { return };

    let mut all_ranges = Vec::with_capacity(matches.len());
    let mut active_ranges: Vec<web_sys::Range> = Vec::new();
    for (i, &(from, to)) in matches.iter().enumerate() {
        if let Some(r) = range_for(&document, &container, from, to) {
            if i == active {
                active_ranges.push(r.clone());
            }
            all_ranges.push(r);
        }
    }

    if all_ranges.is_empty() {
        clear();
        return;
    }
    if let Some(h) = make_highlight(&window, &all_ranges) {
        registry_set(&registry, ALL_NAME, &h);
    }
    if active_ranges.is_empty() {
        registry_delete(&registry, ACTIVE_NAME);
    } else if let Some(h) = make_highlight(&window, &active_ranges) {
        registry_set(&registry, ACTIVE_NAME, &h);
    }
}

/// Remove both find highlights. Safe to call when nothing is registered.
pub fn clear() {
    if let Some(registry) = highlight_registry() {
        registry_delete(&registry, ALL_NAME);
        registry_delete(&registry, ACTIVE_NAME);
    }
}
