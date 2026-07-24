// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::blobs;
use crate::editor::commands;
use crate::editor::model::{Fragment, MarkType, Node, NodeType, Slice};
use crate::editor::plugins::HistoryPlugin;
use crate::editor::selection::Selection;
use crate::editor::state::{EditorState, Transaction};
use crate::editor::view::EditorView;
use crate::editor::yrs_bridge;

use super::calendar_modal::{
    CalendarModal, CalendarModalMode, CalendarModalState, ModalOutcome,
};
use super::code_lang_chip::{CodeLangChip, CodeLangChipState};
use super::editor_context_menu::{EditorContextCommand, EditorContextMenu};
use super::kanban_card_modal::{
    KanbanCardModal, KanbanCardModalMode, KanbanCardModalState, KanbanCardOutcome,
};
use super::mermaid_modal::{MermaidModal, MermaidModalOutcome, MermaidModalState};
use super::toolbar::ToolbarCommand;

/// Props for the editor component.
#[derive(Clone)]
pub struct EditorProps {
    /// Initial document content as yrs bytes. If None, creates an empty doc.
    pub initial_content: Option<Vec<u8>>,
    /// Callback when the document changes (for auto-save).
    pub on_change: Callback<Vec<u8>>,
    /// Callback to report the current editor state (for toolbar).
    pub on_state_change: Callback<EditorState>,
    /// Signal for receiving toolbar commands.
    pub command_signal: ReadSignal<Option<ToolbarCommand>>,
    /// Signal for receiving remote document updates from collaborators.
    /// When set, the editor replaces its document content and re-renders.
    pub remote_state: ReadSignal<Option<EditorState>>,
    /// Document ID (needed for blob upload).
    pub doc_id: String,
    /// Callback fired when .editor-container scrolls (for overlay repositioning).
    pub on_scroll: Option<Callback<()>>,
    /// Callback with step maps after a doc-changing transaction (for comment anchor remapping).
    pub on_mapping: Option<Callback<(Vec<crate::editor::transform::StepMap>, Node)>>,
    /// Fired when the user picks "Comment" from the editor's right-
    /// click menu. The page owns the comment-popup state (block-id +
    /// anchors + popup position), so the handler bubbles up rather
    /// than living inside the editor. None disables the menu item.
    pub on_request_comment: Option<Callback<()>>,
    /// When true, the editor is rendered read-only: no contenteditable, no
    /// input listeners, toolbar commands still land as no-ops. Used for
    /// trashed documents.
    #[allow(dead_code)]
    pub readonly: bool,
}

/// Apply a transaction to the editor view and notify callbacks.
/// If `history` is provided, records the transaction for undo/redo.
fn apply_and_notify(
    view: &EditorView,
    txn: Transaction,
    history: Option<&Rc<RefCell<HistoryPlugin>>>,
    on_change: &Callback<Vec<u8>>,
    on_state_change: &Callback<EditorState>,
    on_mapping: Option<&Callback<(Vec<crate::editor::transform::StepMap>, Node)>>,
) {
    let old_state = view.state();
    if let Some(h) = history {
        h.borrow_mut().record(&txn, &old_state.doc);
    }
    let step_maps = txn.maps.clone();
    let old_doc = old_state.doc.clone();
    let new_state = old_state.apply(txn);
    view.update_state(new_state.clone());
    on_state_change.run(new_state.clone());
    if new_state.doc != old_doc {
        // Phase 1 observability — fires once per doc-changing
        // transaction. Pairs with the server's
        // `ws.messages_total{type=update}` to surface "edits the
        // user typed but the server never received" (the current
        // edits-not-persisting bug class).
        crate::observability::inc(crate::observability::EDITOR_TRANSACTIONS);
        on_change.run(yrs_bridge::doc_to_ydoc_bytes(&new_state.doc));
        if let Some(cb) = on_mapping {
            cb.run((step_maps, old_doc));
        }
    }
}

/// Task 7 — recompute the language-chip overlay state from the
/// current editor state + live DOM selection. Chip shows iff the
/// caret is inside a code block. Coordinates are relative to
/// `wrapper` (the chip's positioned offset parent — `.editor-
/// container`, a sibling of `.editor-content`, not the
/// contenteditable div itself).
fn refresh_code_lang_chip(
    state: &EditorState,
    wrapper: &web_sys::Element,
    chip: RwSignal<Option<CodeLangChipState>>,
) {
    // Compute the new value first (early-return via `?` for the "no code
    // block here" cases) and only write the signal if it actually changed.
    // This runs inside a reactive Effect on every dispatch, so an
    // unconditional `chip.set(...)` would force a full chip `<select>`
    // rebuild on every keystroke inside a code block; `get_untracked` avoids
    // adding a dependency on `chip` itself.
    let new = (|| {
        let current = commands::code_block_language(state)?;
        // Find the code block's <pre> from the DOM selection anchor.
        let pre = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_selection().ok().flatten())
            .and_then(|s| s.anchor_node())
            .and_then(|n| match n.dyn_ref::<web_sys::Element>() {
                Some(el) => Some(el.clone()),
                None => n.parent_element(),
            })
            .and_then(|el| el.closest("pre").ok().flatten())?;
        let pre_rect = pre.get_bounding_client_rect();
        let wrap_rect = wrapper.get_bounding_client_rect();
        // `pre_rect`/`wrap_rect` are viewport-space (getBoundingClientRect), but
        // the chip is absolutely positioned in `wrapper`'s (`.editor-container`)
        // content space, which scrolls independently of the viewport. Add back
        // the wrapper's scroll offset so the chip tracks the code block instead
        // of drifting as the document scrolls.
        let top = pre_rect.top() - wrap_rect.top() + wrapper.scroll_top() as f64 + 4.0;
        // `right` doesn't need the symmetric `scroll_left()` compensation:
        // `.editor-content` is capped at `max-width` and never exceeds
        // `.editor-container`'s own width, and `.editor-content pre` has its
        // own `overflow-x: auto` that absorbs long code lines internally — so
        // `.editor-container` itself never accumulates horizontal scroll from
        // a code block and `scroll_left()` is always 0 on this path.
        let right = wrap_rect.right() - pre_rect.right() + 4.0;
        Some(CodeLangChipState {
            top,
            right,
            current,
        })
    })();
    if chip.get_untracked() != new {
        chip.set(new);
    }
}

/// Find the model position just after the top-level block containing the cursor.
/// #136 — outcome of a click somewhere inside a `.calendar-block`.
/// The observer stops-propagates the click and hands the outcome
/// to the appropriate dispatcher: modal-open opens the form,
/// attr-update mutates the Calendar container.
enum CalendarClickOutcome {
    /// Open the event modal.
    OpenModal(CalendarModalState),
    /// Merge `updates` into the Calendar container's attrs.
    /// Used for view-toggle + prev/next/today.
    UpdateAttrs {
        block_id: String,
        updates: HashMap<String, String>,
    },
}

// ─── #136 Calendar drag support ─────────────────────────────────
//
// Pointer-driven drag pipeline for moving and resizing events.
// State lives in a thread_local because the pointerdown/move/up
// closures need to share mutable access without passing signals
// around. On pointerup with an activated drag we compute the new
// event attrs and write to `set_drag_commit`, which a reactive
// Effect picks up and dispatches through `edit_calendar_event`.

const DRAG_THRESHOLD_PX: f64 = 4.0;
const HOUR_HEIGHT_PX: f64 = 40.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragKind {
    AllDay,
    Timed,
}

struct DragCandidate {
    block_id: String,
    event_id: String,
    mode: DragMode,
    kind: DragKind,
    initial_x: f64,
    initial_y: f64,
    // Snapshot the event's current times so the delta / target
    // math has a stable baseline through the whole drag.
    start_date: String,
    end_date: String,
    start_at: String,
    end_at: String,
    element: send_wrapper::SendWrapper<web_sys::Element>,
    activated: bool,
}

thread_local! {
    static DRAG_STATE: RefCell<Option<DragCandidate>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
struct DragCommit {
    block_id: String,
    event_id: String,
    new_attrs: HashMap<String, String>,
}

/// pointerdown: if the target is a draggable event or a resize
/// handle inside a `.calendar-block`, snapshot the state. Nothing
/// visible happens until the pointer moves past
/// `DRAG_THRESHOLD_PX` (see [`drag_on_pointer_move`]).
/// Set (or clear) the viewport-wide drag cursor. `.kanban-card--dragging`
/// and `.calendar-event--dragging` only affect the source element,
/// which is under `pointer-events: none` for most of the drag —
/// meaning the OS cursor is whatever the element under the pointer
/// asks for, not "grabbing". Stamping the cursor on `<body>` covers
/// every hit-test target uniformly for as long as the drag is
/// active. Called on drag activate and on any release/cancel.
fn set_body_drag_cursor(active: bool) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Some(body) = doc.body() else { return };
    let style = body.style();
    if active {
        let _ = style.set_property("cursor", "grabbing");
    } else {
        let _ = style.remove_property("cursor");
    }
}

fn drag_on_pointer_down(ev: &web_sys::PointerEvent) {
    // Only left-button drags.
    if ev.button() != 0 {
        return;
    }
    let Some(target) = ev.target() else { return };
    let Ok(target_el) = target.dyn_into::<web_sys::Element>() else {
        return;
    };
    // Resize handle takes precedence over the containing draggable
    // — a click on the handle should resize, not move.
    let (mode, kind_str, event_el) =
        if let Ok(Some(handle)) = target_el.closest("[data-calendar-resize]") {
            let kind = handle.get_attribute("data-calendar-resize").unwrap_or_default();
            let ev_el = handle
                .closest("[data-calendar-draggable]")
                .ok()
                .flatten();
            let Some(ev_el) = ev_el else { return };
            (DragMode::Resize, kind, ev_el)
        } else if let Ok(Some(el)) = target_el.closest("[data-calendar-draggable]") {
            let kind = el.get_attribute("data-calendar-draggable").unwrap_or_default();
            (DragMode::Move, kind, el)
        } else {
            return;
        };
    let kind = match kind_str.as_str() {
        "timed" => DragKind::Timed,
        _ => DragKind::AllDay,
    };
    // Suppress the browser's native text-selection anchor —
    // matches the equivalent kanban_drag_on_pointer_down call.
    // Combined with the `.calendar-event { user-select: none }`
    // CSS, this stops the pointer sweep from highlighting the
    // event's content text as the user drags.
    ev.prevent_default();
    let Ok(Some(block_el)) = event_el.closest(".calendar-block") else {
        return;
    };
    let Some(block_id) = block_el.get_attribute("data-block-id") else {
        return;
    };
    let Some(event_id) = event_el.get_attribute("data-event-id") else {
        return;
    };
    let start_date = event_el.get_attribute("data-start-date").unwrap_or_default();
    let end_date = event_el.get_attribute("data-end-date").unwrap_or_default();
    let start_at = event_el.get_attribute("data-start-at").unwrap_or_default();
    let end_at = event_el.get_attribute("data-end-at").unwrap_or_default();

    DRAG_STATE.with(|s| {
        *s.borrow_mut() = Some(DragCandidate {
            block_id,
            event_id,
            mode,
            kind,
            initial_x: ev.client_x() as f64,
            initial_y: ev.client_y() as f64,
            start_date,
            end_date,
            start_at,
            end_at,
            element: send_wrapper::SendWrapper::new(event_el),
            activated: false,
        });
    });
}

/// pointermove: promote candidate to active drag past threshold;
/// apply visual feedback (translate for move, height for resize).
fn drag_on_pointer_move(ev: &web_sys::PointerEvent) {
    DRAG_STATE.with(|s| {
        let mut opt = s.borrow_mut();
        let Some(state) = opt.as_mut() else { return };
        let dx = ev.client_x() as f64 - state.initial_x;
        let dy = ev.client_y() as f64 - state.initial_y;
        if !state.activated {
            if (dx * dx + dy * dy).sqrt() < DRAG_THRESHOLD_PX {
                return;
            }
            state.activated = true;
            let _ = state.element.class_list().add_1("calendar-event--dragging");
            set_body_drag_cursor(true);
        }
        // Visual feedback while drag is active. For Move we
        // translate; for Resize we adjust height (timed) or width
        // (all-day span).
        match (state.mode, state.kind) {
            (DragMode::Move, _) => {
                let _ = state.element.set_attribute(
                    "style",
                    &compose_drag_style(&state.element, dx, dy),
                );
            }
            (DragMode::Resize, DragKind::Timed) => {
                // Recover the current inline top from the initial
                // style if present; otherwise best-effort dy from
                // the offsetHeight.
                let base_h = timed_height_px(&state.element);
                let new_h = (base_h + dy).max(20.0);
                let base_style = timed_base_style(&state.element);
                let _ = state
                    .element
                    .set_attribute("style", &format!("{base_style} height: {new_h}px;"));
            }
            (DragMode::Resize, DragKind::AllDay) => {
                let _ = state.element.set_attribute(
                    "style",
                    &format!("width: calc(100% + {dx}px);"),
                );
            }
        }
    });
}

fn compose_drag_style(el: &web_sys::Element, dx: f64, dy: f64) -> String {
    // Preserve any top/height inline styles (set by the timed
    // renderer) while adding translate on top of them.
    let base = el
        .get_attribute("data-drag-base-style")
        .unwrap_or_else(|| el.get_attribute("style").unwrap_or_default());
    let _ = el.set_attribute("data-drag-base-style", &base);
    format!("{base} transform: translate({dx}px, {dy}px);")
}

fn timed_base_style(el: &web_sys::Element) -> String {
    el.get_attribute("data-drag-base-style")
        .or_else(|| el.get_attribute("style"))
        .map(|s| {
            // Strip out any prior `height:` so we can re-append.
            s.split(';')
                .filter(|piece| !piece.trim_start().starts_with("height"))
                .collect::<Vec<_>>()
                .join(";")
        })
        .unwrap_or_default()
}

fn timed_height_px(el: &web_sys::Element) -> f64 {
    if let Ok(html) = el.clone().dyn_into::<web_sys::HtmlElement>() {
        return html.offset_height() as f64;
    }
    HOUR_HEIGHT_PX
}

/// pointerup: if a drag was activated, compute the new event
/// attrs from the drop position and hand them off to the reactive
/// dispatcher via `set_drag_commit`.
fn drag_on_pointer_up(
    ev: &web_sys::PointerEvent,
    set_drag_commit: WriteSignal<Option<DragCommit>>,
) {
    let Some(state) = DRAG_STATE.with(|s| s.borrow_mut().take()) else {
        return;
    };
    if !state.activated {
        // Not a drag — let the normal click observer take over.
        return;
    }
    // Prevent the click that would otherwise fire on pointerup
    // (would open the edit modal on top of the just-committed
    // drag). preventDefault on pointerup + a "click swallowed"
    // flag is the usual dance; here the reactive commit is
    // idempotent so it's cheaper to just stop propagation.
    ev.stop_propagation();
    ev.prevent_default();

    let _ = state.element.class_list().remove_1("calendar-event--dragging");
    // Clear our transform / height overrides so the next
    // render-pass picks up the newly-committed model values.
    let base = state
        .element
        .get_attribute("data-drag-base-style")
        .unwrap_or_default();
    let _ = state.element.set_attribute("style", &base);
    let _ = state.element.remove_attribute("data-drag-base-style");
    set_body_drag_cursor(false);

    let commit = drag_compute_commit(&state, ev);
    if let Some(c) = commit {
        set_drag_commit.set(Some(c));
    }
}

/// pointercancel: browser or OS aborted the pointer stream mid-drag
/// (tab hidden, touch cancelled, focus stolen). Restores visual
/// state and clears the drag candidate — same cleanup as pointerup
/// but without the commit / dispatch. Without this, an interrupted
/// drag leaves DRAG_STATE populated and the dragged element with
/// stale inline transform / height overrides until the next
/// pointerdown clears them.
fn drag_on_pointer_cancel() {
    let Some(state) = DRAG_STATE.with(|s| s.borrow_mut().take()) else {
        return;
    };
    let _ = state.element.class_list().remove_1("calendar-event--dragging");
    let base = state
        .element
        .get_attribute("data-drag-base-style")
        .unwrap_or_default();
    let _ = state.element.set_attribute("style", &base);
    let _ = state.element.remove_attribute("data-drag-base-style");
    set_body_drag_cursor(false);
}

/// Compute the `edit_calendar_event` attrs from the drag's release
/// position. Uses `elementFromPoint` to resolve the drop target's
/// day (and hour when in a time-grid view).
fn drag_compute_commit(
    state: &DragCandidate,
    ev: &web_sys::PointerEvent,
) -> Option<DragCommit> {
    let doc = web_sys::window()?.document()?;
    // Hide the dragged element for hit-testing so
    // elementFromPoint reveals the underlying drop target.
    let _ = state.element.class_list().add_1("calendar-event--hidden-hit");
    let hit = doc.element_from_point(ev.client_x() as f32, ev.client_y() as f32);
    let _ = state
        .element
        .class_list()
        .remove_1("calendar-event--hidden-hit");
    let target = hit?;
    let (drop_date, drop_time) = drop_target_date_time(&target)?;
    let mut new_attrs = HashMap::new();
    match (state.mode, state.kind) {
        (DragMode::Move, DragKind::AllDay) => {
            // Shift both dates by the same day-delta.
            let old_start = &state.start_date;
            let old_end = if state.end_date.is_empty() {
                old_start.clone()
            } else {
                state.end_date.clone()
            };
            let delta = day_delta(old_start, &drop_date)?;
            let new_start = shift_ymd(old_start, delta)?;
            let new_end = shift_ymd(&old_end, delta)?;
            new_attrs.insert("allDay".into(), "true".into());
            new_attrs.insert("startDate".into(), new_start);
            new_attrs.insert("endDate".into(), new_end);
        }
        (DragMode::Resize, DragKind::AllDay) => {
            // Extend endDate to the drop day; guard against
            // dropping BEFORE startDate (fall back to a no-op).
            if !state.start_date.is_empty() && drop_date >= state.start_date {
                new_attrs.insert("allDay".into(), "true".into());
                new_attrs.insert("startDate".into(), state.start_date.clone());
                new_attrs.insert("endDate".into(), drop_date);
            }
        }
        (DragMode::Move, DragKind::Timed) => {
            // Shift both startAt and endAt by the delta between
            // the old and new (date + time).
            let old_start_ms = js_date_ms(&state.start_at)?;
            let old_end_ms = if state.end_at.is_empty() {
                old_start_ms
            } else {
                js_date_ms(&state.end_at)?
            };
            let new_start_ms = compose_local_ms(&drop_date, drop_time.as_deref())?;
            let delta_ms = new_start_ms - old_start_ms;
            let new_end_ms = old_end_ms + delta_ms;
            new_attrs.insert("allDay".into(), "false".into());
            new_attrs.insert("startAt".into(), ms_to_iso(new_start_ms));
            new_attrs.insert("endAt".into(), ms_to_iso(new_end_ms));
        }
        (DragMode::Resize, DragKind::Timed) => {
            let old_start_ms = js_date_ms(&state.start_at)?;
            let new_end_ms = compose_local_ms(&drop_date, drop_time.as_deref())?;
            if new_end_ms > old_start_ms {
                new_attrs.insert("allDay".into(), "false".into());
                new_attrs.insert("startAt".into(), state.start_at.clone());
                new_attrs.insert("endAt".into(), ms_to_iso(new_end_ms));
            }
        }
    }
    if new_attrs.is_empty() {
        return None;
    }
    Some(DragCommit {
        block_id: state.block_id.clone(),
        event_id: state.event_id.clone(),
        new_attrs,
    })
}

/// Walk from a hit-tested element up to the nearest
/// `[data-calendar-date]`, returning the date + (optional) time.
fn drop_target_date_time(el: &web_sys::Element) -> Option<(String, Option<String>)> {
    let anchor = el.closest("[data-calendar-date]").ok().flatten()?;
    let date = anchor.get_attribute("data-calendar-date")?;
    let time = anchor.get_attribute("data-calendar-time");
    Some((date, time))
}

fn day_delta(from: &str, to: &str) -> Option<i64> {
    let (f_y, f_m, f_d) = parse_ymd(from)?;
    let (t_y, t_m, t_d) = parse_ymd(to)?;
    let f_ms = js_sys::Date::new_with_year_month_day(f_y, (f_m - 1) as i32, f_d as i32)
        .get_time();
    let t_ms = js_sys::Date::new_with_year_month_day(t_y, (t_m - 1) as i32, t_d as i32)
        .get_time();
    Some(((t_ms - f_ms) / (24.0 * 60.0 * 60.0 * 1000.0)).round() as i64)
}

fn shift_ymd(ymd: &str, days: i64) -> Option<String> {
    let (y, m, d) = parse_ymd(ymd)?;
    let base = js_sys::Date::new_with_year_month_day(y, (m - 1) as i32, d as i32);
    let new_ms = base.get_time() + (days as f64) * 24.0 * 60.0 * 60.0 * 1000.0;
    let shifted = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(new_ms));
    Some(format!(
        "{:04}-{:02}-{:02}",
        shifted.get_full_year(),
        shifted.get_month() + 1,
        shifted.get_date(),
    ))
}

fn js_date_ms(iso: &str) -> Option<f64> {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(iso));
    let ms = d.get_time();
    if ms.is_nan() { None } else { Some(ms) }
}

fn compose_local_ms(date: &str, time: Option<&str>) -> Option<f64> {
    let (y, m, d) = parse_ymd(date)?;
    let (h, mi) = match time {
        Some(t) => {
            let mut parts = t.splitn(2, ':');
            let h: u32 = parts.next()?.parse().ok()?;
            let mi: u32 = parts.next()?.parse().ok()?;
            (h, mi)
        }
        None => (0, 0),
    };
    let date_js = js_sys::Date::new_with_year_month_day_hr_min_sec(
        y,
        (m - 1) as i32,
        d as i32,
        h as i32,
        mi as i32,
        0,
    );
    Some(date_js.get_time())
}

fn ms_to_iso(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    String::from(d.to_iso_string())
}

// ─── #137 Kanban click routing ──────────────────────────────────

/// One column-scope operation queued by the click observer. The
/// signal dispatcher in the component below reads it and calls
/// the matching `commands::*_kanban_column` function.
#[derive(Debug, Clone)]
enum KanbanColumnAction {
    Add { kanban_id: String },
    Rename { kanban_id: String, column_id: String, prompt_default: String },
    Remove { kanban_id: String, column_id: String },
    /// Phase 4a — prompt user for a numeric WIP limit. Empty
    /// submission clears; non-numeric is rejected silently.
    SetWipLimit { kanban_id: String, column_id: String, prompt_default: String },
}

/// Copy a deep link to `block_id` in the CURRENT document to the
/// clipboard: `{origin}{pathname}#b=<blockId>`. Uses the same
/// `navigator.clipboard.writeText` reflection pattern as
/// `sidebar::copy_doc_link`.
fn copy_block_link(block_id: &str) {
    let Some(window) = web_sys::window() else { return };
    let Ok(origin) = window.location().origin() else { return };
    let Ok(path) = window.location().pathname() else { return };
    let href = format!("{origin}{path}#b={block_id}");
    let write_text = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
        .and_then(|clip| js_sys::Reflect::get(&clip, &"writeText".into()))
        .and_then(|func| func.dyn_into::<js_sys::Function>());
    if let Ok(write_text) = write_text {
        let clip = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
            .unwrap_or(wasm_bindgen::JsValue::NULL);
        let _ = write_text.call1(&clip, &href.into());
    }
}

/// window.prompt() wrapper — returns None on cancel/absent window.
fn window_prompt(message: &str, default: &str) -> Option<String> {
    web_sys::window()?
        .prompt_with_message_and_default(message, default)
        .ok()
        .flatten()
}

/// window.confirm() wrapper — false on absent window (so a
/// non-browser test env doesn't fire destructive commands).
fn window_confirm(message: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(message).ok())
        .unwrap_or(false)
}

/// One command emitted by the kanban click observer. The signal
/// dispatcher below fires the matching `commands::*_kanban_*`
/// function.
#[derive(Debug, Clone)]
enum KanbanClickOutcome {
    OpenAddCardModal(crate::components::kanban_card_modal::KanbanCardModalState),
    OpenEditCardModal(crate::components::kanban_card_modal::KanbanCardModalState),
    AddColumn { kanban_id: String },
    RenameColumn { kanban_id: String, column_id: String, prompt_default: String },
    RemoveColumn { kanban_id: String, column_id: String },
    /// Phase 4a — user clicked the count pill on a column
    /// header. `prompt_default` is the current wipLimit stringly
    /// (empty when unset).
    SetWipLimit { kanban_id: String, column_id: String, prompt_default: String },
}

fn kanban_click_outcome(ev: &web_sys::MouseEvent) -> Option<KanbanClickOutcome> {
    let target = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let action_el = target.closest("[data-kanban-action]").ok()??;
    let action = action_el.get_attribute("data-kanban-action")?;
    let block_el = action_el.closest(".kanban-block").ok()??;
    let kanban_id = block_el.get_attribute("data-block-id")?;
    match action.as_str() {
        "add-card" => {
            // The + Add card button sits inside a `.kanban-column`;
            // the column's `data-block-id` identifies the parent.
            let column_el = action_el.closest(".kanban-column").ok()??;
            let column_id = column_el.get_attribute("data-block-id")?;
            let state = crate::components::kanban_card_modal::KanbanCardModalState::new_add(
                column_id,
            );
            Some(KanbanClickOutcome::OpenAddCardModal(state))
        }
        "edit-card" => {
            let card_id = action_el.get_attribute("data-block-id")?;
            let title = action_el.get_attribute("data-title").unwrap_or_default();
            let content = action_el
                .get_attribute("data-content")
                .unwrap_or_default();
            let color = extract_kanban_color_class(&action_el)
                .unwrap_or_else(|| "blue".into());
            let due_at = action_el.get_attribute("data-due-at").unwrap_or_default();
            let labels = action_el.get_attribute("data-labels").unwrap_or_default();
            let assignee_id = action_el
                .get_attribute("data-assignee-id")
                .unwrap_or_default();
            let assignee_name = action_el
                .get_attribute("data-assignee-name")
                .unwrap_or_default();
            let state = crate::components::kanban_card_modal::KanbanCardModalState {
                mode: crate::components::kanban_card_modal::KanbanCardModalMode::Edit {
                    card_id,
                },
                title,
                content,
                color,
                due_at,
                labels,
                assignee_id,
                assignee_name,
            };
            Some(KanbanClickOutcome::OpenEditCardModal(state))
        }
        "add-column" => {
            let _ = kanban_id;
            Some(KanbanClickOutcome::AddColumn {
                kanban_id: block_el.get_attribute("data-block-id")?,
            })
        }
        "rename-column" => {
            let column_el = action_el.closest(".kanban-column").ok()??;
            let column_id = column_el.get_attribute("data-block-id")?;
            let title = column_el
                .get_attribute("data-title")
                .unwrap_or_default();
            Some(KanbanClickOutcome::RenameColumn {
                kanban_id: block_el.get_attribute("data-block-id")?,
                column_id,
                prompt_default: title,
            })
        }
        "remove-column" => {
            let column_el = action_el.closest(".kanban-column").ok()??;
            let column_id = column_el.get_attribute("data-block-id")?;
            Some(KanbanClickOutcome::RemoveColumn {
                kanban_id: block_el.get_attribute("data-block-id")?,
                column_id,
            })
        }
        "set-wip-limit" => {
            let column_el = action_el.closest(".kanban-column").ok()??;
            let column_id = column_el.get_attribute("data-block-id")?;
            let current = action_el.get_attribute("data-wip-limit").unwrap_or_default();
            Some(KanbanClickOutcome::SetWipLimit {
                kanban_id: block_el.get_attribute("data-block-id")?,
                column_id,
                prompt_default: current,
            })
        }
        _ => None,
    }
}

/// Pull the `kanban-card--<color>` modifier out of a `.kanban-card`
/// element's class list. Same shape as `extract_event_color_class`.
fn extract_kanban_color_class(el: &web_sys::Element) -> Option<String> {
    let class = el.get_attribute("class")?;
    for token in class.split_whitespace() {
        if let Some(color) = token.strip_prefix("kanban-card--") {
            return Some(color.to_string());
        }
    }
    None
}

/// Convert a KanbanCardModalState into the attribute bag the
/// `edit_kanban_card` / `add_kanban_card` commands expect.
fn kanban_modal_state_to_attrs(
    s: &crate::components::kanban_card_modal::KanbanCardModalState,
) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    attrs.insert("title".into(), s.title.clone());
    attrs.insert("content".into(), s.content.clone());
    attrs.insert("color".into(), s.color.clone());
    // Phase 4b/4c fields — only insert when non-empty. Combined
    // with edit_kanban_card's MODAL_OWNED clear step, this means
    // an empty value in the modal clears the attribute on the
    // card rather than persisting an empty string.
    if !s.due_at.is_empty() {
        attrs.insert("dueAt".into(), s.due_at.clone());
    }
    if !s.labels.is_empty() {
        attrs.insert("labels".into(), s.labels.clone());
    }
    if !s.assignee_id.is_empty() {
        attrs.insert("assigneeId".into(), s.assignee_id.clone());
        attrs.insert("assigneeName".into(), s.assignee_name.clone());
    }
    attrs
}

// ─── #137 Kanban drag (Phase 3) ─────────────────────────────────

/// Snapshot captured on pointerdown when the target is a
/// `[data-kanban-draggable="card"]`. `activated` flips to true
/// once the pointer has moved past `DRAG_THRESHOLD_PX` — before
/// that, the pointerdown could still resolve as a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KanbanDragKind {
    Card,
    /// Phase 4a — column reorder. The drag handle is the column
    /// header (`data-kanban-draggable="column"`); on drop, the
    /// column moves to the slot the pointer is over inside the
    /// Kanban strip.
    Column,
}

struct KanbanDragCandidate {
    kind: KanbanDragKind,
    /// Card blockId for Card drags, column blockId for Column drags.
    item_id: String,
    initial_x: f64,
    initial_y: f64,
    /// The card's viewport-relative top-left at drag start. Used
    /// together with the pointer delta to place the card as
    /// `position: fixed; left/top` — escapes the source column's
    /// `overflow: hidden` clip so the visual can actually track
    /// the pointer across columns.
    initial_rect_left: f64,
    initial_rect_top: f64,
    initial_rect_width: f64,
    initial_rect_height: f64,
    element: send_wrapper::SendWrapper<web_sys::Element>,
    /// A blank div inserted at the source's slot while the drag
    /// is active. Never moves — always marks where the item came
    /// from. Removed on pointerup / pointercancel.
    source_ghost: Option<send_wrapper::SendWrapper<web_sys::Element>>,
    /// Separate live indicator that follows the pointer to
    /// wherever the drop would land. Only present when the pointer
    /// is over a valid target; removed as soon as the pointer
    /// leaves. Its final position on pointerup determines the
    /// commit — if absent, the drag cancels (returns card to
    /// source with no CRDT churn).
    drop_indicator: Option<send_wrapper::SendWrapper<web_sys::Element>>,
    activated: bool,
    /// Original inline style, restored on pointerup so the next
    /// render doesn't inherit our drag-time transform.
    base_style: String,
}

thread_local! {
    static KANBAN_DRAG_STATE: RefCell<Option<KanbanDragCandidate>> =
        const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
enum KanbanDragCommit {
    /// Move a card to `to_column_id`. `to_index = None` inserts
    /// at the tail; `Some(i)` inserts before the i-th child.
    Card {
        to_column_id: String,
        card_id: String,
        to_index: Option<usize>,
    },
    /// Move a column within its Kanban strip. `to_index` is the
    /// target slot as consumed by `commands::move_kanban_column`
    /// (pre-delete indexing).
    Column {
        column_id: String,
        to_index: usize,
    },
}

fn kanban_drag_on_pointer_down(ev: &web_sys::PointerEvent) {
    if ev.button() != 0 {
        return;
    }
    let Some(target) = ev.target() else { return };
    let Ok(target_el) = target.dyn_into::<web_sys::Element>() else {
        return;
    };
    // Two draggable kinds: card body (data-kanban-draggable="card")
    // or column header (data-kanban-draggable="column"). Prefer
    // the innermost match — a card sitting inside a column header
    // shouldn't happen, but the closest() call climbs to the
    // nearest ancestor with the attribute so specificity is
    // preserved automatically.
    let Ok(Some(handle_el)) = target_el.closest("[data-kanban-draggable]")
    else {
        return;
    };
    let kind = match handle_el.get_attribute("data-kanban-draggable")
        .as_deref()
    {
        Some("card") => KanbanDragKind::Card,
        Some("column") => KanbanDragKind::Column,
        _ => return,
    };
    // For Column drags, the drag element (what we move + apply
    // position: fixed to) is the whole .kanban-column, not the
    // header. The header is just the handle.
    let drag_element = match kind {
        KanbanDragKind::Card => handle_el.clone(),
        KanbanDragKind::Column => {
            let Ok(Some(col)) = handle_el.closest(".kanban-column") else { return };
            col
        }
    };
    let Some(item_id) = drag_element.get_attribute("data-block-id") else {
        return;
    };
    // Suppress the browser's native text-selection drag.
    ev.prevent_default();
    let base_style = drag_element.get_attribute("style").unwrap_or_default();
    let rect = drag_element.get_bounding_client_rect();
    KANBAN_DRAG_STATE.with(|s| {
        *s.borrow_mut() = Some(KanbanDragCandidate {
            kind,
            item_id,
            initial_x: ev.client_x() as f64,
            initial_y: ev.client_y() as f64,
            initial_rect_left: rect.left(),
            initial_rect_top: rect.top(),
            initial_rect_width: rect.width(),
            initial_rect_height: rect.height(),
            element: send_wrapper::SendWrapper::new(drag_element),
            source_ghost: None,
            drop_indicator: None,
            activated: false,
            base_style,
        });
    });
}

fn kanban_drag_on_pointer_move(ev: &web_sys::PointerEvent) {
    KANBAN_DRAG_STATE.with(|s| {
        let mut opt = s.borrow_mut();
        let Some(state) = opt.as_mut() else { return };
        let dx = ev.client_x() as f64 - state.initial_x;
        let dy = ev.client_y() as f64 - state.initial_y;
        if !state.activated {
            if (dx * dx + dy * dy).sqrt() < DRAG_THRESHOLD_PX {
                return;
            }
            state.activated = true;
            let dragging_class = match state.kind {
                KanbanDragKind::Card => "kanban-card--dragging",
                KanbanDragKind::Column => "kanban-column--dragging",
            };
            let _ = state.element.class_list().add_1(dragging_class);
            set_body_drag_cursor(true);
            // Source ghost: blank div at the source slot marking
            // where the item came from. Never moves during the
            // drag — the drop indicator is a separate element
            // that follows the pointer to valid targets.
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                if let Ok(ph) = doc.create_element("div") {
                    let (ghost_class, style) = match state.kind {
                        KanbanDragKind::Card => (
                            "kanban-card-placeholder",
                            format!("height: {}px;", state.initial_rect_height),
                        ),
                        KanbanDragKind::Column => (
                            "kanban-column-placeholder",
                            format!(
                                "width: {}px; height: {}px;",
                                state.initial_rect_width,
                                state.initial_rect_height,
                            ),
                        ),
                    };
                    let _ = ph.set_attribute("class", ghost_class);
                    let _ = ph.set_attribute("style", &style);
                    if let Some(parent) = state.element.parent_element() {
                        let _ = parent.insert_before(
                            ph.as_ref(),
                            Some(state.element.as_ref()),
                        );
                        state.source_ghost =
                            Some(send_wrapper::SendWrapper::new(ph));
                    }
                }
            }
        }
        // `position: fixed` escapes the source column's overflow
        // clip so the card actually tracks the pointer across
        // columns. `width` is pinned to the original so the card
        // doesn't resize as it leaves its flex-parent's constraints.
        let styled = format!(
            "{}; position: fixed; left: {}px; top: {}px; width: {}px; height: {}px; \
             z-index: 1000; pointer-events: none;",
            state.base_style,
            state.initial_rect_left + dx,
            state.initial_rect_top + dy,
            state.initial_rect_width,
            state.initial_rect_height,
        );
        let _ = state.element.set_attribute("style", &styled);
        // Drop indicator: create-or-move a SEPARATE placeholder
        // that follows the pointer, ONLY when the pointer is
        // over a valid target. If the pointer wanders off any
        // target the indicator is removed, so a release out-of-
        // bounds cleanly cancels.
        update_kanban_drop_indicator(state, ev);
    });
}

/// Ensure a drop indicator exists at the target under the
/// pointer, or remove it when the pointer isn't over any target.
/// The source ghost is left alone.
fn update_kanban_drop_indicator(
    state: &mut KanbanDragCandidate,
    ev: &web_sys::PointerEvent,
) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    // Take the existing indicator so we can move OR drop it.
    let existing = state.drop_indicator.take();
    let target = doc.element_from_point(
        ev.client_x() as f32,
        ev.client_y() as f32,
    );
    // Skip hits on our own indicator or the source ghost —
    // they'd otherwise be moving hit targets that resolve to
    // "no valid drop."
    let target = target.filter(|t| {
        if let Some(ref e) = existing {
            if t.is_same_node(Some(e.as_ref())) { return false; }
        }
        if let Some(ref g) = state.source_ghost {
            if t.is_same_node(Some(g.as_ref())) { return false; }
        }
        true
    });
    let Some(target) = target else {
        // No target — remove any existing indicator so a release
        // here cancels rather than committing at last-hover.
        if let Some(ind) = existing {
            if let Some(parent) = ind.parent_element() {
                let _ = parent.remove_child(ind.as_ref());
            }
        }
        return;
    };
    // Resolve destination: (parent, before_sibling). If the drop
    // isn't over anything valid, remove the indicator entirely.
    let dest = match state.kind {
        KanbanDragKind::Card => resolve_kanban_card_drop(&target),
        KanbanDragKind::Column => resolve_kanban_column_drop(&target, ev),
    };
    let Some((parent, before)) = dest else {
        if let Some(ind) = existing {
            if let Some(pp) = ind.parent_element() {
                let _ = pp.remove_child(ind.as_ref());
            }
        }
        return;
    };
    // Get-or-create the indicator, styled per drag kind.
    let indicator = if let Some(existing) = existing {
        existing
    } else {
        let Ok(el) = doc.create_element("div") else { return };
        let (class, style) = match state.kind {
            KanbanDragKind::Card => (
                "kanban-card-placeholder kanban-drop-indicator",
                format!("height: {}px;", state.initial_rect_height),
            ),
            KanbanDragKind::Column => (
                "kanban-column-placeholder kanban-drop-indicator",
                format!(
                    "width: {}px; height: {}px;",
                    state.initial_rect_width,
                    state.initial_rect_height,
                ),
            ),
        };
        let _ = el.set_attribute("class", class);
        let _ = el.set_attribute("style", &style);
        send_wrapper::SendWrapper::new(el)
    };
    let _ = parent.insert_before(indicator.as_ref(), before.as_deref());
    state.drop_indicator = Some(indicator);
}

/// For a card drag: given the element under the pointer, return
/// the drop parent + before-sibling. `None` = no valid target.
fn resolve_kanban_card_drop(
    target: &web_sys::Element,
) -> Option<(web_sys::Element, Option<web_sys::Element>)> {
    if let Ok(Some(card)) = target.closest(".kanban-card[data-block-id]") {
        let parent = card.parent_element()?;
        return Some((parent, Some(card)));
    }
    if let Ok(Some(list)) = target.closest("[data-kanban-drop-column]") {
        return Some((list.clone(), None)); // append to tail
    }
    None
}

/// For a column drag: same shape as the card resolver but inside
/// the `.kanban-strip`. Uses the pointer's x vs column midpoint
/// to decide before vs after the hovered column.
fn resolve_kanban_column_drop(
    target: &web_sys::Element,
    ev: &web_sys::PointerEvent,
) -> Option<(web_sys::Element, Option<web_sys::Element>)> {
    if let Ok(Some(col)) = target.closest(".kanban-column") {
        let strip = col.parent_element()?;
        if !strip.class_list().contains("kanban-strip") {
            return None;
        }
        let rect = col.get_bounding_client_rect();
        let midpoint = rect.left() + rect.width() / 2.0;
        if (ev.client_x() as f64) < midpoint {
            return Some((strip, Some(col)));
        }
        return Some((strip, col.next_element_sibling()));
    }
    if let Ok(Some(strip)) = target.closest(".kanban-strip") {
        return Some((strip, None));
    }
    None
}

/// pointerup: if a drag was activated, resolve the drop target
/// and hand a `KanbanDragCommit` off to the reactive dispatcher.
fn kanban_drag_on_pointer_up(
    ev: &web_sys::PointerEvent,
    set_commit: WriteSignal<Option<KanbanDragCommit>>,
) {
    let Some(state) = KANBAN_DRAG_STATE.with(|s| s.borrow_mut().take()) else {
        return;
    };
    // Always restore visual state, activated or not.
    let dragging_class = match state.kind {
        KanbanDragKind::Card => "kanban-card--dragging",
        KanbanDragKind::Column => "kanban-column--dragging",
    };
    let _ = state.element.class_list().remove_1(dragging_class);
    let _ = state.element.set_attribute("style", &state.base_style);
    set_body_drag_cursor(false);
    // Remove both source ghost and drop indicator regardless of
    // whether the drag activated. Below-threshold releases return
    // without dispatching; activated releases with no indicator
    // (dragged out of bounds) also cancel here.
    let cleanup = |el: &Option<send_wrapper::SendWrapper<web_sys::Element>>| {
        if let Some(e) = el.as_ref() {
            if let Some(parent) = e.parent_element() {
                let _ = parent.remove_child(e.as_ref());
            }
        }
    };
    if !state.activated {
        cleanup(&state.source_ghost);
        cleanup(&state.drop_indicator);
        return;
    }
    // The click event that would follow this pointerup would open
    // the edit modal on top of the just-completed drop. Prevent it.
    ev.stop_propagation();
    ev.prevent_default();

    // Compute the commit BEFORE removing the drop indicator —
    // its live DOM position is the user's visible drop intent.
    // If the pointer was released off any valid target, the
    // indicator was cleared during the last pointermove and
    // the commit falls out as None → drag cancels cleanly.
    let commit = kanban_drag_compute_commit(&state);
    cleanup(&state.source_ghost);
    cleanup(&state.drop_indicator);
    let Some(commit) = commit else { return };
    set_commit.set(Some(commit));
}

/// pointercancel: same cleanup as `kanban_drag_on_pointer_up` but
/// without the compute-commit / dispatch. Restores the source
/// card's style so an interrupted drag doesn't leave it stuck at
/// `position: fixed`.
fn kanban_drag_on_pointer_cancel() {
    let Some(state) = KANBAN_DRAG_STATE.with(|s| s.borrow_mut().take()) else {
        return;
    };
    let dragging_class = match state.kind {
        KanbanDragKind::Card => "kanban-card--dragging",
        KanbanDragKind::Column => "kanban-column--dragging",
    };
    let _ = state.element.class_list().remove_1(dragging_class);
    let _ = state.element.set_attribute("style", &state.base_style);
    set_body_drag_cursor(false);
    let cleanup = |el: &Option<send_wrapper::SendWrapper<web_sys::Element>>| {
        if let Some(e) = el.as_ref() {
            if let Some(parent) = e.parent_element() {
                let _ = parent.remove_child(e.as_ref());
            }
        }
    };
    cleanup(&state.source_ghost);
    cleanup(&state.drop_indicator);
}

/// Resolve the drop target from the drop indicator's DOM
/// position. If the indicator was removed on the last
/// pointermove (pointer went out of bounds), the drag cancels.
fn kanban_drag_compute_commit(
    state: &KanbanDragCandidate,
) -> Option<KanbanDragCommit> {
    match state.kind {
        KanbanDragKind::Card => kanban_drag_compute_card_commit(state),
        KanbanDragKind::Column => kanban_drag_compute_column_commit(state),
    }
}

/// Column drop: figure out the target index inside the Kanban
/// strip from the drop-indicator's DOM sibling order.
fn kanban_drag_compute_column_commit(
    state: &KanbanDragCandidate,
) -> Option<KanbanDragCommit> {
    let ind = state.drop_indicator.as_ref()?;
    let strip = ind.parent_element()?;
    if !strip.class_list().contains("kanban-strip") {
        return None;
    }
    // Walk strip children, count columns before the indicator.
    // That count is the target index in PRE-delete numbering —
    // `move_kanban_column` handles the pre→post shift internally.
    let mut idx = 0usize;
    let mut cur = strip.first_element_child();
    while let Some(el) = cur {
        if el.is_same_node(Some(ind.as_ref())) {
            break;
        }
        if el.class_list().contains("kanban-column") {
            idx += 1;
        }
        cur = el.next_element_sibling();
    }
    Some(KanbanDragCommit::Column {
        column_id: state.item_id.clone(),
        to_index: idx,
    })
}

fn kanban_drag_compute_card_commit(
    state: &KanbanDragCandidate,
) -> Option<KanbanDragCommit> {
    let ind = state.drop_indicator.as_ref()?;
    let list_el = ind.closest("[data-kanban-drop-column]").ok().flatten()?;
    let col_el = list_el.closest(".kanban-column").ok().flatten()?;
    let to_column_id = col_el.get_attribute("data-block-id")?;

    // Walk the list_el children up to the indicator, counting
    // real cards. If the indicator has a card AFTER it, we're
    // inserting before that card at `Some(idx)`. Otherwise it's
    // a tail drop (None).
    let mut idx = 0usize;
    let mut cur = list_el.first_element_child();
    let mut found_ind = false;
    while let Some(el) = cur {
        if el.is_same_node(Some(ind.as_ref())) {
            found_ind = true;
            break;
        }
        if el.matches(".kanban-card[data-block-id]").unwrap_or(false) {
            idx += 1;
        }
        cur = el.next_element_sibling();
    }
    if !found_ind {
        return None;
    }
    let mut sibling = ind.next_element_sibling();
    let mut has_next_card = false;
    while let Some(el) = sibling {
        if el.matches(".kanban-card[data-block-id]").unwrap_or(false) {
            has_next_card = true;
            break;
        }
        sibling = el.next_element_sibling();
    }
    Some(KanbanDragCommit::Card {
        to_column_id,
        card_id: state.item_id.clone(),
        to_index: if has_next_card { Some(idx) } else { None },
    })
}

/// Parse a click on the editor container. Returns the outcome to
/// route through the modal state / command dispatcher, or `None`
/// for clicks that don't hit a calendar interactive element (so
/// the click falls through to normal editor handling).
fn calendar_click_outcome(
    ev: &web_sys::MouseEvent,
) -> Option<CalendarClickOutcome> {
    let target = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let action_el = target.closest("[data-calendar-action]").ok()??;
    let action = action_el.get_attribute("data-calendar-action")?;
    let block_el = action_el.closest(".calendar-block").ok()??;
    let block_id = block_el.get_attribute("data-block-id")?;
    let tz = block_el.get_attribute("data-timezone").unwrap_or_default();

    match action.as_str() {
        "add-event" => {
            let date = action_el.get_attribute("data-calendar-date")?;
            let mut state = CalendarModalState::new_add(block_id, date.clone(), tz);
            // If the click came from an hour cell in week/day view
            // the cell carries a `data-calendar-time` (e.g. "09:00")
            // — flip the modal to a timed shape pre-filled at that
            // hour, ending one hour later.
            if let Some(time) = action_el.get_attribute("data-calendar-time") {
                state.all_day = false;
                state.start_date = date.clone();
                state.end_date = date;
                state.start_time = time.clone();
                state.end_time = one_hour_after(&time).unwrap_or(time);
            }
            Some(CalendarClickOutcome::OpenModal(state))
        }
        "edit-event" => {
            let event_id = action_el.get_attribute("data-event-id")?;
            let content = action_el.text_content().unwrap_or_default();
            let content = if content.trim() == "(no title)" {
                String::new()
            } else {
                content
            };
            let color = extract_event_color_class(&action_el).unwrap_or_else(|| "blue".into());
            let all_day = action_el
                .get_attribute("data-all-day")
                .map(|v| v == "true")
                .unwrap_or(true);
            let (start_date, end_date, start_time, end_time) = if all_day {
                let sd = action_el.get_attribute("data-start-date").unwrap_or_default();
                let ed = action_el.get_attribute("data-end-date").unwrap_or_else(|| sd.clone());
                (sd, ed, "09:00".into(), "10:00".into())
            } else {
                // Read the tz-adjusted date + time back out of the
                // stored UTC ISO so the modal fields match what the
                // user sees in the grid.
                let sa = action_el.get_attribute("data-start-at").unwrap_or_default();
                let ea = action_el.get_attribute("data-end-at").unwrap_or_else(|| sa.clone());
                let (sd, st) = local_parts_of_iso(&sa, &tz);
                let (ed, et) = local_parts_of_iso(&ea, &tz);
                (sd, ed, st, et)
            };
            Some(CalendarClickOutcome::OpenModal(CalendarModalState {
                mode: CalendarModalMode::Edit { block_id, event_id },
                content,
                color,
                all_day,
                start_date,
                end_date,
                start_time,
                end_time,
                timezone: tz,
            }))
        }
        "set-view" => {
            // Which view button was clicked. Adjust `cursor` too:
            // month view uses YYYY-MM, week/day use YYYY-MM-DD.
            let new_view = action_el.get_attribute("data-calendar-view")?;
            let mut updates = HashMap::new();
            updates.insert("view".to_string(), new_view.clone());
            let existing_cursor = block_el.get_attribute("data-cursor").unwrap_or_default();
            let adjusted = adjust_cursor_for_view(&existing_cursor, &new_view);
            updates.insert("cursor".to_string(), adjusted);
            Some(CalendarClickOutcome::UpdateAttrs { block_id, updates })
        }
        "prev" | "next" => {
            let existing_cursor = block_el.get_attribute("data-cursor").unwrap_or_default();
            let view = block_el
                .get_attribute("data-view")
                .unwrap_or_else(|| "month".into());
            let shifted = shift_cursor(&existing_cursor, &view, action.as_str() == "next")?;
            let mut updates = HashMap::new();
            updates.insert("cursor".to_string(), shifted);
            Some(CalendarClickOutcome::UpdateAttrs { block_id, updates })
        }
        "today" => {
            let view = block_el
                .get_attribute("data-view")
                .unwrap_or_else(|| "month".into());
            let cursor = today_cursor(&view);
            let mut updates = HashMap::new();
            updates.insert("cursor".to_string(), cursor);
            Some(CalendarClickOutcome::UpdateAttrs { block_id, updates })
        }
        _ => None,
    }
}

/// Parse a click on the editor container for a Mermaid
/// click-to-edit hit. Returns the modal state to open, or `None`
/// for clicks that don't hit `[data-mermaid-action="edit"]` (so the
/// click falls through to normal editor handling).
///
/// The block's current `source` is read off the `data-source`
/// attribute `MermaidView::render` stamps on the `.mermaid-block`
/// wrapper (see `editor/blocks/mermaid.rs`) — there's no separate
/// model lookup here, mirroring how `calendar_click_outcome` reads
/// event fields straight off the DOM.
fn mermaid_click_outcome(ev: &web_sys::MouseEvent) -> Option<MermaidModalState> {
    let target = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let action_el = target.closest("[data-mermaid-action]").ok()??;
    let block_el = action_el.closest(".mermaid-block").ok()??;
    let block_id = block_el.get_attribute("data-block-id")?;
    let source = block_el.get_attribute("data-source").unwrap_or_default();
    Some(MermaidModalState { block_id, source })
}

/// If the user is switching from month → day/week (or vice
/// versa), the cursor's shape needs to match. Month expects
/// `YYYY-MM`; day/week expect `YYYY-MM-DD`. Best-effort — falls
/// back to today when the shape can't be converted.
fn adjust_cursor_for_view(cursor: &str, new_view: &str) -> String {
    // Cursor comes verbatim from a DOM `data-cursor` attribute so it
    // could hold any string a peer wrote to the CRDT. Byte-slicing
    // was panicking on multi-byte characters at fixed offsets;
    // route through the same `parse_ym`/`parse_ymd` helpers used
    // by `shift_cursor` so bad input falls to `today_cursor` cleanly.
    match new_view {
        "month" => {
            if let Some((y, m)) = parse_ymd(cursor).map(|(y, m, _)| (y, m)) {
                format!("{y:04}-{m:02}")
            } else if let Some((y, m)) = parse_ym(cursor) {
                format!("{y:04}-{m:02}")
            } else {
                today_cursor(new_view)
            }
        }
        _ => {
            if let Some((y, m, d)) = parse_ymd(cursor) {
                format!("{y:04}-{m:02}-{d:02}")
            } else if let Some((y, m)) = parse_ym(cursor) {
                format!("{y:04}-{m:02}-01")
            } else {
                today_cursor(new_view)
            }
        }
    }
}

/// Compute the previous / next cursor for the given view. Month
/// steps by month; week steps by 7 days; day steps by 1 day.
/// Returns None on malformed input so the click is a no-op.
fn shift_cursor(cursor: &str, view: &str, forward: bool) -> Option<String> {
    match view {
        "month" => {
            let (y, m) = parse_ym(cursor)?;
            let (y2, m2) = if forward {
                if m == 12 { (y + 1, 1) } else { (y, m + 1) }
            } else if m == 1 {
                (y.checked_sub(1)?, 12)
            } else {
                (y, m - 1)
            };
            Some(format!("{y2:04}-{m2:02}"))
        }
        _ => {
            // Week: 7 days. Day: 1 day.
            let step = if view == "week" { 7 } else { 1 };
            let (y, m, d) = parse_ymd(cursor)?;
            let mut result = (y, m, d);
            for _ in 0..step {
                result = step_day(result, forward)?;
            }
            let (y2, m2, d2) = result;
            Some(format!("{y2:04}-{m2:02}-{d2:02}"))
        }
    }
}

fn today_cursor(view: &str) -> String {
    let now = js_sys::Date::new_0();
    let y = now.get_full_year();
    let m = now.get_month() + 1;
    let d = now.get_date();
    match view {
        "month" => format!("{y:04}-{m:02}"),
        _ => format!("{y:04}-{m:02}-{d:02}"),
    }
}

fn parse_ym(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    (1..=12).contains(&m).then_some((y, m))
}

fn parse_ymd(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    // Reject impossible calendar dates (Feb 31 / Apr 31 / Feb 29
    // in a non-leap year). The day range depends on the month.
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }
        }
        _ => 30,
    }
}

fn step_day((y, m, d): (u32, u32, u32), forward: bool) -> Option<(u32, u32, u32)> {
    if forward {
        if d < days_in_month(y, m) {
            Some((y, m, d + 1))
        } else if m < 12 {
            Some((y, m + 1, 1))
        } else {
            Some((y + 1, 1, 1))
        }
    } else if d > 1 {
        Some((y, m, d - 1))
    } else if m > 1 {
        let pm = m - 1;
        Some((y, pm, days_in_month(y, pm)))
    } else {
        // Underflowing past year 0 used to loop forever
        // (`saturating_sub(1)` capped at 0 and we came right back
        // here). Fail with None so `shift_cursor` treats a Prev
        // click at (0000-01-01) as a no-op.
        let prev_year = y.checked_sub(1)?;
        Some((prev_year, 12, 31))
    }
}

/// Pull the `calendar-event--<color>` modifier out of a
/// `.calendar-event` element's class list.
fn extract_event_color_class(el: &web_sys::Element) -> Option<String> {
    let class = el.get_attribute("class")?;
    for token in class.split_whitespace() {
        if let Some(color) = token.strip_prefix("calendar-event--") {
            return Some(color.to_string());
        }
    }
    None
}

/// Convert a `CalendarModalState` into the attribute bag the
/// commands (and the backend validator) expect on a
/// `CalendarEvent` node. All-day and timed events use disjoint
/// attribute sets — see `crates/collab/src/blocks/calendar.rs`
/// for the schema.
fn modal_state_to_attrs(s: &CalendarModalState) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    attrs.insert("color".to_string(), s.color.clone());
    if s.content.is_empty() {
        attrs.insert("content".to_string(), String::new());
    } else {
        attrs.insert("content".to_string(), s.content.clone());
    }
    if s.all_day {
        attrs.insert("allDay".to_string(), "true".to_string());
        attrs.insert("startDate".to_string(), s.start_date.clone());
        attrs.insert("endDate".to_string(), s.end_date.clone());
    } else {
        attrs.insert("allDay".to_string(), "false".to_string());
        // The time inputs return `HH:MM` interpreted as being in
        // the block's declared timezone. Convert (date + time) in
        // that tz to a UTC instant via the tz-aware helper in
        // `blocks::calendar`. Cross-tz co-editing then sees the
        // same real-world instant, rendered in each viewer's
        // chosen tz.
        if let Some(iso) = wall_to_utc_iso_in_tz(&s.start_date, &s.start_time, &s.timezone) {
            attrs.insert("startAt".to_string(), iso);
        }
        if let Some(iso) = wall_to_utc_iso_in_tz(&s.end_date, &s.end_time, &s.timezone) {
            attrs.insert("endAt".to_string(), iso);
        }
    }
    attrs
}

/// Given a wall-clock date + time in `tz`, return the RFC 3339
/// UTC ISO string of that instant. Empty `tz` falls back to
/// browser-local. Delegates to the tz math in
/// `editor::blocks::calendar::local_wall_to_utc_iso`.
fn wall_to_utc_iso_in_tz(date: &str, time: &str, tz: &str) -> Option<String> {
    let (y, m, d) = parse_ymd(date)?;
    let mut tparts = time.splitn(2, ':');
    let h: u32 = tparts.next()?.parse().ok()?;
    let mi: u32 = tparts.next()?.parse().ok()?;
    if h >= 24 || mi >= 60 {
        return None;
    }
    crate::editor::blocks::calendar::local_wall_to_utc_iso(y, m, d, h, mi, tz)
}

/// Given a UTC ISO string, return the `(YYYY-MM-DD, HH:MM)` of
/// that instant AS SEEN in `tz`. Used to pre-fill the Edit modal
/// with the time the user sees on the grid.
fn local_parts_of_iso(iso: &str, tz: &str) -> (String, String) {
    if iso.is_empty() {
        return (String::new(), String::new());
    }
    match crate::editor::blocks::calendar::parts_in_tz(iso, tz) {
        Some((y, m, d, h, mi)) => {
            (format!("{y:04}-{m:02}-{d:02}"), format!("{h:02}:{mi:02}"))
        }
        None => (String::new(), String::new()),
    }
}

/// Compose a browser-local `YYYY-MM-DD` + `HH:MM` into an RFC 3339
/// UTC timestamp via `js_sys::Date`. Kept for backward-compat
/// with call sites that don't have a tz on hand; new call sites
/// should route through `wall_to_utc_iso_in_tz`.
#[allow(dead_code)]
fn local_datetime_to_utc_iso(date: &str, time: &str) -> Option<String> {
    let (y, m, d) = parse_ymd(date)?;
    let mut tparts = time.splitn(2, ':');
    let h: u32 = tparts.next()?.parse().ok()?;
    let mi: u32 = tparts.next()?.parse().ok()?;
    if h >= 24 || mi >= 60 {
        return None;
    }
    let d_js = js_sys::Date::new_with_year_month_day_hr_min_sec(
        y,
        (m - 1) as i32,
        d as i32,
        h as i32,
        mi as i32,
        0,
    );
    Some(String::from(d_js.to_iso_string()))
}

/// Split an RFC 3339 timestamp of shape `YYYY-MM-DDTHH:MM:SSZ`
/// (optionally with fractional seconds) into `(date, HH:MM)`.
/// Returns empty strings if the shape doesn't match.
///
/// A well-formed RFC 3339 timestamp is ASCII by construction; we
/// reject non-ASCII input up front to avoid a byte-slicing panic
/// on a poisoned DOM `data-start-at` / `data-end-at` value.
/// Add one hour to an `HH:MM` string, wrapping at 24:00 back to
/// 00:00 (rare — the picker's suggested "one hour later" from
/// 23:xx). Returns `None` on malformed input.
fn one_hour_after(hm: &str) -> Option<String> {
    let mut parts = hm.splitn(2, ':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    let next_h = (h + 1) % 24;
    Some(format!("{next_h:02}:{m:02}"))
}

/// UTC-substring shortcut kept for callers that don't have a tz.
/// New code should use `local_parts_of_iso` which respects the
/// block's tz.
#[allow(dead_code)]
fn split_rfc3339(s: &str) -> (String, String) {
    if !s.is_ascii() || s.len() < 16 {
        return (String::new(), String::new());
    }
    let bytes = s.as_bytes();
    if bytes[10] != b'T' {
        return (String::new(), String::new());
    }
    (s[..10].to_string(), s[11..16].to_string())
}

fn insert_pos_after_cursor_block(state: &EditorState) -> usize {
    let cursor = state.selection.from();
    let mut offset = 0;
    if let Node::Element { content, .. } = &state.doc {
        for child in &content.children {
            let size = child.node_size();
            if cursor >= offset && cursor < offset + size {
                return offset + size;
            }
            offset += size;
        }
    }
    state.doc.content_size()
}

/// Handle the UploadImage command: open a file picker, upload to S3, insert image node.
fn handle_image_upload(
    doc_id: &str,
    view_ref: &Rc<RefCell<Option<EditorView>>>,
    history: &Rc<RefCell<HistoryPlugin>>,
    on_change: &Callback<Vec<u8>>,
    on_state_change: &Callback<EditorState>,
) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(input) = document.create_element("input") else { return };

    let _ = input.set_attribute("type", "file");
    let _ = input.set_attribute("accept", "image/*");
    let _ = input.set_attribute("style", "display:none");
    let _ = document.body().unwrap().append_child(&input);

    let input_el = input.clone();
    let doc_id = doc_id.to_string();
    let view_ref = Rc::clone(view_ref);
    let history = Rc::clone(history);
    let on_change = on_change.clone();
    let on_state_change = on_state_change.clone();

    let on_change_closure = Closure::wrap(Box::new(move |_: web_sys::Event| {
        let input_el = input_el.clone();
        let doc_id = doc_id.clone();
        let view_ref = Rc::clone(&view_ref);
        let history = Rc::clone(&history);
        let on_change = on_change.clone();
        let on_state_change = on_state_change.clone();

        leptos::task::spawn_local(async move {
            let html_input: web_sys::HtmlInputElement = input_el.clone().dyn_into().unwrap();
            let Some(files) = html_input.files() else { return };
            let Some(file) = files.get(0) else { return };

            let filename = file.name();
            let content_type = file.type_();
            let content_type = if content_type.is_empty() {
                "application/octet-stream".to_string()
            } else {
                content_type
            };

            // Read file bytes
            let Ok(array_buffer) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else {
                return;
            };
            let bytes = js_sys::Uint8Array::new(&array_buffer).to_vec();

            // Upload: get presigned URL → PUT to S3 → get download URL
            let upload = match blobs::request_upload_url(&doc_id, &filename, &content_type).await {
                Ok(u) => u,
                Err(e) => {
                    web_sys::console::error_1(&format!("Upload URL failed: {e}").into());
                    return;
                }
            };

            if let Err(e) = blobs::upload_to_s3(&upload.upload_url, &bytes, &content_type).await {
                web_sys::console::error_1(&format!("S3 upload failed: {e}").into());
                return;
            }

            let download_url = match blobs::request_download_url(&doc_id, &upload.blob_id, &upload.key).await {
                Ok(u) => u,
                Err(e) => {
                    web_sys::console::error_1(&format!("Download URL failed: {e}").into());
                    return;
                }
            };

            // Insert image node after the current block
            let v = view_ref.borrow();
            let Some(v) = v.as_ref() else { return };
            let state = v.state();

            let mut attrs = HashMap::new();
            attrs.insert("src".to_string(), download_url);
            attrs.insert("alt".to_string(), filename);
            let img = Node::element_with_attrs(NodeType::Image, attrs, Fragment::empty());

            let insert_pos = insert_pos_after_cursor_block(&state);
            let slice = Slice::new(Fragment::from(vec![img]), 0, 0);
            let mut txn_result = state.transaction().replace(insert_pos, insert_pos, slice);
            if let Ok(ref mut txn) = txn_result {
                txn.selection = Selection::cursor(insert_pos + 1);
            }
            if let Ok(txn) = txn_result {
                apply_and_notify(v, txn, Some(&history), &on_change, &on_state_change, None);
            }

            input_el.remove();
        });
    }) as Box<dyn Fn(web_sys::Event)>);

    input
        .add_event_listener_with_callback("change", on_change_closure.as_ref().unchecked_ref())
        .unwrap_or(());
    on_change_closure.forget();

    if let Ok(html_input) = input.dyn_into::<web_sys::HtmlElement>() {
        html_input.click();
    }
}

/// The main editor component. Wraps EditorView in a Leptos component.
#[component]
pub fn EditorComponent(props: EditorProps) -> impl IntoView {
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let view_ref: Rc<RefCell<Option<EditorView>>> = Rc::new(RefCell::new(None));
    let history_ref: Rc<RefCell<HistoryPlugin>> = Rc::new(RefCell::new(HistoryPlugin::new()));

    // Mentions spec §5 (Task 3) — a lone same-origin doc-URL paste hands
    // its `PendingMentionPaste` out of `EditorView::on_paste` (plain
    // closure, not reactive) through this signal, mirroring the
    // `pending_ctx_cmd` indirection below: the async resolve + guarded
    // replace live in an Effect that can safely borrow `view_ref`.
    let (pending_mention_paste, set_pending_mention_paste) =
        signal::<Option<crate::editor::mention_url::PendingMentionPaste>>(None);

    // Task 3 review fix: a fresh, monotonically increasing id per pasted
    // mention URL. Deliberately NOT `Date::now()`/`Math.random()` — a
    // plain counter is enough to distinguish "this exact paste's undo
    // entry" from anything else, and is trivially reproducible in native
    // tests. Tags `HistoryPlugin`'s top entry right after the paste
    // dispatches (below), then rides along on the async-resolved replace
    // transaction's `history-merge-tag` meta so `HistoryPlugin::record`
    // can refuse to merge into the wrong entry if something else landed
    // on top while the resolve was in flight.
    let mention_paste_tag_counter: Rc<Cell<u64>> = Rc::new(Cell::new(0));

    // #136 — Calendar modal state. `None` = hidden. A delegated
    // click listener on `.editor-content` populates this on
    // clicks inside `.calendar-block`; the modal callback
    // dispatches Add/Edit/Delete commands.
    let calendar_modal_state: RwSignal<Option<CalendarModalState>> = RwSignal::new(None);

    // Task 8 — Mermaid edit modal state. `None` = hidden. A
    // delegated click listener on `.editor-content` populates this
    // on clicks inside `.mermaid-block`; the modal callback
    // dispatches `commands::update_mermaid_source`.
    let mermaid_modal_state: RwSignal<Option<MermaidModalState>> = RwSignal::new(None);

    // Task 7 — language-selector chip overlay. `None` = hidden.
    // Rendered OUTSIDE `.editor-content` (a sibling inside the
    // positioned `.editor-container` wrapper — see `editor_wrapper_ref`
    // below), so it can never disturb the DOM<->model position
    // walkers and is never wiped by `render()`'s `set_inner_html("")`.
    //
    // Visibility + `current` are caret-based: recomputed after every
    // dispatch, not just ones the user makes with the mouse/keyboard
    // inside the editor. `apply_and_notify` (above) is the single
    // function every dispatch path in this component funnels its
    // resulting `EditorState` through — the toolbar-command effect,
    // every modal outcome, drag-drop, remote CRDT updates, and
    // EditorView's own keydown/click/selectionchange dispatch — so
    // wrapping `props.on_state_change` once here, and having every
    // call site below clone the wrapper instead of `props.on_state_change`
    // directly, is the one hook point that covers all of them.
    let code_lang_chip_state: RwSignal<Option<CodeLangChipState>> = RwSignal::new(None);
    let editor_wrapper_ref = NodeRef::<leptos::html::Div>::new();
    let (chip_state_tick, set_chip_state_tick) = signal::<Option<EditorState>>(None);
    let on_state_change_shared: Callback<EditorState> = {
        let outer = props.on_state_change.clone();
        Callback::new(move |state: EditorState| {
            outer.run(state.clone());
            set_chip_state_tick.set(Some(state));
        })
    };
    Effect::new(move |_| {
        let Some(state) = chip_state_tick.get() else { return };
        let Some(wrapper) = editor_wrapper_ref.get() else { return };
        let wrapper_html: web_sys::HtmlElement = wrapper.into();
        refresh_code_lang_chip(&state, &wrapper_html, code_lang_chip_state);
    });

    // Initialize the editor after the DOM element is mounted
    let view_ref_init = Rc::clone(&view_ref);
    let history_ref_init = Rc::clone(&history_ref);
    let props_clone = props.clone();
    let on_state_change_init = on_state_change_shared.clone();

    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        if view_ref_init.borrow().is_some() { return } // already initialized

        let html_element: web_sys::HtmlElement = container.into();

        let doc = if let Some(ref bytes) = props_clone.initial_content {
            yrs_bridge::ydoc_bytes_to_doc(bytes).unwrap_or_else(|_| Node::empty_doc())
        } else {
            Node::empty_doc()
        };

        let state = EditorState::create_default(doc);
        on_state_change_init.run(state.clone());

        // Use Weak to break the Rc cycle: dispatch -> view_ref -> EditorView -> dispatch
        let view_ref_weak: Weak<RefCell<Option<EditorView>>> = Rc::downgrade(&view_ref_init);
        let on_change = props_clone.on_change.clone();
        let on_state_change = on_state_change_init.clone();
        let on_mapping_dispatch = props_clone.on_mapping.clone();

        let dispatch = move |txn: Transaction| {
            let Some(view_rc) = view_ref_weak.upgrade() else { return };
            let view = view_rc.borrow();
            let Some(view) = view.as_ref() else { return };
            // History recording is handled by the view's dispatch wrapper.
            apply_and_notify(view, txn, None, &on_change, &on_state_change, on_mapping_dispatch.as_ref());
        };

        let on_mention_paste =
            move |p: crate::editor::mention_url::PendingMentionPaste| {
                set_pending_mention_paste.set(Some(p));
            };

        let editor_view = EditorView::new_with_options(
            html_element,
            state,
            dispatch,
            Rc::clone(&history_ref_init),
            props_clone.readonly,
            on_mention_paste,
        );
        *view_ref_init.borrow_mut() = Some(editor_view);
    });

    // Mentions spec §5 (Task 3) — drains `pending_mention_paste` and runs
    // the async resolve → guarded replace. Shape mirrors
    // `handle_image_upload`: `spawn_local(async move { ...await...
    // then re-borrow `view_ref` for the CURRENT state and dispatch via
    // `apply_and_notify` })`, the established pattern in this file for
    // "async command result re-enters the reactive world." `Some(&history)`
    // (not `None`, unlike the plain-dispatch closure above) because this
    // path bypasses the view's own dispatch wrapper — `apply_and_notify`
    // must record history itself, same as the toolbar-command Effect.
    let view_ref_mention = Rc::clone(&view_ref);
    let history_ref_mention = Rc::clone(&history_ref);
    let on_change_mention = props.on_change.clone();
    let on_state_change_mention = on_state_change_shared.clone();
    let on_mapping_mention = props.on_mapping.clone();
    let mention_paste_tag_counter_effect = Rc::clone(&mention_paste_tag_counter);

    Effect::new(move |_| {
        let Some(p) = pending_mention_paste.get() else { return };
        // Drain immediately so a later read doesn't re-fire the same paste.
        set_pending_mention_paste.set(None);

        let view_ref = Rc::clone(&view_ref_mention);
        let history = Rc::clone(&history_ref_mention);
        let on_change = on_change_mention.clone();
        let on_state_change = on_state_change_mention.clone();
        let on_mapping = on_mapping_mention.clone();

        // Task 3 review fix: tag the paste's undo entry NOW, synchronously,
        // right after the URL-insert paste has already been dispatched and
        // recorded (by `EditorView`'s own wrapped `dispatch`, before
        // `mention_paste_hook` — and thus this Effect — ever fires). This
        // is still the same entry the paste created: nothing else can have
        // been recorded between that dispatch and this Effect running.
        // From here on, the id travels with `p`/the async task, and
        // `HistoryPlugin::record` will refuse to merge later if this tag
        // is no longer on top by the time the resolve returns.
        let tag = mention_paste_tag_counter_effect.get() + 1;
        mention_paste_tag_counter_effect.set(tag);
        history.borrow_mut().tag_last_entry(tag);

        leptos::task::spawn_local(async move {
            let targets = vec![(p.parsed.doc_id.clone(), p.parsed.block_id.clone())];
            let Ok(results) = crate::api::documents::resolve_mentions(&targets).await else {
                return; // network error: the pasted URL stays a plain URL
            };
            let Some(r) = results.first() else { return };
            if r.status != "ok" {
                return; // notFound (incl. no-access, by design): URL stays
            }

            // async ladder: block resolved → anchor attrs (snippet +
            // target_block_id); fragment present but block missing →
            // document mention that KEEPS target_block_id (dangling, so
            // the chip can show its own notice); no fragment → plain
            // document mention.
            let block_found = r.block_found.unwrap_or(false);
            let mut attrs = HashMap::new();
            attrs.insert("url".to_string(), p.url.clone());
            attrs.insert("doc_id".to_string(), p.parsed.doc_id.clone());
            attrs.insert(
                "target_block_id".to_string(),
                p.parsed.block_id.clone().unwrap_or_default(),
            );
            attrs.insert("title".to_string(), r.title.clone().unwrap_or_default());
            attrs.insert(
                "snippet".to_string(),
                if block_found { r.snippet.clone().unwrap_or_default() } else { String::new() },
            );

            let view = view_ref.borrow();
            let Some(view) = view.as_ref() else { return };
            // CURRENT state (post-await), not the pre-await snapshot — the
            // concurrent-edit guard inside `replace_text_with_doc_mention`
            // is only meaningful against what the document looks like now.
            let current = view.state();
            if let Some(txn) =
                commands::replace_text_with_doc_mention(&current, p.from, p.to, &p.url, attrs, tag)
            {
                apply_and_notify(view, txn, Some(&history), &on_change, &on_state_change, on_mapping.as_ref());
            }
            // None: the range no longer holds the raw URL (user kept
            // typing, undid the paste, a remote edit landed there) — leave
            // it as-is, per spec §5 case c.
        });
    });

    // #136 — delegated click listener for `.calendar-block`
    // interactions. Modal-open outcomes populate
    // `calendar_modal_state`; attr-update outcomes go into
    // `calendar_attr_update_signal` where the reactive dispatcher
    // below picks them up.
    //
    // The listener is attached once when the container mounts and
    // removed via `on_cleanup` when the component tears down —
    // without the cleanup, every SPA navigation to a new doc
    // leaked one Closure + captured signals per hop. Because
    // `web_sys::Closure` + `HtmlElement` aren't `Send + Sync`,
    // they ride in a `SendWrapper` (same pattern the modal
    // callback uses); wasm-bindgen is single-threaded so the Send
    // bound is a formality the runtime never exercises.
    let modal_state_click = calendar_modal_state;
    let (calendar_attr_update_signal, set_calendar_attr_update) =
        signal::<Option<(String, HashMap<String, String>)>>(None);
    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        let already: web_sys::HtmlElement = container.clone().into();
        if already.get_attribute("data-calendar-observer").is_some() {
            return;
        }
        let el: web_sys::HtmlElement = container.into();
        let _ = el.set_attribute("data-calendar-observer", "attached");
        let listener = Closure::wrap(Box::new(move |ev: web_sys::MouseEvent| {
            let Some(outcome) = calendar_click_outcome(&ev) else {
                return;
            };
            // Prevent the click from bubbling to the editor's
            // selection handler — the calendar owns this click.
            ev.stop_propagation();
            ev.prevent_default();
            match outcome {
                CalendarClickOutcome::OpenModal(s) => {
                    modal_state_click.set(Some(s));
                }
                CalendarClickOutcome::UpdateAttrs { block_id, updates } => {
                    set_calendar_attr_update.set(Some((block_id, updates)));
                }
            }
        }) as Box<dyn Fn(web_sys::MouseEvent)>);
        let _ = el.add_event_listener_with_callback(
            "click",
            listener.as_ref().unchecked_ref(),
        );
        let cleanup_el = send_wrapper::SendWrapper::new(el);
        let cleanup_listener = send_wrapper::SendWrapper::new(listener);
        on_cleanup(move || {
            let el = cleanup_el.take();
            let listener = cleanup_listener.take();
            let _ = el.remove_event_listener_with_callback(
                "click",
                listener.as_ref().unchecked_ref(),
            );
            drop(listener);
        });
    });

    // #136 — Dispatcher for Calendar-attr updates emitted by the
    // click observer above. Runs on the reactive signal (rather
    // than inline in the listener) so the dispatch closure can
    // borrow `view_ref` without wrapping in a SendWrapper.
    let view_ref_attr = Rc::clone(&view_ref);
    let history_ref_attr = Rc::clone(&history_ref);
    let on_change_attr = props.on_change.clone();
    let on_state_change_attr = on_state_change_shared.clone();
    let on_mapping_attr = props.on_mapping.clone();
    Effect::new(move |_| {
        let Some((block_id, updates)) = calendar_attr_update_signal.get() else {
            return;
        };
        let view = view_ref_attr.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&history_ref_attr);
        let on_change_dispatch = on_change_attr.clone();
        let on_state_change_dispatch = on_state_change_attr.clone();
        let on_mapping_dispatch = on_mapping_attr.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        commands::update_calendar_attrs(&block_id, updates, &state, Some(&dispatch_fn));
    });

    // #137 — Kanban click routing. The observer sits alongside the
    // Calendar observer above, so a single click passes through
    // both `data-kanban-action` and `data-calendar-action` guards
    // and only fires the branch that matches. Modal-open outcomes
    // land in `kanban_card_modal_state`; column ops
    // (add/rename/remove) go through `kanban_column_action_signal`
    // where the reactive dispatcher below picks them up. Same
    // rationale as the calendar attr signal: the dispatch closure
    // borrows `view_ref` without a `SendWrapper`.
    let kanban_card_modal_state: RwSignal<Option<KanbanCardModalState>> = RwSignal::new(None);
    let (kanban_column_action_signal, set_kanban_column_action) =
        signal::<Option<KanbanColumnAction>>(None);
    let kanban_modal_click = kanban_card_modal_state;
    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        let already: web_sys::HtmlElement = container.clone().into();
        if already.get_attribute("data-kanban-observer").is_some() {
            return;
        }
        let el: web_sys::HtmlElement = container.into();
        let _ = el.set_attribute("data-kanban-observer", "attached");
        let listener = Closure::wrap(Box::new(move |ev: web_sys::MouseEvent| {
            let Some(outcome) = kanban_click_outcome(&ev) else {
                return;
            };
            // Kanban owns the click — stop it before the editor's
            // selection handler collapses onto the leaf card.
            ev.stop_propagation();
            ev.prevent_default();
            match outcome {
                KanbanClickOutcome::OpenAddCardModal(s)
                | KanbanClickOutcome::OpenEditCardModal(s) => {
                    kanban_modal_click.set(Some(s));
                }
                KanbanClickOutcome::AddColumn { kanban_id } => {
                    set_kanban_column_action
                        .set(Some(KanbanColumnAction::Add { kanban_id }));
                }
                KanbanClickOutcome::RenameColumn {
                    kanban_id,
                    column_id,
                    prompt_default,
                } => {
                    set_kanban_column_action.set(Some(KanbanColumnAction::Rename {
                        kanban_id,
                        column_id,
                        prompt_default,
                    }));
                }
                KanbanClickOutcome::RemoveColumn { kanban_id, column_id } => {
                    set_kanban_column_action.set(Some(KanbanColumnAction::Remove {
                        kanban_id,
                        column_id,
                    }));
                }
                KanbanClickOutcome::SetWipLimit {
                    kanban_id,
                    column_id,
                    prompt_default,
                } => {
                    set_kanban_column_action.set(Some(KanbanColumnAction::SetWipLimit {
                        kanban_id,
                        column_id,
                        prompt_default,
                    }));
                }
            }
        }) as Box<dyn Fn(web_sys::MouseEvent)>);
        let _ = el.add_event_listener_with_callback(
            "click",
            listener.as_ref().unchecked_ref(),
        );
        let cleanup_el = send_wrapper::SendWrapper::new(el);
        let cleanup_listener = send_wrapper::SendWrapper::new(listener);
        on_cleanup(move || {
            let el = cleanup_el.take();
            let listener = cleanup_listener.take();
            let _ = el.remove_event_listener_with_callback(
                "click",
                listener.as_ref().unchecked_ref(),
            );
            drop(listener);
        });
    });

    // Task 8 — delegated click listener for `.mermaid-block`
    // click-to-edit. Sits alongside the Calendar/Kanban observers
    // above, so a single click passes through all three
    // `data-*-action` guards and only fires the branch that
    // matches. Opens `mermaid_modal_state`; the modal's Save
    // outcome is picked up by `on_mermaid_outcome` below.
    let mermaid_modal_click = mermaid_modal_state;
    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        let already: web_sys::HtmlElement = container.clone().into();
        if already.get_attribute("data-mermaid-observer").is_some() {
            return;
        }
        let el: web_sys::HtmlElement = container.into();
        let _ = el.set_attribute("data-mermaid-observer", "attached");
        let listener = Closure::wrap(Box::new(move |ev: web_sys::MouseEvent| {
            let Some(modal_state) = mermaid_click_outcome(&ev) else {
                return;
            };
            // Mermaid owns the click — stop it before the editor's
            // selection handler collapses onto the leaf atom.
            ev.stop_propagation();
            ev.prevent_default();
            mermaid_modal_click.set(Some(modal_state));
        }) as Box<dyn Fn(web_sys::MouseEvent)>);
        let _ = el.add_event_listener_with_callback(
            "click",
            listener.as_ref().unchecked_ref(),
        );
        let cleanup_el = send_wrapper::SendWrapper::new(el);
        let cleanup_listener = send_wrapper::SendWrapper::new(listener);
        on_cleanup(move || {
            let el = cleanup_el.take();
            let listener = cleanup_listener.take();
            let _ = el.remove_event_listener_with_callback(
                "click",
                listener.as_ref().unchecked_ref(),
            );
            drop(listener);
        });
    });

    // #137 — Column-op dispatcher. Rename/Remove use
    // `window.prompt` / `window.confirm` for v1 — good enough for
    // the "type a column name" and "are you sure" moments, and it
    // sidesteps building a second modal in Phase 2. Follow-up
    // ticket can swap in an inline editor.
    let view_ref_col = Rc::clone(&view_ref);
    let history_ref_col = Rc::clone(&history_ref);
    let on_change_col = props.on_change.clone();
    let on_state_change_col = on_state_change_shared.clone();
    let on_mapping_col = props.on_mapping.clone();
    Effect::new(move |_| {
        let Some(action) = kanban_column_action_signal.get() else {
            return;
        };
        let view = view_ref_col.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&history_ref_col);
        let on_change_dispatch = on_change_col.clone();
        let on_state_change_dispatch = on_state_change_col.clone();
        let on_mapping_dispatch = on_mapping_col.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        match action {
            KanbanColumnAction::Add { kanban_id } => {
                let title = crate::i18n::translate("kanban-untitled-column", None);
                commands::add_kanban_column(
                    &kanban_id,
                    title,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            KanbanColumnAction::Rename {
                kanban_id,
                column_id,
                prompt_default,
            } => {
                if let Some(new_title) = window_prompt(
                    &crate::i18n::translate("kanban-column-rename-prompt", None),
                    &prompt_default,
                ) {
                    let trimmed = new_title.trim();
                    if !trimmed.is_empty() {
                        commands::rename_kanban_column(
                            &kanban_id,
                            &column_id,
                            trimmed.to_string(),
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                }
            }
            KanbanColumnAction::Remove { kanban_id, column_id } => {
                if window_confirm(&crate::i18n::translate(
                    "kanban-column-delete-confirm",
                    None,
                )) {
                    commands::remove_kanban_column(
                        &kanban_id,
                        &column_id,
                        &state,
                        Some(&dispatch_fn),
                    );
                }
            }
            KanbanColumnAction::SetWipLimit {
                kanban_id: _,
                column_id,
                prompt_default,
            } => {
                if let Some(raw) = window_prompt(
                    &crate::i18n::translate("kanban-column-wip-limit-prompt", None),
                    &prompt_default,
                ) {
                    let trimmed = raw.trim();
                    // Empty → clear the limit (unlimited).
                    if trimmed.is_empty() {
                        commands::set_kanban_column_wip_limit(
                            &column_id,
                            None,
                            &state,
                            Some(&dispatch_fn),
                        );
                    } else if let Ok(n) = trimmed.parse::<u32>() {
                        commands::set_kanban_column_wip_limit(
                            &column_id,
                            Some(n),
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                    // Non-numeric non-empty input is silently
                    // ignored — the prompt closes on any submit.
                }
            }
        }
    });

    // #137 — Modal outcome dispatcher. Save on Add mode inserts a
    // card; Save on Edit mode replaces the card's attribute bag;
    // Delete removes; Cancel is a noop. Same SendWrapper-around-Rc
    // pattern as the Calendar callback below — Leptos requires
    // `Send + Sync` and wasm-bindgen is single-threaded.
    let view_ref_kanban_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&view_ref));
    let history_ref_kanban_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&history_ref));
    let on_change_kanban_modal = props.on_change.clone();
    let on_state_change_kanban_modal = on_state_change_shared.clone();
    let on_mapping_kanban_modal = props.on_mapping.clone();
    let on_kanban_outcome = Callback::new(move |outcome: KanbanCardOutcome| {
        let view = view_ref_kanban_modal.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&*history_ref_kanban_modal);
        let on_change_dispatch = on_change_kanban_modal.clone();
        let on_state_change_dispatch = on_state_change_kanban_modal.clone();
        let on_mapping_dispatch = on_mapping_kanban_modal.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        match outcome {
            KanbanCardOutcome::Cancel => {}
            KanbanCardOutcome::Delete { card_id } => {
                commands::remove_kanban_card(&card_id, &state, Some(&dispatch_fn));
            }
            KanbanCardOutcome::Save(s) => {
                let attrs = kanban_modal_state_to_attrs(&s);
                match s.mode.clone() {
                    KanbanCardModalMode::Add { column_id } => {
                        commands::add_kanban_card(
                            &column_id,
                            attrs,
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                    KanbanCardModalMode::Edit { card_id } => {
                        commands::edit_kanban_card(
                            &card_id,
                            attrs,
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                }
            }
        }
    });

    // Task 8 — Mermaid modal outcome dispatcher. Save writes the
    // block's `source` attribute; Cancel is a noop. Same
    // SendWrapper-around-Rc pattern as the Calendar/Kanban
    // callbacks above — Leptos requires `Send + Sync` and
    // wasm-bindgen is single-threaded.
    let view_ref_mermaid_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&view_ref));
    let history_ref_mermaid_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&history_ref));
    let on_change_mermaid_modal = props.on_change.clone();
    let on_state_change_mermaid_modal = on_state_change_shared.clone();
    let on_mapping_mermaid_modal = props.on_mapping.clone();
    let on_mermaid_outcome = Callback::new(move |outcome: MermaidModalOutcome| {
        let view = view_ref_mermaid_modal.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&*history_ref_mermaid_modal);
        let on_change_dispatch = on_change_mermaid_modal.clone();
        let on_state_change_dispatch = on_state_change_mermaid_modal.clone();
        let on_mapping_dispatch = on_mapping_mermaid_modal.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        match outcome {
            MermaidModalOutcome::Cancel => {}
            MermaidModalOutcome::Save { block_id, source } => {
                commands::update_mermaid_source(
                    &block_id,
                    source,
                    &state,
                    Some(&dispatch_fn),
                );
            }
        }
    });

    // Task 7 — code-block language-chip selection dispatcher. Same
    // borrow/dispatch scaffolding as `on_mermaid_outcome` above: the
    // view is read fresh at select-time, the transaction goes
    // through the shared history + on_change/on_state_change/
    // on_mapping routing so undo and the toolbar both see it.
    // `on_state_change_shared` also drives `refresh_code_lang_chip`
    // (via `chip_state_tick`), so the chip's `current` reflects the
    // new value immediately after this dispatch.
    let view_ref_code_lang_chip =
        send_wrapper::SendWrapper::new(Rc::clone(&view_ref));
    let history_ref_code_lang_chip =
        send_wrapper::SendWrapper::new(Rc::clone(&history_ref));
    let on_change_code_lang_chip = props.on_change.clone();
    let on_state_change_code_lang_chip = on_state_change_shared.clone();
    let on_mapping_code_lang_chip = props.on_mapping.clone();
    let on_code_lang_select = Callback::new(move |tag: String| {
        let view = view_ref_code_lang_chip.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&*history_ref_code_lang_chip);
        let on_change_dispatch = on_change_code_lang_chip.clone();
        let on_state_change_dispatch = on_state_change_code_lang_chip.clone();
        let on_mapping_dispatch = on_mapping_code_lang_chip.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        commands::set_code_block_language(&tag, &state, Some(&dispatch_fn));
    });

    // #136 — Pointer-driven drag pipeline for calendar events.
    // pointerdown seeds `DRAG_STATE`; pointermove promotes past
    // the movement threshold and applies visual feedback;
    // pointerup computes the new attrs and writes a
    // `DragCommit` to the signal below. The signal indirection
    // matches the attr-update dispatcher pattern above (avoids
    // wrapping view_ref in a SendWrapper).
    let (drag_commit_signal, set_drag_commit) = signal::<Option<DragCommit>>(None);
    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        let el_check: web_sys::HtmlElement = container.clone().into();
        if el_check.get_attribute("data-calendar-drag-observer").is_some() {
            return;
        }
        let el: web_sys::HtmlElement = container.into();
        let _ = el.set_attribute("data-calendar-drag-observer", "attached");
        // pointerdown lives on the container so the browser routes
        // it through the editor's own hit region first.
        let down = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
            drag_on_pointer_down(&ev);
        }) as Box<dyn Fn(web_sys::PointerEvent)>);
        let _ = el.add_event_listener_with_callback(
            "pointerdown",
            down.as_ref().unchecked_ref(),
        );
        // move + up ride on the document because the pointer may
        // wander outside the editor during a drag.
        let doc = web_sys::window().and_then(|w| w.document());
        let (move_c, up_c, cancel_c) = if let Some(doc) = doc.as_ref() {
            let move_c = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
                drag_on_pointer_move(&ev);
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let up_c = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
                drag_on_pointer_up(&ev, set_drag_commit);
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let _ = doc.add_event_listener_with_callback(
                "pointermove",
                move_c.as_ref().unchecked_ref(),
            );
            let _ = doc.add_event_listener_with_callback(
                "pointerup",
                up_c.as_ref().unchecked_ref(),
            );
            let cancel_c = Closure::wrap(Box::new(move |_ev: web_sys::PointerEvent| {
                drag_on_pointer_cancel();
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let _ = doc.add_event_listener_with_callback(
                "pointercancel",
                cancel_c.as_ref().unchecked_ref(),
            );
            (Some(move_c), Some(up_c), Some(cancel_c))
        } else {
            (None, None, None)
        };
        let cleanup_el = send_wrapper::SendWrapper::new(el);
        let cleanup_down = send_wrapper::SendWrapper::new(down);
        let cleanup_doc = send_wrapper::SendWrapper::new(doc);
        let cleanup_move = send_wrapper::SendWrapper::new(move_c);
        let cleanup_up = send_wrapper::SendWrapper::new(up_c);
        let cleanup_cancel = send_wrapper::SendWrapper::new(cancel_c);
        on_cleanup(move || {
            let el = cleanup_el.take();
            let down = cleanup_down.take();
            let _ = el.remove_event_listener_with_callback(
                "pointerdown",
                down.as_ref().unchecked_ref(),
            );
            drop(down);
            if let (Some(doc), Some(move_c), Some(up_c), Some(cancel_c)) =
                (cleanup_doc.take(), cleanup_move.take(), cleanup_up.take(), cleanup_cancel.take())
            {
                let _ = doc.remove_event_listener_with_callback(
                    "pointermove",
                    move_c.as_ref().unchecked_ref(),
                );
                let _ = doc.remove_event_listener_with_callback(
                    "pointerup",
                    up_c.as_ref().unchecked_ref(),
                );
                let _ = doc.remove_event_listener_with_callback(
                    "pointercancel",
                    cancel_c.as_ref().unchecked_ref(),
                );
                drop(move_c);
                drop(up_c);
                drop(cancel_c);
            }
        });
    });

    // Drag-commit dispatcher: on a completed drag, run
    // `edit_calendar_event` with the recomputed attrs.
    let view_ref_drag = Rc::clone(&view_ref);
    let history_ref_drag = Rc::clone(&history_ref);
    let on_change_drag = props.on_change.clone();
    let on_state_change_drag = on_state_change_shared.clone();
    let on_mapping_drag = props.on_mapping.clone();
    Effect::new(move |_| {
        let Some(commit) = drag_commit_signal.get() else { return };
        let view = view_ref_drag.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&history_ref_drag);
        let on_change_dispatch = on_change_drag.clone();
        let on_state_change_dispatch = on_state_change_drag.clone();
        let on_mapping_dispatch = on_mapping_drag.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        commands::edit_calendar_event(
            &commit.block_id,
            &commit.event_id,
            commit.new_attrs,
            &state,
            Some(&dispatch_fn),
        );
    });

    // #137 Phase 3 — Kanban card drag pipeline. Same three-listener
    // shape as the calendar drag above: pointerdown on the editor
    // container seeds `KANBAN_DRAG_STATE`; pointermove + pointerup
    // ride on `document` so the drag can wander outside the editor.
    // The commit signal indirection is what lets us borrow
    // `view_ref` without a SendWrapper.
    let (kanban_drag_commit_signal, set_kanban_drag_commit) =
        signal::<Option<KanbanDragCommit>>(None);
    Effect::new(move |_| {
        let Some(container) = container_ref.get() else { return };
        let el_check: web_sys::HtmlElement = container.clone().into();
        if el_check.get_attribute("data-kanban-drag-observer").is_some() {
            return;
        }
        let el: web_sys::HtmlElement = container.into();
        let _ = el.set_attribute("data-kanban-drag-observer", "attached");
        let down = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
            kanban_drag_on_pointer_down(&ev);
        }) as Box<dyn Fn(web_sys::PointerEvent)>);
        let _ = el.add_event_listener_with_callback(
            "pointerdown",
            down.as_ref().unchecked_ref(),
        );
        let doc = web_sys::window().and_then(|w| w.document());
        let (move_c, up_c, cancel_c) = if let Some(doc) = doc.as_ref() {
            let move_c = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
                kanban_drag_on_pointer_move(&ev);
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let up_c = Closure::wrap(Box::new(move |ev: web_sys::PointerEvent| {
                kanban_drag_on_pointer_up(&ev, set_kanban_drag_commit);
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let cancel_c = Closure::wrap(Box::new(move |_ev: web_sys::PointerEvent| {
                kanban_drag_on_pointer_cancel();
            }) as Box<dyn Fn(web_sys::PointerEvent)>);
            let _ = doc.add_event_listener_with_callback(
                "pointermove",
                move_c.as_ref().unchecked_ref(),
            );
            let _ = doc.add_event_listener_with_callback(
                "pointerup",
                up_c.as_ref().unchecked_ref(),
            );
            let _ = doc.add_event_listener_with_callback(
                "pointercancel",
                cancel_c.as_ref().unchecked_ref(),
            );
            (Some(move_c), Some(up_c), Some(cancel_c))
        } else {
            (None, None, None)
        };
        let cleanup_el = send_wrapper::SendWrapper::new(el);
        let cleanup_down = send_wrapper::SendWrapper::new(down);
        let cleanup_doc = send_wrapper::SendWrapper::new(doc);
        let cleanup_move = send_wrapper::SendWrapper::new(move_c);
        let cleanup_up = send_wrapper::SendWrapper::new(up_c);
        let cleanup_cancel = send_wrapper::SendWrapper::new(cancel_c);
        on_cleanup(move || {
            let el = cleanup_el.take();
            let down = cleanup_down.take();
            let _ = el.remove_event_listener_with_callback(
                "pointerdown",
                down.as_ref().unchecked_ref(),
            );
            drop(down);
            if let (Some(doc), Some(move_c), Some(up_c), Some(cancel_c)) =
                (cleanup_doc.take(), cleanup_move.take(), cleanup_up.take(), cleanup_cancel.take())
            {
                let _ = doc.remove_event_listener_with_callback(
                    "pointermove",
                    move_c.as_ref().unchecked_ref(),
                );
                let _ = doc.remove_event_listener_with_callback(
                    "pointerup",
                    up_c.as_ref().unchecked_ref(),
                );
                let _ = doc.remove_event_listener_with_callback(
                    "pointercancel",
                    cancel_c.as_ref().unchecked_ref(),
                );
                drop(move_c);
                drop(up_c);
                drop(cancel_c);
            }
        });
    });

    // Kanban drag-commit dispatcher: on a completed card drop,
    // run `move_kanban_card` with the resolved column ids +
    // insertion index.
    let view_ref_kdrag = Rc::clone(&view_ref);
    let history_ref_kdrag = Rc::clone(&history_ref);
    let on_change_kdrag = props.on_change.clone();
    let on_state_change_kdrag = on_state_change_shared.clone();
    let on_mapping_kdrag = props.on_mapping.clone();
    Effect::new(move |_| {
        let Some(commit) = kanban_drag_commit_signal.get() else { return };
        let view = view_ref_kdrag.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&history_ref_kdrag);
        let on_change_dispatch = on_change_kdrag.clone();
        let on_state_change_dispatch = on_state_change_kdrag.clone();
        let on_mapping_dispatch = on_mapping_kdrag.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        match commit {
            KanbanDragCommit::Card { to_column_id, card_id, to_index } => {
                commands::move_kanban_card(
                    &to_column_id,
                    &card_id,
                    to_index,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            KanbanDragCommit::Column { column_id, to_index } => {
                commands::move_kanban_column(
                    &column_id,
                    to_index,
                    &state,
                    Some(&dispatch_fn),
                );
            }
        }
    });

    // Apply remote document updates from collaborators.
    let view_ref_remote = Rc::clone(&view_ref);
    let on_state_change_remote = on_state_change_shared.clone();
    let remote_state_signal = props.remote_state;
    let history_ref_remote = Rc::clone(&history_ref);
    Effect::new(move |_| {
        let Some(new_state) = remote_state_signal.get() else { return };
        let view = view_ref_remote.borrow();
        let Some(view) = view.as_ref() else { return };

        // #151 generalization: a remote/concurrent edit swaps the document
        // wholesale. Carry the recorded undo/redo stack into the new
        // coordinate space first, so a later local undo doesn't apply its
        // steps at now-stale offsets — the #151 corruption, in its remote
        // form. The map is char-precise for the dominant single-edit case;
        // `remap_through` shifts every recorded step (correct for a
        // concurrent edit, which stays applied across all undos — unlike a
        // local edit, which `record` deliberately does NOT remap through).
        // `undo`/`redo` already decline cleanly if a remapped step no
        // longer applies, so an imprecise remap degrades to a skipped undo,
        // never a corrupt document.
        let old_doc = view.state().doc.clone();
        let map = crate::editor::transform::step_map_for_doc_swap(&old_doc, &new_state.doc);
        history_ref_remote
            .borrow_mut()
            .remap_through(std::slice::from_ref(&map));

        view.update_state(new_state.clone());
        on_state_change_remote.run(new_state);
    });

    // Process toolbar commands reactively
    let view_ref_cmd = Rc::clone(&view_ref);
    let history_ref_cmd = Rc::clone(&history_ref);
    let on_change_cmd = props.on_change.clone();
    let on_state_change_cmd = on_state_change_shared.clone();
    let on_mapping_cmd = props.on_mapping.clone();

    Effect::new(move |_| {
        let Some(cmd) = props.command_signal.get() else { return };

        let view = view_ref_cmd.borrow();
        let Some(view) = view.as_ref() else { return };

        // Sync DOM selection to model before executing the command,
        // so toolbar actions see the user's actual selection, not a
        // stale cursor.
        //
        // One refinement: don't let a collapsed DOM cursor override a
        // non-empty model range. After a focus-trap restoration (the
        // command palette closing → focus returning to the editor),
        // headless Chromium does NOT re-extend the previous Ctrl+A
        // range — `window.getSelection()` reports a fresh cursor,
        // typically at position 0. Letting that cursor override would
        // clobber the user's deliberately-saved range and `toggle_mark`
        // would silently no-op against an empty selection. The
        // frontend-doctor's command-palette-actions scenario surfaces
        // this as `boldApplied: false` even though every other step
        // passes. Trusting the model in the (cursor < range) case
        // preserves the user's intent; trusting the DOM in every
        // other case keeps the original anti-stale-cursor protection.
        let state = {
            let mut s = view.state();
            if let Some(dom_sel) = view.read_dom_selection() {
                let dom_collapsed_over_range =
                    dom_sel.empty() && !s.selection.empty();
                if !dom_collapsed_over_range {
                    s.selection = dom_sel;
                }
            }
            s
        };
        let history = Rc::clone(&history_ref_cmd);
        let on_change = on_change_cmd.clone();
        let on_state_change = on_state_change_cmd.clone();
        let on_mapping_ref = on_mapping_cmd.as_ref();

        let dispatch_fn = |txn: Transaction| {
            let v = view_ref_cmd.borrow();
            let Some(v) = v.as_ref() else { return };
            apply_and_notify(v, txn, Some(&history), &on_change, &on_state_change, on_mapping_ref);
        };

        match cmd {
            ToolbarCommand::ToggleBold => { commands::toggle_mark(MarkType::Bold, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleItalic => { commands::toggle_mark(MarkType::Italic, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleUnderline => { commands::toggle_mark(MarkType::Underline, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleStrike => { commands::toggle_mark(MarkType::Strike, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleCode => { commands::toggle_mark(MarkType::Code, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleSubscript => { commands::toggle_mark(MarkType::Subscript, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleSuperscript => { commands::toggle_mark(MarkType::Superscript, &state, Some(&dispatch_fn)); }
            ToolbarCommand::SetParagraph => { commands::set_paragraph(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SetHeading(level) => { commands::set_heading(level, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleBulletList => { commands::toggle_list(NodeType::BulletList, NodeType::ListItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleOrderedList => { commands::toggle_list(NodeType::OrderedList, NodeType::ListItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleTaskList => { commands::toggle_list(NodeType::TaskList, NodeType::TaskItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleBlockquote => { commands::toggle_blockquote(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SetCodeBlock => { commands::set_code_block(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SetAlignment(ref align) => { commands::set_alignment(align, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ClearFormatting => { commands::clear_formatting(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SelectRange { from, to } => { commands::select_range(from, to, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ReplaceRange { from, to, ref text } => { commands::replace_range(from, to, text, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ReplaceAll { ref matches, ref text } => { commands::replace_all(matches, text, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertHorizontalRule => { commands::insert_horizontal_rule(&state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertTable => { commands::insert_table(3, 3, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleLink(ref href) => { commands::toggle_link(href, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertDocLink { from, to, ref title, ref href } => { commands::insert_doc_link(from, to, title, href, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertUserMention { from, to, ref display, ref user_id } => { commands::insert_user_mention(from, to, display, user_id, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertAiText { from, to, ref text } => { commands::insert_ai_text(from, to, text, &state, Some(&dispatch_fn)); }
            // OpenAskDialog is a page-scope command handled in
            // pages/document.rs::on_command; this arm just
            // absorbs it silently if it ever leaks through.
            ToolbarCommand::OpenAskDialog { .. } => {}
            ToolbarCommand::ToggleTextColor(ref color) => { commands::toggle_color_mark(MarkType::TextColor, color, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleHighlight(ref color) => { commands::toggle_color_mark(MarkType::Highlight, color, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertComment => {}
            ToolbarCommand::UploadImage => {
                handle_image_upload(&props.doc_id, &view_ref_cmd, &history_ref_cmd, &on_change_cmd, &on_state_change_cmd);
            }
            ToolbarCommand::InsertEmbed { ref url, ref provider, height, ref title } => {
                commands::insert_embed(
                    url,
                    provider,
                    height,
                    title.as_deref(),
                    &state,
                    Some(&dispatch_fn),
                );
            }
            ToolbarCommand::InsertLiveApp(id) => {
                commands::insert_live_app(id, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::Undo => {
                if let Some(txn) = history_ref_cmd.borrow_mut().undo(&state) {
                    dispatch_fn(txn);
                }
            }
            ToolbarCommand::Redo => {
                if let Some(txn) = history_ref_cmd.borrow_mut().redo(&state) {
                    dispatch_fn(txn);
                }
            }
            // Spreadsheet-only: no-op in document mode.
            ToolbarCommand::SetNumberFormat(_) => {}
        }
    });

    // ─── Right-click context menu ─────────────────────────────────
    //
    // Suppress the OS context menu over `.editor-content` and surface
    // the app's own menu with Cut / Copy / Paste / Comment / format
    // toggles / Insert link. The dispatch flows back through the same
    // `commands::*` and view-borrow path the toolbar Effect above
    // uses, so the menu items share their behavior with the toolbar
    // and the keyboard shortcuts.
    let (ctx_menu_visible, set_ctx_menu_visible) = signal(false);
    let (ctx_menu_x, set_ctx_menu_x) = signal(0.0f64);
    let (ctx_menu_y, set_ctx_menu_y) = signal(0.0f64);

    // Updated by the on:contextmenu handler at the moment the menu
    // opens, so the menu items see the selection state at the
    // moment of the right-click. A reactive `Signal::derive` would
    // need to capture the !Send+!Sync `view_ref` Rc; this signal-
    // backed approach keeps the closure free of the Rc.
    let (selection_empty, set_selection_empty) = signal(true);

    // The menu's `on_command` Callback writes to this signal; an
    // Effect below watches the signal and runs the actual dispatch.
    // Indirection is necessary because the Callback must be Send +
    // Sync (Leptos requirement), but the dispatch logic captures
    // Rc<RefCell<EditorView>> which is neither. The Effect runs on
    // the local thread and can borrow the Rc safely.
    let (pending_ctx_cmd, set_pending_ctx_cmd) =
        signal::<Option<EditorContextCommand>>(None);

    let view_ref_cmd2 = Rc::clone(&view_ref);
    let history_ref_cmd2 = Rc::clone(&history_ref);
    let on_change_cmd2 = props.on_change.clone();
    let on_state_change_cmd2 = on_state_change_shared.clone();
    let on_mapping_cmd2 = props.on_mapping.clone();
    let on_request_comment_prop = props.on_request_comment;

    Effect::new(move |_| {
        let Some(cmd) = pending_ctx_cmd.get() else { return };
        // Drain the signal so the same command doesn't re-fire if a
        // later read picks up the stale value.
        set_pending_ctx_cmd.set(None);

        match cmd {
            EditorContextCommand::Cut => {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Ok(html_doc) = doc.dyn_into::<web_sys::HtmlDocument>() {
                        let _ = html_doc.exec_command("cut");
                    }
                }
                return;
            }
            EditorContextCommand::Copy => {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Ok(html_doc) = doc.dyn_into::<web_sys::HtmlDocument>() {
                        let _ = html_doc.exec_command("copy");
                    }
                }
                return;
            }
            EditorContextCommand::Paste => {
                // `document.execCommand('paste')` is disabled in
                // modern browsers for security; use the async
                // Clipboard API and insert the result as markdown
                // so a pasted "**bold**" lands as actual bold.
                let view_rc = Rc::clone(&view_ref_cmd2);
                let history_rc = Rc::clone(&history_ref_cmd2);
                let on_change = on_change_cmd2.clone();
                let on_state_change = on_state_change_cmd2.clone();
                let on_mapping = on_mapping_cmd2.clone();
                leptos::task::spawn_local(async move {
                    let Some(window) = web_sys::window() else { return };
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.read_text();
                    let Ok(val) = wasm_bindgen_futures::JsFuture::from(promise).await
                    else { return };
                    let Some(text) = val.as_string() else { return };
                    if text.is_empty() { return }
                    let view = view_rc.borrow();
                    let Some(view) = view.as_ref() else { return };
                    let state = view.state();
                    let slice = crate::editor::markdown::parse_from_markdown(&text);
                    if slice.content.children.is_empty() { return }
                    if let Ok(txn) = state.transaction().replace_selection(slice) {
                        apply_and_notify(
                            view,
                            txn,
                            Some(&history_rc),
                            &on_change,
                            &on_state_change,
                            on_mapping.as_ref(),
                        );
                    }
                });
                return;
            }
            EditorContextCommand::CopyBlockLink => {
                // Block containing the selection head; block_id_at walks to
                // the innermost block carrying a blockId.
                let view = view_ref_cmd2.borrow();
                if let Some(view) = view.as_ref() {
                    let state = view.state();
                    let pos = state.selection.from();
                    if let Some(block_id) = state.doc.block_id_at(pos) {
                        copy_block_link(&block_id);
                    }
                }
                return;
            }
            EditorContextCommand::Comment => {
                if let Some(cb) = on_request_comment_prop {
                    cb.run(());
                }
                return;
            }
            _ => {}
        }

        // Format toggles + Insert link mirror the toolbar Effect's
        // dispatch path: borrow the view + state, run the matching
        // command, then push the resulting transaction through
        // apply_and_notify so history records it.
        let view = view_ref_cmd2.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = {
            let mut s = view.state();
            if let Some(dom_sel) = view.read_dom_selection() {
                let dom_collapsed_over_range =
                    dom_sel.empty() && !s.selection.empty();
                if !dom_collapsed_over_range {
                    s.selection = dom_sel;
                }
            }
            s
        };
        let history = Rc::clone(&history_ref_cmd2);
        let on_change = on_change_cmd2.clone();
        let on_state_change = on_state_change_cmd2.clone();
        let on_mapping = on_mapping_cmd2.clone();
        let dispatch_fn = |txn: Transaction| {
            let v = view_ref_cmd2.borrow();
            let Some(v) = v.as_ref() else { return };
            apply_and_notify(
                v,
                txn,
                Some(&history),
                &on_change,
                &on_state_change,
                on_mapping.as_ref(),
            );
        };

        match cmd {
            EditorContextCommand::ToggleBold => {
                commands::toggle_mark(MarkType::Bold, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::ToggleItalic => {
                commands::toggle_mark(MarkType::Italic, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::ToggleUnderline => {
                commands::toggle_mark(MarkType::Underline, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::ToggleStrike => {
                commands::toggle_mark(MarkType::Strike, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::ToggleCode => {
                commands::toggle_mark(MarkType::Code, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::SetParagraph => {
                commands::set_paragraph(&state, Some(&dispatch_fn));
            }
            EditorContextCommand::SetHeading1 => {
                commands::set_heading(1, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::SetHeading2 => {
                commands::set_heading(2, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::SetHeading3 => {
                commands::set_heading(3, &state, Some(&dispatch_fn));
            }
            EditorContextCommand::ToggleBulletList => {
                commands::toggle_list(
                    NodeType::BulletList,
                    NodeType::ListItem,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            EditorContextCommand::ToggleOrderedList => {
                commands::toggle_list(
                    NodeType::OrderedList,
                    NodeType::ListItem,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            EditorContextCommand::ToggleTaskList => {
                commands::toggle_list(
                    NodeType::TaskList,
                    NodeType::TaskItem,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            EditorContextCommand::ToggleBlockquote => {
                commands::toggle_blockquote(&state, Some(&dispatch_fn));
            }
            EditorContextCommand::SetCodeBlock => {
                commands::set_code_block(&state, Some(&dispatch_fn));
            }
            EditorContextCommand::AlignLeft => {
                commands::set_alignment("left", &state, Some(&dispatch_fn));
            }
            EditorContextCommand::AlignCenter => {
                commands::set_alignment("center", &state, Some(&dispatch_fn));
            }
            EditorContextCommand::AlignRight => {
                commands::set_alignment("right", &state, Some(&dispatch_fn));
            }
            EditorContextCommand::InsertLink => {
                // Same prompt + toggle path as Ctrl+K in view.rs's
                // keydown handler — keep the two in sync.
                let has_link = commands::mark_active_at_cursor_public(
                    &state,
                    MarkType::Link,
                );
                if has_link {
                    commands::toggle_link("", &state, Some(&dispatch_fn));
                } else if !state.selection.empty() {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(href)) = window.prompt_with_message("Enter URL:") {
                            let href = href.trim().to_string();
                            if !href.is_empty() {
                                commands::toggle_link(&href, &state, Some(&dispatch_fn));
                            }
                        }
                    }
                }
            }
            // Clipboard + block link + Comment handled above.
            EditorContextCommand::Cut
            | EditorContextCommand::Copy
            | EditorContextCommand::Paste
            | EditorContextCommand::CopyBlockLink
            | EditorContextCommand::Comment => {}
        }
    });

    let on_ctx_command = Callback::new(move |cmd: EditorContextCommand| {
        set_pending_ctx_cmd.set(Some(cmd));
    });

    // #136 — build the ModalOutcome handler that converts modal
    // Save/Delete into `commands::{add,edit,remove}_calendar_event`
    // dispatches. Cancel is a no-op — the modal already closes on
    // its own signal write.
    //
    // Rc<RefCell<_>> isn't `Send + Sync`, but Leptos Callbacks
    // require both — same pattern as the collab_client cleanup
    // closure in pages/document.rs. Ride the Rcs in a SendWrapper;
    // wasm-bindgen is single-threaded so the Send bound is a
    // formality the runtime never actually exercises.
    let view_ref_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&view_ref));
    let history_ref_modal =
        send_wrapper::SendWrapper::new(Rc::clone(&history_ref));
    let on_change_modal = props.on_change.clone();
    let on_state_change_modal = on_state_change_shared.clone();
    let on_mapping_modal = props.on_mapping.clone();
    let on_modal_outcome = Callback::new(move |outcome: ModalOutcome| {
        let view = view_ref_modal.borrow();
        let Some(view) = view.as_ref() else { return };
        let state = view.state();
        let history_ref_dispatch = Rc::clone(&*history_ref_modal);
        let on_change_dispatch = on_change_modal.clone();
        let on_state_change_dispatch = on_state_change_modal.clone();
        let on_mapping_dispatch = on_mapping_modal.clone();
        let dispatch_fn = move |txn: Transaction| {
            apply_and_notify(
                view,
                txn,
                Some(&history_ref_dispatch),
                &on_change_dispatch,
                &on_state_change_dispatch,
                on_mapping_dispatch.as_ref(),
            );
        };
        match outcome {
            ModalOutcome::Cancel => {}
            ModalOutcome::Delete { block_id, event_id } => {
                commands::remove_calendar_event(
                    &block_id,
                    &event_id,
                    &state,
                    Some(&dispatch_fn),
                );
            }
            ModalOutcome::Save(s) => {
                let attrs = modal_state_to_attrs(&s);
                match s.mode.clone() {
                    CalendarModalMode::Add { block_id, .. } => {
                        commands::add_calendar_event(
                            &block_id,
                            attrs,
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                    CalendarModalMode::Edit { block_id, event_id } => {
                        commands::edit_calendar_event(
                            &block_id,
                            &event_id,
                            attrs,
                            &state,
                            Some(&dispatch_fn),
                        );
                    }
                }
            }
        }
    });

    let on_scroll = props.on_scroll.clone();
    let view_ref_ctx_open = Rc::clone(&view_ref);
    view! {
        <div
            node_ref=editor_wrapper_ref
            class="editor-container"
            on:scroll=move |_| {
                if let Some(ref cb) = on_scroll {
                    cb.run(());
                }
            }
        >
            <div
                node_ref=container_ref
                class="editor-content"
                on:contextmenu=move |e: web_sys::MouseEvent| {
                    // Suppress the OS menu and surface ours. Compute
                    // selection_empty at open time from the current
                    // DOM selection so disabled-state matches the
                    // moment of the right-click.
                    e.prevent_default();
                    let empty = view_ref_ctx_open
                        .borrow()
                        .as_ref()
                        .and_then(|v| v.read_dom_selection())
                        .map(|s| s.empty())
                        .unwrap_or(true);
                    set_selection_empty.set(empty);
                    set_ctx_menu_x.set(e.client_x() as f64);
                    set_ctx_menu_y.set(e.client_y() as f64);
                    set_ctx_menu_visible.set(true);
                }
            ></div>
            <EditorContextMenu
                visible=ctx_menu_visible
                x=ctx_menu_x
                y=ctx_menu_y
                selection_empty=selection_empty.into()
                on_command=on_ctx_command
                on_close=Callback::new(move |()| set_ctx_menu_visible.set(false))
            />
            <CalendarModal
                state=calendar_modal_state
                on_outcome=on_modal_outcome
            />
            <KanbanCardModal
                state=kanban_card_modal_state
                on_outcome=on_kanban_outcome
            />
            <MermaidModal
                state=mermaid_modal_state
                on_outcome=on_mermaid_outcome
            />
            <CodeLangChip
                state=code_lang_chip_state
                on_select=on_code_lang_select
            />
        </div>
    }
}
