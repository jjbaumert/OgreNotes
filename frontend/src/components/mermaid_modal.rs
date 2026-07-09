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

/// Everything the modal needs to render + carry back to the
/// caller. Held in a `RwSignal<Option<MermaidModalState>>` by
/// `editor_component.rs`; `None` means the modal is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidModalState {
    pub block_id: String,
    pub source: String,
}

/// Everything the parent needs to route the modal's result.
#[derive(Debug, Clone)]
pub enum MermaidModalOutcome {
    Save { block_id: String, source: String },
    Cancel,
}

#[component]
pub fn MermaidModal(
    /// `Some` → open; `None` → hidden. Parent writes; modal reads.
    #[prop(into)] state: RwSignal<Option<MermaidModalState>>,
    on_outcome: Callback<MermaidModalOutcome>,
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
    on_outcome: Callback<MermaidModalOutcome>,
    dialog_ref: NodeRef<leptos::html::Div>,
) -> impl IntoView {
    // Working copy of the source, staged until Save.
    let (source, set_source) = signal(initial.source.clone());
    let block_id_for_save = initial.block_id.clone();

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
            on_outcome.run(MermaidModalOutcome::Cancel);
        }
    });
    let save_cb = Callback::new({
        let state = state;
        let on_outcome = on_outcome.clone();
        let block_id = block_id_for_save.clone();
        move |()| {
            let src = source.get();
            state.set(None);
            on_outcome.run(MermaidModalOutcome::Save {
                block_id: block_id.clone(),
                source: src,
            });
        }
    });

    // Live preview: rendered on each keystroke through the same
    // `ogrenotes_mermaid::render` pipeline the block view uses.
    // SVG → `inner_html` is trusted output from our own renderer
    // (source is XML-escaped internally); the error message is a
    // plain Leptos text node, so it's escaped automatically.
    let preview = move || {
        let src = source.get();
        let out = ogrenotes_mermaid::render(&src);
        match out.svg {
            Some(svg) => view! { <div class="mermaid-svg" inner_html=svg></div> }.into_any(),
            None => {
                let msg = out
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "diagram error".into());
                view! { <p class="mermaid-error">{msg}</p> }.into_any()
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
                    <h3>"Edit Diagram"</h3>
                </div>
                <div class="calendar-modal-body mermaid-modal-body">
                    <textarea
                        class="mermaid-source"
                        autofocus
                        prop:value=move || source.get()
                        on:input=move |e| set_source.set(event_target_value(&e))
                    ></textarea>
                    <div class="mermaid-preview">{preview}</div>
                </div>
                <div class="calendar-modal-actions">
                    <span class="calendar-modal-spacer"></span>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| a11y::defer_close(close_cb)
                    >
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-primary"
                        on:click=move |_| a11y::defer_close(save_cb)
                    >
                        "Save"
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
