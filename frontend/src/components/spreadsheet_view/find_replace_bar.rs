// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Find / Replace toolbar.
//!
//! Visible when `find_visible` is true. Live-searches the engine on
//! every input keystroke (case-insensitive substring on the displayed
//! text), tracks the current match index, and supports Enter/Next to
//! cycle, Replace + Replace All against the selection, and Escape to
//! dismiss. Mutating the engine (Replace / Replace All) calls the
//! parent's `persist` closure so the change goes through the standard
//! save/CRDT path.

use std::sync::Mutex;

use leptos::prelude::*;

use crate::spreadsheet::eval::SpreadsheetEngine;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_find_replace_bar(
    find_visible: ReadSignal<bool>,
    set_find_visible: WriteSignal<bool>,
    find_query: ReadSignal<String>,
    set_find_query: WriteSignal<String>,
    find_matches: ReadSignal<Vec<(usize, usize)>>,
    set_find_matches: WriteSignal<Vec<(usize, usize)>>,
    find_index: ReadSignal<usize>,
    set_find_index: WriteSignal<usize>,
    replace_text: ReadSignal<String>,
    set_replace_text: WriteSignal<String>,
    grid_rows: ReadSignal<usize>,
    grid_cols: ReadSignal<usize>,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
    select_cell: impl Fn(usize, usize, bool) + Copy + Send + Sync + 'static,
    scroll_active_into_view: impl Fn() + Copy + Send + Sync + 'static,
    refocus_wrapper: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    move || {
        if !find_visible.get() { return view! { <span></span> }.into_any(); }
        view! {
            <div class="ss-find-bar">
                <input
                    type="text"
                    class="ss-find-input"
                    placeholder=crate::t!("ss-find-placeholder")
                    prop:value=move || find_query.get()
                    on:input=move |e| {
                        let q = event_target_value(&e);
                        set_find_query.set(q.clone());
                        // Search all cells
                        if q.is_empty() {
                            set_find_matches.set(Vec::new());
                            return;
                        }
                        let eng = engine.lock().unwrap();
                        let rows = grid_rows.get_untracked();
                        let cols = grid_cols.get_untracked();
                        let q_upper = q.to_uppercase();
                        let mut matches = Vec::new();
                        for r in 0..rows {
                            for c in 0..cols {
                                if eng.get_display((c, r)).to_uppercase().contains(&q_upper) {
                                    matches.push((r, c));
                                }
                            }
                        }
                        drop(eng);
                        set_find_index.set(0);
                        set_find_matches.set(matches);
                    }
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Enter" {
                            e.prevent_default();
                            let matches = find_matches.get_untracked();
                            if !matches.is_empty() {
                                let idx = (find_index.get_untracked() + 1) % matches.len();
                                set_find_index.set(idx);
                                let (r, c) = matches[idx];
                                select_cell(r, c, false);
                                scroll_active_into_view();
                            }
                        } else if e.key() == "Escape" {
                            set_find_visible.set(false);
                            refocus_wrapper();
                        }
                    }
                />
                <span class="ss-find-count">
                    {move || {
                        let m = find_matches.get();
                        if m.is_empty() { crate::t!("ss-find-no-results") }
                        else { format!("{}/{}", find_index.get() + 1, m.len()) }
                    }}
                </span>
                <button class="ss-find-btn" on:click=move |_| {
                    let matches = find_matches.get_untracked();
                    if !matches.is_empty() {
                        let idx = (find_index.get_untracked() + 1) % matches.len();
                        set_find_index.set(idx);
                        let (r, c) = matches[idx];
                        select_cell(r, c, false);
                        scroll_active_into_view();
                    }
                }>{crate::t!("ss-find-next")}</button>
                <input
                    type="text"
                    class="ss-find-input"
                    placeholder=crate::t!("ss-replace-placeholder")
                    prop:value=move || replace_text.get()
                    on:input=move |e| set_replace_text.set(event_target_value(&e))
                />
                <button class="ss-find-btn" on:click=move |_| {
                    let matches = find_matches.get_untracked();
                    let idx = find_index.get_untracked();
                    let repl = replace_text.get_untracked();
                    if let Some(&(r, c)) = matches.get(idx) {
                        engine.lock().unwrap().set_cell((c, r), &repl);
                        persist();
                    }
                }>{crate::t!("ss-find-replace")}</button>
                <button class="ss-find-btn" on:click=move |_| {
                    let matches = find_matches.get_untracked();
                    let repl = replace_text.get_untracked();
                    let mut eng = engine.lock().unwrap();
                    for &(r, c) in &matches {
                        eng.set_cell((c, r), &repl);
                    }
                    drop(eng);
                    set_find_matches.set(Vec::new());
                    persist();
                }>{crate::t!("ss-find-replace-all")}</button>
                <button class="ss-find-close" on:click=move |_| {
                    set_find_visible.set(false);
                    refocus_wrapper();
                }>"\u{2715}"</button>
            </div>
        }.into_any()
    }
}
