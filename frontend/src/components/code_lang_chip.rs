// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Language-selector chip for code blocks: a small native `<select>`
//! pinned to the top-right of the code block that contains the
//! caret. Rendered OUTSIDE the contenteditable tree (positioned
//! sibling overlay), so the editor's DOM↔model position walkers
//! never see it and `render()`'s full rebuild never destroys it.
//!
//! A native `<select>` keeps this keyboard-accessible for free and
//! avoids bespoke popover/focus code.

use leptos::prelude::*;

use ogrenotes_highlight::Language;

/// `Some` → visible at (top, right) within the editor overlay;
/// `None` → hidden. `current` is the block's raw `language` attr
/// ("" = plain text; may be an unsupported tag like "mermaid").
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLangChipState {
    pub top: f64,
    pub right: f64,
    pub current: String,
}

#[component]
pub fn CodeLangChip(
    #[prop(into)] state: RwSignal<Option<CodeLangChipState>>,
    /// Fires with the canonical tag of the chosen language
    /// ("" for Plain text).
    on_select: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || state.get().is_some()>
            {move || state.get().map(|s| {
                let current = s.current.clone();
                let known = Language::from_tag(&current).is_some();
                view! {
                    <div
                        class="code-lang-chip"
                        style=format!("top:{}px;right:{}px;", s.top, s.right)
                    >
                        <select
                            aria-label="Code block language"
                            on:change=move |ev| {
                                on_select.run(event_target_value(&ev));
                            }
                        >
                            <option value="" selected=current.is_empty()>
                                "Plain text"
                            </option>
                            // Unsupported tag (e.g. markdown-imported
                            // "mermaid"): show it, unhighlighted, so the
                            // user sees what's set rather than a lie.
                            <Show when={
                                let current = current.clone();
                                move || !current.is_empty() && !known
                            }>
                                <option value=current.clone() selected=true>
                                    {current.clone()}
                                </option>
                            </Show>
                            {Language::ALL
                                .into_iter()
                                .map(|lang| {
                                    let tag = lang.tag();
                                    view! {
                                        <option
                                            value=tag
                                            selected={
                                                Language::from_tag(&s.current)
                                                    == Some(lang)
                                            }
                                        >
                                            {lang.label()}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </div>
                }
            })}
        </Show>
    }
}
