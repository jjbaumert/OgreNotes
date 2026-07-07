// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #137 — Kanban card add/edit modal.
//!
//! Opens on click of a `.kanban-card` (Edit mode) or on the
//! `+ Add card` button at the tail of a column (Add mode). Same
//! defer-close pattern as `calendar_modal` to guard against the
//! Firefox "closure invoked after being dropped" panic.

use leptos::prelude::*;

use crate::a11y;

/// Six-hue palette shared with Calendar.
pub const COLORS: &[(&str, &str)] = &[
    ("red", "#dc2626"),
    ("orange", "#ea580c"),
    ("yellow", "#ca8a04"),
    ("green", "#16a34a"),
    ("blue", "#2563eb"),
    ("violet", "#7c3aed"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KanbanCardModalMode {
    /// Adding a new card to `column_id`.
    Add { column_id: String },
    /// Editing an existing card.
    Edit { card_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KanbanCardModalState {
    pub mode: KanbanCardModalMode,
    pub title: String,
    pub content: String,
    pub color: String,
    /// Phase 4b — YYYY-MM-DD. Empty when unset. Rendered as a
    /// pill on the card; visually "overdue" when past today.
    pub due_at: String,
    /// Phase 4b — semicolon-separated `name|color` pairs, e.g.
    /// `"bug|red;ux|blue"`. Empty when the card has no labels.
    /// Format is stringly on purpose — matches the same pattern
    /// the schema uses for `color` etc. and avoids needing a
    /// child-node shape for something this small.
    pub labels: String,
    /// Phase 4c — user id of the assignee. Empty when unassigned.
    pub assignee_id: String,
    /// Phase 4c — display name captured at pick time so the
    /// avatar chip renders without a directory lookup each time.
    pub assignee_name: String,
}

impl KanbanCardModalState {
    pub fn new_add(column_id: String) -> Self {
        Self {
            mode: KanbanCardModalMode::Add { column_id },
            title: String::new(),
            content: String::new(),
            color: "blue".into(),
            due_at: String::new(),
            labels: String::new(),
            assignee_id: String::new(),
            assignee_name: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum KanbanCardOutcome {
    Save(KanbanCardModalState),
    Delete { card_id: String },
    Cancel,
}

#[component]
pub fn KanbanCardModal(
    #[prop(into)] state: RwSignal<Option<KanbanCardModalState>>,
    on_outcome: Callback<KanbanCardOutcome>,
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
    initial: KanbanCardModalState,
    state: RwSignal<Option<KanbanCardModalState>>,
    on_outcome: Callback<KanbanCardOutcome>,
    dialog_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    let (title, set_title) = signal(initial.title.clone());
    let (content, set_content) = signal(initial.content.clone());
    let (color, set_color) = signal(initial.color.clone());
    let (due_at, set_due_at) = signal(initial.due_at.clone());
    let (labels, set_labels) = signal(initial.labels.clone());
    let (assignee_id, set_assignee_id) = signal(initial.assignee_id.clone());
    let (assignee_name, set_assignee_name) = signal(initial.assignee_name.clone());
    // Phase 4c — assignee picker results. Populated when the user
    // types 2+ chars into the assignee input. Clicking a row
    // writes assignee_id + assignee_name from the picked user.
    // Free-form typing without a click still saves whatever's in
    // assignee_name at Save time — assignee_id is only overwritten
    // by a real pick, so it stays empty until then and the model
    // records the name as an unresolved-user label.
    let (assignee_results, set_assignee_results) =
        signal::<Vec<crate::api::users::SearchResult>>(Vec::new());
    // Monotonic counter used to defeat race-conditions between
    // debounce timers: each keystroke increments; when a
    // timeout fires it only proceeds if its captured seq still
    // matches. Kills two failure modes: a stale timeout
    // clobbering fresh results, and a fresh timeout landing
    // AFTER the user has already cleared the input.
    let assignee_query_seq = leptos::prelude::RwSignal::new(0u64);

    let is_edit = matches!(initial.mode, KanbanCardModalMode::Edit { .. });
    let mode_for_delete = initial.mode.clone();
    let mode_for_save = initial.mode.clone();

    let close_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            state.set(None);
            on_outcome.run(KanbanCardOutcome::Cancel);
        }
    });
    let save_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            let out = KanbanCardModalState {
                mode: mode_for_save.clone(),
                title: title.get(),
                content: content.get(),
                color: color.get(),
                due_at: due_at.get(),
                labels: labels.get(),
                assignee_id: assignee_id.get(),
                assignee_name: assignee_name.get(),
            };
            // Reject Save with an empty title — the backend
            // validator rejects it anyway and a "silent no-op"
            // save from the modal is confusing.
            if out.title.trim().is_empty() {
                return;
            }
            state.set(None);
            on_outcome.run(KanbanCardOutcome::Save(out));
        }
    });
    let delete_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            if let KanbanCardModalMode::Edit { card_id } = mode_for_delete.clone() {
                state.set(None);
                on_outcome.run(KanbanCardOutcome::Delete { card_id });
            }
        }
    });

    let title_label = if is_edit {
        crate::t!("kanban-modal-edit-title")
    } else {
        crate::t!("kanban-modal-add-title")
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
                        // (users need to type newlines in the
                        // content field) and except when a button
                        // is focused (Enter should activate that
                        // button's own action, e.g. Cancel or a
                        // picker suggestion). Shift+Enter falls
                        // through to the default so users can
                        // insert a newline in the textarea via
                        // that combination.
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
                    <h3>{title_label}</h3>
                </div>
                <div class="calendar-modal-body">
                    <label class="calendar-modal-field">
                        <span>{crate::t!("kanban-modal-title-label")}</span>
                        <input
                            type="text"
                            required=true
                            // Match `MAX_CARD_TITLE_LEN` in
                            // crates/collab/src/blocks/kanban.rs so the
                            // client hits the same wall the paste/import
                            // validator uses. Server-side apply_update
                            // doesn't check this yet — client cap is a
                            // hardening layer, not the last line.
                            maxlength="120"
                            prop:value=move || title.get()
                            on:input=move |e| set_title.set(event_target_value(&e))
                            autofocus
                        />
                    </label>
                    <label class="calendar-modal-field">
                        <span>{crate::t!("kanban-modal-content-label")}</span>
                        <textarea
                            rows=3
                            maxlength="500"
                            prop:value=move || content.get()
                            on:input=move |e| set_content.set(event_target_value(&e))
                        ></textarea>
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
                                        class=move || if is_active.get() {
                                            "calendar-modal-color calendar-modal-color--active"
                                                .to_string()
                                        } else {
                                            "calendar-modal-color".to_string()
                                        }
                                        style=format!("background: {hex}")
                                        aria-label=name
                                        on:click=move |_| set_color.set(name.to_string())
                                    />
                                }
                            }).collect_view()}
                        </span>
                    </fieldset>
                    <label class="calendar-modal-field">
                        <span>{crate::t!("kanban-modal-due-label")}</span>
                        <input
                            type="date"
                            prop:value=move || due_at.get()
                            on:input=move |e| set_due_at.set(event_target_value(&e))
                        />
                    </label>
                    <label class="calendar-modal-field">
                        <span>{crate::t!("kanban-modal-labels-label")}</span>
                        <input
                            type="text"
                            placeholder="bug|red;ux|blue"
                            // Rough ceiling matching typical multi-tag
                            // lists — well above the paste/import
                            // validator's per-label cap and short enough
                            // to keep the DDB write reasonable.
                            maxlength="400"
                            prop:value=move || labels.get()
                            on:input=move |e| set_labels.set(event_target_value(&e))
                        />
                    </label>
                    <label class="calendar-modal-field kanban-modal-assignee-field">
                        <span>{crate::t!("kanban-modal-assignee-label")}</span>
                        <input
                            type="text"
                            placeholder="username"
                            maxlength="120"
                            prop:value=move || assignee_name.get()
                            on:input=move |e| {
                                let v = event_target_value(&e);
                                // Clear the resolved id so a free-form edit
                                // doesn't keep pointing at the previously
                                // picked user. Save-time fallback: id stays
                                // empty (name is stored as an unresolved
                                // label).
                                set_assignee_id.set(String::new());
                                set_assignee_name.set(v.clone());
                                if v.len() < 2 {
                                    set_assignee_results.set(Vec::new());
                                    // Bump the seq counter so any
                                    // still-queued keystroke's timeout
                                    // becomes a no-op when it fires.
                                    assignee_query_seq.update(|n| *n += 1);
                                    return;
                                }
                                // 250 ms debounce — the security audit
                                // flagged that per-keystroke DDB scans
                                // are a client-driven DoS. Sequence
                                // counter guards against a stale timeout
                                // clobbering a newer response.
                                let seq_now = {
                                    assignee_query_seq.update(|n| *n += 1);
                                    assignee_query_seq.get_untracked()
                                };
                                gloo_timers::callback::Timeout::new(250, move || {
                                    if assignee_query_seq.get_untracked() != seq_now {
                                        return;
                                    }
                                    leptos::task::spawn_local(async move {
                                        if let Ok(resp) =
                                            crate::api::users::search_users(&v).await
                                        {
                                            if assignee_query_seq.get_untracked() == seq_now {
                                                set_assignee_results.set(resp.users);
                                            }
                                        }
                                    });
                                }).forget();
                            }
                        />
                        <Show when=move || !assignee_results.get().is_empty()>
                            <div class="kanban-modal-user-picker">
                                {move || assignee_results.get()
                                    .into_iter()
                                    .take(6)
                                    .map(|u| {
                                        let uid = u.user_id.clone();
                                        let uname = u.name.clone();
                                        let email = u.email.clone();
                                        view! {
                                            <button
                                                type="button"
                                                class="kanban-modal-user-picker-item"
                                                on:click=move |_| {
                                                    set_assignee_id.set(uid.clone());
                                                    set_assignee_name.set(uname.clone());
                                                    set_assignee_results.set(Vec::new());
                                                }
                                            >
                                                <span class="kanban-modal-user-picker-name">
                                                    {u.name}
                                                </span>
                                                <span class="kanban-modal-user-picker-email">
                                                    {email}
                                                </span>
                                            </button>
                                        }
                                    })
                                    .collect_view()}
                            </div>
                        </Show>
                    </label>
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
        .or_else(|| {
            e.target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|el| el.value())
        })
        .unwrap_or_default()
}
