// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #136 — Calendar event add/edit modal.
//!
//! Opens in response to clicks on a `.calendar-block`:
//! - Clicking an empty day cell opens Add mode with the date
//!   pre-filled and today's tz.
//! - Clicking an event span opens Edit mode with the event's
//!   current attrs pre-filled.
//!
//! The click observer lives in `editor_component.rs`; this module
//! owns the modal's Leptos state and view. On Save/Delete, the
//! callbacks emit `ModalOutcome` variants that the parent maps to
//! `commands::{add_calendar_event, edit_calendar_event,
//! remove_calendar_event}`.

use leptos::prelude::*;

use crate::a11y;

/// The six event colors — same as the backend
/// `blocks::calendar::COLORS`. Kept in one place per side because
/// the color chip UI needs the labels and hex fallbacks bundled;
/// the source of truth on the wire is still the backend enum.
pub const COLORS: &[(&str, &str)] = &[
    ("red", "#dc2626"),
    ("orange", "#ea580c"),
    ("yellow", "#ca8a04"),
    ("green", "#16a34a"),
    ("blue", "#2563eb"),
    ("violet", "#7c3aed"),
];

/// Which mode the modal is in — determines the header label,
/// whether the Delete button shows, and what command runs on
/// Save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarModalMode {
    /// User clicked an empty day cell; we're creating a new event.
    /// `date` is `YYYY-MM-DD` from the cell's `data-calendar-date`.
    Add {
        block_id: String,
        date: String,
    },
    /// User clicked an existing event; we're editing.
    Edit {
        block_id: String,
        event_id: String,
    },
}

/// Everything the modal needs to render + carry back to the
/// caller. Held in a `RwSignal<Option<CalendarModalState>>` by
/// `editor_component.rs`; `None` means the modal is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarModalState {
    pub mode: CalendarModalMode,
    pub content: String,
    pub color: String,
    pub all_day: bool,
    /// `YYYY-MM-DD` when all-day; the date portion when timed.
    pub start_date: String,
    pub end_date: String,
    /// `HH:MM` when timed; empty when all-day.
    pub start_time: String,
    pub end_time: String,
    /// The Calendar block's IANA timezone (e.g.
    /// `"America/Los_Angeles"`). On Save the outer dispatcher
    /// interprets the wall-clock `start_time` / `end_time` as
    /// being in this tz and converts to the stored UTC instant.
    /// Empty string falls back to browser-local.
    pub timezone: String,
}

impl CalendarModalState {
    /// Build an Add-mode initial state for the given block/date.
    /// `timezone` is the block's declared display tz — plumbed
    /// through so the Save handler converts (date+time) in the
    /// user's expected tz, not the browser's.
    pub fn new_add(block_id: String, date: String, timezone: String) -> Self {
        Self {
            mode: CalendarModalMode::Add {
                block_id,
                date: date.clone(),
            },
            content: String::new(),
            color: "blue".into(),
            all_day: true,
            start_date: date.clone(),
            end_date: date,
            start_time: "09:00".into(),
            end_time: "10:00".into(),
            timezone,
        }
    }
}

/// Everything the parent needs to route the modal's result. On
/// Save, the parent inspects `mode` on the returned state to
/// decide whether to add or edit. On Delete, the event id is in
/// `mode` (`Edit` only).
#[derive(Debug, Clone)]
pub enum ModalOutcome {
    Save(CalendarModalState),
    Delete { block_id: String, event_id: String },
    Cancel,
}

#[component]
pub fn CalendarModal(
    /// `Some` → open; `None` → hidden. Parent writes; modal reads.
    #[prop(into)] state: RwSignal<Option<CalendarModalState>>,
    on_outcome: Callback<ModalOutcome>,
) -> impl IntoView {
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    let visible = Signal::derive(move || state.get().is_some());
    a11y::install_focus_trap(dialog_ref, visible);

    view! {
        <Show when=move || state.get().is_some()>
            {move || state.get().map(|initial| {
                render_modal(initial, state, on_outcome.clone(), dialog_ref)
            })}
        </Show>
    }
}

fn render_modal(
    initial: CalendarModalState,
    state: RwSignal<Option<CalendarModalState>>,
    on_outcome: Callback<ModalOutcome>,
    dialog_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    // Per-field working copies. Editing them stages the change;
    // Save commits by wrapping into `CalendarModalState` and
    // firing `ModalOutcome::Save`.
    let (content, set_content) = signal(initial.content.clone());
    let (color, set_color) = signal(initial.color.clone());
    let (all_day, set_all_day) = signal(initial.all_day);
    let (start_date, set_start_date) = signal(initial.start_date.clone());
    let (end_date, set_end_date) = signal(initial.end_date.clone());
    let (start_time, set_start_time) = signal(initial.start_time.clone());
    let (end_time, set_end_time) = signal(initial.end_time.clone());

    let mode_for_close = initial.mode.clone();
    let is_edit = matches!(initial.mode, CalendarModalMode::Edit { .. });
    let mode_for_delete = initial.mode.clone();
    let mode_for_save = initial.mode.clone();

    let _ = mode_for_close;
    // Every close path flips `state.set(None)`, which collapses the
    // outer `<Show>` on the same reactive turn and drops the
    // wasm-bindgen closures on the modal's inner divs. If we ran
    // synchronously, the still-bubbling click / keydown would then
    // re-enter one of those dropped closures — the Firefox
    // "closure invoked after being dropped" panic every other
    // modal in the app guards against via `a11y::defer_close`.
    // Route Cancel / Save / Delete through the same deferral.
    let close_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            state.set(None);
            on_outcome.run(ModalOutcome::Cancel);
        }
    });
    let save_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        let tz = initial.timezone.clone();
        move |()| {
            let out = CalendarModalState {
                mode: mode_for_save.clone(),
                content: content.get(),
                color: color.get(),
                all_day: all_day.get(),
                start_date: start_date.get(),
                end_date: end_date.get(),
                start_time: start_time.get(),
                end_time: end_time.get(),
                timezone: tz.clone(),
            };
            // Reject a Save with empty required fields — an empty
            // <input type="date"> submits "" and downstream would
            // persist an orphan event that never renders. Silently
            // do nothing so the user sees the modal stays open;
            // the browser also shows the native "please fill out
            // this field" UI once we mark the date input required.
            if out.start_date.is_empty() || out.end_date.is_empty() {
                return;
            }
            if !out.all_day
                && (out.start_time.is_empty() || out.end_time.is_empty())
            {
                return;
            }
            state.set(None);
            on_outcome.run(ModalOutcome::Save(out));
        }
    });
    let delete_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            if let CalendarModalMode::Edit {
                block_id,
                event_id,
            } = mode_for_delete.clone()
            {
                state.set(None);
                on_outcome.run(ModalOutcome::Delete { block_id, event_id });
            }
        }
    });

    let title = if is_edit {
        crate::t!("calendar-modal-edit-title")
    } else {
        crate::t!("calendar-modal-add-title")
    };

    view! {
        <div
            class="confirm-backdrop"
            on:click=move |_| a11y::defer_close(close_cb)
        >
            <div
                node_ref=dialog_ref
                class="calendar-modal"
                role="dialog"
                aria-modal="true"
                on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                on:keydown=move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Escape" {
                        a11y::defer_close(close_cb);
                    } else if e.key() == "Enter" && !e.shift_key() {
                        // Enter saves — except inside a textarea
                        // (multi-line description) or when a
                        // button is focused (Enter should trigger
                        // that button's own action). Shift+Enter
                        // falls through so users can still insert
                        // a newline in the textarea.
                        use wasm_bindgen::JsCast;
                        let target_tag = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
                            .map(|el| el.tag_name().to_ascii_lowercase())
                            .unwrap_or_default();
                        if target_tag == "textarea" || target_tag == "button" {
                            return;
                        }
                        e.prevent_default();
                        a11y::defer_close(save_cb);
                    }
                }
            >
                <div class="confirm-header">
                    <h3>{title}</h3>
                </div>
                <div class="calendar-modal-body">
                    <label class="calendar-modal-field">
                        <span>{crate::t!("calendar-modal-content-label")}</span>
                        <input
                            type="text"
                            prop:value=move || content.get()
                            on:input=move |e| set_content.set(event_target_value(&e))
                            autofocus
                        />
                    </label>

                    <fieldset class="calendar-modal-field calendar-modal-color-field">
                        <legend>{crate::t!("calendar-modal-color-label")}</legend>
                        <span class="calendar-modal-color-chips">
                            {COLORS.iter().map(|(name, hex)| {
                                let name: &'static str = name;
                                let hex: &'static str = hex;
                                let is_active = Signal::derive(move || color.get() == name);
                                view! {
                                    <button
                                        type="button"
                                        class=move || {
                                            if is_active.get() {
                                                format!("calendar-modal-color calendar-modal-color--active")
                                            } else {
                                                "calendar-modal-color".to_string()
                                            }
                                        }
                                        style=format!("background: {hex}")
                                        aria-label=name
                                        on:click=move |_| set_color.set(name.to_string())
                                    />
                                }
                            }).collect_view()}
                        </span>
                    </fieldset>

                    <label class="calendar-modal-field calendar-modal-allday">
                        <input
                            type="checkbox"
                            prop:checked=move || all_day.get()
                            on:change=move |e| set_all_day.set(event_target_checked(&e))
                        />
                        <span>{crate::t!("calendar-modal-allday-label")}</span>
                    </label>

                    <div class="calendar-modal-field calendar-modal-datetime">
                        <label>
                            <span>{crate::t!("calendar-modal-start-label")}</span>
                            <input
                                type="date"
                                required=true
                                prop:value=move || start_date.get()
                                on:input=move |e| set_start_date.set(event_target_value(&e))
                            />
                            <Show when=move || !all_day.get()>
                                <input
                                    type="time"
                                    prop:value=move || start_time.get()
                                    on:input=move |e| set_start_time.set(event_target_value(&e))
                                />
                            </Show>
                        </label>
                        <label>
                            <span>{crate::t!("calendar-modal-end-label")}</span>
                            <input
                                type="date"
                                required=true
                                prop:value=move || end_date.get()
                                on:input=move |e| set_end_date.set(event_target_value(&e))
                            />
                            <Show when=move || !all_day.get()>
                                <input
                                    type="time"
                                    prop:value=move || end_time.get()
                                    on:input=move |e| set_end_time.set(event_target_value(&e))
                                />
                            </Show>
                        </label>
                    </div>
                </div>
                <div class="calendar-modal-actions">
                    <Show when=move || is_edit>
                        <button
                            class="btn btn-danger"
                            on:click=move |_| a11y::defer_close(delete_cb)
                        >
                            {crate::t!("calendar-modal-delete")}
                        </button>
                    </Show>
                    <span class="calendar-modal-spacer"></span>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| a11y::defer_close(close_cb)
                    >
                        {crate::t!("common-cancel")}
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| a11y::defer_close(save_cb)
                    >
                        {crate::t!("calendar-modal-save")}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn event_target_value(e: &web_sys::Event) -> String {
    use wasm_bindgen::JsCast;
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}

fn event_target_checked(e: &web_sys::Event) -> bool {
    use wasm_bindgen::JsCast;
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or_default()
}
