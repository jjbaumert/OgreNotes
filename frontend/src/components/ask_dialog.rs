// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.2 piece B — Ask UI.
//!
//! Modal-style chat surface that consumes `crate::api::ask::ask_stream`
//! and renders the streaming SSE events. Single question, single
//! answer per open — the dialog resets on close. Multi-turn chat is
//! a v2 carry-forward.
//!
//! Reuses M-P8's focus-trap + role="dialog" + aria-modal pattern,
//! plus the existing `.search-*` CSS for visual continuity with the
//! command palette. Status messages render in an aria-live region
//! so screen readers announce the agent's tool-use progress.
//!
//! Source citations land in the `sources` signal as the `Source`
//! SSE events arrive; piece C polishes the rendering. For piece B
//! they appear inline at the bottom of the answer with a basic
//! link-to-doc affordance.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::a11y;
use crate::api::ask::{self, AskError, AskEvent};

#[derive(Clone, Debug, PartialEq)]
struct Citation {
    doc_id: String,
    title: String,
    doc_type: String,
}

/// Map the DocType wire string to a single emoji glyph used as the
/// citation's provider icon. Falls back to a document glyph for
/// unknown types — keeps the renderer forward-compatible when the
/// backend grows new doc types.
fn provider_icon(doc_type: &str) -> &'static str {
    match doc_type {
        "spreadsheet" => "\u{1F4CA}", // 📊
        "chat" => "\u{1F4AC}",        // 💬
        _ => "\u{1F4C4}",             // 📄
    }
}

#[component]
pub fn AskDialog(
    /// Controls visibility. Parent toggles this to open/close.
    #[prop(into)] visible: Signal<bool>,
    on_close: Callback<()>,
    /// #148 `@ask` flow — when Some, the dialog opens with the
    /// question pre-filled and auto-submits. Read on each open.
    #[prop(into, default = Signal::derive(|| None))]
    initial_prompt: Signal<Option<String>>,
    /// #148 `@ask` flow — when Some, an "Insert into document"
    /// button appears alongside Cancel. Clicking it fires the
    /// callback with the accumulated answer text; the parent
    /// wires it to `ToolbarCommand::InsertAiText`. Nil for the
    /// home / settings / palette-open call sites, which stay
    /// read-only.
    #[prop(default = None)] on_insert: Option<Callback<String>>,
    /// #148 v2 — Agent (default) vs Direct. Direct disables the
    /// backend tools loop so the assistant answers from the
    /// prompt alone — used by the directive wrappers whose
    /// composed prompt already carries the source doc/selection.
    #[prop(into, default = Signal::derive(|| crate::api::ask::AskMode::Agent))]
    ask_mode: Signal<crate::api::ask::AskMode>,
    /// #148 v2 — text appended to whatever the user submits from
    /// the input, invisible to the user. Used by directive
    /// wrappers to keep source-doc text out of the input field
    /// while still delivering it to the assistant. `None` for
    /// free-form Ask AI.
    #[prop(into, default = Signal::derive(|| None))]
    hidden_suffix: Signal<Option<String>>,
) -> impl IntoView {
    // Input + streaming state.
    let (question, set_question) = signal::<String>(String::new());
    let (status, set_status) = signal::<String>(String::new());
    let (answer, set_answer) = signal::<String>(String::new());
    let (sources, set_sources) = signal::<Vec<Citation>>(Vec::new());
    let (error, set_error) = signal::<Option<String>>(None);
    // True from submit until the Done/Error event closes the stream.
    let (loading, set_loading) = signal::<bool>(false);

    // Focus trap + restore on close. Same wiring every M-P8 modal
    // uses. The input below auto-focuses via Effect.
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible);

    let input_ref = NodeRef::<leptos::html::Input>::new();
    // Auto-focus the input on open + reset state on close.
    Effect::new(move |_| {
        if visible.get() {
            // Reset on each open so the dialog doesn't show a stale
            // answer from the previous session.
            let prefill = initial_prompt.get_untracked().unwrap_or_default();
            set_question.set(prefill.clone());
            set_status.set(String::new());
            set_answer.set(String::new());
            set_sources.set(Vec::new());
            set_error.set(None);
            set_loading.set(false);
            // Microtask-defer the focus call so the Show subtree has
            // mounted the input before we touch it — same pattern as
            // the search_dialog's autofocus.
            let el = input_ref;
            spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(0).await;
                if let Some(input) = el.get_untracked() {
                    let _ = input.focus();
                }
            });
        }
    });

    let submit = Callback::new(move |()| {
        let visible_q = question.get_untracked();
        let visible_q = visible_q.trim().to_string();
        if visible_q.is_empty() || loading.get_untracked() {
            return;
        }
        // Assemble the wire prompt: visible instruction + any
        // hidden suffix the caller supplied. The user only ever
        // sees / edits `visible_q`; the source-text-carrying
        // suffix rides invisibly.
        let q = match hidden_suffix.get_untracked() {
            Some(suffix) if !suffix.is_empty() => {
                format!("{visible_q}\n\n{suffix}")
            }
            _ => visible_q,
        };
        set_status.set(String::new());
        set_answer.set(String::new());
        set_sources.set(Vec::new());
        set_error.set(None);
        set_loading.set(true);

        spawn_local(async move {
            let on_event = move |ev: AskEvent| match ev {
                AskEvent::Status(msg) => set_status.set(msg),
                AskEvent::Text(chunk) => {
                    // The backend may emit multiple text events per
                    // answer (one per agent round in MAX_TOOL_ROUNDS);
                    // concatenate so the UI shows the full response.
                    set_answer.update(|s| s.push_str(&chunk));
                }
                AskEvent::Source { doc_id, title, doc_type } => {
                    set_sources.update(|v| {
                        // Deduplicate — the agent may cite the same
                        // doc more than once across tool rounds.
                        if !v.iter().any(|c| c.doc_id == doc_id) {
                            v.push(Citation { doc_id, title, doc_type });
                        }
                    });
                }
                AskEvent::Done => {
                    set_status.set(String::new());
                    set_loading.set(false);
                }
                AskEvent::Error(msg) => {
                    set_error.set(Some(msg));
                    set_status.set(String::new());
                    set_loading.set(false);
                }
            };

            let mode = ask_mode.get_untracked();
            match ask::ask_stream_with_mode(&q, mode, on_event).await {
                Ok(()) => {
                    // Stream closed cleanly; the Done event already
                    // flipped `loading` off. If we get here without
                    // Done (e.g. server hung up early), flip it now
                    // so the dialog isn't stuck spinning.
                    if loading.get_untracked() {
                        set_loading.set(false);
                        set_status.set(String::new());
                    }
                }
                Err(e) => {
                    set_loading.set(false);
                    set_status.set(String::new());
                    set_error.set(Some(match e {
                        AskError::Http(429, _) => {
                            crate::t!("ask-error-rate-limit")
                        }
                        AskError::Http(403, _) => {
                            crate::t!("ask-error-disabled")
                        }
                        AskError::Http(503, _) => {
                            crate::t!("ask-error-unavailable")
                        }
                        other => other.to_string(),
                    }));
                }
            }
        });
    });

    // #148 — the @-menu path (Ask AI + directive wrappers) opens
    // the dialog with the composed prompt pre-filled in the
    // input. The user reviews / edits it and submits with Enter,
    // matching the palette / settings / home-page call sites'
    // review-before-run posture. Composed prompts for the AI
    // wrappers can be very long (e.g. `@summarize` appends the
    // whole doc), so submitting only on the user's explicit
    // Enter avoids sending a wrong-shaped or unintended prompt.

    view! {
        <Show when=move || visible.get()>
            <div class="search-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="search-dialog ask-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="ask-dialog-title"
                    on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Escape" {
                            a11y::defer_close(on_close);
                            return;
                        }
                        if let Some(node) = dialog_ref.get() {
                            a11y::handle_tab_trap(&e, node.as_ref());
                        }
                    }
                >
                    // Header (sr-only title + visible badge).
                    <div class="ask-header">
                        <h3 id="ask-dialog-title" class="visually-hidden">
                            {crate::t!("ask-dialog-title")}
                        </h3>
                        <span class="ask-badge">{crate::t!("ask-badge")}</span>
                    </div>

                    <div class="search-input-wrapper">
                        <span class="search-icon" aria-hidden="true">"\u{2728}"</span>
                        <input
                            node_ref=input_ref
                            type="text"
                            class="search-input"
                            placeholder=crate::t!("ask-placeholder")
                            aria-label=crate::t!("ask-placeholder")
                            prop:value=move || question.get()
                            on:input=move |e| set_question.set(event_target_value(&e))
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if e.key() == "Enter" && !e.shift_key() {
                                    e.prevent_default();
                                    submit.run(());
                                }
                            }
                            disabled=move || loading.get()
                        />
                        <Show when=move || loading.get()>
                            <span class="ask-spinner" aria-hidden="true">"\u{27F3}"</span>
                        </Show>
                    </div>

                    <div class="ask-body">
                        // Status updates — aria-live=polite so AT
                        // announces tool-use progress without
                        // interrupting the answer reading.
                        <Show when=move || !status.get().is_empty()>
                            <div
                                class="ask-status"
                                role="status"
                                aria-live="polite"
                            >
                                {move || status.get()}
                            </div>
                        </Show>

                        // Error — assertive so the failure
                        // pre-empts whatever the agent was saying.
                        <Show when=move || error.get().is_some()>
                            <div class="ask-error" role="alert">
                                {move || error.get().unwrap_or_default()}
                            </div>
                        </Show>

                        // Answer text — the agent's final response.
                        // pre-wrap preserves the newlines the agent
                        // emits but doesn't render Markdown; piece C
                        // can layer that on if needed.
                        <Show when=move || !answer.get().is_empty()>
                            <div class="ask-answer">
                                {move || answer.get()}
                            </div>
                        </Show>

                        // Sources — flagged in piece B, polished in
                        // piece C. Each opens the doc in a new tab
                        // so the Q&A survives navigation.
                        <Show when=move || !sources.get().is_empty()>
                            <div class="ask-sources">
                                <h4 class="ask-sources-heading">
                                    {crate::t!("ask-sources-heading")}
                                </h4>
                                <ol class="ask-sources-list">
                                    {move || sources.get().into_iter().enumerate().map(|(i, c)| {
                                        let href = format!("/d/{}/doc", c.doc_id);
                                        let icon = provider_icon(&c.doc_type);
                                        let n = i + 1;
                                        view! {
                                            <li class="ask-source-item">
                                                <span class="ask-source-index" aria-hidden="true">
                                                    {format!("[{n}]")}
                                                </span>
                                                <span class="ask-source-icon" aria-hidden="true">
                                                    {icon}
                                                </span>
                                                <a
                                                    class="ask-source-link"
                                                    href=href
                                                    target="_blank"
                                                    rel="noopener"
                                                >{c.title}</a>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ol>
                            </div>
                        </Show>

                        // Empty-state hint shown before the user
                        // sends the first question.
                        <Show when=move ||
                            answer.get().is_empty()
                                && status.get().is_empty()
                                && error.get().is_none()
                                && !loading.get()
                        >
                            <div class="ask-hint">
                                {crate::t!("ask-empty-hint")}
                            </div>
                        </Show>

                        // #148 `@ask` — Insert / Cancel button row.
                        // Only rendered when the parent supplied an
                        // `on_insert` callback (the @-menu path).
                        // Home / settings mount the dialog for
                        // read-only chat and skip this row.
                        <Show when=move ||
                            on_insert.is_some()
                                && !answer.get().is_empty()
                                && !loading.get()
                        >
                            <div class="ask-insert-actions">
                                <button
                                    type="button"
                                    class="btn btn-secondary"
                                    on:click=move |_| a11y::defer_close(on_close)
                                >
                                    {crate::t!("common-cancel")}
                                </button>
                                <button
                                    type="button"
                                    class="btn btn-primary"
                                    on:click=move |_| {
                                        if let Some(cb) = on_insert {
                                            cb.run(answer.get_untracked());
                                        }
                                        a11y::defer_close(on_close);
                                    }
                                >
                                    {crate::t!("ask-insert-into-document")}
                                </button>
                            </div>
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }
}
