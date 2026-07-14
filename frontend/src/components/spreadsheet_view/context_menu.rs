// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Right-click context menu.
//!
//! Visible when `ctx_menu_visible` is true; positioned by `ctx_menu_x`
//! / `ctx_menu_y`. The top level is a compact, Excel-style list whose
//! advanced actions are grouped into submenus — Insert, Delete, Sort,
//! Format (conditional formatting, merge, validation, lock), Comment,
//! Hide/Unhide + freeze, and Data (CSV import, named ranges). Each
//! leaf reaches into the engine and bumps persistence.
//!
//! Chrome (backdrop, Escape, keyboard navigation, viewport clamping,
//! submenu flip near the right edge) comes from the shared
//! `components::menu` primitive; this module builds the `MenuEntry`
//! tree. Submenus open on hover, click, or ArrowRight — including the
//! nested Conditional Formatting and Data Validation levels.
//!
//! State surface is wide enough that a flat `fn` signature would have
//! ~25 params, so dependencies are bundled in a `ContextMenuDeps`
//! struct. Closure types stay generic so each call site keeps its
//! statically-resolved closure (no `Box<dyn Fn>` boxing).

use std::collections::HashSet;
use std::sync::Mutex;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::menu::{ContextMenu, MenuEntry};
use crate::spreadsheet::eval::{
    ChartConfig, ChartType, ConditionalCondition, ConditionalFormat, IconSetKind,
    SpreadsheetEngine, ValidationRule,
};
use crate::spreadsheet::parser::{CellRef, RangeRef, is_valid_named_range_name};

use super::persistence::parse_csv_line;
use super::sel_bounds;

type UndoEntries = Vec<((usize, usize), String, String)>;

/// #75: a non-contiguous (ctrl-click) selection has no single source
/// rectangle, so single-rect operations (Sort, Insert Chart, Insert
/// Pivot) must refuse rather than silently acting on just the primary
/// rect. Pure predicate (no side effects) so it's unit-testable.
fn op_blocked_by_multi_region(extras: &[(usize, usize, usize, usize)]) -> bool {
    !extras.is_empty()
}

/// Wrapper that surfaces the Excel-style "won't work on a multiple
/// selection" alert and returns true when the caller should abort.
fn refuse_on_multi_region(extras: &[(usize, usize, usize, usize)]) -> bool {
    if !op_blocked_by_multi_region(extras) {
        return false;
    }
    if let Some(w) = web_sys::window() {
        let _ = w.alert_with_message(&crate::t!("ss-multi-region-op"));
    }
    true
}

/// Dependency bundle for the context-menu render function.
///
/// All signal halves the menu needs to read or write, plus the engine
/// handle and the seven caller-side closures the menu invokes.
pub(super) struct ContextMenuDeps<P, R, S, IR, IC, DR, DC>
where
    P: Fn() + Copy + Send + Sync + 'static,
    R: Fn(UndoEntries) + Copy + Send + Sync + 'static,
    S: Fn(usize, bool) + Copy + Send + Sync + 'static,
    IR: Fn(usize) + Copy + Send + Sync + 'static,
    IC: Fn(usize) + Copy + Send + Sync + 'static,
    DR: Fn(usize) + Copy + Send + Sync + 'static,
    DC: Fn(usize) + Copy + Send + Sync + 'static,
{
    pub engine: &'static Mutex<SpreadsheetEngine>,
    /// Liveness flag for `engine`. The CSV import handler in this
    /// menu does `<input type="file">.change → spawn_local → engine.lock()`;
    /// the spawn suspends across an `.await`, and the SpreadsheetView
    /// may unmount in between (freeing engine via on_cleanup). The
    /// handler loads this flag after the await and bails out if the
    /// view has dropped. Arc-backed so the flag itself outlives
    /// component disposal.
    pub alive: std::sync::Arc<std::sync::atomic::AtomicBool>,

    // Visibility + position
    pub ctx_menu_visible: ReadSignal<bool>,
    pub set_ctx_menu_visible: WriteSignal<bool>,
    pub ctx_menu_x: ReadSignal<f64>,
    pub ctx_menu_y: ReadSignal<f64>,

    // Cursor + selection
    pub active_row: ReadSignal<usize>,
    pub active_col: ReadSignal<usize>,
    pub sel_row: ReadSignal<usize>,
    pub sel_col: ReadSignal<usize>,
    // #75: non-contiguous (ctrl-click) extra regions. Sort / Insert Chart
    // / Insert Pivot operate on a single rectangle, so they refuse rather
    // than silently acting on just the primary rect when extras exist.
    pub extra_sel_regions: ReadSignal<Vec<(usize, usize, usize, usize)>>,
    // #54: copy the primary selection as a GFM markdown table (the inverse
    // of the markdown-table paste detector).
    pub copy_as_markdown: leptos::prelude::Callback<()>,

    // Frozen panes
    pub frozen_rows: ReadSignal<usize>,
    pub set_frozen_rows: WriteSignal<usize>,
    pub frozen_cols: ReadSignal<usize>,
    pub set_frozen_cols: WriteSignal<usize>,

    // Hidden rows/cols + grid extent (CSV-import touches the latter)
    pub set_hidden_rows: WriteSignal<HashSet<usize>>,
    pub set_hidden_cols: WriteSignal<HashSet<usize>>,
    pub set_grid_rows: WriteSignal<usize>,
    pub set_grid_cols: WriteSignal<usize>,
    pub set_col_widths: WriteSignal<Vec<f64>>,

    // Pivot editor — set by "Insert Pivot Table" menu action; the
    // editor sidebar reads this signal to decide which (if any)
    // pivot is currently being edited.
    pub set_pivot_editor_open: WriteSignal<Option<(usize, usize)>>,

    // Sort dialog — set by the "Sort..." menu action; the dialog
    // reads this signal for its open/closed state and seed values.
    pub set_sort_dialog_open: WriteSignal<Option<super::sort_dialog::SortDialogContext>>,
    pub sort_keys: ReadSignal<Vec<(usize, bool)>>,
    pub grid_rows: ReadSignal<usize>,
    pub grid_cols: ReadSignal<usize>,

    // Threaded comments (Phase 5 / cell-comment-threads). The doc
    // id flows into the cell-comment context-menu item which
    // pre-creates the thread via `comments::create_thread` before
    // firing `on_open_cell_comment` to ask the page to surface the
    // popup. `active_sheet` is read when synthesizing the
    // deterministic per-cell block_id; see the click handler for
    // the rationale.
    pub doc_id: String,
    pub active_sheet: ReadSignal<usize>,
    pub on_open_cell_comment: leptos::prelude::Callback<super::CellCommentOpen>,

    // Caller-side closures
    pub persist: P,
    pub record_undo: R,
    pub sort_by_column: S,
    pub insert_row_at: IR,
    pub insert_col_at: IC,
    pub delete_row_at: DR,
    pub delete_col_at: DC,
}

pub(super) fn render_context_menu<P, R, S, IR, IC, DR, DC>(
    deps: ContextMenuDeps<P, R, S, IR, IC, DR, DC>,
) -> impl IntoView
where
    P: Fn() + Copy + Send + Sync + 'static,
    R: Fn(UndoEntries) + Copy + Send + Sync + 'static,
    S: Fn(usize, bool) + Copy + Send + Sync + 'static,
    IR: Fn(usize) + Copy + Send + Sync + 'static,
    IC: Fn(usize) + Copy + Send + Sync + 'static,
    DR: Fn(usize) + Copy + Send + Sync + 'static,
    DC: Fn(usize) + Copy + Send + Sync + 'static,
{
    let ContextMenuDeps {
        engine,
        alive,
        ctx_menu_visible,
        set_ctx_menu_visible,
        ctx_menu_x,
        ctx_menu_y,
        active_row,
        active_col,
        sel_row,
        sel_col,
        extra_sel_regions,
        copy_as_markdown,
        frozen_rows,
        set_frozen_rows,
        frozen_cols,
        set_frozen_cols,
        set_hidden_rows,
        set_hidden_cols,
        set_grid_rows,
        set_grid_cols,
        set_col_widths,
        set_pivot_editor_open,
        set_sort_dialog_open,
        sort_keys,
        grid_rows,
        grid_cols,
        doc_id,
        active_sheet,
        on_open_cell_comment,
        persist,
        record_undo,
        sort_by_column,
        insert_row_at,
        insert_col_at,
        delete_row_at,
        delete_col_at,
    } = deps;

    // The builder runs inside the shared menu's reactive render, so
    // `.get()` reads (selection bounds, freeze state) keep the labels
    // live; every activation closes the menu via the shared chrome —
    // the old per-handler `close()` calls are gone.
    let entries = Callback::new(move |()| {
        let r = active_row.get();
        let c = active_col.get();

        // ─── Insert ▸ ──────────────────────────────────────────
        let insert_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-insert"),
            vec![
                MenuEntry::action(crate::t!("ss-ctx-insert-row-above"), move || {
                    insert_row_at(r);
                }),
                MenuEntry::action(crate::t!("ss-ctx-insert-row-below"), move || {
                    insert_row_at(r + 1);
                }),
                MenuEntry::action(crate::t!("ss-ctx-insert-col-left"), move || {
                    insert_col_at(c);
                }),
                MenuEntry::action(crate::t!("ss-ctx-insert-col-right"), move || {
                    insert_col_at(c + 1);
                }),
                MenuEntry::Separator,
                MenuEntry::action(crate::t!("ss-ctx-insert-chart"), move || {
                    // #75: a chart's data range is one rectangle.
                    if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                        return;
                    }
                    if let Some(window) = web_sys::window() {
                        // Prompt for type (bar/line/pie). Invalid input
                        // surfaces an alert instead of silently doing
                        // nothing — that silence was the user-visible
                        // "nothing happens" in #67.
                        let type_input = window
                            .prompt_with_message(&crate::t!("ss-ctx-chart-type-prompt"))
                            .ok()
                            .flatten();
                        let Some(type_str) = type_input else { return };
                        let chart_type = match type_str.trim().to_lowercase().as_str() {
                            "bar" => Some(ChartType::Bar),
                            "line" => Some(ChartType::Line),
                            "pie" => Some(ChartType::Pie),
                            _ => None,
                        };
                        let Some(chart_type) = chart_type else {
                            let _ = window
                                .alert_with_message(&crate::t!("ss-ctx-chart-unknown-type"));
                            return;
                        };
                        let (r1, c1, r2, c2) =
                            sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
                        let title = window
                            .prompt_with_message(&crate::t!("ss-ctx-chart-title-prompt"))
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        engine.lock().unwrap().charts.push(ChartConfig {
                            chart_type,
                            data_range: ((c1, r1), (c2, r2)),
                            title,
                        });
                        persist();
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-insert-pivot"), move || {
                    // "Insert Pivot Table" — creates an empty pivot
                    // anchored two columns right of the user's selection
                    // (so the spill won't immediately overlap source
                    // data) and opens the sidebar editor on it. The user
                    // then drags fields from the source columns into the
                    // four zones.
                    // #75: a pivot's source range is one rectangle.
                    if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                        return;
                    }
                    let (r1, c1, r2, c2) =
                        sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
                    if r2 == r1 || c2 == c1 {
                        if let Some(window) = web_sys::window() {
                            let _ =
                                window.alert_with_message(&crate::t!("ss-ctx-pivot-needs-multi"));
                        }
                        return;
                    }
                    let source_a1 = format!(
                        "{}{}:{}{}",
                        crate::spreadsheet::parser::col_to_letters(c1),
                        r1 + 1,
                        crate::spreadsheet::parser::col_to_letters(c2),
                        r2 + 1,
                    );
                    // Anchor: drop two columns to the right of the
                    // source's right edge so the pivot spill won't
                    // collide with the source data on first eval.
                    let anchor = (c2 + 2, r1);
                    let pt = crate::spreadsheet::pivot::PivotTable::new_local_at(anchor, source_a1);
                    engine.lock().unwrap().add_pivot(pt);
                    set_pivot_editor_open.set(Some(anchor));
                    persist();
                }),
            ],
        );

        // ─── Delete ▸ ──────────────────────────────────────────
        // Delete rows / columns honor the full selection bounds.
        // Iterate from the highest index down so deleting one row
        // doesn't shift the indices of rows still in the queue.
        let (r1, c1, r2, c2) = sel_bounds(sel_row.get(), sel_col.get(), r, c);
        let row_count = r2 - r1 + 1;
        let col_count = c2 - c1 + 1;
        let row_label = if row_count > 1 {
            crate::t!("ss-ctx-delete-rows", count = row_count.to_string())
        } else {
            crate::t!("ss-ctx-delete-row")
        };
        let col_label = if col_count > 1 {
            crate::t!("ss-ctx-delete-cols", count = col_count.to_string())
        } else {
            crate::t!("ss-ctx-delete-col")
        };
        let delete_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-delete"),
            vec![
                MenuEntry::action(row_label, move || {
                    for ri in (r1..=r2).rev() {
                        delete_row_at(ri);
                    }
                }),
                MenuEntry::action(col_label, move || {
                    for ci in (c1..=c2).rev() {
                        delete_col_at(ci);
                    }
                }),
            ],
        );

        // ─── Sort ▸ ────────────────────────────────────────────
        let sort_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-sort"),
            vec![
                MenuEntry::action(crate::t!("ss-ctx-sort-a-z"), move || {
                    // #75: sort has no defined semantics across
                    // non-contiguous regions — refuse, like Excel.
                    if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                        return;
                    }
                    sort_by_column(c, true);
                }),
                MenuEntry::action(crate::t!("ss-ctx-sort-z-a"), move || {
                    if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                        return;
                    }
                    sort_by_column(c, false);
                }),
                MenuEntry::action(crate::t!("ss-ctx-sort-dialog"), move || {
                    if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                        return;
                    }
                    // "Sort..." opens the multi-key Sort dialog. Seed
                    // initial keys from the previously-applied sort
                    // chain if any, else from the active column. Range
                    // defaults to the entire used grid.
                    let rows = grid_rows.get_untracked().max(1);
                    let cols = grid_cols.get_untracked().max(1);
                    let range_a1 = format!(
                        "A1:{}{}",
                        crate::spreadsheet::parser::col_to_letters(cols - 1),
                        rows,
                    );
                    let prior = sort_keys.get_untracked();
                    let initial_keys = if prior.is_empty() { vec![(c, true)] } else { prior };
                    set_sort_dialog_open.set(Some(super::sort_dialog::SortDialogContext {
                        initial_keys,
                        initial_range_a1: range_a1,
                        initial_has_headers: false,
                    }));
                }),
            ],
        );

        // ─── Format ▸ (cond-fmt ▸, merge, validation ▸, lock) ──
        let cond_fmt_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-cond-fmt"),
            vec![
                MenuEntry::action(crate::t!("ss-ctx-cond-fmt"), move || {
                    // Simple conditional format: prompt for condition
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(cond_str)) =
                            window.prompt_with_message(&crate::t!("ss-ctx-cond-fmt-prompt"))
                        {
                            if let Some(condition) =
                                ConditionalCondition::parse_user_input(&cond_str)
                            {
                                if let Ok(Some(color)) = window
                                    .prompt_with_message(&crate::t!("ss-ctx-cond-fmt-color-prompt"))
                                {
                                    let color = color.trim().to_string();
                                    if !color.is_empty() {
                                        let (r1, c1, r2, c2) = sel_bounds(
                                            sel_row.get_untracked(),
                                            sel_col.get_untracked(),
                                            r,
                                            c,
                                        );
                                        engine.lock().unwrap().add_conditional_format(
                                            (c1, r1),
                                            (c2, r2),
                                            ConditionalFormat::Single { condition, bg_color: color },
                                        );
                                        persist();
                                    }
                                }
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-color-scale"), move || {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(input)) = window.prompt_with_message_and_default(
                            &crate::t!("ss-ctx-color-scale-prompt"),
                            "#ff0000,#00ff00",
                        ) {
                            let parts: Vec<String> = input
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            let rule = match parts.len() {
                                2 => Some(ConditionalFormat::ColorScale {
                                    low: parts[0].clone(),
                                    mid: None,
                                    high: parts[1].clone(),
                                }),
                                3 => Some(ConditionalFormat::ColorScale {
                                    low: parts[0].clone(),
                                    mid: Some(parts[1].clone()),
                                    high: parts[2].clone(),
                                }),
                                _ => None,
                            };
                            if let Some(rule) = rule {
                                let (r1, c1, r2, c2) = sel_bounds(
                                    sel_row.get_untracked(),
                                    sel_col.get_untracked(),
                                    r,
                                    c,
                                );
                                engine
                                    .lock()
                                    .unwrap()
                                    .add_conditional_format((c1, r1), (c2, r2), rule);
                                persist();
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-data-bar"), move || {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(color)) = window.prompt_with_message_and_default(
                            &crate::t!("ss-ctx-data-bar-prompt"),
                            "#3b82f6",
                        ) {
                            let color = color.trim().to_string();
                            if !color.is_empty() {
                                let (r1, c1, r2, c2) = sel_bounds(
                                    sel_row.get_untracked(),
                                    sel_col.get_untracked(),
                                    r,
                                    c,
                                );
                                engine.lock().unwrap().add_conditional_format(
                                    (c1, r1),
                                    (c2, r2),
                                    ConditionalFormat::DataBar { color },
                                );
                                persist();
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-icon-set"), move || {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(input)) = window.prompt_with_message_and_default(
                            &crate::t!("ss-ctx-icon-set-prompt"),
                            "arrows",
                        ) {
                            let kind = match input.trim().to_lowercase().as_str() {
                                "arrows" | "3arrows" => Some(IconSetKind::ThreeArrows),
                                "traffic" | "trafficlights" | "3trafficlights" => {
                                    Some(IconSetKind::ThreeTrafficLights)
                                }
                                _ => None,
                            };
                            if let Some(kind) = kind {
                                let (r1, c1, r2, c2) = sel_bounds(
                                    sel_row.get_untracked(),
                                    sel_col.get_untracked(),
                                    r,
                                    c,
                                );
                                engine.lock().unwrap().add_conditional_format(
                                    (c1, r1),
                                    (c2, r2),
                                    ConditionalFormat::IconSet { kind },
                                );
                                persist();
                            }
                        }
                    }
                }),
            ],
        );
        let validation_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-validation"),
            vec![
                MenuEntry::action(crate::t!("ss-ctx-set-checkbox"), move || {
                    let (r1, c1, r2, c2) =
                        sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
                    let mut eng = engine.lock().unwrap();
                    for ri in r1..=r2 {
                        for ci in c1..=c2 {
                            eng.style_mut((ci, ri)).validation = Some(ValidationRule::Checkbox);
                            // Initialize to FALSE if empty
                            if eng.get_raw((ci, ri)).is_empty() {
                                eng.set_cell((ci, ri), "FALSE");
                            }
                        }
                    }
                    drop(eng);
                    persist();
                }),
                MenuEntry::action(crate::t!("ss-ctx-set-dropdown"), move || {
                    // Simple dropdown: prompt for comma-separated options
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(input)) =
                            window.prompt_with_message(&crate::t!("ss-ctx-dropdown-prompt"))
                        {
                            let opts: Vec<String> = input
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if !opts.is_empty() {
                                let (r1, c1, r2, c2) = sel_bounds(
                                    sel_row.get_untracked(),
                                    sel_col.get_untracked(),
                                    r,
                                    c,
                                );
                                let mut eng = engine.lock().unwrap();
                                for ri in r1..=r2 {
                                    for ci in c1..=c2 {
                                        eng.style_mut((ci, ri)).validation =
                                            Some(ValidationRule::Dropdown(opts.clone()));
                                    }
                                }
                                drop(eng);
                                persist();
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-remove-validation"), move || {
                    let (r1, c1, r2, c2) =
                        sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
                    let mut eng = engine.lock().unwrap();
                    for ri in r1..=r2 {
                        for ci in c1..=c2 {
                            eng.style_mut((ci, ri)).validation = None;
                        }
                    }
                    drop(eng);
                    persist();
                }),
            ],
        );
        let cell_locked = engine
            .lock()
            .unwrap()
            .get_style((c, r))
            .map_or(false, |s| s.locked);
        let lock_label = if cell_locked {
            crate::t!("ss-ctx-unlock-cell")
        } else {
            crate::t!("ss-ctx-lock-cell")
        };
        let mut format_children = vec![cond_fmt_sub];
        // Merge only makes sense across a multi-cell span.
        let (col_span, row_span) = (c2 - c1 + 1, r2 - r1 + 1);
        if col_span > 1 || row_span > 1 {
            format_children.push(MenuEntry::action(crate::t!("ss-ctx-merge-cells"), move || {
                engine.lock().unwrap().merge_cells(c1, r1, col_span, row_span);
                persist();
            }));
        }
        format_children.push(MenuEntry::action(crate::t!("ss-ctx-unmerge-cells"), move || {
            engine.lock().unwrap().unmerge_at(c, r);
            persist();
        }));
        format_children.push(validation_sub);
        format_children.push(MenuEntry::action(lock_label, move || {
            let (r1, c1, r2, c2) =
                sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
            let mut eng = engine.lock().unwrap();
            let is_locked = eng.get_style((c, r)).map_or(false, |s| s.locked);
            for ri in r1..=r2 {
                for ci in c1..=c2 {
                    eng.style_mut((ci, ri)).locked = !is_locked;
                }
            }
            drop(eng);
            persist();
        }));
        let format_sub = MenuEntry::submenu(crate::t!("ss-ctx-menu-format"), format_children);

        // ─── Comment ▸ ─────────────────────────────────────────
        // Threaded comments (Phase 5). Open-or-create is shared with
        // the in-grid comment marker; see `cell_comment`. Three states
        // (existing thread / legacy note to migrate / nothing) all
        // converge on opening the popup in Thread-mode with a real
        // thread_id.
        let comment_label = {
            let eng = engine.lock().unwrap();
            let style = eng.get_style((c, r));
            let has_thread = style.and_then(|s| s.comment_thread_id.as_ref()).is_some();
            let has_legacy = style
                .and_then(|s| s.comment.as_ref())
                .is_some_and(|t| !t.is_empty());
            if has_thread || has_legacy {
                crate::t!("ss-ctx-open-comment")
            } else {
                crate::t!("ss-ctx-add-comment")
            }
        };
        let doc_id_for_btn = doc_id.clone();
        let alive_for_btn = alive.clone();
        let comment_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-comment"),
            vec![
                MenuEntry::action(comment_label, move || {
                    let left = ctx_menu_x.get_untracked();
                    let top = ctx_menu_y.get_untracked();
                    super::cell_comment::open_or_create_cell_comment(
                        engine,
                        doc_id_for_btn.clone(),
                        active_sheet.get_untracked(),
                        c,
                        r,
                        left,
                        top,
                        persist,
                        on_open_cell_comment,
                        alive_for_btn.clone(),
                    );
                }),
                // "Remove" detaches the cell from the thread (or clears
                // the legacy text). v1 limitation: the server-side
                // thread is NOT deleted — it stays resolvable via the
                // conversation pane until a future "resolve thread" UI
                // lands.
                MenuEntry::action(crate::t!("ss-ctx-remove-comment"), move || {
                    {
                        let mut eng = engine.lock().unwrap();
                        let style = eng.style_mut((c, r));
                        style.comment = None;
                        style.comment_thread_id = None;
                    }
                    persist();
                }),
            ],
        );

        // ─── Hide / Unhide ▸ (+ freeze) ────────────────────────
        let freeze_rows_label = if frozen_rows.get() > 0 {
            crate::t!("ss-ctx-unfreeze-rows")
        } else {
            crate::t!("ss-ctx-freeze-rows")
        };
        let freeze_cols_label = if frozen_cols.get() > 0 {
            crate::t!("ss-ctx-unfreeze-cols")
        } else {
            crate::t!("ss-ctx-freeze-cols")
        };
        let hide_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-hide"),
            vec![
                MenuEntry::action(freeze_rows_label, move || {
                    // "Freeze rows above" freezes every row strictly
                    // above the right-clicked cell, so a click on row r
                    // freezes rows [0, r) — count `r`. Mirror to engine
                    // state so the count round-trips through document
                    // save/load.
                    let new_count = if frozen_rows.get_untracked() > 0 { 0 } else { r };
                    set_frozen_rows.set(new_count);
                    engine.lock().unwrap().frozen_rows = new_count;
                    persist();
                }),
                MenuEntry::action(freeze_cols_label, move || {
                    // Same off-by-one applied to columns: "Freeze
                    // columns left" freezes [0, c).
                    let new_count = if frozen_cols.get_untracked() > 0 { 0 } else { c };
                    set_frozen_cols.set(new_count);
                    engine.lock().unwrap().frozen_cols = new_count;
                    persist();
                }),
                MenuEntry::Separator,
                MenuEntry::action(crate::t!("ss-ctx-hide-row"), move || {
                    set_hidden_rows.update(|h| {
                        h.insert(r);
                    });
                    engine.lock().unwrap().hidden_rows.insert(r);
                    persist();
                }),
                MenuEntry::action(crate::t!("ss-ctx-hide-col"), move || {
                    set_hidden_cols.update(|h| {
                        h.insert(c);
                    });
                    engine.lock().unwrap().hidden_cols.insert(c);
                    persist();
                }),
                MenuEntry::action(crate::t!("ss-ctx-unhide-all-rows"), move || {
                    set_hidden_rows.set(HashSet::new());
                    engine.lock().unwrap().hidden_rows.clear();
                    persist();
                }),
                MenuEntry::action(crate::t!("ss-ctx-unhide-all-cols"), move || {
                    set_hidden_cols.set(HashSet::new());
                    engine.lock().unwrap().hidden_cols.clear();
                    persist();
                }),
            ],
        );

        // ─── Data ▸ (import + named ranges) ────────────────────
        let alive_for_csv = std::sync::Arc::clone(&alive);
        let data_sub = MenuEntry::submenu(
            crate::t!("ss-ctx-menu-data"),
            vec![
                MenuEntry::action(crate::t!("ss-ctx-import-csv"), move || {
                    // Import CSV via file picker
                    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                        if let Ok(input) = doc.create_element("input") {
                            let _ = input.set_attribute("type", "file");
                            let _ = input.set_attribute("accept", ".csv,text/csv");
                            let _ = input.set_attribute("style", "display:none");
                            let _ = doc.body().unwrap().append_child(&input);

                            let input_el = input.clone();
                            let alive_for_handler = std::sync::Arc::clone(&alive_for_csv);
                            // #77: `Closure::once` (not `wrap` + `forget`) so the
                            // captured environment is freed after the single fire,
                            // instead of leaking on every import. Wired to BOTH
                            // `change` (file picked) and `cancel` (dialog dismissed) —
                            // exactly one fires — and the hidden input is removed on
                            // either path so a cancelled dialog leaves nothing behind.
                            let on_pick =
                                wasm_bindgen::closure::Closure::once(move |_: web_sys::Event| {
                                    let alive = alive_for_handler;
                                    leptos::task::spawn_local(async move {
                                        let html_input: web_sys::HtmlInputElement =
                                            input_el.clone().dyn_into().unwrap();
                                        let file = html_input.files().and_then(|f| f.get(0));
                                        if let Some(file) = file {
                                            if let Ok(text_js) =
                                                wasm_bindgen_futures::JsFuture::from(file.text())
                                                    .await
                                            {
                                                // Bail out if SpreadsheetView unmounted while
                                                // the file was being read — engine has been
                                                // freed by on_cleanup.
                                                if alive.load(std::sync::atomic::Ordering::SeqCst) {
                                                    let text =
                                                        text_js.as_string().unwrap_or_default();
                                                    let mut eng = engine.lock().unwrap();
                                                    let mut max_r = 0usize;
                                                    let mut max_c = 0usize;
                                                    for (ri, line) in text.lines().enumerate() {
                                                        if line.is_empty() {
                                                            continue;
                                                        }
                                                        for (ci, val) in
                                                            parse_csv_line(line).iter().enumerate()
                                                        {
                                                            eng.set_cell((ci, ri), val);
                                                            max_c = max_c.max(ci + 1);
                                                        }
                                                        max_r = max_r.max(ri + 1);
                                                    }
                                                    drop(eng);
                                                    set_grid_rows.set(max_r.max(10));
                                                    set_grid_cols.set(max_c.max(10));
                                                    set_col_widths.update(|w| {
                                                        while w.len() < max_c {
                                                            w.push(80.0);
                                                        }
                                                    });
                                                    persist();
                                                }
                                            }
                                        }
                                        // Always remove the hidden input — both the picked
                                        // and the cancelled path land here.
                                        input_el.remove();
                                    });
                                });

                            let cb = on_pick.as_ref().unchecked_ref();
                            let _ = input.add_event_listener_with_callback("change", cb);
                            let _ = input.add_event_listener_with_callback("cancel", cb);
                            on_pick.forget();
                            if let Ok(html_input) = input.dyn_into::<web_sys::HtmlElement>() {
                                html_input.click();
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-define-name"), move || {
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(name)) = window
                            .prompt_with_message_and_default(&crate::t!("ss-ctx-name-prompt"), "")
                        {
                            let name = name.trim().to_string();
                            if !name.is_empty() && is_valid_named_range_name(&name) {
                                let (r1, c1, r2, c2) = sel_bounds(
                                    sel_row.get_untracked(),
                                    sel_col.get_untracked(),
                                    r,
                                    c,
                                );
                                let range = RangeRef {
                                    start: CellRef { col: c1, row: r1, abs_col: true, abs_row: true },
                                    end: CellRef { col: c2, row: r2, abs_col: true, abs_row: true },
                                };
                                engine.lock().unwrap().set_named_range(&name, range);
                                persist();
                            }
                        }
                    }
                }),
                MenuEntry::action(crate::t!("ss-ctx-remove-name"), move || {
                    if let Some(window) = web_sys::window() {
                        let names = engine.lock().unwrap().named_ranges();
                        if names.is_empty() {
                            let _ =
                                window.alert_with_message(&crate::t!("ss-ctx-no-named-ranges"));
                        } else {
                            let listing = names
                                .iter()
                                .map(|(n, _)| n.as_str())
                                .collect::<Vec<_>>()
                                .join(", ");
                            if let Ok(Some(name)) = window.prompt_with_message_and_default(
                                &crate::t!("ss-ctx-remove-name-prompt", names = listing),
                                "",
                            ) {
                                let name = name.trim();
                                if !name.is_empty() {
                                    engine.lock().unwrap().remove_named_range(name);
                                    persist();
                                }
                            }
                        }
                    }
                }),
            ],
        );

        vec![
            insert_sub,
            delete_sub,
            // ─── Clear contents (leaf) ─────────────────────────
            MenuEntry::action(crate::t!("ss-ctx-clear-contents"), move || {
                let (r1, c1, r2, c2) =
                    sel_bounds(sel_row.get_untracked(), sel_col.get_untracked(), r, c);
                let mut eng = engine.lock().unwrap();
                let mut entries = Vec::new();
                for ri in r1..=r2 {
                    for ci in c1..=c2 {
                        let old = eng.get_raw((ci, ri)).to_string();
                        eng.set_cell((ci, ri), "");
                        entries.push(((ci, ri), old, String::new()));
                    }
                }
                drop(eng);
                record_undo(entries);
                persist();
            }),
            // ─── Copy as markdown (leaf, #54) ──────────────────
            MenuEntry::action(crate::t!("ss-ctx-copy-markdown"), move || {
                // Single-rectangle op: refuse a non-contiguous selection
                // (consistent with the other clipboard guards, #75).
                if refuse_on_multi_region(&extra_sel_regions.get_untracked()) {
                    return;
                }
                copy_as_markdown.run(());
            }),
            MenuEntry::Separator,
            sort_sub,
            format_sub,
            MenuEntry::Separator,
            comment_sub,
            hide_sub,
            data_sub,
        ]
    });

    view! {
        <ContextMenu
            visible=ctx_menu_visible
            x=ctx_menu_x
            y=ctx_menu_y
            entries=entries
            on_close=Callback::new(move |()| set_ctx_menu_visible.set(false))
        />
    }
}

#[cfg(test)]
mod tests {
    use super::op_blocked_by_multi_region;

    // #75: Sort / Insert Chart / Insert Pivot operate on a single
    // rectangle and must refuse when ctrl-click extra regions exist,
    // rather than silently acting on just the primary rect.
    #[test]
    fn op_blocked_only_when_extra_regions_present() {
        assert!(!op_blocked_by_multi_region(&[]));
        assert!(op_blocked_by_multi_region(&[(0, 0, 1, 1)]));
        assert!(op_blocked_by_multi_region(&[(0, 0, 0, 0), (2, 2, 3, 3)]));
    }
}
