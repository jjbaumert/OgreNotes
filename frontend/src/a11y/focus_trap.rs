// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Focus management primitives for modal dialogs (Phase 5 M-P8
//! piece A).
//!
//! Two-call API:
//!   1. `install_focus_trap(dialog_ref, visible)` from the component
//!      body — saves the previously-focused element when `visible`
//!      flips true, moves focus to the first focusable element
//!      inside the dialog, and restores focus on close.
//!   2. `handle_tab_trap(&keyboard_event, &dialog_el)` from the
//!      dialog's outermost `on:keydown` — cycles Tab / Shift+Tab
//!      within the dialog's focusables.
//!
//! Why both: the install side runs once per open/close transition,
//! but Tab interception has to ride on every keystroke. Splitting
//! lets us avoid attaching+detaching a JS keydown listener on every
//! mount — Leptos's on:keydown is already wired into the JSX path.

use std::cell::Cell;

use leptos::html::Div;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlElement, KeyboardEvent};

thread_local! {
    /// True while a focus_trap restore is in flight — set immediately
    /// before `el.focus()` lands and cleared one microtask after.
    /// The editor's `selectionchange` handler checks this and skips
    /// updates during the window so the auto-collapsed contenteditable
    /// selection doesn't clobber the user's pre-modal range/cursor.
    /// WASM is single-threaded so a thread_local Cell is enough.
    static FOCUS_RESTORE_IN_PROGRESS: Cell<bool> = const { Cell::new(false) };
}

/// True while [`install_focus_trap`] is restoring focus to the
/// element that owned it before the modal opened. Editor-internal
/// selectionchange handlers check this so they don't react to the
/// fresh-cursor-at-position-0 that headless Chromium synthesizes
/// when focus lands back on the contenteditable.
pub fn is_focus_restore_in_progress() -> bool {
    FOCUS_RESTORE_IN_PROGRESS.with(|c| c.get())
}

/// CSS selector matching focusable descendants. Conservative: we
/// don't include `iframe` or `audio[controls]` because they're not
/// targets we mount inside our dialogs today; if a future modal
/// hosts one, extend this selector.
const FOCUSABLE_SELECTOR: &str = "a[href], button:not([disabled]), \
    input:not([disabled]), textarea:not([disabled]), \
    select:not([disabled]), [tabindex]:not([tabindex=\"-1\"])";

/// Pick the element to focus when a dialog opens.
///
/// Prefers an element the dialog explicitly marked with `data-autofocus`
/// (typically its primary text input) over the first focusable
/// descendant. The first focusable is frequently a close button — the
/// wrong place to land for a dialog whose job is to collect text, like
/// the comment composer, where the user expects to start typing
/// immediately. Falls back to the first focusable when nothing is
/// marked, preserving the prior behavior for dialogs that don't opt in.
fn initial_focus_target(container: &Element) -> Option<HtmlElement> {
    container
        .query_selector("[data-autofocus]")
        .ok()
        .flatten()
        .or_else(|| container.query_selector(FOCUSABLE_SELECTOR).ok().flatten())
        .and_then(|el| el.dyn_into::<HtmlElement>().ok())
}

/// Save-on-open / restore-on-close focus management. Idempotent —
/// fires once per `visible` transition because Effect tracks the
/// signal.
///
/// `dialog_ref` must point at the dialog's container element (the
/// `.share-dialog`, `.confirm-dialog`, etc., **not** the backdrop).
/// The Show wrapper means `dialog_ref.get()` returns None until
/// after mount; we await one microtask before querying focusables.
pub fn install_focus_trap(
    dialog_ref: NodeRef<Div>,
    visible: Signal<bool>,
) {
    let saved: StoredValue<Option<HtmlElement>> = StoredValue::new(None);

    Effect::new(move |_| {
        let v = visible.get();
        if v {
            // Snapshot the element that opened us — typically the
            // button that toggled `visible` to true. We need this
            // before changing focus, so do it synchronously here
            // rather than inside the spawn_local below.
            let prev = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.active_element())
                .and_then(|e| e.dyn_into::<HtmlElement>().ok());
            saved.set_value(prev);

            // Defer the focus call by one microtask so the Show
            // branch has flushed the dialog into the DOM. Same
            // pattern the search_dialog uses to focus its input.
            // `.get_untracked()` because tracking doesn't apply
            // inside an async block; the Effect already tracks
            // `visible` to fire at the right moment.
            let el = dialog_ref;
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(0).await;
                if let Some(node) = el.get_untracked() {
                    if let Some(target) = initial_focus_target(node.as_ref()) {
                        let _ = target.focus();
                    }
                }
            });
        } else {
            // Restore focus on close, deferred by one microtask
            // for the same reason the open path defers — but with
            // a stronger correctness argument here. `visible` was
            // flipped to false synchronously inside whatever event
            // handler called `on_close.run(())`. The Show subtree
            // then tears down, which drops the wasm-bindgen
            // closures attached to the dialog's inner divs. If we
            // call `el.focus()` synchronously here, that runs
            // INSIDE the same reactive turn that's mid-teardown —
            // the original event keeps bubbling, hits a now-dropped
            // outer on:keydown closure, and wasm-bindgen panics
            // with "closure invoked recursively or after being
            // dropped." Deferring the focus to the next microtask
            // lets the teardown complete + the event finish
            // bubbling before we touch the DOM.
            //
            // Phase 6 regression-fix for the
            // command-palette-actions doctor scenario; the panic
            // also surfaced (silently) in trash-flow and any other
            // modal close in M-P8 piece A.
            let saved_now = saved.get_value();
            if saved_now.is_some() {
                leptos::task::spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(0).await;
                    // Flip the in-progress flag BEFORE focus() so the
                    // selectionchange the browser fires in response
                    // arrives while the editor's listener knows to
                    // ignore it. One additional microtask lets the
                    // event bubble through; then we clear so normal
                    // editing resumes.
                    FOCUS_RESTORE_IN_PROGRESS.with(|c| c.set(true));
                    if let Some(el) = saved_now {
                        let _ = el.focus();
                    }
                    gloo_timers::future::TimeoutFuture::new(0).await;
                    FOCUS_RESTORE_IN_PROGRESS.with(|c| c.set(false));
                });
            }
        }
    });
}

/// Tab/Shift+Tab cycle inside the dialog. Call from the dialog's
/// outermost `on:keydown` handler with a reference to the dialog
/// container `Element` (typically from `dialog_ref.get()` → as_ref).
/// No-op for keys other than Tab.
pub fn handle_tab_trap(e: &KeyboardEvent, dialog_el: &Element) {
    if e.key() != "Tab" {
        return;
    }
    let Ok(list) = dialog_el.query_selector_all(FOCUSABLE_SELECTOR) else {
        return;
    };
    let n = list.length();
    if n == 0 {
        // Empty dialog: swallow Tab so focus doesn't escape into
        // the background.
        e.prevent_default();
        return;
    }
    let first = list
        .item(0)
        .and_then(|n| n.dyn_into::<HtmlElement>().ok());
    let last = list
        .item(n - 1)
        .and_then(|n| n.dyn_into::<HtmlElement>().ok());
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };

    let active = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element());
    let Some(active) = active else { return };

    if e.shift_key() {
        // Shift+Tab from the first focusable wraps to last.
        if active.is_same_node(Some(first.as_ref())) {
            e.prevent_default();
            let _ = last.focus();
        }
    } else {
        // Tab from the last focusable wraps to first.
        if active.is_same_node(Some(last.as_ref())) {
            e.prevent_default();
            let _ = first.focus();
        }
    }
}

/// Defer a modal-close callback by one microtask. Use this from
/// any `on:keydown` handler that needs to trigger `on_close.run(())`
/// — call it instead of running the callback synchronously.
///
/// Why the defer: the original keydown event keeps bubbling after
/// the handler returns. When `on_close` synchronously flips a
/// `visible` signal to false, Leptos tears down the `<Show>`
/// subtree on that same reactive turn — dropping every
/// wasm-bindgen closure on the dialog's inner divs. The bubble
/// then reaches one of those dropped closures and wasm-bindgen
/// panics with "closure invoked recursively or after being
/// dropped." Deferring the close by one microtask lets the event
/// finish bubbling before the teardown.
///
/// Same defer pattern `install_focus_trap` uses for the focus-
/// restoration side of the close path. Both halves of the close
/// must defer for the modal to tear down cleanly.
pub fn defer_close(on_close: Callback<()>) {
    defer(move || on_close.run(()));
}

/// Defer an arbitrary closure by one microtask.
///
/// Same rationale as [`defer_close`], but for close paths that flip a raw
/// signal (e.g. a dropdown's `set_open(false)` or a backdrop's
/// `set_visible(false)`) rather than running an `on_close` callback.
/// Flipping the signal synchronously inside an `on:click` tears the
/// owning `<Show>` subtree down mid-event, which can re-invoke a dropped
/// closure — the Firefox "closure invoked recursively or after being
/// dropped" panic. Deferring lets the event finish before the teardown.
pub fn defer(f: impl FnOnce() + 'static) {
    spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(0).await;
        f();
    });
}

/// Like [`defer_close`], but runs `then` *after* the close has been
/// dispatched and the next microtask has settled. Used when the
/// follow-up action depends on focus + DOM selection having been
/// restored to the modal-owning element.
///
/// The command palette's Action-mode Enter handler is the motivating
/// case: dispatching `editor.bold` while the palette input still has
/// focus leaves the editor's DOM selection unobservable to the
/// command's selection-sync path (the palette input is outside the
/// editor container, so `read_dom_selection_from(container)` returns
/// `None`). Running the command after the close lets the editor
/// regain focus and the highlighted range first, so the command
/// applies against the intended selection.
pub fn defer_close_then_run(on_close: Callback<()>, then: impl FnOnce() + 'static) {
    spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(0).await;
        on_close.run(());
        gloo_timers::future::TimeoutFuture::new(0).await;
        then();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Regression: a dialog that opens to collect text must land focus
    /// on its text entry, not its close button. The comment popup
    /// renders the close `<button>` before the `<textarea>`, so without
    /// the `data-autofocus` preference the trap focused the close
    /// button and the user had to click into the field before typing.
    #[wasm_bindgen_test]
    fn initial_focus_prefers_data_autofocus_over_first_focusable() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = doc.create_element("div").unwrap();
        // DOM order mirrors the comment popup: close button first (the
        // first focusable), text entry second.
        let button = doc.create_element("button").unwrap();
        let textarea = doc.create_element("textarea").unwrap();
        textarea.set_attribute("data-autofocus", "true").unwrap();
        container.append_child(&button).unwrap();
        container.append_child(&textarea).unwrap();
        doc.body().unwrap().append_child(&container).unwrap();

        let target =
            initial_focus_target(&container).expect("a focus target should be found");
        assert_eq!(
            target.tag_name().to_lowercase(),
            "textarea",
            "focus must land on the marked text entry, not the close button",
        );

        container.remove();
    }

    /// Dialogs that don't opt in keep the prior behavior: first
    /// focusable wins.
    #[wasm_bindgen_test]
    fn initial_focus_falls_back_to_first_focusable() {
        let doc = web_sys::window().unwrap().document().unwrap();
        let container = doc.create_element("div").unwrap();
        let first = doc.create_element("button").unwrap();
        first.set_attribute("id", "first").unwrap();
        let second = doc.create_element("button").unwrap();
        second.set_attribute("id", "second").unwrap();
        container.append_child(&first).unwrap();
        container.append_child(&second).unwrap();
        doc.body().unwrap().append_child(&container).unwrap();

        let target =
            initial_focus_target(&container).expect("a focusable should be found");
        assert_eq!(
            target.id(),
            "first",
            "with nothing marked, the first focusable wins",
        );

        container.remove();
    }
}
