// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #147: in-app Find & Replace bar. Finds matches in the live editor model
//! (so it works across mark boundaries and stays in sync with edits) and
//! drives navigation + replacement through `ToolbarCommand`s the editor
//! applies as real, collab-safe transactions.
//!
//! Every match is highlighted at once via the CSS Custom Highlight API (see
//! [`crate::editor::find_highlight`]), with the active match shown distinctly;
//! the active match is also selected so the editor scrolls it into view.

use leptos::prelude::*;
use web_sys::KeyboardEvent;

use crate::components::toolbar::ToolbarCommand;
use crate::editor::state::EditorState;

#[component]
pub fn FindReplaceBar(
    #[prop(into)] visible: Signal<bool>,
    #[prop(into)] editor_state: Signal<Option<EditorState>>,
    on_command: Callback<ToolbarCommand>,
    on_close: Callback<()>,
) -> impl IntoView {
    let query = RwSignal::new(String::new());
    let replace_text = RwSignal::new(String::new());
    let current = RwSignal::new(0usize);

    // Matches recompute from the live doc + query — so they stay correct as
    // the user edits or after a replace. Skipped while the bar is closed.
    let matches = Memo::new(move |_| {
        if !visible.get() {
            return Vec::<(usize, usize)>::new();
        }
        let q = query.get();
        if q.is_empty() {
            return Vec::new();
        }
        editor_state
            .get()
            .map(|s| crate::editor::find::find_matches(&s.doc, &q))
            .unwrap_or_default()
    });

    // Keep the active index in range as the match set changes.
    Effect::new(move |_| {
        let len = matches.get().len();
        if len == 0 {
            current.set(0);
        } else if current.get_untracked() >= len {
            current.set(len - 1);
        }
    });

    // Highlight every match (CSS Custom Highlight API), the active one distinct.
    // Deferred so the editor DOM reflects the latest doc (e.g. after a replace)
    // before model positions are mapped to DOM ranges; cleared when the bar is
    // closed or the query has no matches.
    Effect::new(move |_| {
        let ms = matches.get();
        let cur = current.get();
        let vis = visible.get();
        crate::a11y::defer(move || {
            if !vis || ms.is_empty() {
                crate::editor::find_highlight::clear();
            } else {
                crate::editor::find_highlight::apply(&ms, cur);
            }
        });
    });
    // Drop the highlights if the bar is unmounted (e.g. navigating away).
    on_cleanup(|| crate::editor::find_highlight::clear());

    let go = move |idx: usize| {
        let ms = matches.get_untracked();
        if ms.is_empty() {
            return;
        }
        let i = idx % ms.len();
        current.set(i);
        let (from, to) = ms[i];
        on_command.run(ToolbarCommand::SelectRange { from, to });
    };
    let next = move || {
        let ms = matches.get_untracked();
        if !ms.is_empty() {
            go((current.get_untracked() + 1) % ms.len());
        }
    };
    let prev = move || {
        let ms = matches.get_untracked();
        let n = ms.len();
        if n > 0 {
            go((current.get_untracked() + n - 1) % n);
        }
    };
    let do_replace = move || {
        let ms = matches.get_untracked();
        if let Some(&(from, to)) = ms.get(current.get_untracked()) {
            on_command.run(ToolbarCommand::ReplaceRange {
                from,
                to,
                text: replace_text.get_untracked(),
            });
        }
    };
    let do_replace_all = move || {
        let ms = matches.get_untracked();
        if !ms.is_empty() {
            on_command.run(ToolbarCommand::ReplaceAll {
                matches: ms,
                text: replace_text.get_untracked(),
            });
        }
    };

    view! {
        <Show when=move || visible.get()>
            <div class="find-replace-bar" role="search">
                <input
                    class="find-input"
                    type="text"
                    placeholder=crate::t!("find-placeholder")
                    aria-label=crate::t!("find-placeholder")
                    prop:value=move || query.get()
                    on:input=move |e| query.set(event_target_value(&e))
                    on:keydown=move |e: KeyboardEvent| {
                        if e.key() == "Enter" {
                            e.prevent_default();
                            if e.shift_key() { prev() } else { next() }
                        } else if e.key() == "Escape" {
                            on_close.run(());
                        }
                    }
                />
                <span class="find-count">
                    {move || {
                        let len = matches.get().len();
                        if len == 0 {
                            crate::t!("find-no-results")
                        } else {
                            format!("{}/{}", current.get() + 1, len)
                        }
                    }}
                </span>
                <button class="find-btn" title=crate::t!("find-prev")
                    aria-label=crate::t!("find-prev") on:click=move |_| prev()
                >"\u{2191}"</button>
                <button class="find-btn" title=crate::t!("find-next")
                    aria-label=crate::t!("find-next") on:click=move |_| next()
                >"\u{2193}"</button>
                <input
                    class="find-input find-replace-input"
                    type="text"
                    placeholder=crate::t!("find-replace-placeholder")
                    aria-label=crate::t!("find-replace-placeholder")
                    prop:value=move || replace_text.get()
                    on:input=move |e| replace_text.set(event_target_value(&e))
                />
                <button class="find-btn" on:click=move |_| do_replace()>
                    {crate::t!("find-replace")}
                </button>
                <button class="find-btn" on:click=move |_| do_replace_all()>
                    {crate::t!("find-replace-all")}
                </button>
                <button class="find-btn find-close" title=crate::t!("common-close")
                    aria-label=crate::t!("common-close") on:click=move |_| on_close.run(())
                >"\u{2715}"</button>
            </div>
        </Show>
    }
}
