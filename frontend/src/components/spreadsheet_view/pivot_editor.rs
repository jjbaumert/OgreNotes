// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pivot editor sidebar (M-S2 v2 phase 3).
//!
//! Right-side panel that opens when the user picks "Insert Pivot
//! Table" from the context menu (or clicks an existing pivot's
//! anchor cell, when phase 3.x adds click-into-spill detection).
//! Mutates `engine.pivots` directly via clone-mutate-replace and
//! calls `persist` after each change so the doc round-trips.
//!
//! v0a scope:
//! - Field list with type icons + checkboxes; checkbox click
//!   default-routes by detected source-column type (text → Rows,
//!   date → Cols, number → Values).
//! - Native HTML5 drag-drop from the field list into any zone
//!   (Rows / Cols / Values / Filters).
//! - Per-zone chip with ✕ remove. Values chip carries a
//!   summarize-fn dropdown.
//! - Layout dropdown (Compact / Outline / Tabular) + Grand totals
//!   dropdown (None / Rows / Cols / Both).
//!
//! Deferred to phase 3.x:
//! - Drag-to-reorder within a zone, drag from zone to zone, drag
//!   back to field list (use ✕ for now).
//! - Search box on the field list.
//! - Suggestion panel.
//! - Date-granularity / NumericBin-width popover (defaults to
//!   Direct on add; Date or NumericBin require manual JSON edit).
//! - Click-into-spill detection.

use std::collections::HashSet;
use std::sync::Mutex;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::spreadsheet::eval::{CellValue, SpreadsheetEngine};
use crate::spreadsheet::pivot::{
    DateGranularity, GrandTotals, LayoutStyle, PivotFilterCondition, PivotFilterSpec,
    PivotGroup, PivotGroupKind, PivotTable, PivotValue, SortOrder, SourceRange,
    SubtotalsPos, SummarizeFn, ValueLayout,
};

#[derive(Copy, Clone, PartialEq, Eq)]
enum FieldType { Text, Number, Date }

fn field_type_icon(t: FieldType) -> &'static str {
    match t {
        FieldType::Text => "T",
        FieldType::Number => "#",
        FieldType::Date => "📅",
    }
}

/// Inspect the source column to infer whether it holds text, numbers
/// or dates. Samples up to 8 non-empty cells and takes the majority
/// vote, so a stray "Notes: pending" string in a date column doesn't
/// permanently mistype the column. Ties → Text. Used for the field-
/// list type icon and for default-routing checkbox clicks
/// (text → Rows, date → Cols, number → Values).
fn infer_field_type(eng: &SpreadsheetEngine, col: usize, src_top_row: usize, src_bottom_row: usize) -> FieldType {
    let mut text = 0_u32;
    let mut number = 0_u32;
    let mut date = 0_u32;
    let mut sampled = 0_u32;
    for r in (src_top_row + 1)..=src_bottom_row {
        if sampled >= 8 { break; }
        match eng.get_value((col, r)) {
            CellValue::Number(_) => { number += 1; sampled += 1; }
            CellValue::Text(s) => {
                let trimmed = s.trim();
                let dashed = trimmed.matches('-').count() == 2;
                let slashed = trimmed.matches('/').count() == 2;
                if (dashed || slashed) && trimmed.len() >= 8 {
                    date += 1;
                } else {
                    text += 1;
                }
                sampled += 1;
            }
            _ => continue,
        }
    }
    if sampled == 0 { return FieldType::Text; }
    // Tiebreakers favor Text per the docstring — a column whose
    // first 8 cells are 4-text/4-number routes to Rows (text), not
    // Values (number). Date wins all ties because date detection is
    // the strictest classifier (substring + delimiter + length).
    if date > text && date >= number { FieldType::Date }
    else if number > text { FieldType::Number }
    else { FieldType::Text }
}

fn header_label(eng: &SpreadsheetEngine, col: usize, src_top_row: usize) -> String {
    let label = eng.get_display((col, src_top_row));
    if label.is_empty() { format!("Column {}", col + 1) } else { label }
}

/// Distinct displayed values in `col` between rows `[src_top_row + 1,
/// src_bottom_row]`. Insertion order preserved so the filter popover
/// shows "North / South / East / West" in the order they appear in
/// the source rather than alphabetised — matches Excel autofilter.
/// Empty strings are skipped (an explicit "blanks" filter is a v2.x
/// follow-up).
fn column_unique_values(
    eng: &SpreadsheetEngine,
    col: usize,
    src_top_row: usize,
    src_bottom_row: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in (src_top_row + 1)..=src_bottom_row {
        let v = eng.get_display((col, r));
        if v.is_empty() { continue; }
        if seen.insert(v.clone()) { out.push(v); }
    }
    out
}

/// Parse a `RangeRef` from an A1 string. Returns the
/// `(top_left_col, top_left_row, bottom_right_col, bottom_right_row)`
/// tuple. Falls back to a 1×1 range at A1 if parsing fails.
fn parse_range(range_a1: &str) -> (usize, usize, usize, usize) {
    use crate::spreadsheet::parser::{parse_formula, Expr};
    match parse_formula(range_a1) {
        Ok(Expr::Range(r)) => (r.start.col, r.start.row, r.end.col, r.end.row),
        _ => (0, 0, 0, 0),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_pivot_editor(
    pivot_editor_open: ReadSignal<Option<(usize, usize)>>,
    set_pivot_editor_open: WriteSignal<Option<(usize, usize)>>,
    grid_version: ReadSignal<u32>,
    // Lifted so the parent (spreadsheet_view) can also write it from
    // a filter-row cell click — clicking the page-field cell in the
    // pivot output opens the sidebar AND auto-expands the chip's
    // popover for that filter index.
    filter_popover_open: ReadSignal<Option<usize>>,
    set_filter_popover_open: WriteSignal<Option<usize>>,
    // Row-/col-header value picker. `(0, idx)` = row group `idx`,
    // `(1, idx)` = col group `idx`. Set by the parent on cell-click
    // detection of the "Region ▾" / col-group "Product ▾" cells.
    group_picker_open: ReadSignal<Option<(u8, usize)>>,
    set_group_picker_open: WriteSignal<Option<(u8, usize)>>,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // Search input lives at the editor scope, NOT inside `render_panel`
    // — `render_panel` is rebuilt every grid_version bump (e.g. on
    // every drag), and a panel-scoped signal would be reconstructed
    // each time, blanking the user's typed search string mid-drag.
    // The `last_anchor` cell tracks which pivot the search applies
    // to so we reset the search when the editor switches pivots.
    let (search, set_search) = signal(String::new());
    let last_anchor = std::sync::Arc::new(Mutex::new(None::<(usize, usize)>));
    move || {
        // Track grid_version so the editor re-renders when the
        // engine recomputes a pivot.
        let _ = grid_version.get();
        let Some(anchor) = pivot_editor_open.get() else {
            return view! { <span></span> }.into_any();
        };
        // If the editor switched pivots, reset the search filter and
        // close any open filter / group-picker popover.
        {
            let mut guard = last_anchor.lock().unwrap();
            if *guard != Some(anchor) {
                set_search.set(String::new());
                set_filter_popover_open.set(None);
                set_group_picker_open.set(None);
                *guard = Some(anchor);
            }
        }
        // Snapshot the pivot under the lock — we render against the
        // clone so the lock isn't held across the view.
        let pivot = match engine.lock().unwrap().get_pivot(anchor).cloned() {
            Some(p) => p,
            None => {
                // Pivot disappeared (concurrent edit?). Close the
                // editor rather than render an empty shell.
                set_pivot_editor_open.set(None);
                return view! { <span></span> }.into_any();
            }
        };
        render_panel(
            anchor, pivot, search, set_search,
            filter_popover_open, set_filter_popover_open,
            group_picker_open, set_group_picker_open,
            set_pivot_editor_open, engine, persist,
        ).into_any()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_panel(
    anchor: (usize, usize),
    pivot: PivotTable,
    search: ReadSignal<String>,
    set_search: WriteSignal<String>,
    filter_popover_open: ReadSignal<Option<usize>>,
    set_filter_popover_open: WriteSignal<Option<usize>>,
    group_picker_open: ReadSignal<Option<(u8, usize)>>,
    set_group_picker_open: WriteSignal<Option<(u8, usize)>>,
    set_pivot_editor_open: WriteSignal<Option<(usize, usize)>>,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let SourceRange::Local { ref range_a1 } = pivot.source else {
        // Foreign-source editing UI is phase 3.x.
        let range_label = match &pivot.source {
            SourceRange::Foreign { doc_id, sheet_name, range_a1 } =>
                format!("{}!{}!{}", doc_id, sheet_name, range_a1),
            SourceRange::Local { range_a1 } => range_a1.clone(),
        };
        return view! {
            <div class="ss-pivot-editor">
                <div class="ss-pivot-editor-header">
                    <span class="ss-pivot-editor-title">{crate::t!("ss-pivot-title")}</span>
                    <button class="ss-pivot-close" on:click=move |_| {
                        set_pivot_editor_open.set(None);
                    }>"\u{2715}"</button>
                </div>
                <div class="ss-pivot-editor-body">
                    <div class="ss-pivot-source-row">
                        {crate::t!("ss-pivot-foreign-source-label")}" "{range_label}
                    </div>
                    <div class="ss-pivot-empty-hint">
                        {crate::t!("ss-pivot-foreign-hint")}
                    </div>
                </div>
            </div>
        }.into_any();
    };

    let range_a1 = range_a1.clone();
    let (src_c1, src_r1, src_c2, src_r2) = parse_range(&range_a1);

    // Field-list columns derived from the source's top-row headers.
    let mut fields: Vec<(usize, String, FieldType)> = Vec::new();
    {
        let eng = engine.lock().unwrap();
        for col in src_c1..=src_c2 {
            let label = header_label(&eng, col, src_r1);
            let ty = infer_field_type(&eng, col, src_r1, src_r2);
            fields.push((col, label, ty));
        }
    }

    // `search` / `set_search` are owned by `render_pivot_editor` so
    // the typed string survives grid_version bumps (e.g. mid-drag
    // recomputes). The editor resets the search whenever the active
    // pivot anchor changes — see `last_anchor` tracking up there.

    // ─── Layout dropdown / Grand-totals dropdown ─────────
    let layout_select = {
        let layout = pivot.layout_style;
        view! {
            <label class="ss-pivot-toplabel">{crate::t!("ss-pivot-layout")}
                <select on:change=move |e: web_sys::Event| {
                    let v = event_target_value(&e);
                    let style = match v.as_str() {
                        "outline" => LayoutStyle::Outline,
                        "tabular" => LayoutStyle::Tabular,
                        _ => LayoutStyle::Compact,
                    };
                    let mt = move |p: &mut PivotTable| { p.layout_style = style; };
                    let mut eng = engine.lock().unwrap();
                    if let Some(current) = eng.get_pivot(anchor).cloned() {
                        let mut next = current;
                        mt(&mut next);
                        eng.add_pivot(next);
                        drop(eng);
                        persist();
                    }
                }>
                    <option value="compact" selected=move || matches!(layout, LayoutStyle::Compact)>{crate::t!("ss-pivot-layout-compact")}</option>
                    <option value="outline" selected=move || matches!(layout, LayoutStyle::Outline)>{crate::t!("ss-pivot-layout-outline")}</option>
                    <option value="tabular" selected=move || matches!(layout, LayoutStyle::Tabular)>{crate::t!("ss-pivot-layout-tabular")}</option>
                </select>
            </label>
        }
    };
    let totals_select = {
        let totals = pivot.grand_totals;
        view! {
            <label class="ss-pivot-toplabel">{crate::t!("ss-pivot-totals")}
                <select on:change=move |e: web_sys::Event| {
                    let v = event_target_value(&e);
                    let g = match v.as_str() {
                        "none" => GrandTotals::None,
                        "rows" => GrandTotals::Rows,
                        "cols" => GrandTotals::Cols,
                        _ => GrandTotals::Both,
                    };
                    let mut eng = engine.lock().unwrap();
                    if let Some(current) = eng.get_pivot(anchor).cloned() {
                        let mut next = current;
                        next.grand_totals = g;
                        eng.add_pivot(next);
                        drop(eng);
                        persist();
                    }
                }>
                    <option value="none" selected=move || matches!(totals, GrandTotals::None)>{crate::t!("ss-pivot-totals-none")}</option>
                    <option value="rows" selected=move || matches!(totals, GrandTotals::Rows)>{crate::t!("ss-pivot-totals-rows")}</option>
                    <option value="cols" selected=move || matches!(totals, GrandTotals::Cols)>{crate::t!("ss-pivot-totals-cols")}</option>
                    <option value="both" selected=move || matches!(totals, GrandTotals::Both)>{crate::t!("ss-pivot-totals-both")}</option>
                </select>
            </label>
        }
    };

    // ─── Field list (with search filter) ─────────────────
    //
    // The list is wrapped in a reactive closure so it re-runs when
    // `search` changes; case-insensitive substring match on the
    // column header label. The pivot snapshot used for `in_use`
    // checks is captured at the outer render closure (re-runs on
    // grid_version), so checkbox states stay in sync without
    // requiring a re-lock per filter keystroke.
    let pivot_for_check = pivot.clone();
    let field_list_rows = move || {
        let q = search.get().to_lowercase();
        let pivot_for_check = pivot_for_check.clone();
        fields.iter()
            .filter(|(_, label, _)| q.is_empty() || label.to_lowercase().contains(&q))
            .map(|(col, label, ty)| {
                let col = *col;
                let ty = *ty;
                let label = label.clone();
                let on_check = move |e: web_sys::Event| {
                    let target = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                    let checked = target.map(|t| t.checked()).unwrap_or(false);
                    let mut eng = engine.lock().unwrap();
                    if let Some(current) = eng.get_pivot(anchor).cloned() {
                        let mut next = current;
                        if checked {
                            let already = field_in_use(&next, col);
                            if !already {
                                match ty {
                                    FieldType::Text => next.rows.push(PivotGroup { source_col: col, ..PivotGroup::default() }),
                                    FieldType::Date => {
                                        next.cols.push(PivotGroup {
                                            source_col: col,
                                            kind: PivotGroupKind::Date(DateGranularity::Month),
                                            ..PivotGroup::default()
                                        });
                                    }
                                    FieldType::Number => next.values.push(PivotValue {
                                        source_col: col,
                                        summarize_fn: SummarizeFn::Sum,
                                        display_name: None,
                                    }),
                                }
                            }
                        } else {
                            remove_field_from_zones(&mut next, col);
                        }
                        eng.add_pivot(next);
                        drop(eng);
                        persist();
                    }
                };
                let in_use = field_in_use(&pivot_for_check, col);
                let dragstart_col = col;
                view! {
                    <div class="ss-pivot-field"
                        draggable="true"
                        on:dragstart=move |e: web_sys::DragEvent| {
                            if let Some(dt) = e.data_transfer() {
                                let _ = dt.set_data("text/plain", &format!("field:{dragstart_col}"));
                                dt.set_effect_allowed("copy");
                            }
                        }>
                        <input type="checkbox"
                            prop:checked=in_use
                            on:change=on_check />
                        <span class="ss-pivot-field-icon">{field_type_icon(ty)}</span>
                        <span class="ss-pivot-field-label">{label}</span>
                    </div>
                }
            })
            .collect_view()
    };

    // ─── Per-zone chips ─────────────────────────────────
    fn render_chip_remove(
        anchor: (usize, usize),
        zone: ZoneKind,
        col: usize,
        engine: &'static Mutex<SpreadsheetEngine>,
        persist: impl Fn() + Copy + Send + Sync + 'static,
    ) -> impl IntoView {
        view! {
            <button class="ss-pivot-chip-x"
                on:click=move |_| {
                    let mut eng = engine.lock().unwrap();
                    if let Some(current) = eng.get_pivot(anchor).cloned() {
                        let mut next = current;
                        match zone {
                            ZoneKind::Rows => next.rows.retain(|g| g.source_col != col),
                            ZoneKind::Cols => next.cols.retain(|g| g.source_col != col),
                            ZoneKind::Values => next.values.retain(|v| v.source_col != col),
                            ZoneKind::Filters => next.filters.retain(|f| f.source_col != col),
                        }
                        eng.add_pivot(next);
                        drop(eng);
                        persist();
                    }
                }>"\u{2715}"</button>
        }
    }

    let row_chips = pivot.rows.iter().enumerate().map(|(idx, g)| {
        let col = g.source_col;
        let label = header_label(&engine.lock().unwrap(), col, src_r1);
        let kind = g.kind.clone();
        view! {
            <div class="ss-pivot-chip"
                draggable="true"
                on:dragstart=move |e: web_sys::DragEvent| {
                    if let Some(dt) = e.data_transfer() {
                        let _ = dt.set_data("text/plain", &format!("zone:rows:{idx}"));
                        dt.set_effect_allowed("move");
                    }
                }>
                <span class="ss-pivot-chip-label">{label}</span>
                {render_kind_controls(anchor, ZoneKind::Rows, idx, kind, engine, persist)}
                {render_chip_remove(anchor, ZoneKind::Rows, col, engine, persist)}
            </div>
        }
    }).collect_view();
    let col_chips = pivot.cols.iter().enumerate().map(|(idx, g)| {
        let col = g.source_col;
        let label = header_label(&engine.lock().unwrap(), col, src_r1);
        let kind = g.kind.clone();
        view! {
            <div class="ss-pivot-chip"
                draggable="true"
                on:dragstart=move |e: web_sys::DragEvent| {
                    if let Some(dt) = e.data_transfer() {
                        let _ = dt.set_data("text/plain", &format!("zone:cols:{idx}"));
                        dt.set_effect_allowed("move");
                    }
                }>
                <span class="ss-pivot-chip-label">{label}</span>
                {render_kind_controls(anchor, ZoneKind::Cols, idx, kind, engine, persist)}
                {render_chip_remove(anchor, ZoneKind::Cols, col, engine, persist)}
            </div>
        }
    }).collect_view();
    let value_chips = pivot.values.iter().enumerate().map(|(idx, v)| {
        let col = v.source_col;
        let current_fn = v.summarize_fn;
        let label = header_label(&engine.lock().unwrap(), col, src_r1);
        view! {
            <div class="ss-pivot-chip"
                draggable="true"
                on:dragstart=move |e: web_sys::DragEvent| {
                    if let Some(dt) = e.data_transfer() {
                        let _ = dt.set_data("text/plain", &format!("zone:values:{idx}"));
                        dt.set_effect_allowed("move");
                    }
                }>
                <span class="ss-pivot-chip-label">{label}</span>
                <select class="ss-pivot-chip-agg"
                    on:change=move |e: web_sys::Event| {
                        let v = event_target_value(&e);
                        let new_fn = parse_summarize_fn(&v);
                        let mut eng = engine.lock().unwrap();
                        if let Some(current) = eng.get_pivot(anchor).cloned() {
                            let mut next = current;
                            if let Some(entry) = next.values.get_mut(idx) {
                                entry.summarize_fn = new_fn;
                            }
                            eng.add_pivot(next);
                            drop(eng);
                            persist();
                        }
                    }>
                    {summarize_fn_options(current_fn)}
                </select>
                {render_chip_remove(anchor, ZoneKind::Values, col, engine, persist)}
            </div>
        }
    }).collect_view();
    let filter_chips = pivot.filters.iter().enumerate().map(|(idx, f)| {
        let col = f.source_col;
        let label = header_label(&engine.lock().unwrap(), col, src_r1);
        let cond_label = filter_cond_label(&f.condition);
        view! {
            <div class="ss-pivot-chip"
                draggable="true"
                on:dragstart=move |e: web_sys::DragEvent| {
                    if let Some(dt) = e.data_transfer() {
                        let _ = dt.set_data("text/plain", &format!("zone:filters:{idx}"));
                        dt.set_effect_allowed("move");
                    }
                }>
                <span class="ss-pivot-chip-label">{label}":"</span>
                <button class="ss-pivot-chip-filter-toggle"
                    title=crate::t!("ss-pivot-edit-filter-tooltip")
                    on:click=move |_| {
                        // Toggle popover on this chip. The popover is
                        // rendered after the Filters zone (see below)
                        // and reads `filter_popover_open` to show
                        // itself.
                        let current = filter_popover_open.get_untracked();
                        if current == Some(idx) {
                            set_filter_popover_open.set(None);
                        } else {
                            set_filter_popover_open.set(Some(idx));
                        }
                    }>
                    {cond_label}" \u{25BE}"
                </button>
                {render_chip_remove(anchor, ZoneKind::Filters, col, engine, persist)}
            </div>
        }
    }).collect_view();

    // Row-/col-header value picker. Cell click in the grid sets
    // `group_picker_open = Some((axis, idx))`; this view reads it
    // reactively and renders a checkbox list of the unique values
    // in the group's source column. Toggling rewrites the group's
    // `visible_values` whitelist; "All" clears it (None == show
    // every value).
    let group_picker_view = {
        let pivot_for_picker = pivot.clone();
        move || {
            let Some((axis, idx)) = group_picker_open.get() else {
                return view! { <span></span> }.into_any();
            };
            let groups: &[PivotGroup] = match axis {
                0 => &pivot_for_picker.rows,
                1 => &pivot_for_picker.cols,
                _ => return view! { <span></span> }.into_any(),
            };
            let Some(g) = groups.get(idx).cloned() else {
                return view! { <span></span> }.into_any();
            };
            let col = g.source_col;
            let uniques: Vec<String> = {
                let eng = engine.lock().unwrap();
                column_unique_values(&eng, col, src_r1, src_r2)
            };
            let header_text = header_label(&engine.lock().unwrap(), col, src_r1);
            // Currently-selected set: None means "all"; otherwise
            // explicit whitelist.
            let selected: HashSet<String> = match &g.visible_values {
                Some(v) => v.iter().cloned().collect(),
                None => uniques.iter().cloned().collect(),
            };
            let axis_label = if axis == 0 { crate::t!("ss-pivot-axis-row") } else { crate::t!("ss-pivot-axis-col") };
            let on_toggle = {
                let uniques = uniques.clone();
                move |val: String, checked: bool| {
                    let mut eng = engine.lock().unwrap();
                    let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                    let mut next = current;
                    let groups: &mut Vec<PivotGroup> = match axis {
                        0 => &mut next.rows,
                        1 => &mut next.cols,
                        _ => return,
                    };
                    let Some(g) = groups.get_mut(idx) else { return; };
                    let mut set: Vec<String> = match &g.visible_values {
                        Some(v) => v.clone(),
                        None => uniques.clone(),
                    };
                    if checked {
                        if !set.contains(&val) { set.push(val); }
                    } else {
                        set.retain(|s| s != &val);
                    }
                    g.visible_values = Some(set);
                    eng.add_pivot(next);
                    drop(eng);
                    persist();
                }
            };
            let on_all = move |_| {
                let mut eng = engine.lock().unwrap();
                let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                let mut next = current;
                let groups: &mut Vec<PivotGroup> = match axis {
                    0 => &mut next.rows,
                    1 => &mut next.cols,
                    _ => return,
                };
                if let Some(g) = groups.get_mut(idx) {
                    // None is the canonical "show all" marker — keeps
                    // the doc small (no per-row whitelist persisted)
                    // and stays correct when the source range gains
                    // new distinct values.
                    g.visible_values = None;
                }
                eng.add_pivot(next);
                drop(eng);
                persist();
            };
            let on_clear = move |_| {
                let mut eng = engine.lock().unwrap();
                let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                let mut next = current;
                let groups: &mut Vec<PivotGroup> = match axis {
                    0 => &mut next.rows,
                    1 => &mut next.cols,
                    _ => return,
                };
                if let Some(g) = groups.get_mut(idx) {
                    g.visible_values = Some(Vec::new());
                }
                eng.add_pivot(next);
                drop(eng);
                persist();
            };
            let value_rows = uniques.iter().map(|val| {
                let val = val.clone();
                let val_for_label = val.clone();
                let checked = selected.contains(&val);
                let on_toggle = on_toggle.clone();
                view! {
                    <label class="ss-pivot-filter-row">
                        <input type="checkbox"
                            prop:checked=checked
                            on:change=move |e: web_sys::Event| {
                                let target = e.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                let c = target.map(|t| t.checked()).unwrap_or(false);
                                on_toggle(val.clone(), c);
                            } />
                        <span>{val_for_label}</span>
                    </label>
                }
            }).collect_view();
            view! {
                <div class="ss-pivot-filter-popover">
                    <div class="ss-pivot-filter-popover-header">
                        <span>{crate::t!("ss-pivot-labels-header", axis = axis_label, col = header_text)}</span>
                        <button class="ss-pivot-close"
                            title=crate::t!("ss-pivot-close-tooltip")
                            on:click=move |_| set_group_picker_open.set(None)>
                            "\u{2715}"
                        </button>
                    </div>
                    <div class="ss-pivot-filter-popover-actions">
                        <button class="ss-pivot-filter-action" on:click=on_all>{crate::t!("ss-pivot-filter-all")}</button>
                        <button class="ss-pivot-filter-action" on:click=on_clear>{crate::t!("ss-pivot-filter-none")}</button>
                    </div>
                    <div class="ss-pivot-filter-popover-list">
                        {value_rows}
                    </div>
                </div>
            }.into_any()
        }
    };

    // The popover is rendered as a reactive sibling of the Filters
    // zone — its visibility tracks `filter_popover_open` directly,
    // separate from the panel-level grid_version subscription, so
    // toggling it open/closed doesn't force a full panel re-render.
    let filter_popover_view = {
        let pivot_for_popover = pivot.clone();
        move || {
            let Some(idx) = filter_popover_open.get() else {
                return view! { <span></span> }.into_any();
            };
            let Some(filter) = pivot_for_popover.filters.get(idx).cloned() else {
                return view! { <span></span> }.into_any();
            };
            let col = filter.source_col;
            let uniques: Vec<String> = {
                let eng = engine.lock().unwrap();
                column_unique_values(&eng, col, src_r1, src_r2)
            };
            // Currently-selected set: only ValueIn shows checkboxes;
            // for any other condition variant we treat "all checked"
            // as the visible default and let the user opt in to
            // ValueIn semantics by toggling.
            let selected: HashSet<String> = match &filter.condition {
                PivotFilterCondition::ValueIn(v) => v.iter().cloned().collect(),
                _ => uniques.iter().cloned().collect(),
            };
            let header_text = header_label(&engine.lock().unwrap(), col, src_r1);
            let on_toggle = {
                let uniques = uniques.clone();
                move |val: String, checked: bool| {
                    let mut eng = engine.lock().unwrap();
                    let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                    let mut next = current;
                    let Some(spec) = next.filters.get_mut(idx) else { return; };
                    let mut set: Vec<String> = match &spec.condition {
                        PivotFilterCondition::ValueIn(v) => v.clone(),
                        // Treat any non-ValueIn variant as "everything was
                        // included"; transitioning here gives us a starting
                        // point that matches the visible default.
                        _ => uniques.clone(),
                    };
                    if checked {
                        if !set.contains(&val) { set.push(val); }
                    } else {
                        set.retain(|s| s != &val);
                    }
                    spec.condition = PivotFilterCondition::ValueIn(set);
                    eng.add_pivot(next);
                    drop(eng);
                    persist();
                }
            };
            let on_select_all = {
                let uniques = uniques.clone();
                move |_| {
                    let mut eng = engine.lock().unwrap();
                    let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                    let mut next = current;
                    if let Some(spec) = next.filters.get_mut(idx) {
                        spec.condition = PivotFilterCondition::ValueIn(uniques.clone());
                    }
                    eng.add_pivot(next);
                    drop(eng);
                    persist();
                }
            };
            let on_clear = move |_| {
                let mut eng = engine.lock().unwrap();
                let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
                let mut next = current;
                if let Some(spec) = next.filters.get_mut(idx) {
                    spec.condition = PivotFilterCondition::ValueIn(Vec::new());
                }
                eng.add_pivot(next);
                drop(eng);
                persist();
            };
            let value_rows = uniques.iter().map(|val| {
                let val = val.clone();
                let val_for_label = val.clone();
                let checked = selected.contains(&val);
                let on_toggle = on_toggle.clone();
                view! {
                    <label class="ss-pivot-filter-row">
                        <input type="checkbox"
                            prop:checked=checked
                            on:change=move |e: web_sys::Event| {
                                let target = e.target()
                                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
                                let c = target.map(|t| t.checked()).unwrap_or(false);
                                on_toggle(val.clone(), c);
                            } />
                        <span>{val_for_label}</span>
                    </label>
                }
            }).collect_view();
            view! {
                <div class="ss-pivot-filter-popover">
                    <div class="ss-pivot-filter-popover-header">
                        <span>{crate::t!("ss-pivot-filter-prefix", col = header_text)}</span>
                        <button class="ss-pivot-close"
                            title=crate::t!("ss-pivot-close-tooltip")
                            on:click=move |_| set_filter_popover_open.set(None)>
                            "\u{2715}"
                        </button>
                    </div>
                    <div class="ss-pivot-filter-popover-actions">
                        <button class="ss-pivot-filter-action" on:click=on_select_all>{crate::t!("ss-pivot-filter-all")}</button>
                        <button class="ss-pivot-filter-action" on:click=on_clear>{crate::t!("ss-pivot-filter-none")}</button>
                    </div>
                    <div class="ss-pivot-filter-popover-list">
                        {value_rows}
                    </div>
                </div>
            }.into_any()
        }
    };

    // ─── Drop handlers ──────────────────────────────────
    let drop_to_rows = make_drop_handler(anchor, ZoneKind::Rows, engine, persist);
    let drop_to_cols = make_drop_handler(anchor, ZoneKind::Cols, engine, persist);
    let drop_to_values = make_drop_handler(anchor, ZoneKind::Values, engine, persist);
    let drop_to_filters = make_drop_handler(anchor, ZoneKind::Filters, engine, persist);

    // ─── Remove pivot button ────────────────────────────
    let on_remove = move |_| {
        engine.lock().unwrap().remove_pivot(anchor);
        persist();
        set_pivot_editor_open.set(None);
    };

    let main_view = view! {
        <div class="ss-pivot-editor">
            <div class="ss-pivot-editor-header">
                <span class="ss-pivot-editor-title">{crate::t!("ss-pivot-title")}</span>
                <button class="ss-pivot-close"
                    title=crate::t!("ss-pivot-close-editor-tooltip")
                    on:click=move |_| set_pivot_editor_open.set(None)>"\u{2715}"</button>
            </div>
            <div class="ss-pivot-editor-body">
                <div class="ss-pivot-source-row">
                    {crate::t!("ss-pivot-source-label")}" "<code>{range_a1.clone()}</code>
                </div>
                <div class="ss-pivot-toprow">
                    {layout_select}
                    {totals_select}
                    <button class="ss-pivot-remove" on:click=on_remove>{crate::t!("ss-pivot-delete")}</button>
                </div>

                <div class="ss-pivot-section-title">{crate::t!("ss-pivot-section-fields")}</div>
                <input type="search"
                    class="ss-pivot-field-search"
                    placeholder=crate::t!("ss-pivot-search-placeholder")
                    prop:value=move || search.get()
                    on:input=move |e: web_sys::Event| set_search.set(event_target_value(&e))
                />
                <div class="ss-pivot-field-list">
                    {field_list_rows}
                </div>

                <div class="ss-pivot-section-title">{crate::t!("ss-pivot-section-rows")}</div>
                <div class="ss-pivot-zone"
                    on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); }
                    on:drop=drop_to_rows>
                    {row_chips}
                </div>

                <div class="ss-pivot-section-title">{crate::t!("ss-pivot-section-cols")}</div>
                <div class="ss-pivot-zone"
                    on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); }
                    on:drop=drop_to_cols>
                    {col_chips}
                </div>

                <div class="ss-pivot-section-title">{crate::t!("ss-pivot-section-values")}</div>
                <div class="ss-pivot-zone"
                    on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); }
                    on:drop=drop_to_values>
                    {value_chips}
                </div>

                <div class="ss-pivot-section-title">{crate::t!("ss-pivot-section-filters")}</div>
                <div class="ss-pivot-zone"
                    on:dragover=move |e: web_sys::DragEvent| { e.prevent_default(); }
                    on:drop=drop_to_filters>
                    {filter_chips}
                </div>
                {filter_popover_view}
                {group_picker_view}
            </div>
        </div>
    };
    main_view.into_any()
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ZoneKind { Rows, Cols, Values, Filters }

/// Parsed drag-payload from the editor's `text/plain` channel.
/// Two shapes:
/// - `"field:<col>"` — drag from the field-list (creates a new
///   chip in the drop zone, with a default kind/agg).
/// - `"zone:<kind>:<idx>"` — drag from inside a zone (reorders
///   within the zone or moves between zones). The chip's existing
///   PivotGroup / PivotValue / PivotFilterSpec is preserved.
enum DragPayload {
    FromField(usize),
    FromZone(ZoneKind, usize),
}

fn parse_drag_payload(s: &str) -> Option<DragPayload> {
    if let Some(rest) = s.strip_prefix("field:") {
        return rest.parse::<usize>().ok().map(DragPayload::FromField);
    }
    if let Some(rest) = s.strip_prefix("zone:") {
        let mut parts = rest.splitn(2, ':');
        let kind = parts.next()?;
        let idx = parts.next()?.parse::<usize>().ok()?;
        let zk = match kind {
            "rows" => ZoneKind::Rows,
            "cols" => ZoneKind::Cols,
            "values" => ZoneKind::Values,
            "filters" => ZoneKind::Filters,
            _ => return None,
        };
        return Some(DragPayload::FromZone(zk, idx));
    }
    None
}

/// Pick a starting `PivotFilterCondition` for a freshly-dropped
/// filter. Local sources read every distinct value in the column and
/// pre-populate `ValueIn` so the filter is "include everything" until
/// the user unchecks values via the chip popover. Foreign sources
/// don't have data resolvable here, so they fall back to `NotEmpty`
/// (the chip popover will still let the user toggle values once the
/// foreign data is loaded — `column_unique_values` reads from the
/// engine each time it renders).
fn default_filter_condition(
    eng: &SpreadsheetEngine,
    source: &SourceRange,
    col: usize,
) -> PivotFilterCondition {
    if let SourceRange::Local { range_a1 } = source {
        let (_, top, _, bottom) = parse_range(range_a1);
        let uniques = column_unique_values(eng, col, top, bottom);
        return PivotFilterCondition::ValueIn(uniques);
    }
    PivotFilterCondition::NotEmpty
}

fn make_drop_handler(
    anchor: (usize, usize),
    target_zone: ZoneKind,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
) -> impl Fn(web_sys::DragEvent) + 'static {
    move |e: web_sys::DragEvent| {
        e.prevent_default();
        let Some(dt) = e.data_transfer() else { return; };
        let Ok(payload_str) = dt.get_data("text/plain") else { return; };
        let Some(payload) = parse_drag_payload(&payload_str) else { return; };
        let mut eng = engine.lock().unwrap();
        let Some(current) = eng.get_pivot(anchor).cloned() else { return; };
        let mut next = current;
        match payload {
            DragPayload::FromField(col) => {
                let already_here = match target_zone {
                    ZoneKind::Rows => next.rows.iter().any(|g| g.source_col == col),
                    ZoneKind::Cols => next.cols.iter().any(|g| g.source_col == col),
                    ZoneKind::Values => next.values.iter().any(|v| v.source_col == col),
                    ZoneKind::Filters => next.filters.iter().any(|f| f.source_col == col),
                };
                if already_here { return; }
                remove_field_from_zones(&mut next, col);
                match target_zone {
                    ZoneKind::Rows => next.rows.push(PivotGroup { source_col: col, ..PivotGroup::default() }),
                    ZoneKind::Cols => next.cols.push(PivotGroup { source_col: col, ..PivotGroup::default() }),
                    ZoneKind::Values => next.values.push(PivotValue {
                        source_col: col,
                        summarize_fn: SummarizeFn::Sum,
                        display_name: None,
                    }),
                    ZoneKind::Filters => {
                        // Default to ValueIn(<every unique value>) so
                        // dropping a column on Filters initially
                        // includes everything; the user opens the chip
                        // popover and unchecks values to exclude. The
                        // previous default (NotEmpty) had no UI for
                        // editing and silently kept the user from ever
                        // configuring a real filter.
                        let condition = default_filter_condition(&eng, &next.source, col);
                        next.filters.push(PivotFilterSpec { source_col: col, condition });
                    }
                }
            }
            DragPayload::FromZone(src_zone, src_idx) => {
                // Take the entry out of its source zone, then insert
                // at the end of the target zone. v0b doesn't position
                // by cursor Y — drops always append. Cross-zone moves
                // preserve the chip's full config (sort order, agg
                // fn, etc.) when source/target shapes are compatible
                // (Rows ↔ Cols both carry PivotGroup); shape-mismatch
                // moves fall back to a fresh default entry.
                if !move_zone_entry(&mut next, src_zone, src_idx, target_zone) { return; }
            }
        }
        eng.add_pivot(next);
        drop(eng);
        persist();
    }
}

/// Move (or reorder) one chip between zones. Returns false on no-op
/// (e.g. invalid src_idx).
fn move_zone_entry(
    p: &mut PivotTable,
    src: ZoneKind,
    src_idx: usize,
    dst: ZoneKind,
) -> bool {
    // Pull the source entry out and remember it as a "chip" — three
    // shapes that map onto the four zones (Rows + Cols share Group;
    // Values is its own; Filters is its own).
    enum Pulled {
        Group(PivotGroup),
        Value(PivotValue),
        Filter(PivotFilterSpec),
    }
    let pulled = match src {
        ZoneKind::Rows => {
            if src_idx >= p.rows.len() { return false; }
            Pulled::Group(p.rows.remove(src_idx))
        }
        ZoneKind::Cols => {
            if src_idx >= p.cols.len() { return false; }
            Pulled::Group(p.cols.remove(src_idx))
        }
        ZoneKind::Values => {
            if src_idx >= p.values.len() { return false; }
            Pulled::Value(p.values.remove(src_idx))
        }
        ZoneKind::Filters => {
            if src_idx >= p.filters.len() { return false; }
            Pulled::Filter(p.filters.remove(src_idx))
        }
    };
    match (pulled, dst) {
        // Same-shape moves preserve all config.
        (Pulled::Group(g), ZoneKind::Rows) => p.rows.push(g),
        (Pulled::Group(g), ZoneKind::Cols) => p.cols.push(g),
        (Pulled::Value(v), ZoneKind::Values) => p.values.push(v),
        (Pulled::Filter(f), ZoneKind::Filters) => p.filters.push(f),
        // Cross-shape moves keep only the source column; the
        // destination zone needs a different shape so we synthesize
        // a default with the dragged chip's source_col.
        (Pulled::Group(g), ZoneKind::Values) => p.values.push(PivotValue {
            source_col: g.source_col,
            summarize_fn: SummarizeFn::Sum,
            display_name: None,
        }),
        (Pulled::Group(g), ZoneKind::Filters) => p.filters.push(PivotFilterSpec {
            source_col: g.source_col,
            condition: PivotFilterCondition::NotEmpty,
        }),
        (Pulled::Value(v), ZoneKind::Rows) => p.rows.push(PivotGroup {
            source_col: v.source_col, ..PivotGroup::default()
        }),
        (Pulled::Value(v), ZoneKind::Cols) => p.cols.push(PivotGroup {
            source_col: v.source_col, ..PivotGroup::default()
        }),
        (Pulled::Value(v), ZoneKind::Filters) => p.filters.push(PivotFilterSpec {
            source_col: v.source_col,
            condition: PivotFilterCondition::NotEmpty,
        }),
        (Pulled::Filter(f), ZoneKind::Rows) => p.rows.push(PivotGroup {
            source_col: f.source_col, ..PivotGroup::default()
        }),
        (Pulled::Filter(f), ZoneKind::Cols) => p.cols.push(PivotGroup {
            source_col: f.source_col, ..PivotGroup::default()
        }),
        (Pulled::Filter(f), ZoneKind::Values) => p.values.push(PivotValue {
            source_col: f.source_col,
            summarize_fn: SummarizeFn::Sum,
            display_name: None,
        }),
    }
    true
}

fn field_in_use(p: &PivotTable, col: usize) -> bool {
    p.rows.iter().any(|g| g.source_col == col)
        || p.cols.iter().any(|g| g.source_col == col)
        || p.values.iter().any(|v| v.source_col == col)
        || p.filters.iter().any(|f| f.source_col == col)
}

fn remove_field_from_zones(p: &mut PivotTable, col: usize) {
    p.rows.retain(|g| g.source_col != col);
    p.cols.retain(|g| g.source_col != col);
    p.values.retain(|v| v.source_col != col);
    p.filters.retain(|f| f.source_col != col);
}

/// Inline kind controls for a Rows/Cols chip:
/// - `Direct` → the cell stays plain (drag handle + label only).
/// - `Date(g)` → a small `<select>` of granularity options.
/// - `NumericBin { width, .. }` → a numeric input for the bin width.
///
/// Mutating either control rebuilds the chip's `kind` and routes
/// through the standard clone-mutate-replace flow on the engine.
fn render_kind_controls(
    anchor: (usize, usize),
    zone: ZoneKind,
    idx: usize,
    kind: PivotGroupKind,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
) -> AnyView {
    match kind {
        PivotGroupKind::Direct => view! { <span></span> }.into_any(),
        PivotGroupKind::Date(current) => {
            view! {
                <select class="ss-pivot-chip-kind"
                    on:change=move |e: web_sys::Event| {
                        let v = event_target_value(&e);
                        let g = match v.as_str() {
                            "year" => DateGranularity::Year,
                            "quarter" => DateGranularity::Quarter,
                            "day" => DateGranularity::Day,
                            "hour" => DateGranularity::Hour,
                            _ => DateGranularity::Month,
                        };
                        let mut eng = engine.lock().unwrap();
                        if let Some(cur) = eng.get_pivot(anchor).cloned() {
                            let mut next = cur;
                            let groups = match zone {
                                ZoneKind::Rows => &mut next.rows,
                                ZoneKind::Cols => &mut next.cols,
                                _ => return,
                            };
                            if let Some(entry) = groups.get_mut(idx) {
                                entry.kind = PivotGroupKind::Date(g);
                            }
                            eng.add_pivot(next);
                            drop(eng);
                            persist();
                        }
                    }>
                    <option value="year" selected=move || matches!(current, DateGranularity::Year)>{crate::t!("ss-pivot-date-year")}</option>
                    <option value="quarter" selected=move || matches!(current, DateGranularity::Quarter)>{crate::t!("ss-pivot-date-quarter")}</option>
                    <option value="month" selected=move || matches!(current, DateGranularity::Month)>{crate::t!("ss-pivot-date-month")}</option>
                    <option value="day" selected=move || matches!(current, DateGranularity::Day)>{crate::t!("ss-pivot-date-day")}</option>
                    <option value="hour" selected=move || matches!(current, DateGranularity::Hour)>{crate::t!("ss-pivot-date-hour")}</option>
                </select>
            }.into_any()
        }
        PivotGroupKind::NumericBin { width, start } => {
            view! {
                <input type="number"
                    class="ss-pivot-chip-kind ss-pivot-chip-bin"
                    prop:value=width
                    title=crate::t!("ss-pivot-bin-width-tooltip")
                    on:change=move |e: web_sys::Event| {
                        let v = event_target_value(&e);
                        let Ok(w) = v.parse::<f64>() else { return; };
                        if !w.is_finite() || w <= 0.0 { return; }
                        let mut eng = engine.lock().unwrap();
                        if let Some(cur) = eng.get_pivot(anchor).cloned() {
                            let mut next = cur;
                            let groups = match zone {
                                ZoneKind::Rows => &mut next.rows,
                                ZoneKind::Cols => &mut next.cols,
                                _ => return,
                            };
                            if let Some(entry) = groups.get_mut(idx) {
                                entry.kind = PivotGroupKind::NumericBin { width: w, start };
                            }
                            eng.add_pivot(next);
                            drop(eng);
                            persist();
                        }
                    }
                />
            }.into_any()
        }
    }
}

fn parse_summarize_fn(v: &str) -> SummarizeFn {
    match v {
        "count" => SummarizeFn::Count,
        "countA" => SummarizeFn::CountA,
        "average" => SummarizeFn::Average,
        "min" => SummarizeFn::Min,
        "max" => SummarizeFn::Max,
        "median" => SummarizeFn::Median,
        "product" => SummarizeFn::Product,
        "stdDev" => SummarizeFn::StdDev,
        "stdDevP" => SummarizeFn::StdDevP,
        "var" => SummarizeFn::Var,
        "varP" => SummarizeFn::VarP,
        _ => SummarizeFn::Sum,
    }
}

fn summarize_fn_options(current: SummarizeFn) -> impl IntoView {
    let mk = move |val: &'static str, label: &'static str, sf: SummarizeFn| {
        view! {
            <option value=val selected=move || sf == current>{label}</option>
        }
    };
    view! {
        <>
            {mk("sum", "SUM", SummarizeFn::Sum)}
            {mk("count", "COUNT", SummarizeFn::Count)}
            {mk("countA", "COUNTA", SummarizeFn::CountA)}
            {mk("average", "AVG", SummarizeFn::Average)}
            {mk("min", "MIN", SummarizeFn::Min)}
            {mk("max", "MAX", SummarizeFn::Max)}
            {mk("median", "MEDIAN", SummarizeFn::Median)}
            {mk("product", "PRODUCT", SummarizeFn::Product)}
            {mk("stdDev", "STDDEV", SummarizeFn::StdDev)}
            {mk("stdDevP", "STDDEVP", SummarizeFn::StdDevP)}
            {mk("var", "VAR", SummarizeFn::Var)}
            {mk("varP", "VARP", SummarizeFn::VarP)}
        </>
    }
}

fn filter_cond_label(c: &PivotFilterCondition) -> String {
    match c {
        PivotFilterCondition::ValueIn(v) => format!("in [{}]", v.join(", ")),
        PivotFilterCondition::NumberGreater(n) => format!("> {n}"),
        PivotFilterCondition::NumberLess(n) => format!("< {n}"),
        PivotFilterCondition::NumberEqual(n) => format!("= {n}"),
        PivotFilterCondition::NumberBetween(lo, hi) => format!("{lo}–{hi}"),
        PivotFilterCondition::TextContains(s) => format!("contains \"{s}\""),
        PivotFilterCondition::TextEquals(s) => format!("= \"{s}\""),
        PivotFilterCondition::TextStartsWith(s) => format!("starts \"{s}\""),
        PivotFilterCondition::Empty => "empty".to_string(),
        PivotFilterCondition::NotEmpty => "not empty".to_string(),
    }
}

// Make the unused enums / fields silent for v0a (referenced by
// the value_layout / subtotals_position roundtrip but not yet
// exposed as UI controls).
#[allow(dead_code)] fn _unused_value_layout(_v: ValueLayout) {}
#[allow(dead_code)] fn _unused_subtotals(_v: SubtotalsPos) {}
#[allow(dead_code)] fn _unused_sort(_v: SortOrder) {}
