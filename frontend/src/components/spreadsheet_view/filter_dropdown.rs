// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Column-filter popover.
//!
//! When `filter_col` is `Some(col)`, renders a backdrop + popup showing
//! the unique displayed values in that column, each toggleable to
//! show/hide every row matching the value via `hidden_rows`. "Show All"
//! resets the hidden-rows set. Clicking the backdrop or any item
//! dismisses the popup by setting `filter_col` back to `None`.

use std::collections::HashSet;
use std::sync::Mutex;

use leptos::prelude::*;

use crate::spreadsheet::eval::{ConditionalCondition, SpreadsheetEngine};
use crate::spreadsheet::parser::col_to_letters;

pub(super) fn render_filter_dropdown(
    filter_col: ReadSignal<Option<usize>>,
    set_filter_col: WriteSignal<Option<usize>>,
    grid_rows: ReadSignal<usize>,
    hidden_rows: ReadSignal<HashSet<usize>>,
    set_hidden_rows: WriteSignal<HashSet<usize>>,
    engine: &'static Mutex<SpreadsheetEngine>,
) -> impl IntoView {
    move || {
        let Some(col) = filter_col.get() else {
            return view! { <span></span> }.into_any();
        };
        let eng = engine.lock().unwrap();
        let rows = grid_rows.get_untracked();
        // Collect unique values in this column
        let mut unique_vals: Vec<String> = Vec::new();
        for r in 0..rows {
            let val = eng.get_display((col, r));
            if !unique_vals.contains(&val) {
                unique_vals.push(val);
            }
        }
        drop(eng);
        let current_hidden = hidden_rows.get();

        view! {
            <div class="ss-ctx-backdrop" on:click=move |_| set_filter_col.set(None)></div>
            <div class="ss-filter-popup">
                <div class="ss-filter-header">{crate::t!("ss-filter-header", col = col_to_letters(col))}</div>
                <button class="ss-ctx-item" on:click=move |_| {
                    set_hidden_rows.set(HashSet::new());
                    set_filter_col.set(None);
                }>{crate::t!("ss-filter-show-all")}</button>
                <button class="ss-ctx-item" on:click=move |_| {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(input)) = window.prompt_with_message(
                            &crate::t!("ss-filter-custom-prompt"),
                        ) {
                            if let Some(condition) = ConditionalCondition::parse_user_input(&input) {
                                let eng = engine.lock().unwrap();
                                let rows = grid_rows.get_untracked();
                                let mut hide = HashSet::new();
                                for r in 0..rows {
                                    let val = eng.get_value((col, r));
                                    if !condition.matches(val) {
                                        hide.insert(r);
                                    }
                                }
                                drop(eng);
                                // Custom filter REPLACES `hidden_rows`
                                // wholesale — including rows hidden by
                                // a prior filter on a *different*
                                // column. v1 limitation: there's no
                                // per-column filter state; all filters
                                // share one set. Documenting this in
                                // place of fixing it because per-column
                                // tracking is a larger refactor and the
                                // single-shared-set behavior was the
                                // pre-existing model for the value-list
                                // filter as well.
                                set_hidden_rows.set(hide);
                            }
                        }
                    }
                    set_filter_col.set(None);
                }>{crate::t!("ss-filter-custom-button")}</button>
                <div class="ss-ctx-sep"></div>
                {unique_vals.into_iter().map(|val| {
                    let val_for_check = val.clone();
                    let val_for_label = val.clone();
                    let display_val = if val.is_empty() { crate::t!("ss-filter-empty-value") } else { val.clone() };
                    // Check if any row with this value is hidden
                    let is_hidden = {
                        let eng = engine.lock().unwrap();
                        let rows = grid_rows.get_untracked();
                        (0..rows).any(|r| {
                            eng.get_display((col, r)) == val_for_check && current_hidden.contains(&r)
                        })
                    };
                    view! {
                        <button class="ss-ctx-item" on:click=move |_| {
                            let eng = engine.lock().unwrap();
                            let rows = grid_rows.get_untracked();
                            set_hidden_rows.update(|hidden| {
                                for r in 0..rows {
                                    if eng.get_display((col, r)) == val_for_label {
                                        if is_hidden {
                                            hidden.remove(&r);
                                        } else {
                                            hidden.insert(r);
                                        }
                                    }
                                }
                            });
                            drop(eng);
                            set_filter_col.set(None);
                        }>
                            {if is_hidden { "\u{2610} " } else { "\u{2611} " }}
                            {display_val}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>
        }.into_any()
    }
}
