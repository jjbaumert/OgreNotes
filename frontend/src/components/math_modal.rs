// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Equation edit modal: a LaTeX textarea with a live MathML preview.
//!
//! Opens on click of a `.math-block`'s `[data-math-action="edit"]`
//! hook (delegated listener in `editor_component.rs`, which seeds the
//! modal from the block's `data-source` attribute). Same defer-close
//! pattern as `mermaid_modal`: every close path routes through
//! `a11y::defer_close` so a still-bubbling event never re-enters a
//! dropped closure.

use leptos::prelude::*;

use crate::a11y;

/// Everything the modal needs to render and carry back to the caller.
/// `None` in the parent's signal means the modal is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathModalState {
    pub block_id: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub enum MathModalOutcome {
    Save { block_id: String, source: String },
    Cancel,
}

/// Why Save is blocked, mirroring the server's write gate in
/// `crates/collab/src/blocks/math.rs::validate_attrs` so the modal never
/// dispatches an update the server would reject (which would lose the
/// edit on refresh). A source that doesn't parse *can* be saved: it
/// renders its error in place, like a Mermaid diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveBlockedReason {
    Empty,
    TooLong,
}

pub fn save_blocked(source: &str) -> Option<SaveBlockedReason> {
    if source.trim().is_empty() {
        Some(SaveBlockedReason::Empty)
    } else if source.chars().count() > ogrenotes_math::MAX_SOURCE_LEN {
        Some(SaveBlockedReason::TooLong)
    } else {
        None
    }
}

#[component]
pub fn MathModal(
    /// `Some` → open; `None` → hidden. Parent writes; modal reads.
    #[prop(into)] state: RwSignal<Option<MathModalState>>,
    on_outcome: Callback<MathModalOutcome>,
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
    initial: MathModalState,
    state: RwSignal<Option<MathModalState>>,
    on_outcome: Callback<MathModalOutcome>,
    dialog_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    // Working copy of the source, staged until Save.
    let (source, set_source) = signal(initial.source.clone());
    let block_id = initial.block_id.clone();

    let close_cb = Callback::new({
        let on_outcome = on_outcome.clone();
        move |()| {
            state.set(None);
            on_outcome.run(MathModalOutcome::Cancel);
        }
    });
    let save_cb = Callback::new({
        let on_outcome = on_outcome.clone();
        move |()| {
            let src = source.get();
            // Second guard behind the disabled Save button.
            if save_blocked(&src).is_some() {
                return;
            }
            state.set(None);
            on_outcome.run(MathModalOutcome::Save { block_id: block_id.clone(), source: src });
        }
    });
    let blocked_reason = Signal::derive(move || save_blocked(&source.get()));

    // Live preview through the same renderer the block view uses. An
    // equation renders in microseconds, so no debounce. MathML →
    // `inner_html` is trusted output of our renderer; the error is a
    // Leptos text node, escaped automatically.
    let preview = move || {
        let src = source.get();
        if src.trim().is_empty() {
            return ().into_any();
        }
        match ogrenotes_math::to_mathml(&src, ogrenotes_math::Display::Block) {
            Ok(mathml) => view! { <div class="math-render" inner_html=mathml></div> }.into_any(),
            Err(e) => view! { <p class="math-error">{e.to_string()}</p> }.into_any(),
        }
    };

    view! {
        <div
            class="confirm-backdrop"
            on:click=move |_| a11y::defer_close(close_cb)
        >
            <div
                node_ref=dialog_ref
                class="calendar-modal math-modal"
                role="dialog"
                aria-modal="true"
                aria-labelledby="math-modal-title"
                on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                on:keydown=move |e: web_sys::KeyboardEvent| {
                    // Escape closes; Ctrl/Cmd+Enter saves. Plain Enter
                    // inserts a newline (multi-line environments).
                    if e.key() == "Escape" {
                        a11y::defer_close(close_cb);
                    } else if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
                        e.prevent_default();
                        a11y::defer_close(save_cb);
                    }
                }
            >
                <div class="confirm-header">
                    <h3 id="math-modal-title">{crate::t!("math-modal-title")}</h3>
                </div>
                <div class="calendar-modal-body math-modal-body">
                    <textarea
                        class="math-source"
                        autofocus
                        spellcheck="false"
                        aria-label=crate::t!("math-modal-source-label")
                        prop:value=move || source.get()
                        on:input=move |e| set_source.set(event_target_value(&e))
                    ></textarea>
                    <div class="math-preview" aria-live="polite">{preview}</div>
                </div>
                <div class="calendar-modal-actions">
                    {move || {
                        blocked_reason.get().map(|reason| {
                            let msg = match reason {
                                SaveBlockedReason::Empty => crate::t!("math-modal-error-empty"),
                                SaveBlockedReason::TooLong => crate::t!(
                                    "math-modal-error-too-long",
                                    max = ogrenotes_math::MAX_SOURCE_LEN as i64
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
                        {crate::t!("math-modal-save")}
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

    #[test]
    fn save_blocked_mirrors_the_write_gate() {
        assert_eq!(save_blocked("x^2"), None);
        // Unparseable is saveable: it renders its error in place.
        assert_eq!(save_blocked("\\foo{"), None);
        assert_eq!(save_blocked(""), Some(SaveBlockedReason::Empty));
        assert_eq!(save_blocked(" \n\t"), Some(SaveBlockedReason::Empty));
        let at_cap = "x".repeat(ogrenotes_math::MAX_SOURCE_LEN);
        assert_eq!(save_blocked(&at_cap), None);
        let over = "x".repeat(ogrenotes_math::MAX_SOURCE_LEN + 1);
        assert_eq!(save_blocked(&over), Some(SaveBlockedReason::TooLong));
    }
}
