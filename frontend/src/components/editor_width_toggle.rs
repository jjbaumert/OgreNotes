// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Segmented S/M/L control for the editor content width.

use leptos::prelude::*;

use crate::editor_width::WidthMode;

/// Segmented S/M/L control for the editor content width.
/// Visible glyphs stay literal; the accessible names are internationalized.
#[component]
pub fn EditorWidthToggle(
    #[prop(into)] mode: Signal<WidthMode>,
    on_select: Callback<WidthMode>,
) -> impl IntoView {
    view! {
        <div
            class="editor-width-modes"
            role="group"
            aria-label=crate::t!("editor-width-group")
        >
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Narrow
                title=crate::t!("editor-width-narrow")
                aria-label=crate::t!("editor-width-narrow")
                aria-pressed=move || (mode.get() == WidthMode::Narrow).to_string()
                on:click=move |_| on_select.run(WidthMode::Narrow)
            >
                "S"
            </button>
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Medium
                title=crate::t!("editor-width-medium")
                aria-label=crate::t!("editor-width-medium")
                aria-pressed=move || (mode.get() == WidthMode::Medium).to_string()
                on:click=move |_| on_select.run(WidthMode::Medium)
            >
                "M"
            </button>
            <button
                type="button"
                class="editor-width-mode-btn"
                class:selected=move || mode.get() == WidthMode::Wide
                title=crate::t!("editor-width-wide")
                aria-label=crate::t!("editor-width-wide")
                aria-pressed=move || (mode.get() == WidthMode::Wide).to_string()
                on:click=move |_| on_select.run(WidthMode::Wide)
            >
                "L"
            </button>
        </div>
    }
}
