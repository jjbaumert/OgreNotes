// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Render a `RichBlock` from a history-version diff the way the editor
//! would render it. Mirrors the structural choices in
//! `editor/view.rs::render_node`: paragraphs as `<p>`, headings as
//! `<h1>`/`<h2>`/`<h3>`, lists as `<ul>`/`<ol>`, list items as `<li>`,
//! blockquotes as `<blockquote>`, code blocks as `<pre><code>` (no
//! marks), tables as `<table>` with `<tr>`/`<td>`. Inline marks (bold,
//! italic, code, strikethrough, underline, links) wrap text the same way
//! the editor wraps them. Images render as a small placeholder.

use leptos::prelude::*;

use crate::api::history::{InlineRun, Mark, RichBlock};
use crate::editor::view::is_safe_color;

/// Render a single `RichBlock` as a Leptos view tree.
#[component]
pub fn DiffBlockView(block: RichBlock) -> impl IntoView {
    render_block(block)
}

fn render_block(block: RichBlock) -> AnyView {
    match block.node_type.as_str() {
        "paragraph" => view! {
            <p class="diff-block">{render_inline(block.inline)}</p>
        }
        .into_any(),
        "heading" => {
            let level = block
                .attrs
                .get("level")
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 3);
            let inline = render_inline(block.inline);
            match level {
                1 => view! { <h1 class="diff-block">{inline}</h1> }.into_any(),
                2 => view! { <h2 class="diff-block">{inline}</h2> }.into_any(),
                _ => view! { <h3 class="diff-block">{inline}</h3> }.into_any(),
            }
        }
        "bullet_list" => view! {
            <ul class="diff-block">{render_children(block.children)}</ul>
        }
        .into_any(),
        "ordered_list" => view! {
            <ol class="diff-block">{render_children(block.children)}</ol>
        }
        .into_any(),
        "list_item" => view! {
            <li class="diff-block-item">{render_children(block.children)}</li>
        }
        .into_any(),
        "task_list" => view! {
            <ul class="diff-block diff-task-list">{render_children(block.children)}</ul>
        }
        .into_any(),
        "task_item" => {
            let checked = block.attrs.get("checked").map(|s| s == "true").unwrap_or(false);
            let mark = if checked { "[x]" } else { "[ ]" };
            view! {
                <li class="diff-block-item diff-task-item">
                    <span class="diff-task-mark">{mark}</span>
                    {render_children(block.children)}
                </li>
            }
            .into_any()
        }
        "blockquote" => view! {
            <blockquote class="diff-block">{render_children(block.children)}</blockquote>
        }
        .into_any(),
        "code_block" => {
            // Code blocks don't carry inline marks. Concatenate the run
            // text (the diff walker still emits one run per text chunk
            // even when there are no marks).
            let text: String = block.inline.iter().map(|r| r.text.clone()).collect();
            let lang = block.attrs.get("language").cloned().unwrap_or_default();
            let class = if lang.is_empty() {
                "diff-block-code".to_string()
            } else {
                format!("diff-block-code language-{lang}")
            };
            view! {
                <pre class="diff-block">
                    <code class=class>{text}</code>
                </pre>
            }
            .into_any()
        }
        "horizontal_rule" => view! { <hr class="diff-block" /> }.into_any(),
        "image" => {
            let alt = block.attrs.get("alt").cloned().unwrap_or_default();
            let label = if alt.is_empty() {
                "[image]".to_string()
            } else {
                format!("[image: {alt}]")
            };
            view! { <span class="diff-image-placeholder">{label}</span> }.into_any()
        }
        "table" => view! {
            <table class="diff-block diff-table">
                <tbody>{render_children(block.children)}</tbody>
            </table>
        }
        .into_any(),
        "table_row" => view! {
            <tr class="diff-block-row">{render_children(block.children)}</tr>
        }
        .into_any(),
        "table_cell" => view! {
            <td class="diff-block-cell">{render_children(block.children)}</td>
        }
        .into_any(),
        "table_header" => view! {
            <th class="diff-block-cell diff-block-cell-header">
                {render_children(block.children)}
            </th>
        }
        .into_any(),
        _ => {
            // Unknown node type: render whatever we can — children first,
            // then any inline runs — inside a generic container.
            view! {
                <div class="diff-block diff-block-unknown">
                    {render_children(block.children)}
                    {render_inline(block.inline)}
                </div>
            }
            .into_any()
        }
    }
}

fn render_children(children: Vec<RichBlock>) -> Vec<AnyView> {
    children.into_iter().map(render_block).collect()
}

fn render_inline(runs: Vec<InlineRun>) -> Vec<AnyView> {
    runs.into_iter().map(render_run).collect()
}

/// Wrap a text run in mark elements in canonical outermost-to-innermost
/// order (matching `editor/view.rs::create_mark_element`):
///   Link → Bold → Italic → Underline → Strike → Code → TextColor → Highlight
/// Links render as underlined text only — the diff modal deliberately
/// drops the `href` (the user said link rendering as plain underline is
/// fine, and the modal is read-only so live navigation isn't useful).
fn render_run(run: InlineRun) -> AnyView {
    // Sort marks into the canonical wrapping order so two runs with the
    // same logical formatting always produce the same DOM nesting.
    let mut marks = run.marks.clone();
    marks.sort_by_key(mark_priority);

    let mut view: AnyView = view! { <span>{run.text}</span> }.into_any();
    // Apply marks innermost-first by walking the sorted list in reverse.
    for mark in marks.into_iter().rev() {
        view = wrap_with_mark(view, mark);
    }
    view
}

fn mark_priority(mark: &Mark) -> u8 {
    match mark {
        Mark::Link { .. } => 0,
        Mark::Bold => 1,
        Mark::Italic => 2,
        Mark::Underline => 3,
        Mark::Strike => 4,
        Mark::Code => 5,
        Mark::TextColor { .. } => 6,
        Mark::Highlight { .. } => 7,
        Mark::Subscript => 8,
        Mark::Superscript => 9,
    }
}

fn wrap_with_mark(inner: AnyView, mark: Mark) -> AnyView {
    match mark {
        Mark::Bold => view! { <strong>{inner}</strong> }.into_any(),
        Mark::Italic => view! { <em>{inner}</em> }.into_any(),
        Mark::Underline => view! { <u>{inner}</u> }.into_any(),
        Mark::Strike => view! { <s>{inner}</s> }.into_any(),
        Mark::Code => view! { <code>{inner}</code> }.into_any(),
        Mark::Subscript => view! { <sub>{inner}</sub> }.into_any(),
        Mark::Superscript => view! { <sup>{inner}</sup> }.into_any(),
        Mark::Link { .. } => view! { <u class="diff-link">{inner}</u> }.into_any(),
        Mark::TextColor { color } => {
            // Defense in depth — the CRDT can hold arbitrary strings, so
            // sanitize before splicing into a style attribute. Same
            // allow-list as the live editor (editor/view.rs::is_safe_color).
            if is_safe_color(&color) {
                view! { <span style=format!("color:{color}")>{inner}</span> }.into_any()
            } else {
                inner
            }
        }
        Mark::Highlight { color } => {
            if is_safe_color(&color) {
                view! { <span style=format!("background-color:{color}")>{inner}</span> }
                    .into_any()
            } else {
                inner
            }
        }
    }
}
