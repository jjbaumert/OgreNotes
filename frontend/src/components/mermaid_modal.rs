// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mermaid diagram edit modal: a source textarea with a live SVG
//! preview.
//!
//! Opens on click of a `.mermaid-block`'s
//! `[data-mermaid-action="edit"]` hook; the delegated click
//! listener lives in `editor_component.rs`, which reads the
//! block's current `source` off the DOM (a `data-source` attribute
//! stamped by `MermaidView::render`, see
//! `editor/blocks/mermaid.rs`) to seed the modal.
//!
//! Same defer-close pattern as `calendar_modal` / `kanban_card_modal`
//! to guard against the Firefox "closure invoked recursively or
//! after being dropped" panic: every close path (backdrop click,
//! Escape, Cancel, Save) routes through `a11y::defer_close` rather
//! than flipping `state` synchronously inside the triggering event
//! handler.

use leptos::prelude::*;

use crate::a11y;
use crate::editor::commands::MermaidUpdateError;

/// Pause in typing before the live preview re-renders.
const PREVIEW_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Everything the modal needs to render + carry back to the
/// caller. Held in a `RwSignal<Option<MermaidModalState>>` by
/// `editor_component.rs`; `None` means the modal is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidModalState {
    pub block_id: String,
    pub source: String,
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
pub(crate) fn mermaid_click_outcome(ev: &web_sys::MouseEvent) -> Option<MermaidModalState> {
    use wasm_bindgen::JsCast;
    let target = ev.target()?.dyn_into::<web_sys::Element>().ok()?;
    let action_el = target.closest("[data-mermaid-action]").ok()??;
    let block_el = action_el.closest(".mermaid-block").ok()??;
    let block_id = block_el.get_attribute("data-block-id")?;
    let source = block_el.get_attribute("data-source").unwrap_or_default();
    Some(MermaidModalState { block_id, source })
}

/// Everything the parent needs to route the modal's result.
#[derive(Debug, Clone)]
pub enum MermaidModalOutcome {
    Save {
        block_id: String,
        original_source: String,
        source: String,
    },
    Cancel,
}

/// Why Save is blocked for a given draft, mirroring the server-side gate
/// in `crates/collab/src/blocks/mermaid.rs::validate_attrs` so the modal
/// never dispatches a WS update the server is guaranteed to reject (which
/// would otherwise silently diverge client/server state and lose the
/// user's edit on refresh). `None` means the draft is save-able.
///
/// Pure and DOM-free so it can be unit-tested natively (no wasm target
/// needed) — see the `save_blocked` tests below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveBlockedReason {
    Empty,
    TooLong,
}

/// Mirrors `crates/collab/src/blocks/mermaid.rs::validate_attrs`: empty
/// (or whitespace-only) source, or source over
/// `ogrenotes_mermaid::MAX_SOURCE_LEN` chars, both hard-fail the server's
/// write gate.
pub fn save_blocked(source: &str) -> Option<SaveBlockedReason> {
    if source.trim().is_empty() {
        Some(SaveBlockedReason::Empty)
    } else if source.chars().count() > ogrenotes_mermaid::MAX_SOURCE_LEN {
        Some(SaveBlockedReason::TooLong)
    } else {
        None
    }
}

#[component]
pub fn MermaidModal(
    /// `Some` → open; `None` → hidden. Parent writes; modal reads.
    #[prop(into)] state: RwSignal<Option<MermaidModalState>>,
    on_outcome: Callback<MermaidModalOutcome, Result<(), MermaidUpdateError>>,
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
    initial: MermaidModalState,
    state: RwSignal<Option<MermaidModalState>>,
    on_outcome: Callback<MermaidModalOutcome, Result<(), MermaidUpdateError>>,
    dialog_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    // Working copy of the source, staged until Save.
    let (source, set_source) = signal(initial.source.clone());
    // What the preview shows: the source once typing pauses. Rendering
    // a large diagram per keystroke on the main thread made the
    // textarea lag (the design spec calls for a debounced preview).
    let (preview_source, set_preview_source) = signal(initial.source.clone());
    let pending_preview = StoredValue::new(None::<TimeoutHandle>);
    on_cleanup(move || {
        if let Some(Some(h)) = pending_preview.try_get_value() {
            h.clear();
        }
    });
    let block_id_for_save = initial.block_id.clone();
    let original_source = initial.source.clone();
    let save_error = RwSignal::new(None::<MermaidUpdateError>);

    // Every close path flips `state.set(None)`, which collapses
    // the outer `<Show>` on the same reactive turn and drops the
    // wasm-bindgen closures on the modal's inner divs. If we ran
    // synchronously, the still-bubbling click/keydown would then
    // re-enter one of those dropped closures — the modal-close
    // panic every other modal in the app guards against via
    // `a11y::defer_close`. Route Cancel / Save through the same
    // deferral.
    let close_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        move |()| {
            state.set(None);
            let _ = on_outcome.run(MermaidModalOutcome::Cancel);
        }
    });
    let save_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        let block_id = block_id_for_save.clone();
        move |()| {
            let src = source.get_untracked();
            // Second guard behind the disabled Save button: mirror the
            // server's hard gate (empty / over MAX_SOURCE_LEN) so a
            // dispatched Save can never be rejected by the write gate —
            // that divergence would lose the user's edit on refresh.
            if save_blocked(&src).is_some() {
                return;
            }
            let result = on_outcome.run(MermaidModalOutcome::Save {
                block_id: block_id.clone(),
                original_source: original_source.clone(),
                source: src,
            });
            match result {
                Ok(()) => {
                    state.set(None);
                    let block_id = block_id.clone();
                    // Saving replaces the block DOM, including the opening button.
                    // Let teardown and the focus trap settle before finding its replacement.
                    leptos::task::spawn_local(async move {
                        gloo_timers::future::TimeoutFuture::new(0).await;
                        gloo_timers::future::TimeoutFuture::new(0).await;
                        use wasm_bindgen::JsCast;
                        if let Some(document) =
                            web_sys::window().and_then(|window| window.document())
                        {
                            if let Ok(blocks) = document.query_selector_all(".mermaid-block") {
                                for index in 0..blocks.length() {
                                    let Some(block) = blocks
                                        .item(index)
                                        .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
                                    else {
                                        continue;
                                    };
                                    if block.get_attribute("data-block-id").as_deref()
                                        == Some(block_id.as_str())
                                    {
                                        if let Some(button) = block
                                            .query_selector(".mermaid-edit-control")
                                            .ok()
                                            .flatten()
                                            .and_then(|element| {
                                                element.dyn_into::<web_sys::HtmlElement>().ok()
                                            })
                                        {
                                            let _ = button.focus();
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    });
                }
                Err(error) => save_error.set(Some(error)),
            }
        }
    });
    let blocked_reason = Signal::derive(move || save_blocked(&source.get()));

    // Live preview: rendered once typing pauses, through the same
    // `ogrenotes_mermaid::render` pipeline the block view uses.
    // SVG → `inner_html` is trusted output from our own renderer
    // (source is XML-escaped internally); the error message is a
    // plain Leptos text node, so it's escaped automatically.
    let preview = move || {
        let src = preview_source.get();
        let out = ogrenotes_mermaid::render(&src);
        match out.svg {
            Some(svg) => view! { <div class="mermaid-svg" inner_html=svg></div> }.into_any(),
            None => {
                let msg = out
                    .error
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "diagram error".into());
                view! { <p class="mermaid-error" role="status">{msg}</p> }.into_any()
            }
        }
    };

    view! {
        <div
            class="confirm-backdrop"
            on:click=move |_| a11y::defer_close(close_cb)
        >
            <div
                node_ref=dialog_ref
                class="calendar-modal mermaid-modal"
                role="dialog"
                aria-modal="true"
                aria-labelledby="mermaid-modal-title"
                on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                on:keydown=move |e: web_sys::KeyboardEvent| {
                    // Escape closes. Enter is deliberately NOT
                    // wired to Save here (unlike calendar/kanban) —
                    // the textarea IS the diagram source, so every
                    // Enter keystroke must insert a newline rather
                    // than submit.
                    if e.key() == "Escape" {
                        a11y::defer_close(close_cb);
                    }
                }
            >
                <div class="confirm-header">
                    <h3 id="mermaid-modal-title">{crate::t!("mermaid-modal-title")}</h3>
                </div>
                <div class="calendar-modal-body mermaid-modal-body">
                    <textarea
                        class="mermaid-source"
                        aria-label=crate::t!("mermaid-modal-source-label")
                        data-autofocus="true"
                        prop:value=move || source.get()
                        on:input=move |e| {
                            let value = event_target_value(&e);
                            set_source.set(value.clone());
                            if let Some(h) = pending_preview.get_value() {
                                h.clear();
                            }
                            let h = set_timeout_with_handle(
                                move || {
                                    let _ = set_preview_source.try_set(value);
                                },
                                PREVIEW_DEBOUNCE,
                            )
                            .ok();
                            pending_preview.set_value(h);
                        }
                    ></textarea>
                    <div class="mermaid-preview">{preview}</div>
                </div>
                <div class="calendar-modal-actions">
                    {move || save_error.get().map(|error| {
                        let message = match error {
                            MermaidUpdateError::Changed => crate::t!("mermaid-modal-error-changed"),
                            MermaidUpdateError::Unavailable => crate::t!("mermaid-modal-error-unavailable"),
                        };
                        view! { <span class="mermaid-save-conflict" role="alert">{message}</span> }
                    })}
                    {move || {
                        blocked_reason.get().map(|reason| {
                            let msg = match reason {
                                SaveBlockedReason::Empty => crate::t!("mermaid-modal-error-empty"),
                                SaveBlockedReason::TooLong => crate::t!(
                                    "mermaid-modal-error-too-long",
                                    max = ogrenotes_mermaid::MAX_SOURCE_LEN as i64
                                ),
                            };
                            view! { <span class="mermaid-modal-error" role="alert">{msg}</span> }.into_any()
                        })
                    }}
                    <span class="calendar-modal-spacer"></span>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| a11y::defer_close(close_cb)
                    >
                        {crate::t!("common-cancel")}
                    </button>
                    <button
                        class="btn btn-primary"
                        prop:disabled=move || blocked_reason.get().is_some()
                        on:click=move |_| a11y::defer_close(save_cb)
                    >
                        {crate::t!("mermaid-modal-save")}
                    </button>
                </div>
            </div>
        </div>
    }
}

fn event_target_value(e: &web_sys::Event) -> String {
    use wasm_bindgen::JsCast;
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure, DOM-free: mirrors `crates/collab/src/blocks/mermaid.rs`'s
    // `validate_attrs` gate so Save never dispatches a WS update the
    // server is guaranteed to reject.

    #[test]
    fn save_blocked_none_for_valid_source() {
        assert_eq!(save_blocked("pie\n\"A\" : 1"), None);
    }

    #[test]
    fn save_blocked_empty_for_blank_source() {
        assert_eq!(save_blocked(""), Some(SaveBlockedReason::Empty));
    }

    #[test]
    fn save_blocked_empty_for_whitespace_only_source() {
        assert_eq!(save_blocked("   \n\t  "), Some(SaveBlockedReason::Empty));
    }

    #[test]
    fn save_blocked_none_at_exactly_max_len() {
        let src = "x".repeat(ogrenotes_mermaid::MAX_SOURCE_LEN);
        assert_eq!(save_blocked(&src), None);
    }

    #[test]
    fn save_blocked_too_long_over_max_len() {
        let src = "x".repeat(ogrenotes_mermaid::MAX_SOURCE_LEN + 1);
        assert_eq!(save_blocked(&src), Some(SaveBlockedReason::TooLong));
    }
}
