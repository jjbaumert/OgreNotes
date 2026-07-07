// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::a11y;

/// Styled confirmation dialog. Replaces `window.confirm()` for destructive
/// actions (delete-to-trash, delete-forever). Parent owns the `visible`
/// signal; confirm and cancel callbacks flip it back to false.
#[component]
pub fn ConfirmDialog(
    /// Controls visibility. Parent toggles this to open/close.
    #[prop(into)] visible: Signal<bool>,
    /// Header title (e.g. "Move to Trash").
    #[prop(into)] title: String,
    /// Body text shown to the user.
    #[prop(into)] message: String,
    /// Label for the primary action button.
    #[prop(into)] confirm_label: String,
    /// When true, the confirm button gets the destructive (red) styling.
    #[prop(default = false)] destructive: bool,
    on_confirm: Callback<()>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let confirm_class = if destructive {
        "btn btn-danger"
    } else {
        "btn btn-primary"
    };

    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible);

    view! {
        <Show when=move || visible.get()>
            <div
                class="confirm-backdrop"
                // Defer like the Escape path below: running the callback
                // synchronously flips the parent's `visible` to false on
                // this same click, tearing the <Show> down mid-event —
                // Leptos drops the dialog's closures and re-invokes one,
                // the "closure invoked recursively or after being dropped"
                // panic (reproduced in Firefox on delete-from-menu). One
                // microtask lets the click settle before teardown.
                on:click=move |_| a11y::defer_close(on_cancel)
            >
                <div
                    node_ref=dialog_ref
                    class="confirm-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="confirm-dialog-title"
                    on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Escape" {
                            a11y::defer_close(on_cancel);
                            return;
                        }
                        if let Some(node) = dialog_ref.get() {
                            a11y::handle_tab_trap(&e, node.as_ref());
                        }
                    }
                >
                    <div class="confirm-header">
                        <h3 id="confirm-dialog-title">{title.clone()}</h3>
                    </div>
                    <div class="confirm-body">
                        <p>{message.clone()}</p>
                    </div>
                    <div class="confirm-actions">
                        <button
                            class="btn btn-secondary"
                            // Deferred — see the backdrop handler above (#90 class).
                            on:click=move |_| a11y::defer_close(on_cancel)
                        >
                            {crate::t!("common-cancel")}
                        </button>
                        <button
                            class=confirm_class
                            // Deferred — the confirm path flips `visible`
                            // false (and often navigates away) on this
                            // click; defer so the <Show> teardown happens
                            // after the event settles, not during it.
                            on:click=move |_| a11y::defer_close(on_confirm)
                        >
                            {confirm_label.clone()}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
