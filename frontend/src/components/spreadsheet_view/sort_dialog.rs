// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Excel-style multi-key sort dialog.
//!
//! Replaces the click-to-sort behavior on column headers. Invoked
//! from a toolbar button or the context menu's "Sort..." item.
//! Mutates the engine via the caller-supplied `sort_by_keys_in_range`
//! closure so this module doesn't need direct engine access.
//!
//! Layout (mirrors Excel's Data → Sort dialog):
//! - Range row: A1-style range string editable.
//! - "Sort range has header row" checkbox.
//! - Multi-key list: column dropdown + asc/desc toggle + ✕ remove.
//! - "+ Add level" button.
//! - Apply / Cancel footer.

use std::sync::Mutex;

use leptos::prelude::*;

use crate::spreadsheet::eval::SpreadsheetEngine;
use crate::spreadsheet::parser::{col_to_letters, parse_formula, Expr};

/// Seed state passed by the caller when opening the dialog.
#[derive(Clone, Debug)]
pub(super) struct SortDialogContext {
    pub initial_keys: Vec<(usize, bool)>,
    pub initial_range_a1: String,
    pub initial_has_headers: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_sort_dialog(
    sort_dialog_open: ReadSignal<Option<SortDialogContext>>,
    set_sort_dialog_open: WriteSignal<Option<SortDialogContext>>,
    set_sort_keys: WriteSignal<Vec<(usize, bool)>>,
    grid_cols: ReadSignal<usize>,
    engine: &'static Mutex<SpreadsheetEngine>,
    sort_by_keys_in_range: impl Fn(((usize, usize), (usize, usize)), Vec<(usize, bool)>, bool)
        + Copy + Send + Sync + 'static,
) -> impl IntoView {
    move || {
        let Some(ctx) = sort_dialog_open.get() else {
            return view! { <span></span> }.into_any();
        };
        // Per-open local signals seeded from ctx.
        let keys = RwSignal::new(if ctx.initial_keys.is_empty() {
            vec![(0_usize, true)]
        } else {
            ctx.initial_keys.clone()
        });
        let range_a1 = RwSignal::new(ctx.initial_range_a1.clone());
        let has_headers = RwSignal::new(ctx.initial_has_headers);
        let error = RwSignal::new(String::new());
        render_panel(
            keys, range_a1, has_headers, error,
            set_sort_dialog_open, set_sort_keys,
            grid_cols, engine, sort_by_keys_in_range,
        ).into_any()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_panel(
    keys: RwSignal<Vec<(usize, bool)>>,
    range_a1: RwSignal<String>,
    has_headers: RwSignal<bool>,
    error: RwSignal<String>,
    set_sort_dialog_open: WriteSignal<Option<SortDialogContext>>,
    set_sort_keys: WriteSignal<Vec<(usize, bool)>>,
    grid_cols: ReadSignal<usize>,
    engine: &'static Mutex<SpreadsheetEngine>,
    sort_by_keys_in_range: impl Fn(((usize, usize), (usize, usize)), Vec<(usize, bool)>, bool)
        + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let close = move || {
        set_sort_dialog_open.set(None);
    };

    let on_apply = move |_| {
        let raw_range = range_a1.get_untracked();
        let parsed_range: Option<((usize, usize), (usize, usize))> = match parse_formula(raw_range.trim()) {
            Ok(Expr::Range(r)) => Some((
                (r.start.col.min(r.end.col), r.start.row.min(r.end.row)),
                (r.start.col.max(r.end.col), r.start.row.max(r.end.row)),
            )),
            _ => None,
        };
        let Some(parsed_range) = parsed_range else {
            error.set(crate::t!("ss-sort-err-parse-range"));
            return;
        };
        let chain = keys.get_untracked();
        if chain.is_empty() {
            error.set(crate::t!("ss-sort-err-no-keys"));
            return;
        }
        sort_by_keys_in_range(parsed_range, chain.clone(), has_headers.get_untracked());
        // Persist the chain so a subsequent open of the dialog
        // shows the last-applied keys.
        set_sort_keys.set(chain);
        set_sort_dialog_open.set(None);
    };

    let on_add_level = move |_| {
        keys.update(|ks| {
            // Default new level: first column NOT already in the
            // chain, ascending. If every column is taken, default
            // to col 0 (the user can change it).
            let used: std::collections::HashSet<usize> = ks.iter().map(|(c, _)| *c).collect();
            let total = grid_cols.get_untracked().max(1);
            let first_free = (0..total).find(|c| !used.contains(c)).unwrap_or(0);
            ks.push((first_free, true));
        });
    };

    // The column-dropdown options. Refresh on `has_headers` toggle
    // so labels switch between "A / B / C" and the row-0 cell text.
    let column_options = move |selected_col: usize| {
        let total = grid_cols.get();
        let use_headers = has_headers.get();
        let parsed_range = parse_formula(range_a1.get().trim()).ok();
        let header_row = match &parsed_range {
            Some(Expr::Range(r)) => Some(r.start.row.min(r.end.row)),
            _ => None,
        };
        (0..total).map(move |c| {
            let label = if use_headers {
                if let Some(hr) = header_row {
                    let raw = engine.lock().unwrap().get_display((c, hr));
                    if raw.is_empty() { col_to_letters(c) } else { raw }
                } else {
                    col_to_letters(c)
                }
            } else {
                col_to_letters(c)
            };
            view! {
                <option value=c.to_string() selected=c == selected_col>{label}</option>
            }
        }).collect_view()
    };

    view! {
        <div class="ss-sort-dialog-backdrop" on:click=move |_| close()></div>
        <div class="ss-sort-dialog">
            <div class="ss-sort-dialog-header">
                <span class="ss-sort-dialog-title">{crate::t!("ss-sort-title")}</span>
                <button class="ss-sort-dialog-close"
                    on:click=move |_| close()>"\u{2715}"</button>
            </div>
            <div class="ss-sort-dialog-body">
                <label class="ss-sort-dialog-label">
                    {crate::t!("ss-sort-range-label")}
                    <input type="text" class="ss-sort-dialog-range"
                        prop:value=move || range_a1.get()
                        on:input=move |e| {
                            range_a1.set(event_target_value(&e));
                            error.set(String::new());
                        } />
                </label>
                <label class="ss-sort-dialog-headers">
                    <input type="checkbox"
                        prop:checked=move || has_headers.get()
                        on:change=move |e| {
                            let target = event_target_checked(&e);
                            has_headers.set(target);
                        } />
                    " "{crate::t!("ss-sort-has-headers")}
                </label>

                <div class="ss-sort-dialog-keys">
                    {move || {
                        let chain = keys.get();
                        chain.iter().enumerate().map(|(idx, (col, asc))| {
                            let col = *col;
                            let asc = *asc;
                            let label = if idx == 0 { crate::t!("ss-sort-by-label") } else { crate::t!("ss-sort-then-by-label") };
                            view! {
                                <div class="ss-sort-dialog-key">
                                    <span class="ss-sort-dialog-key-label">{label}</span>
                                    <select class="ss-sort-dialog-key-col"
                                        on:change=move |e| {
                                            let new_col = event_target_value(&e).parse::<usize>().unwrap_or(0);
                                            keys.update(|ks| {
                                                if let Some(entry) = ks.get_mut(idx) {
                                                    entry.0 = new_col;
                                                }
                                            });
                                        }>
                                        {column_options(col)}
                                    </select>
                                    <select class="ss-sort-dialog-key-dir"
                                        on:change=move |e| {
                                            let new_asc = event_target_value(&e) == "asc";
                                            keys.update(|ks| {
                                                if let Some(entry) = ks.get_mut(idx) {
                                                    entry.1 = new_asc;
                                                }
                                            });
                                        }>
                                        <option value="asc" selected=asc>{crate::t!("ss-sort-asc")}</option>
                                        <option value="desc" selected=!asc>{crate::t!("ss-sort-desc")}</option>
                                    </select>
                                    <button class="ss-sort-dialog-key-remove"
                                        title=crate::t!("ss-sort-remove-level-title")
                                        on:click=move |_| {
                                            keys.update(|ks| {
                                                if ks.len() > 1 { ks.remove(idx); }
                                            });
                                        }>"\u{2715}"</button>
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>

                <button class="ss-sort-dialog-add" on:click=on_add_level>
                    {crate::t!("ss-sort-add-level")}
                </button>

                {move || {
                    let msg = error.get();
                    if msg.is_empty() {
                        view! { <span></span> }.into_any()
                    } else {
                        view! { <div class="ss-sort-dialog-error">{msg}</div> }.into_any()
                    }
                }}
            </div>
            <div class="ss-sort-dialog-footer">
                <button class="ss-sort-dialog-cancel"
                    on:click=move |_| close()>{crate::t!("ss-sort-cancel")}</button>
                <button class="ss-sort-dialog-apply"
                    on:click=on_apply>{crate::t!("ss-sort-apply")}</button>
            </div>
        </div>
    }
}
