// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #141: Document Details panel. A read-only dialog surfacing metadata the
//! backend already returns (type, created/last-modified timestamps) plus a
//! client-side word/character count derived from the live editor model.
//!
//! Owner is intentionally omitted: `DocumentResponse` doesn't carry it, and
//! adding it is a deliberate DTO change tracked separately from this panel.

use leptos::prelude::*;

use crate::a11y;
use crate::editor::model::Node;
use crate::editor::state::EditorState;
use crate::i18n::{format_date, format_number, DateStyle};

/// Collect the text of each *textblock* separately so word counting never
/// joins the last word of one block to the first of the next (a plain
/// `text_content()` over the whole doc would).
fn collect_block_texts(node: &Node, out: &mut Vec<String>) {
    if node.node_type().map(|t| t.is_textblock()).unwrap_or(false) {
        out.push(node.text_content());
        return;
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_block_texts(child, out);
        }
    }
}

#[component]
pub fn DocumentDetailsDialog(
    #[prop(into)] visible: Signal<bool>,
    #[prop(into)] title: Signal<String>,
    #[prop(into)] doc_type: Signal<String>,
    #[prop(into)] created_at: Signal<i64>,
    #[prop(into)] updated_at: Signal<i64>,
    #[prop(into)] editor_state: Signal<Option<EditorState>>,
    on_close: Callback<()>,
) -> impl IntoView {
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible);

    // (words, characters). Recomputes only while the panel is open.
    let counts = Memo::new(move |_| {
        if !visible.get() {
            return (0usize, 0usize);
        }
        editor_state
            .get()
            .map(|s| {
                let mut texts = Vec::new();
                collect_block_texts(&s.doc, &mut texts);
                let words = texts.join("\n").split_whitespace().count();
                let chars: usize = texts.iter().map(|t| t.chars().count()).sum();
                (words, chars)
            })
            .unwrap_or((0, 0))
    });

    let type_label = move || {
        if doc_type.get() == "spreadsheet" {
            crate::t!("doc-type-spreadsheet")
        } else {
            crate::t!("doc-type-document")
        }
    };

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="confirm-dialog doc-details-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="doc-details-title"
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
                    <div class="confirm-header">
                        <h3 id="doc-details-title">{crate::t!("document-details-title")}</h3>
                    </div>
                    <div class="confirm-body">
                        <dl class="doc-details-list">
                            <dt>{crate::t!("document-details-name")}</dt>
                            <dd>{move || title.get()}</dd>

                            <dt>{crate::t!("document-details-type")}</dt>
                            <dd>{type_label}</dd>

                            <dt>{crate::t!("document-details-created")}</dt>
                            <dd>{move || format_date(created_at.get(), DateStyle::Long)}</dd>

                            <dt>{crate::t!("document-details-modified")}</dt>
                            <dd>{move || format_date(updated_at.get(), DateStyle::Long)}</dd>

                            <dt>{crate::t!("document-details-words")}</dt>
                            <dd>{move || format_number(counts.get().0 as f64)}</dd>

                            <dt>{crate::t!("document-details-characters")}</dt>
                            <dd>{move || format_number(counts.get().1 as f64)}</dd>
                        </dl>
                    </div>
                    <div class="confirm-actions">
                        <button
                            class="btn btn-primary"
                            on:click=move |_| a11y::defer_close(on_close)
                        >
                            {crate::t!("common-close")}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
