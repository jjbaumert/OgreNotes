// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document <-> SpreadsheetEngine serialization.
//!
//! Pure data-conversion helpers for the spreadsheet view: build a
//! `Node` table from engine state, build a multi-sheet document by
//! splicing one sheet's table into an existing doc, list sheet names
//! out of a doc, hydrate engine state from one of those tables, and
//! parse a single RFC-4180-flavored CSV line for the paste path. No
//! Leptos signals, no DOM access — just `Node` and engine APIs — so
//! everything here is unit-testable from non-WASM tests.

use std::collections::HashMap;

use crate::editor::model::{Fragment, Node, NodeType};
use crate::editor::state::EditorState;
use crate::spreadsheet::eval::{
    CellStyle, ChartConfig, ChartType, ConditionalCondition, ConditionalFormat,
    ForeignDocSnapshot, IconSetKind, SpreadsheetEngine, ValidationRule,
};
use crate::spreadsheet::parser::{CellRef, RangeRef, is_valid_named_range_name};
use crate::spreadsheet::pivot::{
    DateGranularity, GrandTotals, LayoutStyle, PivotFilterCondition, PivotFilterSpec,
    PivotGroup, PivotGroupKind, PivotTable, PivotValue, SortOrder, SourceRange,
    SubtotalsPos, SummarizeFn, ValueLayout,
};

// ─── Cell + table attribute names (HTML-flavored, persisted to docs) ─

const ATTR_BOLD: &str = "bold";
const ATTR_ITALIC: &str = "italic";
const ATTR_UNDERLINE: &str = "underline";
const ATTR_STRIKE: &str = "strike";
const ATTR_BG_COLOR: &str = "bgColor";
const ATTR_TEXT_COLOR: &str = "textColor";
const ATTR_ALIGN: &str = "align";
const ATTR_NUMBER_FORMAT: &str = "numberFormat";
const ATTR_VALIDATION_TYPE: &str = "validationType";
const ATTR_VALIDATION_OPTIONS: &str = "validationOptions";
const ATTR_SHEET_NAME: &str = "sheetName";
const ATTR_LOCKED: &str = "locked";
const ATTR_COMMENT: &str = "comment";
const ATTR_COMMENT_THREAD_ID: &str = "commentThreadId";
const ATTR_HIDDEN_ROWS: &str = "hiddenRows";
const ATTR_HIDDEN_COLS: &str = "hiddenCols";
const ATTR_MERGED_REGIONS: &str = "mergedRegions";
const ATTR_CONDITIONAL_FORMATS: &str = "conditionalFormats";
const ATTR_NAMED_RANGES: &str = "namedRanges";
const ATTR_FROZEN_ROWS: &str = "frozenRows";
const ATTR_FROZEN_COLS: &str = "frozenCols";
const ATTR_PIVOTS: &str = "pivots";
const ATTR_CHARTS: &str = "charts";
// #128: the view-scroll extent (grid_rows / grid_cols) recorded on the
// `<table>` so a deliberately-enlarged grid survives a round-trip even
// though only the used bounding box is materialized. Emitted only when
// the view extent exceeds the data extent (so data-sized sheets stay
// byte-identical to their pre-#128 form). On load the client seeds
// grid_rows/grid_cols from these, falling back to max(data_extent, 10).
const ATTR_GRID_ROWS: &str = "gridRows";
const ATTR_GRID_COLS: &str = "gridCols";
/// Marker on a `<table_cell>` indicating its content was rendered
/// from a pivot's spill (or a dynamic-array formula). The cell's
/// raw text holds the displayed value so XLSX export captures the
/// rendered output. On load, the cell-import loop SKIPS marked
/// cells: the engine's `set_pivots`/spill machinery reinstalls
/// them from the typed `PivotTable` config preserved in
/// `ATTR_PIVOTS`. Non-pivot consumers (e.g. plain XLSX import)
/// simply read the text and ignore the marker.
const ATTR_SPILLED: &str = "spilled";

pub(super) const DEFAULT_SHEET_NAME: &str = "Sheet1";

// ─── Helpers ───────────────────────────────────────────────────

/// Build a single table node from engine data, tagged with a sheet name.
///
/// Stable, position-derived `blockId`s are set on the Table, every
/// TableRow, every TableCell, and every Paragraph inside a cell.
/// Without this, each persist would auto-generate fresh random IDs,
/// `find_match` in `yrs_bridge` would treat every node as new, and
/// every save would emit a full delete+reinsert update from yrs
/// (~120KB for a 78-row sheet) instead of a small incremental diff.
/// With stable IDs, `sync_model_to_ydoc` recognizes structural
/// identity across saves and yrs produces minimal updates: cell-text
/// updates for shifted rows on a delete, single-row removal for
/// trailing rows. Sheet name is the namespace so multi-sheet docs
/// don't collide.
/// Bottom-right `(max_row, max_col)` of the sheet's *used* range, or
/// `None` when nothing is worth persisting. "Used" is wider than
/// `last_used_cell` (which is raw-only and backs Ctrl+End): a cell
/// counts if it holds raw content, an explicit style (number format,
/// border, fill, alignment, lock, or data validation — validation lives
/// inside `CellStyle`), lies in a merged region or a conditional-format
/// range, or is a spill fill cell. Used by `build_table_from_engine`
/// (#128) so formatting-only / merged / spilled cells are not silently
/// trimmed out of the persisted doc.
pub(super) fn used_extent_for_persist(
    engine: &SpreadsheetEngine,
) -> Option<(usize, usize)> {
    let mut max_r = 0usize;
    let mut max_c = 0usize;
    let mut any = false;
    // CellAddr is (col, row).
    for ((c, r), raw) in engine.iter_raw() {
        if !raw.is_empty() {
            max_r = max_r.max(r);
            max_c = max_c.max(c);
            any = true;
        }
    }
    for (c, r) in engine.iter_styled_cells() {
        max_r = max_r.max(r);
        max_c = max_c.max(c);
        any = true;
    }
    for (c, r) in engine.iter_spill_fill_cells() {
        max_r = max_r.max(r);
        max_c = max_c.max(c);
        any = true;
    }
    for &(c, r, cs, rs) in engine.get_merged_regions() {
        max_r = max_r.max(r + rs.saturating_sub(1));
        max_c = max_c.max(c + cs.saturating_sub(1));
        any = true;
    }
    for &(_tl, (bc, br), ref _rules) in engine.get_conditional_formats() {
        max_r = max_r.max(br);
        max_c = max_c.max(bc);
        any = true;
    }
    any.then_some((max_r, max_c))
}

pub(super) fn build_table_from_engine(engine: &SpreadsheetEngine, rows: usize, cols: usize, sheet_name: &str) -> Node {
    // #128: persist only the used bounding box, not the view-scroll
    // extent the caller passes. Navigating/scrolling grows grid_rows /
    // grid_cols, and materializing every empty cell up to there bloats
    // the doc and makes every walker (yrs sync, normalize, hash,
    // engine reload) O(rows×cols) regardless of how few cells hold data.
    // The caller's extent survives as gridRows/gridCols attrs below, so
    // the grid rehydrates to the same size without materializing empties.
    //
    // "Used" spans raw content, explicit styles (number formats, borders,
    // fills, validations), merged regions, conditional-format ranges, and
    // spill fill cells — not just raw values. The old `last_used_cell`
    // was raw-only, so a formatting-only or merged cell beyond the raw
    // extent was silently trimmed away on round-trip; this fixes that and
    // supersedes the 868032e truncation guard (any persist path still
    // covers every occupied cell). An empty sheet yields a 0-row table,
    // kept alive by the normalize_doc spreadsheet-table exception.
    let used = used_extent_for_persist(engine);
    let data_rows = used.map_or(0, |(ur, _)| ur + 1);
    let data_cols = used.map_or(0, |(_, uc)| uc + 1);
    let view_rows = rows;
    let view_cols = cols;
    let table_rows: Vec<Node> = (0..data_rows)
        .map(|r| {
            let cells: Vec<Node> = (0..data_cols)
                .map(|c| {
                    let raw = engine.get_raw((c, r));
                    let is_spilled = engine.is_spill_fill((c, r));
                    // For spill-fill cells, raw is empty (the value is
                    // synthesized from the anchor's CellValue::Array).
                    // Persist the rendered display text so XLSX export
                    // and other consumers reading the doc directly see
                    // the pivot/array output. The ATTR_SPILLED marker
                    // tells our own loader to skip the cell — the
                    // engine reinstalls the value via the spill block
                    // when `set_pivots` runs.
                    let cell_text = if is_spilled {
                        engine.get_display((c, r))
                    } else {
                        raw.to_string()
                    };
                    let para_block_id = format!("ss:{sheet_name}:p:{r}:{c}");
                    let mut para_attrs = std::collections::HashMap::new();
                    para_attrs.insert("blockId".to_string(), para_block_id);
                    let content = if cell_text.is_empty() {
                        Fragment::from(vec![Node::element_with_attrs(
                            NodeType::Paragraph,
                            para_attrs,
                            Fragment::empty(),
                        )])
                    } else {
                        Fragment::from(vec![Node::element_with_attrs(
                            NodeType::Paragraph,
                            para_attrs,
                            Fragment::from(vec![Node::text(&cell_text)]),
                        )])
                    };
                    let mut attrs = std::collections::HashMap::new();
                    attrs.insert("colspan".to_string(), "1".to_string());
                    attrs.insert("rowspan".to_string(), "1".to_string());
                    attrs.insert("blockId".to_string(), format!("ss:{sheet_name}:c:{r}:{c}"));
                    if is_spilled {
                        attrs.insert(ATTR_SPILLED.to_string(), "1".to_string());
                    }
                    if let Some(style) = engine.get_style((c, r)) {
                        if style.bold { attrs.insert(ATTR_BOLD.to_string(), "1".to_string()); }
                        if style.italic { attrs.insert(ATTR_ITALIC.to_string(), "1".to_string()); }
                        if style.underline { attrs.insert(ATTR_UNDERLINE.to_string(), "1".to_string()); }
                        if style.strike { attrs.insert(ATTR_STRIKE.to_string(), "1".to_string()); }
                        if let Some(ref bg) = style.bg_color { attrs.insert(ATTR_BG_COLOR.to_string(), bg.clone()); }
                        if let Some(ref tc) = style.text_color { attrs.insert(ATTR_TEXT_COLOR.to_string(), tc.clone()); }
                        if let Some(ref a) = style.align { attrs.insert(ATTR_ALIGN.to_string(), a.clone()); }
                        if let Some(ref nf) = style.number_format { attrs.insert(ATTR_NUMBER_FORMAT.to_string(), nf.clone()); }
                        if style.locked { attrs.insert(ATTR_LOCKED.to_string(), "1".to_string()); }
                        if let Some(ref c) = style.comment {
                            if !c.is_empty() {
                                attrs.insert(ATTR_COMMENT.to_string(), c.clone());
                            }
                        }
                        if let Some(ref tid) = style.comment_thread_id {
                            if !tid.is_empty() {
                                attrs.insert(ATTR_COMMENT_THREAD_ID.to_string(), tid.clone());
                            }
                        }
                        if let Some(ref v) = style.validation {
                            match v {
                                ValidationRule::Checkbox => { attrs.insert(ATTR_VALIDATION_TYPE.to_string(), "checkbox".to_string()); }
                                ValidationRule::Dropdown(opts) => {
                                    attrs.insert(ATTR_VALIDATION_TYPE.to_string(), "dropdown".to_string());
                                    attrs.insert(ATTR_VALIDATION_OPTIONS.to_string(), opts.join(","));
                                }
                                ValidationRule::Number { min, max } => {
                                    attrs.insert(ATTR_VALIDATION_TYPE.to_string(), "number".to_string());
                                    let mut parts = Vec::new();
                                    if let Some(mn) = min { parts.push(format!("min={mn}")); }
                                    if let Some(mx) = max { parts.push(format!("max={mx}")); }
                                    if !parts.is_empty() { attrs.insert(ATTR_VALIDATION_OPTIONS.to_string(), parts.join(",")); }
                                }
                            }
                        }
                    }
                    Node::element_with_attrs(NodeType::TableCell, attrs, content)
                })
                .collect();
            let mut row_attrs = std::collections::HashMap::new();
            row_attrs.insert("blockId".to_string(), format!("ss:{sheet_name}:r:{r}"));
            Node::element_with_attrs(NodeType::TableRow, row_attrs, Fragment::from(cells))
        })
        .collect();
    let mut table_attrs = std::collections::HashMap::new();
    table_attrs.insert(ATTR_SHEET_NAME.to_string(), sheet_name.to_string());
    table_attrs.insert("blockId".to_string(), format!("ss:{sheet_name}:t"));

    // #128: record the view-scroll extent only when it exceeds the
    // materialized data extent — otherwise the grid would shrink to the
    // data on reload, losing the empty scroll room the user navigated to.
    if view_rows > data_rows {
        table_attrs.insert(ATTR_GRID_ROWS.to_string(), view_rows.to_string());
    }
    if view_cols > data_cols {
        table_attrs.insert(ATTR_GRID_COLS.to_string(), view_cols.to_string());
    }

    // Persist hidden rows/cols
    if !engine.hidden_rows.is_empty() {
        let mut sorted: Vec<usize> = engine.hidden_rows.iter().copied().collect();
        sorted.sort();
        table_attrs.insert(ATTR_HIDDEN_ROWS.to_string(), sorted.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(","));
    }
    if !engine.hidden_cols.is_empty() {
        let mut sorted: Vec<usize> = engine.hidden_cols.iter().copied().collect();
        sorted.sort();
        table_attrs.insert(ATTR_HIDDEN_COLS.to_string(), sorted.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","));
    }

    // Persist merged regions
    let regions = engine.get_merged_regions();
    if !regions.is_empty() {
        let json: Vec<String> = regions.iter().map(|&(c, r, cs, rs)| format!("{{\"c\":{c},\"r\":{r},\"cs\":{cs},\"rs\":{rs}}}")).collect();
        table_attrs.insert(ATTR_MERGED_REGIONS.to_string(), format!("[{}]", json.join(",")));
    }

    // Persist conditional formats. Schema:
    //   [{"tl":[c,r],"br":[c,r],"rules":[ <rule> ... ]}]
    // where <rule> is one of:
    //   {"kind":"single","cond":{"type":"gt","val":10},"bg":"#hex"}
    //   {"kind":"colorScale","low":"#hex","mid":"#hex"|null,"high":"#hex"}
    //   {"kind":"dataBar","color":"#hex"}
    //   {"kind":"iconSet","set":"3Arrows"|"3TrafficLights"}
    let cfs = engine.get_conditional_formats();
    if !cfs.is_empty() {
        let json = serialize_conditional_formats(cfs);
        table_attrs.insert(ATTR_CONDITIONAL_FORMATS.to_string(), json);
    }

    // Persist named ranges. Schema:
    //   [{"name":"PROFIT","tl":[c,r,absC,absR],"br":[c,r,absC,absR]}]
    // The four-element corner tuple records the absolute-axis flags
    // so a name defined on `$B$2:$B$10` stays absolute on round-trip.
    let names = engine.named_ranges();
    if !names.is_empty() {
        table_attrs.insert(ATTR_NAMED_RANGES.to_string(), serialize_named_ranges(&names));
    }

    // Persist frozen-pane counts (M-S2). Emit only when non-zero so
    // workspaces without a freeze stay byte-identical to their
    // pre-S2 serialized form.
    if engine.frozen_rows > 0 {
        table_attrs.insert(ATTR_FROZEN_ROWS.to_string(), engine.frozen_rows.to_string());
    }
    if engine.frozen_cols > 0 {
        table_attrs.insert(ATTR_FROZEN_COLS.to_string(), engine.frozen_cols.to_string());
    }

    // Persist pivot tables (M-S2 v2). Same JSON-in-attr shape as
    // conditionalFormats. Emit only when non-empty so docs without
    // pivots stay byte-identical to their pre-pivot-v2 form.
    let pivots: Vec<&PivotTable> = engine.pivots_iter().map(|(_, p)| p).collect();
    if !pivots.is_empty() {
        table_attrs.insert(ATTR_PIVOTS.to_string(), serialize_pivots(&pivots));
    }

    // Persist embedded charts. Same JSON-in-attr shape; emitted
    // only when the engine actually holds charts so docs without
    // them stay byte-identical to their pre-chart-persistence form.
    if !engine.charts.is_empty() {
        table_attrs.insert(ATTR_CHARTS.to_string(), serialize_charts(&engine.charts));
    }

    Node::element_with_attrs(NodeType::Table, table_attrs, Fragment::from(table_rows))
}

/// Build a full Doc from existing sheets, replacing the active sheet's table with engine data.
pub(super) fn build_doc_with_sheets(
    existing_doc: &Node,
    engine: &SpreadsheetEngine,
    active_sheet: usize,
    rows: usize,
    cols: usize,
    all_names: &[String],
) -> Node {
    let sheet_name = all_names.get(active_sheet).map(|s| s.as_str()).unwrap_or(DEFAULT_SHEET_NAME);
    let new_table = build_table_from_engine(engine, rows, cols, sheet_name);
    if let Node::Element { content, .. } = existing_doc {
        // Preserve non-table children (e.g., metadata), rebuild table list
        let mut children: Vec<Node> = Vec::new();
        let mut table_idx = 0usize;
        for child in &content.children {
            if child.node_type() == Some(NodeType::Table) {
                if table_idx == active_sheet {
                    children.push(new_table.clone());
                } else {
                    // Update sheet name attr on non-active tables if renamed
                    let mut table = child.clone();
                    if let Some(name) = all_names.get(table_idx) {
                        if let Node::Element { ref mut attrs, .. } = table {
                            attrs.insert(ATTR_SHEET_NAME.to_string(), name.clone());
                        }
                    }
                    children.push(table);
                }
                table_idx += 1;
            } else {
                children.push(child.clone()); // preserve non-table nodes
            }
        }
        // If active_sheet is beyond existing tables, append it
        if active_sheet >= table_idx {
            children.push(new_table);
        }
        Node::element_with_content(NodeType::Doc, Fragment::from(children))
    } else {
        Node::element_with_content(NodeType::Doc, Fragment::from(vec![new_table]))
    }
}

/// Rebuild `existing_doc` with the table at `drop_idx` removed.
/// Non-table children (metadata, etc.) and other tables are
/// preserved in their original order. If `drop_idx` is past the
/// last table, returns the doc unchanged. Used by the sheet-tab
/// "Delete sheet" action; the caller is expected to push the new
/// doc through `on_state_change` + `on_change` and adjust the
/// active-sheet index.
pub(super) fn build_doc_dropping_sheet(existing_doc: &Node, drop_idx: usize) -> Node {
    if let Node::Element { content, .. } = existing_doc {
        let mut children: Vec<Node> = Vec::new();
        let mut table_idx = 0_usize;
        for child in &content.children {
            if child.node_type() == Some(NodeType::Table) {
                if table_idx != drop_idx {
                    children.push(child.clone());
                }
                table_idx += 1;
            } else {
                children.push(child.clone());
            }
        }
        return Node::element_with_content(NodeType::Doc, Fragment::from(children));
    }
    existing_doc.clone()
}

/// Extract sheet names from the doc (one per Table node).
pub(super) fn extract_sheet_names(doc: &Node) -> Vec<String> {
    let mut names = Vec::new();
    if let Node::Element { content, .. } = doc {
        for (i, child) in content.children.iter().enumerate() {
            if child.node_type() == Some(NodeType::Table) {
                let name = if let Node::Element { attrs, .. } = child {
                    attrs.get(ATTR_SHEET_NAME).cloned()
                } else { None };
                names.push(name.unwrap_or_else(|| format!("Sheet{}", i + 1)));
            }
        }
    }
    if names.is_empty() { names.push(DEFAULT_SHEET_NAME.to_string()); }
    names
}

/// Sync engine from the nth table in the doc.
/// Walk every Table child of `doc` and produce a flat
/// `ForeignDocSnapshot` keyed by `ATTR_SHEET_NAME` with each sheet
/// rendered as `Vec<Vec<String>>` (row-major, cell display text).
/// Used by the cross-document REFERENCERANGE / REFERENCESHEET path
/// after the view layer has fetched a foreign doc's CRDT bytes and
/// decoded them via `ydoc_bytes_to_doc`.
///
/// Sheets without `ATTR_SHEET_NAME` get an indexed default
/// (`Sheet1`, `Sheet2`, ...) so the foreign doc is still
/// addressable. Cell values come from `text_content()` —
/// formula cells already render as their displayed result, so
/// consumers see the source's published text without re-running
/// the source's formulas.
pub(super) fn snapshot_foreign_doc(doc: &Node) -> ForeignDocSnapshot {
    let mut sheets: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    if let Node::Element { content, .. } = doc {
        let mut table_idx = 0usize;
        for child in &content.children {
            if child.node_type() != Some(NodeType::Table) { continue; }
            let Node::Element { attrs, content: tc, .. } = child else { continue };
            let name = attrs.get(ATTR_SHEET_NAME).cloned()
                .unwrap_or_else(|| format!("Sheet{}", table_idx + 1));
            let mut rows: Vec<Vec<String>> = Vec::new();
            for row_node in &tc.children {
                let Node::Element { content: rc, .. } = row_node else { continue };
                let cells: Vec<String> = rc.children.iter()
                    .map(|cell| cell.text_content())
                    .collect();
                rows.push(cells);
            }
            sheets.insert(name, rows);
            table_idx += 1;
        }
    }
    ForeignDocSnapshot { sheets }
}

pub(super) fn sync_engine_from_doc_sheet(engine: &mut SpreadsheetEngine, state: &EditorState, sheet_index: usize) -> (usize, usize) {
    engine.clear(); // reset engine to avoid stale data from previous sheet
    let mut num_rows = 0usize;
    let mut num_cols = 0usize;

    let Node::Element { content, .. } = &state.doc else { return (0, 0) };
    let table = content.children.iter()
        .filter(|c| c.node_type() == Some(NodeType::Table))
        .nth(sheet_index);
    let Some(Node::Element { content: tc, attrs: table_attrs, .. }) = table else { return (0, 0) };

    for (r, row_node) in tc.children.iter().enumerate() {
        let Node::Element { content: rc, .. } = row_node else { continue };
        for (c, cell_node) in rc.children.iter().enumerate() {
            // Skip spilled cells — their content was persisted only
            // so XLSX export captures the rendered output. The
            // engine reinstalls them when `set_pivots` (or a future
            // dynamic-array reload path) processes the typed config
            // a few lines below.
            let is_spilled = match cell_node {
                Node::Element { attrs, .. } => attrs.get(ATTR_SPILLED).map_or(false, |v| v == "1"),
                _ => false,
            };
            if !is_spilled {
                engine.set_cell((c, r), &cell_node.text_content());
            }
            if let Node::Element { attrs, .. } = cell_node {
                let validation = match attrs.get(ATTR_VALIDATION_TYPE).map(|s| s.as_str()) {
                    Some("checkbox") => Some(ValidationRule::Checkbox),
                    Some("dropdown") => {
                        let opts = attrs.get(ATTR_VALIDATION_OPTIONS)
                            .map(|s| s.split(',').map(|o| o.to_string()).collect())
                            .unwrap_or_default();
                        Some(ValidationRule::Dropdown(opts))
                    }
                    Some("number") => {
                        let opts = attrs.get(ATTR_VALIDATION_OPTIONS).cloned().unwrap_or_default();
                        let mut min = None;
                        let mut max = None;
                        for part in opts.split(',') {
                            if let Some(v) = part.strip_prefix("min=") { min = v.parse().ok(); }
                            if let Some(v) = part.strip_prefix("max=") { max = v.parse().ok(); }
                        }
                        Some(ValidationRule::Number { min, max })
                    }
                    _ => None,
                };
                let style = CellStyle {
                    bold: attrs.get(ATTR_BOLD).map_or(false, |v| v == "1"),
                    italic: attrs.get(ATTR_ITALIC).map_or(false, |v| v == "1"),
                    underline: attrs.get(ATTR_UNDERLINE).map_or(false, |v| v == "1"),
                    strike: attrs.get(ATTR_STRIKE).map_or(false, |v| v == "1"),
                    bg_color: attrs.get(ATTR_BG_COLOR).cloned(),
                    text_color: attrs.get(ATTR_TEXT_COLOR).cloned(),
                    align: attrs.get(ATTR_ALIGN).cloned(),
                    number_format: attrs.get(ATTR_NUMBER_FORMAT).cloned(),
                    validation,
                    locked: attrs.get(ATTR_LOCKED).map_or(false, |v| v == "1"),
                    // Normalize present-but-empty back to `None` so the
                    // round-trip is symmetric with the writer (which
                    // drops empty / absent). Without this, a foreign
                    // doc with `comment=""` would leave a stale
                    // `Some("")` in engine state until the next save.
                    comment: attrs.get(ATTR_COMMENT).filter(|s| !s.is_empty()).cloned(),
                    comment_thread_id: attrs.get(ATTR_COMMENT_THREAD_ID).filter(|s| !s.is_empty()).cloned(),
                };
                if style != CellStyle::default() {
                    engine.set_style((c, r), style);
                }
            }
            num_cols = num_cols.max(c + 1);
        }
        num_rows = num_rows.max(r + 1);
    }

    // Load table-level attributes: hidden rows/cols, merged regions
    if let Some(Node::Element { attrs, .. }) = table {
        if let Some(hr) = attrs.get(ATTR_HIDDEN_ROWS) {
            for idx in hr.split(',').filter_map(|s| s.trim().parse::<usize>().ok()) {
                engine.hidden_rows.insert(idx);
            }
        }
        if let Some(hc) = attrs.get(ATTR_HIDDEN_COLS) {
            for idx in hc.split(',').filter_map(|s| s.trim().parse::<usize>().ok()) {
                engine.hidden_cols.insert(idx);
            }
        }
        if let Some(mr) = attrs.get(ATTR_MERGED_REGIONS) {
            // Simple JSON parse: [{"c":0,"r":0,"cs":2,"rs":1}, ...]
            if let Ok(regions) = serde_json::from_str::<Vec<serde_json::Value>>(mr) {
                for region in regions {
                    let c = region["c"].as_u64().unwrap_or(0) as usize;
                    let r = region["r"].as_u64().unwrap_or(0) as usize;
                    let cs = region["cs"].as_u64().unwrap_or(1) as usize;
                    let rs = region["rs"].as_u64().unwrap_or(1) as usize;
                    engine.merge_cells(c, r, cs, rs);
                }
            }
        }
        // Conditional formats (extends to color scales / data bars in
        // M-S2 step 4). Bad payloads silently load no rules — we'd
        // rather lose conditional formatting than refuse to open the
        // sheet.
        if let Some(cfs) = attrs.get(ATTR_CONDITIONAL_FORMATS) {
            engine.set_conditional_formats(deserialize_conditional_formats(cfs));
        }
        if let Some(nr) = attrs.get(ATTR_NAMED_RANGES) {
            engine.set_named_ranges(deserialize_named_ranges(nr));
        }
        // Frozen-pane counts (M-S2). Treat parse errors as 0 so a
        // mangled attribute can't crash document load.
        if let Some(fr) = attrs.get(ATTR_FROZEN_ROWS) {
            engine.frozen_rows = fr.parse::<usize>().unwrap_or(0);
        }
        if let Some(fc) = attrs.get(ATTR_FROZEN_COLS) {
            engine.frozen_cols = fc.parse::<usize>().unwrap_or(0);
        }
        // Pivot tables (M-S2 v2). Bad payloads silently load no
        // pivots — same policy as conditional formats.
        if let Some(p) = attrs.get(ATTR_PIVOTS) {
            engine.set_pivots(deserialize_pivots(p));
        }
        // Embedded charts.
        if let Some(c) = attrs.get(ATTR_CHARTS) {
            engine.charts = deserialize_charts(c);
        }
    }

    // #128: restore the saved view-scroll extent. build_table_from_engine
    // persists only the used bounding box and records the larger navigated
    // extent as gridRows/gridCols; widen the returned extent to it so the
    // caller (which floors at 10) rehydrates the grid to the same size.
    if let Some(gr) = table_attrs.get(ATTR_GRID_ROWS).and_then(|s| s.parse::<usize>().ok()) {
        num_rows = num_rows.max(gr);
    }
    if let Some(gc) = table_attrs.get(ATTR_GRID_COLS).and_then(|s| s.parse::<usize>().ok()) {
        num_cols = num_cols.max(gc);
    }

    (num_rows, num_cols)
}

/// JSON shape for the `charts` table attribute (one object per
/// chart):
///   [{"type":"bar"|"line"|"pie",
///     "range":[[c1,r1],[c2,r2]],
///     "title":"…"}]
fn serialize_charts(charts: &[ChartConfig]) -> String {
    use serde_json::{json, Value};
    let arr: Vec<Value> = charts.iter().map(|c| {
        let kind = match c.chart_type {
            ChartType::Bar => "bar",
            ChartType::Line => "line",
            ChartType::Pie => "pie",
        };
        json!({
            "type": kind,
            "range": [
                [c.data_range.0.0, c.data_range.0.1],
                [c.data_range.1.0, c.data_range.1.1],
            ],
            "title": c.title,
        })
    }).collect();
    serde_json::Value::Array(arr).to_string()
}

/// Inverse of `serialize_charts`. Malformed entries silently drop
/// rather than blocking the whole doc from loading.
fn deserialize_charts(s: &str) -> Vec<ChartConfig> {
    let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(s) else {
        return Vec::new();
    };
    arr.into_iter().filter_map(|v| {
        let chart_type = match v.get("type").and_then(|x| x.as_str())? {
            "bar" => ChartType::Bar,
            "line" => ChartType::Line,
            "pie" => ChartType::Pie,
            _ => return None,
        };
        let range = v.get("range")?.as_array()?;
        let p0 = range.get(0)?.as_array()?;
        let p1 = range.get(1)?.as_array()?;
        let data_range = (
            (p0.get(0)?.as_u64()? as usize, p0.get(1)?.as_u64()? as usize),
            (p1.get(0)?.as_u64()? as usize, p1.get(1)?.as_u64()? as usize),
        );
        let title = v.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some(ChartConfig { chart_type, data_range, title })
    }).collect()
}

/// Parse a single CSV line, handling quoted fields.
/// Serialize the engine's conditional format vector to a JSON string.
/// Schema is documented at the call site in `build_table_from_engine`.
fn serialize_conditional_formats(
    cfs: &[((usize, usize), (usize, usize), Vec<ConditionalFormat>)],
) -> String {
    use serde_json::{json, Value};
    let arr: Vec<Value> = cfs.iter().map(|(tl, br, rules)| {
        let rules_json: Vec<Value> = rules.iter().map(|rule| match rule {
            ConditionalFormat::Single { condition, bg_color } => {
                json!({"kind": "single", "cond": condition_to_json(condition), "bg": bg_color})
            }
            ConditionalFormat::ColorScale { low, mid, high } => {
                json!({"kind": "colorScale", "low": low, "mid": mid, "high": high})
            }
            ConditionalFormat::DataBar { color } => {
                json!({"kind": "dataBar", "color": color})
            }
            ConditionalFormat::IconSet { kind } => {
                json!({"kind": "iconSet", "set": icon_set_kind_to_str(*kind)})
            }
        }).collect();
        json!({"tl": [tl.0, tl.1], "br": [br.0, br.1], "rules": rules_json})
    }).collect();
    Value::Array(arr).to_string()
}

fn condition_to_json(c: &ConditionalCondition) -> serde_json::Value {
    use serde_json::json;
    match c {
        ConditionalCondition::GreaterThan(n) => json!({"type": "gt", "val": n}),
        ConditionalCondition::LessThan(n) => json!({"type": "lt", "val": n}),
        ConditionalCondition::EqualTo(s) => json!({"type": "eq", "val": s}),
        ConditionalCondition::Between(lo, hi) => json!({"type": "between", "lo": lo, "hi": hi}),
        ConditionalCondition::TextContains(s) => json!({"type": "contains", "val": s}),
        ConditionalCondition::IsEmpty => json!({"type": "empty"}),
        ConditionalCondition::IsNotEmpty => json!({"type": "notEmpty"}),
    }
}

/// Inverse of `serialize_conditional_formats`. Bad rules silently
/// drop so a future schema bump on one rule doesn't take down the
/// whole sheet's formatting.
fn deserialize_conditional_formats(
    s: &str,
) -> Vec<((usize, usize), (usize, usize), Vec<ConditionalFormat>)> {
    use serde_json::Value;
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(s) else { return Vec::new(); };
    arr.into_iter().filter_map(|entry| {
        let tl = entry.get("tl")?.as_array()?;
        let br = entry.get("br")?.as_array()?;
        let tl = (tl.first()?.as_u64()? as usize, tl.get(1)?.as_u64()? as usize);
        let br = (br.first()?.as_u64()? as usize, br.get(1)?.as_u64()? as usize);
        let rules = entry.get("rules")?.as_array()?
            .iter()
            .filter_map(parse_rule)
            .collect::<Vec<_>>();
        Some((tl, br, rules))
    }).collect()
}

fn parse_rule(v: &serde_json::Value) -> Option<ConditionalFormat> {
    let kind = v.get("kind")?.as_str()?;
    match kind {
        "single" => {
            let cond = parse_condition(v.get("cond")?)?;
            let bg = v.get("bg")?.as_str()?.to_string();
            Some(ConditionalFormat::Single { condition: cond, bg_color: bg })
        }
        "colorScale" => Some(ConditionalFormat::ColorScale {
            low: v.get("low")?.as_str()?.to_string(),
            mid: v.get("mid").and_then(|x| x.as_str()).map(|s| s.to_string()),
            high: v.get("high")?.as_str()?.to_string(),
        }),
        "dataBar" => Some(ConditionalFormat::DataBar {
            color: v.get("color")?.as_str()?.to_string(),
        }),
        "iconSet" => Some(ConditionalFormat::IconSet {
            kind: icon_set_kind_from_str(v.get("set")?.as_str()?)?,
        }),
        _ => None,
    }
}

fn icon_set_kind_to_str(k: IconSetKind) -> &'static str {
    match k {
        IconSetKind::ThreeArrows => "3Arrows",
        IconSetKind::ThreeTrafficLights => "3TrafficLights",
    }
}

fn icon_set_kind_from_str(s: &str) -> Option<IconSetKind> {
    match s {
        "3Arrows" => Some(IconSetKind::ThreeArrows),
        "3TrafficLights" => Some(IconSetKind::ThreeTrafficLights),
        _ => None,
    }
}

fn parse_condition(v: &serde_json::Value) -> Option<ConditionalCondition> {
    let t = v.get("type")?.as_str()?;
    match t {
        "gt" => Some(ConditionalCondition::GreaterThan(v.get("val")?.as_f64()?)),
        "lt" => Some(ConditionalCondition::LessThan(v.get("val")?.as_f64()?)),
        "eq" => Some(ConditionalCondition::EqualTo(v.get("val")?.as_str()?.to_string())),
        "between" => Some(ConditionalCondition::Between(
            v.get("lo")?.as_f64()?, v.get("hi")?.as_f64()?,
        )),
        "contains" => Some(ConditionalCondition::TextContains(v.get("val")?.as_str()?.to_string())),
        "empty" => Some(ConditionalCondition::IsEmpty),
        "notEmpty" => Some(ConditionalCondition::IsNotEmpty),
        _ => None,
    }
}

/// JSON-serialize the engine's named ranges.
fn serialize_named_ranges(items: &[(String, RangeRef)]) -> String {
    use serde_json::{json, Value};
    let arr: Vec<Value> = items.iter().map(|(name, r)| {
        let tl = json!([r.start.col, r.start.row, r.start.abs_col, r.start.abs_row]);
        let br = json!([r.end.col, r.end.row, r.end.abs_col, r.end.abs_row]);
        json!({"name": name, "tl": tl, "br": br})
    }).collect();
    Value::Array(arr).to_string()
}

/// Inverse of `serialize_named_ranges`. Bad entries silently drop
/// (same policy as `deserialize_conditional_formats`).
fn deserialize_named_ranges(s: &str) -> Vec<(String, RangeRef)> {
    use serde_json::Value;
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(s) else { return Vec::new(); };
    arr.into_iter().filter_map(|entry| {
        let name = entry.get("name")?.as_str()?.to_string();
        // Drop any name that the parser couldn't tokenize as an Ident.
        // Loading it would create a permanently-unreachable entry and
        // a misleading "name exists but never resolves" UX.
        if !is_valid_named_range_name(&name) { return None; }
        let tl = entry.get("tl")?.as_array()?;
        let br = entry.get("br")?.as_array()?;
        let parse_corner = |v: &Vec<Value>| -> Option<CellRef> {
            Some(CellRef {
                col: v.first()?.as_u64()? as usize,
                row: v.get(1)?.as_u64()? as usize,
                abs_col: v.get(2).and_then(|x| x.as_bool()).unwrap_or(false),
                abs_row: v.get(3).and_then(|x| x.as_bool()).unwrap_or(false),
            })
        };
        let start = parse_corner(tl)?;
        let end = parse_corner(br)?;
        Some((name, RangeRef { start, end }))
    }).collect()
}

// ─── Pivot tables (M-S2 v2) ──────────────────────────────────────
//
// Schema (JSON, stored in a single sheet-level ATTR_PIVOTS attr):
//
//   [{
//     "anchor": [c, r],
//     "source": {"kind": "local", "range": "A1:E100"}
//              | {"kind": "foreign", "doc": "<id>", "sheet": "<name>", "range": "A1:E100"},
//     "rows":   [<group> ...],
//     "cols":   [<group> ...],
//     "values": [<value> ...],
//     "filters": [<filter> ...],
//     "valueLayout": "horizontal" | "vertical",
//     "layout": "compact" | "outline" | "tabular",
//     "totals": "none" | "rows" | "cols" | "both",
//     "subtotals": "above" | "below"
//   }]
//
// where <group> is:
//   {"col": <usize>, "sort": "asc"|"desc"|"none", "showTotals": <bool>,
//    "label": "..."?, "sortByValue": <usize>?,
//    "kind": "direct"
//          | {"date": "year"|"quarter"|"month"|"day"|"hour"}
//          | {"numericBin": {"width": <f64>, "start": <f64>?}}}
//
// <value>: {"col": <usize>, "fn": "sum"|"count"|..., "name": "..."?}
//
// <filter>: {"col": <usize>, "cond": <condition>}
//   <condition> = {"valueIn": [...]}
//               | {"numGt": <n>} | {"numLt": <n>} | {"numEq": <n>}
//               | {"numBetween": [<lo>, <hi>]}
//               | {"textContains": "..."}
//               | {"textEquals": "..."}
//               | {"textStartsWith": "..."}
//               | "empty" | "notEmpty"
//
// All identifiers stay lowercase to match the project's existing
// JSON schemas (conditional formats, named ranges).

fn serialize_pivots(pivots: &[&PivotTable]) -> String {
    use serde_json::{json, Value};
    let arr: Vec<Value> = pivots.iter().map(|p| {
        json!({
            "anchor": [p.anchor.0, p.anchor.1],
            "source": source_to_json(&p.source),
            "rows":   p.rows.iter().map(group_to_json).collect::<Vec<_>>(),
            "cols":   p.cols.iter().map(group_to_json).collect::<Vec<_>>(),
            "values": p.values.iter().map(value_to_json).collect::<Vec<_>>(),
            "filters": p.filters.iter().map(filter_to_json).collect::<Vec<_>>(),
            "valueLayout": match p.value_layout {
                ValueLayout::Horizontal => "horizontal",
                ValueLayout::Vertical => "vertical",
            },
            "layout": match p.layout_style {
                LayoutStyle::Compact => "compact",
                LayoutStyle::Outline => "outline",
                LayoutStyle::Tabular => "tabular",
            },
            "totals": match p.grand_totals {
                GrandTotals::None => "none",
                GrandTotals::Rows => "rows",
                GrandTotals::Cols => "cols",
                GrandTotals::Both => "both",
            },
            "subtotals": match p.subtotals_position {
                SubtotalsPos::Above => "above",
                SubtotalsPos::Below => "below",
            },
        })
    }).collect();
    Value::Array(arr).to_string()
}

fn source_to_json(s: &SourceRange) -> serde_json::Value {
    use serde_json::json;
    match s {
        SourceRange::Local { range_a1 } =>
            json!({"kind": "local", "range": range_a1}),
        SourceRange::Foreign { doc_id, sheet_name, range_a1 } =>
            json!({"kind": "foreign", "doc": doc_id, "sheet": sheet_name, "range": range_a1}),
    }
}

fn group_to_json(g: &PivotGroup) -> serde_json::Value {
    use serde_json::json;
    let mut o = json!({
        "col": g.source_col,
        "sort": match g.sort_order {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
            SortOrder::None => "none",
        },
        "showTotals": g.show_totals,
        "kind": kind_to_json(&g.kind),
    });
    if let Some(ref label) = g.label {
        o["label"] = json!(label);
    }
    if let Some(idx) = g.sort_by_value {
        o["sortByValue"] = json!(idx);
    }
    if let Some(ref vv) = g.visible_values {
        o["visibleValues"] = json!(vv);
    }
    o
}

fn kind_to_json(k: &PivotGroupKind) -> serde_json::Value {
    use serde_json::json;
    match k {
        PivotGroupKind::Direct => json!("direct"),
        PivotGroupKind::Date(g) => json!({"date": match g {
            DateGranularity::Year => "year",
            DateGranularity::Quarter => "quarter",
            DateGranularity::Month => "month",
            DateGranularity::Day => "day",
            DateGranularity::Hour => "hour",
        }}),
        PivotGroupKind::NumericBin { width, start } => {
            let mut bin = serde_json::Map::new();
            bin.insert("width".to_string(), finite_or_null(*width));
            if let Some(s) = start {
                bin.insert("start".to_string(), finite_or_null(*s));
            }
            json!({"numericBin": serde_json::Value::Object(bin)})
        }
    }
}

/// Convert an f64 to a JSON Number, mapping NaN/±Inf to Null.
/// `serde_json::Number::from_f64` already returns None for non-
/// finite values; we surface that as JSON `null` rather than
/// panicking inside the `json!` macro. Deserialize then drops
/// any pivot field that comes back null (since `as_f64` returns
/// None on null), preserving the rest of the pivot.
fn finite_or_null(n: f64) -> serde_json::Value {
    serde_json::Number::from_f64(n)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

fn value_to_json(v: &PivotValue) -> serde_json::Value {
    use serde_json::json;
    let mut o = json!({
        "col": v.source_col,
        "fn": summarize_fn_to_str(v.summarize_fn),
    });
    if let Some(ref name) = v.display_name {
        o["name"] = json!(name);
    }
    o
}

fn summarize_fn_to_str(s: SummarizeFn) -> &'static str {
    match s {
        SummarizeFn::Sum => "sum",
        SummarizeFn::Count => "count",
        SummarizeFn::CountA => "countA",
        SummarizeFn::Average => "average",
        SummarizeFn::Min => "min",
        SummarizeFn::Max => "max",
        SummarizeFn::Median => "median",
        SummarizeFn::Product => "product",
        SummarizeFn::StdDev => "stdDev",
        SummarizeFn::StdDevP => "stdDevP",
        SummarizeFn::Var => "var",
        SummarizeFn::VarP => "varP",
    }
}

fn filter_to_json(f: &PivotFilterSpec) -> serde_json::Value {
    use serde_json::json;
    json!({
        "col": f.source_col,
        "cond": condition_to_json_filter(&f.condition),
    })
}

fn condition_to_json_filter(c: &PivotFilterCondition) -> serde_json::Value {
    use serde_json::json;
    match c {
        PivotFilterCondition::ValueIn(v) => json!({"valueIn": v}),
        PivotFilterCondition::NumberGreater(n) => json!({"numGt": finite_or_null(*n)}),
        PivotFilterCondition::NumberLess(n) => json!({"numLt": finite_or_null(*n)}),
        PivotFilterCondition::NumberEqual(n) => json!({"numEq": finite_or_null(*n)}),
        PivotFilterCondition::NumberBetween(lo, hi) =>
            json!({"numBetween": [finite_or_null(*lo), finite_or_null(*hi)]}),
        PivotFilterCondition::TextContains(s) => json!({"textContains": s}),
        PivotFilterCondition::TextEquals(s) => json!({"textEquals": s}),
        PivotFilterCondition::TextStartsWith(s) => json!({"textStartsWith": s}),
        PivotFilterCondition::Empty => json!("empty"),
        PivotFilterCondition::NotEmpty => json!("notEmpty"),
    }
}

/// Inverse of `serialize_pivots`. Bad pivots silently drop —
/// preferring partial loss over refusing to open the sheet.
fn deserialize_pivots(s: &str) -> Vec<PivotTable> {
    use serde_json::Value;
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(s) else { return Vec::new(); };
    arr.into_iter().filter_map(parse_pivot).collect()
}

fn parse_pivot(v: serde_json::Value) -> Option<PivotTable> {
    let anchor_arr = v.get("anchor")?.as_array()?;
    let anchor = (
        anchor_arr.first()?.as_u64()? as usize,
        anchor_arr.get(1)?.as_u64()? as usize,
    );
    let source = parse_source(v.get("source")?)?;
    let rows = v.get("rows")?.as_array()?.iter().filter_map(parse_group).collect();
    let cols = v.get("cols")?.as_array()?.iter().filter_map(parse_group).collect();
    let values = v.get("values")?.as_array()?.iter().filter_map(parse_value).collect();
    let filters = v.get("filters")?.as_array()?.iter().filter_map(parse_filter).collect();
    let value_layout = match v.get("valueLayout").and_then(|x| x.as_str()) {
        Some("vertical") => ValueLayout::Vertical,
        _ => ValueLayout::Horizontal,
    };
    let layout_style = match v.get("layout").and_then(|x| x.as_str()) {
        Some("outline") => LayoutStyle::Outline,
        Some("tabular") => LayoutStyle::Tabular,
        _ => LayoutStyle::Compact,
    };
    let grand_totals = match v.get("totals").and_then(|x| x.as_str()) {
        Some("none") => GrandTotals::None,
        Some("rows") => GrandTotals::Rows,
        Some("cols") => GrandTotals::Cols,
        _ => GrandTotals::Both,
    };
    let subtotals_position = match v.get("subtotals").and_then(|x| x.as_str()) {
        Some("above") => SubtotalsPos::Above,
        _ => SubtotalsPos::Below,
    };
    Some(PivotTable {
        anchor, source, rows, cols, values, filters,
        value_layout, layout_style, grand_totals, subtotals_position,
    })
}

fn parse_source(v: &serde_json::Value) -> Option<SourceRange> {
    let kind = v.get("kind")?.as_str()?;
    let range_a1 = v.get("range")?.as_str()?.to_string();
    match kind {
        "local" => Some(SourceRange::Local { range_a1 }),
        "foreign" => Some(SourceRange::Foreign {
            doc_id: v.get("doc")?.as_str()?.to_string(),
            sheet_name: v.get("sheet")?.as_str()?.to_string(),
            range_a1,
        }),
        _ => None,
    }
}

fn parse_group(v: &serde_json::Value) -> Option<PivotGroup> {
    let col = v.get("col")?.as_u64()? as usize;
    let sort_order = match v.get("sort").and_then(|x| x.as_str()) {
        Some("asc") => SortOrder::Asc,
        Some("desc") => SortOrder::Desc,
        _ => SortOrder::None,
    };
    let show_totals = v.get("showTotals").and_then(|x| x.as_bool()).unwrap_or(false);
    let label = v.get("label").and_then(|x| x.as_str()).map(|s| s.to_string());
    let sort_by_value = v.get("sortByValue").and_then(|x| x.as_u64()).map(|n| n as usize);
    let kind = parse_kind(v.get("kind")?)?;
    let visible_values = v.get("visibleValues").and_then(|x| x.as_array()).map(|arr| {
        arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>()
    });
    Some(PivotGroup {
        source_col: col, sort_order, show_totals, label, sort_by_value, kind, visible_values,
    })
}

fn parse_kind(v: &serde_json::Value) -> Option<PivotGroupKind> {
    if v.as_str() == Some("direct") { return Some(PivotGroupKind::Direct); }
    if let Some(s) = v.get("date").and_then(|x| x.as_str()) {
        return Some(PivotGroupKind::Date(match s {
            "year" => DateGranularity::Year,
            "quarter" => DateGranularity::Quarter,
            "month" => DateGranularity::Month,
            "day" => DateGranularity::Day,
            "hour" => DateGranularity::Hour,
            _ => return None,
        }));
    }
    if let Some(bin) = v.get("numericBin") {
        let width = bin.get("width")?.as_f64()?;
        let start = bin.get("start").and_then(|x| x.as_f64());
        return Some(PivotGroupKind::NumericBin { width, start });
    }
    None
}

fn parse_value(v: &serde_json::Value) -> Option<PivotValue> {
    let col = v.get("col")?.as_u64()? as usize;
    let fn_str = v.get("fn")?.as_str()?;
    let summarize_fn = summarize_fn_from_str(fn_str)?;
    let display_name = v.get("name").and_then(|x| x.as_str()).map(|s| s.to_string());
    Some(PivotValue { source_col: col, summarize_fn, display_name })
}

fn summarize_fn_from_str(s: &str) -> Option<SummarizeFn> {
    Some(match s {
        "sum" => SummarizeFn::Sum,
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
        _ => return None,
    })
}

fn parse_filter(v: &serde_json::Value) -> Option<PivotFilterSpec> {
    let col = v.get("col")?.as_u64()? as usize;
    let condition = parse_filter_cond(v.get("cond")?)?;
    Some(PivotFilterSpec { source_col: col, condition })
}

fn parse_filter_cond(v: &serde_json::Value) -> Option<PivotFilterCondition> {
    if let Some(s) = v.as_str() {
        return Some(match s {
            "empty" => PivotFilterCondition::Empty,
            "notEmpty" => PivotFilterCondition::NotEmpty,
            _ => return None,
        });
    }
    if let Some(arr) = v.get("valueIn").and_then(|x| x.as_array()) {
        return Some(PivotFilterCondition::ValueIn(
            arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        ));
    }
    if let Some(n) = v.get("numGt").and_then(|x| x.as_f64()) {
        return Some(PivotFilterCondition::NumberGreater(n));
    }
    if let Some(n) = v.get("numLt").and_then(|x| x.as_f64()) {
        return Some(PivotFilterCondition::NumberLess(n));
    }
    if let Some(n) = v.get("numEq").and_then(|x| x.as_f64()) {
        return Some(PivotFilterCondition::NumberEqual(n));
    }
    if let Some(arr) = v.get("numBetween").and_then(|x| x.as_array()) {
        let lo = arr.first()?.as_f64()?;
        let hi = arr.get(1)?.as_f64()?;
        return Some(PivotFilterCondition::NumberBetween(lo, hi));
    }
    if let Some(s) = v.get("textContains").and_then(|x| x.as_str()) {
        return Some(PivotFilterCondition::TextContains(s.to_string()));
    }
    if let Some(s) = v.get("textEquals").and_then(|x| x.as_str()) {
        return Some(PivotFilterCondition::TextEquals(s.to_string()));
    }
    if let Some(s) = v.get("textStartsWith").and_then(|x| x.as_str()) {
        return Some(PivotFilterCondition::TextStartsWith(s.to_string()));
    }
    None
}

pub(super) fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else if ch == '"' {
            in_quotes = true;
        } else if ch == ',' {
            fields.push(current.clone());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    fields.push(current);
    fields
}

/// Try to parse `text` as a GitHub-flavored markdown table.
/// Returns `Some(rows)` (header + data rows, separator dropped)
/// when the input looks like a markdown table, `None` otherwise.
///
/// Detection rule. The separator row is the strong signal that keeps
/// false positives away — TSV never carries a `---|---` second line — so
/// outer pipes are *optional*, which lets us accept Pandoc "simple pipe
/// tables" that omit the leading/trailing border (#54 item 2):
/// - At least 2 non-empty lines after trim.
/// - The first line, trimmed, contains at least one `|` (so a single
///   column can't masquerade as a table).
/// - The second line, trimmed, is a separator row: every cell consists
///   only of `-`, `:`, and whitespace, with at least one `|` present
///   (so a bare `---` thematic break is not mistaken for a separator).
///   Outer pipes on the separator are optional too.
///
/// Parsing rule for non-separator rows: trim, strip an *optional*
/// leading `|` and trailing `|`, then split on *unescaped* `|` — a
/// backslash-escaped pipe (`\|`) is a literal pipe within a cell, not a
/// column delimiter (#54). See `parse_markdown_row`.
pub(super) fn parse_markdown_table(text: &str) -> Option<Vec<Vec<String>>> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 { return None; }
    if !lines[0].contains('|') { return None; }
    if !is_markdown_separator(lines[1]) { return None; }

    let mut rows = Vec::with_capacity(lines.len() - 1);
    rows.push(parse_markdown_row(lines[0]));
    for line in lines.iter().skip(2) {
        rows.push(parse_markdown_row(line));
    }
    Some(rows)
}

/// Split one markdown table row into trimmed cell values, honoring
/// backslash escapes (#54). `\|` is a literal pipe inside a cell (not a
/// delimiter) and `\\` is a literal backslash; both are unescaped in
/// the output. A single structural leading/trailing `|` border is
/// dropped first — but a *trailing* border is only stripped when it's
/// unescaped, so a cell ending in `\|` keeps its pipe.
fn parse_markdown_row(line: &str) -> Vec<String> {
    let s = line.trim();
    // A trimmed table row starts with the structural border `|` (never
    // `\`), so stripping one leading pipe can't eat an escaped pipe.
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = strip_unescaped_suffix_pipe(s);

    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.peek() {
                Some('|') => {
                    cur.push('|');
                    chars.next();
                }
                Some('\\') => {
                    cur.push('\\');
                    chars.next();
                }
                // A backslash before anything else is preserved verbatim;
                // we only special-case the pipe and backslash escapes.
                _ => cur.push('\\'),
            },
            '|' => {
                cells.push(cur.trim().to_string());
                cur = String::new();
            }
            _ => cur.push(ch),
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

/// Strip a single trailing `|` only when it is a structural border —
/// i.e. not the `|` of a cell-final `\|`. Parity of the run of
/// backslashes immediately before the pipe decides: an odd count means
/// the pipe is escaped and must stay.
fn strip_unescaped_suffix_pipe(s: &str) -> &str {
    if let Some(rest) = s.strip_suffix('|') {
        let trailing_backslashes = rest.chars().rev().take_while(|&c| c == '\\').count();
        if trailing_backslashes % 2 == 0 {
            return rest;
        }
    }
    s
}

fn is_markdown_separator(line: &str) -> bool {
    let s = line.trim();
    // A separator must contain at least one `|` — otherwise a bare `---`
    // (a markdown thematic break / horizontal rule) would pass as a
    // single-column separator and misread an ordinary `---` line as a
    // table. Outer border pipes are optional (Pandoc simple tables omit
    // them), so we don't require a leading `|`.
    if !s.contains('|') { return false; }
    // Two-step strip: the second `unwrap_or` must fall back to the
    // prefix-stripped string, NOT the original `s` (which still has
    // its leading `|` and would split into a phantom empty first
    // cell). Caught by `parse_markdown_table_separator_without_trailing_pipe`.
    let stripped = s.strip_prefix('|').unwrap_or(s);
    let inner = stripped.strip_suffix('|').unwrap_or(stripped);
    let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
    if cells.is_empty() { return false; }
    // Each cell must be only `-`, `:`, and whitespace, AND contain
    // at least one `-` (so a stray empty cell doesn't pass).
    cells.iter().all(|c| {
        !c.is_empty()
            && c.contains('-')
            && c.chars().all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
    })
}

/// Parse the first HTML `<table>` in `html` into rows of trimmed cell text.
/// Browsers, Excel, Google Sheets, and Word place a `text/html`
/// representation on the clipboard alongside `text/plain`, so reading and
/// parsing it lets the spreadsheet auto-detect rich tables reliably without
/// the markdown/TSV text heuristic (#54 item 3). The header row, if any, is
/// just the first row (treated like any other cell row, matching the
/// markdown-table paste behavior).
///
/// Each `<td>` / `<th>` is one cell; inner tags are stripped, a small set of
/// HTML entities is decoded, and internal whitespace is collapsed.
/// `<thead>` / `<tbody>` / `<tfoot>` wrappers are transparent (we iterate
/// `<tr>` globally within the table). Returns `None` when there's no table or
/// it has no cells.
///
/// Known simplifications (acceptable for clipboard paste): `colspan` /
/// `rowspan` are ignored (each cell counts once, so a spanned row is
/// narrower than the visual grid), and block elements inside a cell (`<br>`,
/// `</p>`) are dropped rather than turned into line breaks.
pub(super) fn parse_html_table(html: &str) -> Option<Vec<Vec<String>>> {
    // ASCII-lowercase a copy for case-insensitive tag matching. Lowercasing
    // only rewrites A–Z (same byte length), so byte offsets into `lower`
    // index identically into `html` — and every marker we split on (`<table`,
    // `<tr`, `<td`, `>`, `</td>`) is ASCII, so all split points land on char
    // boundaries.
    let lower = html.to_ascii_lowercase();
    let table_start = lower.find("<table")?;
    let table_end = lower[table_start..]
        .find("</table>")
        .map(|i| table_start + i)
        .unwrap_or(lower.len());
    let table_lower = &lower[table_start..table_end];
    let table_orig = &html[table_start..table_end];

    // Row boundaries: each `<tr` opens a row that runs to the next `<tr` (or
    // the table end).
    let mut tr_starts = Vec::new();
    let mut scan = 0;
    while let Some(rel) = table_lower[scan..].find("<tr") {
        tr_starts.push(scan + rel);
        scan += rel + 3;
    }

    let mut rows = Vec::new();
    for (i, &start) in tr_starts.iter().enumerate() {
        let end = tr_starts.get(i + 1).copied().unwrap_or(table_lower.len());
        let row_lower = &table_lower[start..end];
        let row_orig = &table_orig[start..end];

        let mut cells = Vec::new();
        let mut cidx = 0;
        while let Some(rel) = next_cell_open(&row_lower[cidx..]) {
            let tag_start = cidx + rel;
            // Cell content begins after the opening tag's closing `>`.
            let Some(gt) = row_lower[tag_start..].find('>') else {
                break;
            };
            let content_start = tag_start + gt + 1;
            // ...and ends at the next cell boundary: a closing `</td>` /
            // `</th>` or the next cell's opening tag (some HTML omits closing
            // tags), whichever comes first.
            let content_end = next_cell_boundary(&row_lower[content_start..])
                .map(|e| content_start + e)
                .unwrap_or(row_lower.len());
            cells.push(strip_tags_and_decode(&row_orig[content_start..content_end]));
            cidx = content_end;
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }

    if rows.is_empty() {
        None
    } else {
        Some(rows)
    }
}

/// Offset of the next `<td` or `<th` cell-opening tag in `s` (already
/// lowercased), whichever is earlier.
fn next_cell_open(s: &str) -> Option<usize> {
    let td = s.find("<td");
    let th = s.find("<th");
    match (td, th) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Offset of the next cell boundary in `s` (already lowercased) — the
/// earliest of a closing `</td>` / `</th>` or the next cell's opening
/// `<td` / `<th`.
fn next_cell_boundary(s: &str) -> Option<usize> {
    ["</td>", "</th>", "<td", "<th"]
        .iter()
        .filter_map(|m| s.find(m))
        .min()
}

/// Strip HTML tags from a cell's inner markup, decode a small set of common
/// entities, and collapse internal whitespace to single spaces (trimmed).
fn strip_tags_and_decode(s: &str) -> String {
    let mut text = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    // Decode `&amp;` LAST so an already-escaped sequence like `&amp;lt;`
    // decodes to the literal `&lt;`, not `<`.
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node};
    use crate::editor::state::EditorState;
    use crate::spreadsheet::eval::{CellStyle, CellValue, SpreadsheetEngine};

    fn doc_from_table(table: Node) -> Node {
        Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![table]),
        )
    }

    #[test]
    fn frozen_pane_counts_round_trip_through_doc() {
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "hi");
        eng.frozen_rows = 2;
        eng.frozen_cols = 3;

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.frozen_rows, 2);
        assert_eq!(reloaded.frozen_cols, 3);
    }

    #[test]
    fn zero_frozen_counts_omit_attrs_for_byte_compat() {
        let eng = SpreadsheetEngine::new();
        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let Node::Element { attrs, .. } = &table else { panic!("expected element") };
        assert!(!attrs.contains_key(ATTR_FROZEN_ROWS));
        assert!(!attrs.contains_key(ATTR_FROZEN_COLS));
    }

    // ─ Cell comments ─

    #[test]
    fn cell_comment_round_trips_through_doc() {
        use crate::spreadsheet::eval::CellStyle;
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((1, 2), "x");
        eng.set_style((1, 2), CellStyle {
            comment: Some("a note".to_string()),
            ..CellStyle::default()
        });

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(
            reloaded.get_style((1, 2)).and_then(|s| s.comment.clone()),
            Some("a note".to_string()),
        );
    }

    #[test]
    fn cell_comment_thread_id_round_trips_through_doc() {
        // The thread-id linkage rides yrs as a cell attribute, so it
        // must survive a build → load cycle exactly the way the
        // legacy `comment` field does. Empty / absent normalize to
        // None on both sides (symmetric with the writer dropping
        // empty strings).
        use crate::spreadsheet::eval::CellStyle;
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((1, 2), "x");
        eng.set_style((1, 2), CellStyle {
            comment_thread_id: Some("cell-abc123def456ghij".to_string()),
            ..CellStyle::default()
        });

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(
            reloaded.get_style((1, 2)).and_then(|s| s.comment_thread_id.clone()),
            Some("cell-abc123def456ghij".to_string()),
        );
    }

    #[test]
    fn cell_comment_thread_id_and_legacy_comment_coexist() {
        // During the legacy→threads migration window a cell can carry
        // both fields simultaneously (the migrator reads `comment`,
        // POSTs a thread, sets `comment_thread_id`, clears `comment`
        // on the next save). Until that save lands, the document
        // serializes both — confirm neither stomps the other.
        use crate::spreadsheet::eval::CellStyle;
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        eng.set_style((0, 0), CellStyle {
            comment: Some("legacy text".to_string()),
            comment_thread_id: Some("cell-newidnewidnewid1".to_string()),
            ..CellStyle::default()
        });

        let table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        let style = reloaded.get_style((0, 0)).expect("style restored");
        assert_eq!(style.comment.as_deref(), Some("legacy text"));
        assert_eq!(style.comment_thread_id.as_deref(), Some("cell-newidnewidnewid1"));
    }

    // ─ Conditional formats ─

    #[test]
    fn conditional_formats_round_trip_all_rule_kinds() {
        use crate::spreadsheet::eval::{ConditionalCondition, ConditionalFormat, IconSetKind};
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        eng.add_conditional_format((0, 0), (3, 3), ConditionalFormat::Single {
            condition: ConditionalCondition::GreaterThan(10.0),
            bg_color: "#ff0000".into(),
        });
        eng.add_conditional_format((0, 0), (3, 3), ConditionalFormat::ColorScale {
            low: "#000000".into(),
            mid: Some("#888888".into()),
            high: "#ffffff".into(),
        });
        eng.add_conditional_format((0, 0), (3, 3), ConditionalFormat::DataBar {
            color: "#3b82f6".into(),
        });
        eng.add_conditional_format((0, 0), (3, 3), ConditionalFormat::IconSet {
            kind: IconSetKind::ThreeArrows,
        });
        eng.add_conditional_format((0, 0), (3, 3), ConditionalFormat::IconSet {
            kind: IconSetKind::ThreeTrafficLights,
        });

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        let cfs = reloaded.get_conditional_formats();
        assert_eq!(cfs.len(), 1, "rules collapsed onto one (tl, br) entry");
        let rules = &cfs[0].2;
        assert_eq!(rules.len(), 5);
        assert!(matches!(rules[0], ConditionalFormat::Single { .. }));
        assert!(matches!(rules[1], ConditionalFormat::ColorScale { .. }));
        assert!(matches!(rules[2], ConditionalFormat::DataBar { .. }));
        assert!(matches!(
            rules[3],
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        ));
        assert!(matches!(
            rules[4],
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeTrafficLights },
        ));
    }

    #[test]
    fn empty_conditional_formats_omit_attr() {
        let eng = SpreadsheetEngine::new();
        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let Node::Element { attrs, .. } = &table else { panic!("expected element") };
        assert!(!attrs.contains_key(ATTR_CONDITIONAL_FORMATS));
    }

    // ─ Named ranges ─

    #[test]
    fn named_ranges_round_trip_preserves_name_and_corners() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        eng.set_named_range(
            "PROFIT",
            RangeRef {
                start: CellRef { col: 1, row: 1, abs_col: true, abs_row: true },
                end: CellRef { col: 1, row: 9, abs_col: false, abs_row: false },
            },
        );

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        let names = reloaded.named_ranges();
        assert_eq!(names.len(), 1);
        let (n, r) = &names[0];
        assert_eq!(n, "PROFIT");
        assert_eq!(r.start.col, 1);
        assert_eq!(r.start.row, 1);
        assert!(r.start.abs_col);
        assert!(r.start.abs_row);
        assert_eq!(r.end.col, 1);
        assert_eq!(r.end.row, 9);
        assert!(!r.end.abs_col);
        assert!(!r.end.abs_row);
    }

    #[test]
    fn malformed_named_range_drops_silently_on_load() {
        // A doc with a name like "has space" or "1bad" tokenizes as
        // anything but Ident — `Expr::Name` would never match it. The
        // loader must drop these instead of stranding an unreachable
        // entry in the engine.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        let mut table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        if let Node::Element { attrs, .. } = &mut table {
            attrs.insert(
                ATTR_NAMED_RANGES.to_string(),
                r#"[{"name":"has space","tl":[0,0,true,true],"br":[0,0,true,true]},
                    {"name":"1bad","tl":[0,0,true,true],"br":[0,0,true,true]},
                    {"name":"OK","tl":[0,0,true,true],"br":[0,0,true,true]}]"#
                .to_string(),
            );
        }
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        let names = reloaded.named_ranges();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].0, "OK");
    }

    #[test]
    fn snapshot_foreign_doc_extracts_sheets_keyed_by_name() {
        let mut e1 = SpreadsheetEngine::new();
        e1.set_cell((0, 0), "Region");  e1.set_cell((1, 0), "Sales");
        e1.set_cell((0, 1), "East");    e1.set_cell((1, 1), "100");
        let mut e2 = SpreadsheetEngine::new();
        e2.set_cell((0, 0), "alpha");   e2.set_cell((1, 0), "beta");
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![
                build_table_from_engine(&e1, 2, 2, "Q1"),
                build_table_from_engine(&e2, 1, 2, "Notes"),
            ]),
        );

        let snap = snapshot_foreign_doc(&doc);
        assert_eq!(snap.sheets.len(), 2);

        let q1 = snap.sheets.get("Q1").unwrap();
        assert_eq!(q1[0], vec!["Region".to_string(), "Sales".to_string()]);
        assert_eq!(q1[1], vec!["East".to_string(), "100".to_string()]);

        let notes = snap.sheets.get("Notes").unwrap();
        assert_eq!(notes[0], vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn snapshot_foreign_doc_falls_back_to_indexed_name_when_attr_missing() {
        let table = Node::element_with_attrs(
            crate::editor::model::NodeType::Table,
            std::collections::HashMap::new(),
            Fragment::from(Vec::<Node>::new()),
        );
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![table]),
        );
        let snap = snapshot_foreign_doc(&doc);
        assert!(snap.sheets.contains_key("Sheet1"));
    }

    #[test]
    fn empty_named_ranges_omit_attr() {
        let eng = SpreadsheetEngine::new();
        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let Node::Element { attrs, .. } = &table else { panic!("expected element") };
        assert!(!attrs.contains_key(ATTR_NAMED_RANGES));
    }

    #[test]
    fn malformed_conditional_formats_silently_load_no_rules() {
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        let mut table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        if let Node::Element { attrs, .. } = &mut table {
            attrs.insert(ATTR_CONDITIONAL_FORMATS.to_string(), "garbage{".to_string());
        }
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert!(reloaded.get_conditional_formats().is_empty());
    }

    #[test]
    fn explicit_empty_comment_attr_loads_as_none() {
        // Foreign tooling could write `comment=""` even though our own
        // writer drops it. The reader must normalize so the engine
        // never holds `Some("")`, which would leak through `has_comment`
        // checks elsewhere.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        let mut table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        if let Node::Element { content, .. } = &mut table {
            if let Some(Node::Element { content: rc, .. }) = content.children.first_mut() {
                if let Some(Node::Element { attrs, .. }) = rc.children.first_mut() {
                    attrs.insert(ATTR_COMMENT.to_string(), String::new());
                }
            }
        }
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(
            reloaded.get_style((0, 0)).and_then(|s| s.comment.clone()),
            None,
        );
    }

    #[test]
    fn empty_or_absent_comment_omits_attr() {
        use crate::spreadsheet::eval::CellStyle;
        let mut eng = SpreadsheetEngine::new();
        // Cell (0, 0): no style
        eng.set_cell((0, 0), "untouched");
        // Cell (1, 0): style with explicitly-empty comment — must be
        // dropped by the writer to keep the attribute byte-compat with
        // pre-comment documents.
        eng.set_cell((1, 0), "y");
        eng.set_style((1, 0), CellStyle {
            comment: Some(String::new()),
            ..CellStyle::default()
        });

        let table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        let Node::Element { content, .. } = &table else { panic!("expected element") };
        for row in &content.children {
            let Node::Element { content: rc, .. } = row else { continue };
            for cell in &rc.children {
                let Node::Element { attrs, .. } = cell else { continue };
                assert!(!attrs.contains_key(ATTR_COMMENT),
                    "empty/absent comment must not emit ATTR_COMMENT");
            }
        }
    }

    #[test]
    fn mangled_frozen_attr_loads_as_zero() {
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        let mut table = build_table_from_engine(&eng, 2, 2, "Sheet1");
        if let Node::Element { ref mut attrs, .. } = table {
            attrs.insert(ATTR_FROZEN_ROWS.to_string(), "garbage".to_string());
            attrs.insert(ATTR_FROZEN_COLS.to_string(), "-1".to_string());
        }
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.frozen_rows, 0);
        assert_eq!(reloaded.frozen_cols, 0);
    }

    // ─ parse_csv_line ─

    #[test]
    fn parse_csv_line_plain_fields() {
        assert_eq!(parse_csv_line("a,b,c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_csv_line_quoted_field_with_comma() {
        assert_eq!(parse_csv_line(r#"a,"b,c",d"#), vec!["a", "b,c", "d"]);
    }

    #[test]
    fn parse_csv_line_escaped_double_quote() {
        // RFC-4180 doubled-quote escape: "" inside a quoted field → literal "
        assert_eq!(parse_csv_line(r#""he said ""hi""","x""#), vec![r#"he said "hi""#, "x"]);
    }

    #[test]
    fn parse_csv_line_trailing_empty_field() {
        // A trailing comma yields an empty final field, not a dropped one.
        assert_eq!(parse_csv_line("a,b,"), vec!["a", "b", ""]);
    }

    // ─ extract_sheet_names ─

    #[test]
    fn extract_sheet_names_uses_attr_when_present() {
        let mut e1 = SpreadsheetEngine::new();
        e1.set_cell((0, 0), "x");
        let t1 = build_table_from_engine(&e1, 1, 1, "Alpha");
        let t2 = build_table_from_engine(&e1, 1, 1, "Beta");
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![t1, t2]),
        );
        assert_eq!(extract_sheet_names(&doc), vec!["Alpha", "Beta"]);
    }

    #[test]
    fn extract_sheet_names_falls_back_to_indexed_default() {
        // A Table with no ATTR_SHEET_NAME attribute (e.g., a doc from
        // before sheet naming existed) gets an index-based fallback.
        let table = Node::element_with_attrs(
            crate::editor::model::NodeType::Table,
            std::collections::HashMap::new(),
            Fragment::from(Vec::<Node>::new()),
        );
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![table]),
        );
        assert_eq!(extract_sheet_names(&doc), vec!["Sheet1"]);
    }

    #[test]
    fn extract_sheet_names_empty_doc_yields_default() {
        // A doc with no Table children at all still returns one entry
        // so the caller can address sheet 0 without bounds-panicking.
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(Vec::<Node>::new()),
        );
        assert_eq!(extract_sheet_names(&doc), vec![DEFAULT_SHEET_NAME]);
    }

    // ─ build_doc_with_sheets ─

    fn three_sheet_doc() -> Node {
        // Three single-cell sheets named S0/S1/S2 — building block for
        // `build_doc_dropping_sheet` round-trip tests.
        let mut e0 = SpreadsheetEngine::new();
        e0.set_cell((0, 0), "v0");
        let mut e1 = SpreadsheetEngine::new();
        e1.set_cell((0, 0), "v1");
        let mut e2 = SpreadsheetEngine::new();
        e2.set_cell((0, 0), "v2");
        Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![
                build_table_from_engine(&e0, 1, 1, "S0"),
                build_table_from_engine(&e1, 1, 1, "S1"),
                build_table_from_engine(&e2, 1, 1, "S2"),
            ]),
        )
    }

    #[test]
    fn build_doc_dropping_sheet_drops_middle_table() {
        let doc = three_sheet_doc();
        let updated = build_doc_dropping_sheet(&doc, 1);
        assert_eq!(extract_sheet_names(&updated), vec!["S0", "S2"]);
        // Cells survived the drop: S2's data is still readable at index 1.
        let state = EditorState::create_default(updated);
        let mut s1 = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut s1, &state, 1);
        assert_eq!(s1.get_raw((0, 0)), "v2");
    }

    #[test]
    fn build_doc_dropping_sheet_drops_first_table() {
        let doc = three_sheet_doc();
        let updated = build_doc_dropping_sheet(&doc, 0);
        assert_eq!(extract_sheet_names(&updated), vec!["S1", "S2"]);
    }

    #[test]
    fn build_doc_dropping_sheet_drops_last_table() {
        let doc = three_sheet_doc();
        let updated = build_doc_dropping_sheet(&doc, 2);
        assert_eq!(extract_sheet_names(&updated), vec!["S0", "S1"]);
    }

    #[test]
    fn build_doc_dropping_sheet_out_of_bounds_is_noop() {
        // drop_idx past the last table leaves the doc untouched.
        let doc = three_sheet_doc();
        let updated = build_doc_dropping_sheet(&doc, 99);
        assert_eq!(extract_sheet_names(&updated), vec!["S0", "S1", "S2"]);
    }

    #[test]
    fn build_doc_dropping_sheet_preserves_non_table_children() {
        // A doc could mix tables with metadata blocks. Only the
        // targeted Table is removed; non-table siblings persist.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "x");
        let para = Node::element_with_content(
            crate::editor::model::NodeType::Paragraph,
            Fragment::empty(),
        );
        let doc = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![
                para.clone(),
                build_table_from_engine(&e, 1, 1, "S0"),
                para.clone(),
            ]),
        );
        let updated = build_doc_dropping_sheet(&doc, 0);
        // Table at index 0 (the only Table) is gone; both Paragraphs
        // remain.
        let Node::Element { content, .. } = &updated else { panic!("not an Element") };
        assert_eq!(content.children.len(), 2);
        for child in &content.children {
            assert_eq!(child.node_type(), Some(crate::editor::model::NodeType::Paragraph));
        }
    }

    #[test]
    fn build_doc_with_sheets_replaces_active_keeps_others() {
        // Build a 2-sheet doc, then replace sheet 0 with new engine
        // contents while renaming sheet 1. Verify sheet 0's cell text
        // changed, sheet 1's cell text was preserved, and both sheets
        // carry the requested names.
        let mut e_orig = SpreadsheetEngine::new();
        e_orig.set_cell((0, 0), "old0");
        let mut e_other = SpreadsheetEngine::new();
        e_other.set_cell((0, 0), "old1");
        let original = Node::element_with_content(
            crate::editor::model::NodeType::Doc,
            Fragment::from(vec![
                build_table_from_engine(&e_orig, 1, 1, "S0"),
                build_table_from_engine(&e_other, 1, 1, "S1"),
            ]),
        );

        let mut e_new = SpreadsheetEngine::new();
        e_new.set_cell((0, 0), "new0");
        let names = vec!["First".to_string(), "Renamed".to_string()];
        let updated = build_doc_with_sheets(&original, &e_new, 0, 1, 1, &names);

        assert_eq!(extract_sheet_names(&updated), vec!["First", "Renamed"]);

        // Round-trip the updated doc through the loader and inspect both sheets.
        let state = EditorState::create_default(updated);
        let mut s0 = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut s0, &state, 0);
        assert_eq!(s0.get_raw((0, 0)), "new0");
        let mut s1 = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut s1, &state, 1);
        assert_eq!(s1.get_raw((0, 0)), "old1");
    }

    #[test]
    fn build_table_never_truncates_used_cells() {
        // Regression (#72 verification): an edit landed via scroll + click
        // at I65 (col 8, row 64) while grid_rows was still 10 — the
        // persisted table must grow to cover the used extent rather than
        // silently dropping the cell, or the engine re-sync from the doc
        // erases the user's change.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((8, 64), "deep edit");
        let table = build_table_from_engine(&e, 10, 10, "S0");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.get_raw((8, 64)), "deep edit");
    }

    // ─── #128: trim-only persist ───────────────────────────────

    fn table_attr<'a>(table: &'a Node, key: &str) -> Option<&'a str> {
        match table {
            Node::Element { attrs, .. } => attrs.get(key).map(|s| s.as_str()),
            _ => None,
        }
    }

    #[test]
    fn build_table_trims_to_used_bounding_box_not_view_extent() {
        // One cell at (col 2, row 3) with a 50x50 view extent: persist
        // only the 4x3 bounding box, and record the view extent as attrs.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((2, 3), "x");
        let table = build_table_from_engine(&e, 50, 50, "S0");
        assert_eq!(table.child_count(), 4, "rows = used max_row + 1");
        assert_eq!(table_attr(&table, "gridRows"), Some("50"));
        assert_eq!(table_attr(&table, "gridCols"), Some("50"));
    }

    #[test]
    fn build_table_includes_formatting_only_cell() {
        // A styled-but-empty cell beyond the raw extent must widen the
        // used box and survive a round-trip (the old raw-only predicate
        // dropped it silently).
        let mut e = SpreadsheetEngine::new();
        e.set_style((1, 6), CellStyle { bold: true, ..Default::default() });
        let table = build_table_from_engine(&e, 10, 10, "S0");
        assert_eq!(table.child_count(), 7, "row 6 is included via its style");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.get_style((1, 6)).map(|s| s.bold), Some(true));
    }

    #[test]
    fn build_table_includes_merged_region_with_no_content() {
        // A merge with no raw/styled cells must still be persisted to its
        // bottom-right and round-trip.
        let mut e = SpreadsheetEngine::new();
        e.merge_cells(0, 0, 2, 3); // (col,row,cs,rs) → bottom-right (1,2)
        let table = build_table_from_engine(&e, 10, 10, "S0");
        assert_eq!(table.child_count(), 3, "rows = merge bottom row + 1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.get_merge_span(0, 0), (2, 3));
    }

    #[test]
    fn view_extent_restored_on_reload() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "x");
        let table = build_table_from_engine(&e, 80, 40, "S0");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        let (rows, cols) = sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!((rows, cols), (80, 40));
    }

    #[test]
    fn empty_sheet_builds_zero_row_table_with_sheet_name() {
        // An empty sheet trims to a 0-row table — kept alive downstream by
        // normalize_doc's spreadsheet-table exception (tested in model.rs).
        let e = SpreadsheetEngine::new();
        let table = build_table_from_engine(&e, 100, 26, "S0");
        assert_eq!(table.child_count(), 0);
        assert_eq!(table_attr(&table, "sheetName"), Some("S0"));
        assert_eq!(table_attr(&table, "gridRows"), Some("100"));
        assert_eq!(table_attr(&table, "gridCols"), Some("26"));
    }

    // ─── Pivot tables (M-S2 v2) ────────────────────────────────

    fn small_pivot_dataset(eng: &mut SpreadsheetEngine) {
        // header + 5 rows: Region | Product | Revenue.
        eng.set_cell((0, 0), "Region");
        eng.set_cell((1, 0), "Product");
        eng.set_cell((2, 0), "Revenue");
        eng.set_cell((0, 1), "West"); eng.set_cell((1, 1), "A"); eng.set_cell((2, 1), "10");
        eng.set_cell((0, 2), "West"); eng.set_cell((1, 2), "B"); eng.set_cell((2, 2), "20");
        eng.set_cell((0, 3), "East"); eng.set_cell((1, 3), "A"); eng.set_cell((2, 3), "30");
        eng.set_cell((0, 4), "East"); eng.set_cell((1, 4), "A"); eng.set_cell((2, 4), "40");
        eng.set_cell((0, 5), "East"); eng.set_cell((1, 5), "B"); eng.set_cell((2, 5), "50");
    }

    fn make_pivot(anchor: (usize, usize)) -> PivotTable {
        PivotTable {
            anchor,
            source: SourceRange::Local { range_a1: "A1:C6".into() },
            rows: vec![PivotGroup {
                source_col: 0,
                sort_order: SortOrder::Asc,
                show_totals: true,
                label: Some("Region".into()),
                sort_by_value: None,
                kind: PivotGroupKind::Direct,
                visible_values: None,
            }],
            cols: vec![],
            values: vec![PivotValue {
                source_col: 2,
                summarize_fn: SummarizeFn::Sum,
                display_name: None,
            }],
            filters: vec![PivotFilterSpec {
                source_col: 0,
                condition: PivotFilterCondition::ValueIn(vec!["West".into(), "East".into()]),
            }],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Tabular,
            grand_totals: GrandTotals::Rows,
            subtotals_position: SubtotalsPos::Below,
        }
    }

    #[test]
    fn pivot_table_round_trips_through_doc() {
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));

        let table = build_table_from_engine(&eng, 8, 8, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));

        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        // Core fields preserved.
        let pivots: Vec<&PivotTable> = reloaded.pivots_iter().map(|(_, p)| p).collect();
        assert_eq!(pivots.len(), 1);
        let p = pivots[0];
        assert_eq!(p.anchor, (4, 0));
        assert_eq!(p.layout_style, LayoutStyle::Tabular);
        assert_eq!(p.grand_totals, GrandTotals::Rows);
        assert_eq!(p.subtotals_position, SubtotalsPos::Below);
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.rows[0].source_col, 0);
        assert_eq!(p.rows[0].sort_order, SortOrder::Asc);
        assert!(p.rows[0].show_totals);
        assert_eq!(p.rows[0].label.as_deref(), Some("Region"));
        assert_eq!(p.values.len(), 1);
        assert_eq!(p.values[0].summarize_fn, SummarizeFn::Sum);
        assert_eq!(p.filters.len(), 1);
        match &p.filters[0].condition {
            PivotFilterCondition::ValueIn(v) => {
                assert_eq!(v, &vec!["West".to_string(), "East".to_string()]);
            }
            other => panic!("unexpected filter: {other:?}"),
        }
        match &p.source {
            SourceRange::Local { range_a1 } => assert_eq!(range_a1, "A1:C6"),
            other => panic!("unexpected source: {other:?}"),
        }

        // Output is installed at the anchor on reload (via set_pivots →
        // recompute_pivot path), so the spilled grid is present.
        match reloaded.get_value((4, 0)) {
            CellValue::Array(out) => {
                assert!(!out.is_empty(), "pivot output should be non-empty after reload");
            }
            other => panic!("expected pivot output at anchor, got {other:?}"),
        }
    }

    #[test]
    fn pivot_round_trips_all_group_kinds_and_filter_variants() {
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        let pt = PivotTable {
            anchor: (5, 0),
            source: SourceRange::Local { range_a1: "A1:C6".into() },
            rows: vec![
                PivotGroup { source_col: 0, kind: PivotGroupKind::Direct, ..Default::default() },
                PivotGroup {
                    source_col: 2,
                    kind: PivotGroupKind::NumericBin { width: 25.0, start: Some(0.0) },
                    ..Default::default()
                },
            ],
            cols: vec![PivotGroup {
                source_col: 1,
                kind: PivotGroupKind::Date(DateGranularity::Quarter),
                ..Default::default()
            }],
            values: vec![PivotValue {
                source_col: 2,
                summarize_fn: SummarizeFn::Average,
                display_name: Some("AvgRev".into()),
            }],
            filters: vec![
                PivotFilterSpec {
                    source_col: 2,
                    condition: PivotFilterCondition::NumberBetween(10.0, 40.0),
                },
                PivotFilterSpec {
                    source_col: 1,
                    condition: PivotFilterCondition::TextStartsWith("A".into()),
                },
                PivotFilterSpec { source_col: 0, condition: PivotFilterCondition::NotEmpty },
            ],
            value_layout: ValueLayout::Vertical,
            layout_style: LayoutStyle::Outline,
            grand_totals: GrandTotals::Both,
            subtotals_position: SubtotalsPos::Above,
        };
        eng.add_pivot(pt);

        let table = build_table_from_engine(&eng, 8, 8, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        let p = reloaded.pivots_iter().next().expect("pivot lost on reload").1;
        assert_eq!(p.value_layout, ValueLayout::Vertical);
        assert_eq!(p.layout_style, LayoutStyle::Outline);
        assert_eq!(p.grand_totals, GrandTotals::Both);
        assert_eq!(p.subtotals_position, SubtotalsPos::Above);
        // Group kinds.
        assert!(matches!(p.rows[0].kind, PivotGroupKind::Direct));
        match &p.rows[1].kind {
            PivotGroupKind::NumericBin { width, start } => {
                assert_eq!(*width, 25.0);
                assert_eq!(*start, Some(0.0));
            }
            other => panic!("unexpected kind: {other:?}"),
        }
        match &p.cols[0].kind {
            PivotGroupKind::Date(DateGranularity::Quarter) => {}
            other => panic!("unexpected kind: {other:?}"),
        }
        // Value display name.
        assert_eq!(p.values[0].display_name.as_deref(), Some("AvgRev"));
        // Three filter variants survive.
        assert!(matches!(p.filters[0].condition, PivotFilterCondition::NumberBetween(_, _)));
        assert!(matches!(p.filters[1].condition, PivotFilterCondition::TextStartsWith(_)));
        assert!(matches!(p.filters[2].condition, PivotFilterCondition::NotEmpty));
    }

    #[test]
    fn pivot_group_visible_values_round_trips() {
        // Per-group visibility whitelist (the row-/col-header value
        // picker writes here) must survive the doc round-trip.
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        let pt = PivotTable {
            anchor: (5, 0),
            source: SourceRange::Local { range_a1: "A1:C6".into() },
            rows: vec![PivotGroup {
                source_col: 0,
                visible_values: Some(vec!["West".into()]),
                ..Default::default()
            }],
            cols: vec![PivotGroup {
                source_col: 1,
                visible_values: Some(vec!["A".into(), "B".into()]),
                ..Default::default()
            }],
            values: vec![PivotValue {
                source_col: 2,
                summarize_fn: SummarizeFn::Sum,
                display_name: None,
            }],
            filters: vec![],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Tabular,
            grand_totals: GrandTotals::None,
            subtotals_position: SubtotalsPos::Below,
        };
        eng.add_pivot(pt);

        let table = build_table_from_engine(&eng, 8, 8, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        let p = reloaded.pivots_iter().next().expect("pivot lost on reload").1;
        assert_eq!(p.rows[0].visible_values.as_deref(), Some(&["West".to_string()][..]));
        assert_eq!(
            p.cols[0].visible_values.as_deref(),
            Some(&["A".to_string(), "B".to_string()][..]),
        );
    }

    #[test]
    fn pivot_recomputes_when_source_cell_edited() {
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));

        // Capture the pre-edit grand-total row value.
        let before = match eng.get_value((4, 0)) {
            CellValue::Array(out) => out.clone(),
            _ => panic!("expected initial pivot output"),
        };
        // Grand total of Revenue: 10+20+30+40+50 = 150.
        let last_before = before.last().unwrap();
        match last_before.last().unwrap() {
            CellValue::Number(n) => assert_eq!(*n, 150.0),
            other => panic!("unexpected grand total: {other:?}"),
        }

        // Mutate a source cell — Revenue at row 5 from 50 → 500.
        eng.set_cell((2, 5), "500");

        // Pivot output should reflect the change without any explicit
        // refresh from the caller.
        let after = match eng.get_value((4, 0)) {
            CellValue::Array(out) => out.clone(),
            _ => panic!("expected pivot output after edit"),
        };
        let last_after = after.last().unwrap();
        match last_after.last().unwrap() {
            CellValue::Number(n) => assert_eq!(*n, 600.0, "grand total should reflect 500 swap"),
            other => panic!("unexpected grand total: {other:?}"),
        }
    }

    #[test]
    fn malformed_pivots_silently_load_no_pivots() {
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        let mut table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        if let Node::Element { attrs, .. } = &mut table {
            attrs.insert(ATTR_PIVOTS.to_string(), "garbage{".to_string());
        }
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert_eq!(reloaded.pivots_iter().count(), 0);
    }

    #[test]
    fn empty_pivots_omit_attr() {
        let eng = SpreadsheetEngine::new();
        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let Node::Element { attrs, .. } = &table else { panic!("expected element") };
        assert!(!attrs.contains_key(ATTR_PIVOTS));
    }

    // ─── Stable blockIds for yrs incremental updates ─────────

    fn block_ids_at(table: &Node) -> Vec<String> {
        // Walk Table → rows → cells → paragraphs and collect blockIds
        // in document order.
        let mut ids = Vec::new();
        let Node::Element { attrs, content: tc, .. } = table else { return ids };
        if let Some(id) = attrs.get("blockId") { ids.push(id.clone()); }
        for row in &tc.children {
            let Node::Element { attrs, content: rc, .. } = row else { continue };
            if let Some(id) = attrs.get("blockId") { ids.push(id.clone()); }
            for cell in &rc.children {
                let Node::Element { attrs, content: cc, .. } = cell else { continue };
                if let Some(id) = attrs.get("blockId") { ids.push(id.clone()); }
                for para in &cc.children {
                    let Node::Element { attrs, .. } = para else { continue };
                    if let Some(id) = attrs.get("blockId") { ids.push(id.clone()); }
                }
            }
        }
        ids
    }

    #[test]
    fn chart_config_round_trips_through_doc() {
        // Regression for #67: inserting a chart pushed the config
        // into `engine.charts`, persist() rebuilt the doc, the doc-
        // state Effect re-synced the engine via `engine.clear()` —
        // and `clear()` wiped the freshly-inserted chart. The fix
        // is to serialize charts into the table's attribute map
        // and deserialize them on sync. This test exercises that
        // full loop.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.charts.push(ChartConfig {
            chart_type: ChartType::Bar,
            data_range: ((0, 0), (2, 3)),
            title: "My Chart".into(),
        });
        eng.charts.push(ChartConfig {
            chart_type: ChartType::Pie,
            data_range: ((0, 0), (1, 1)),
            title: String::new(),
        });

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        assert_eq!(reloaded.charts.len(), 2);
        assert!(matches!(reloaded.charts[0].chart_type, ChartType::Bar));
        assert_eq!(reloaded.charts[0].data_range, ((0, 0), (2, 3)));
        assert_eq!(reloaded.charts[0].title, "My Chart");
        assert!(matches!(reloaded.charts[1].chart_type, ChartType::Pie));
        assert_eq!(reloaded.charts[1].title, "");
    }

    #[test]
    fn malformed_charts_silently_load_no_charts() {
        // Bad payload in ATTR_CHARTS shouldn't block the doc from
        // loading — matches the policy used for ATTR_PIVOTS and
        // ATTR_CONDITIONAL_FORMATS.
        let mut attrs = HashMap::new();
        attrs.insert(ATTR_SHEET_NAME.to_string(), "Sheet1".to_string());
        attrs.insert(ATTR_CHARTS.to_string(), "not-json".to_string());
        let table = Node::element_with_attrs(NodeType::Table, attrs, Fragment::empty());
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        assert!(reloaded.charts.is_empty());
    }

    #[test]
    fn build_table_emits_stable_block_ids_at_same_positions() {
        // Without stable blockIds, every persist generated fresh
        // random IDs and yrs's `find_match` saw a brand-new tree
        // each time — every save was a full delete+reinsert. With
        // stable IDs derived from (sheet, row, col), two consecutive
        // builds at the same positions must produce IDENTICAL ID
        // sets so yrs can match and emit minimal incremental diffs.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "A");
        eng.set_cell((1, 0), "B");
        eng.set_cell((0, 1), "1");
        eng.set_cell((1, 1), "2");

        let t1 = build_table_from_engine(&eng, 3, 3, "Sheet1");
        let t2 = build_table_from_engine(&eng, 3, 3, "Sheet1");
        let ids1 = block_ids_at(&t1);
        let ids2 = block_ids_at(&t2);
        assert_eq!(ids1, ids2,
            "Two consecutive build_table_from_engine calls must produce \
             identical blockIds at the same logical positions");
        // Spot-check the namespace shape.
        assert!(ids1.contains(&"ss:Sheet1:t".to_string()));
        assert!(ids1.contains(&"ss:Sheet1:r:0".to_string()));
        assert!(ids1.contains(&"ss:Sheet1:c:0:0".to_string()));
        assert!(ids1.contains(&"ss:Sheet1:p:0:0".to_string()));
    }

    #[test]
    fn build_table_block_ids_isolated_per_sheet() {
        // Two sheets with the same (row, col) coordinates must NOT
        // collide on blockIds — yrs would otherwise treat them as
        // the same logical entity.
        let eng = SpreadsheetEngine::new();
        let t_a = build_table_from_engine(&eng, 2, 2, "SheetA");
        let t_b = build_table_from_engine(&eng, 2, 2, "SheetB");
        let ids_a: std::collections::HashSet<String> = block_ids_at(&t_a).into_iter().collect();
        let ids_b: std::collections::HashSet<String> = block_ids_at(&t_b).into_iter().collect();
        // No id should appear in both sets.
        assert!(ids_a.is_disjoint(&ids_b),
            "blockIds across sheets must not overlap; got A={ids_a:?} B={ids_b:?}");
    }

    #[test]
    fn delete_one_row_only_removes_one_block_id() {
        // After deleting row 1 (zero-indexed) from a 4-row sheet,
        // the new build's blockIds should be the original IDs minus
        // exactly the row-3 row + cells + paragraphs (the bottom row
        // disappears; rows 0..2 keep their IDs but content shifts
        // for rows 1 and 2). This is the structural property that
        // makes yrs emit a small "remove row 3" + cell-text-update
        // diff instead of a full table replacement.
        let mut eng = SpreadsheetEngine::new();
        for r in 0..4 {
            for c in 0..3 {
                eng.set_cell((c, r), &format!("r{r}c{c}"));
            }
        }

        let before = build_table_from_engine(&eng, 4, 3, "Sheet1");
        let ids_before: std::collections::HashSet<String> = block_ids_at(&before).into_iter().collect();

        // Simulate a row delete by shifting and clearing the last row.
        for ri in 1..3 {
            for ci in 0..3 {
                let v = eng.get_raw((ci, ri + 1)).to_string();
                eng.set_cell((ci, ri), &v);
            }
        }
        for ci in 0..3 { eng.set_cell((ci, 3), ""); }

        let after = build_table_from_engine(&eng, 3, 3, "Sheet1");
        let ids_after: std::collections::HashSet<String> = block_ids_at(&after).into_iter().collect();

        // After is a strict subset of before — only row-3's IDs
        // (and their cells/paragraphs) are removed.
        assert!(ids_after.is_subset(&ids_before),
            "shrunk-table IDs must be a subset of the original ID set");
        let removed: Vec<&String> = ids_before.difference(&ids_after).collect();
        // Removed: 1 row + 3 cells + 3 paragraphs = 7 IDs.
        assert_eq!(removed.len(), 7,
            "expected 7 IDs removed (row 3 + 3 cells + 3 paragraphs), got {removed:?}");
    }

    #[test]
    fn pivot_spill_cells_persist_rendered_text_with_marker() {
        // When a pivot is active, its spill output cells live ONLY
        // in `engine.values` (raw is empty). Persistence must write
        // their rendered display text + ATTR_SPILLED so XLSX export
        // captures the visible output. On reload, the marker tells
        // the loader to skip those cells; the engine's set_pivots
        // → recompute_pivot reinstalls them via the spill block.
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));

        let table = build_table_from_engine(&eng, 8, 8, "Sheet1");
        let Node::Element { content: tc, .. } = &table else {
            panic!("expected table element")
        };
        // Walk to find a spill cell at (5, 1) — the pivot anchor is
        // (4, 0); its output's column-2 row-1 is the first SUM value.
        let row1 = match &tc.children[1] {
            Node::Element { content, .. } => content,
            _ => panic!("expected row element"),
        };
        let cell51 = match &row1.children[5] {
            Node::Element { attrs, content, .. } => (attrs, content),
            _ => panic!("expected cell element"),
        };
        assert_eq!(cell51.0.get(ATTR_SPILLED).map(|s| s.as_str()), Some("1"));
        // The cell's text content should be a number string (the SUM).
        let text = cell51.1.children.iter()
            .filter_map(|n| match n {
                Node::Element { content, .. } => content.children.iter()
                    .find_map(|c| match c {
                        Node::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    }),
                _ => None,
            })
            .next()
            .unwrap_or_default();
        assert!(!text.is_empty(), "spill cell should carry rendered text, got empty");
    }

    #[test]
    fn pivot_spill_marker_skipped_on_reload_no_double_writes() {
        // Without the load-side skip, the loader would `set_cell` on
        // each spill cell with the persisted text, then `set_pivots`
        // would call `try_register_spill_block` which fails when
        // target cells are non-Empty (rejecting register and leaving
        // those cells as plain values, not spilled). The skip + the
        // engine's reinstall keeps the spill block intact.
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));

        let table = build_table_from_engine(&eng, 8, 8, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        // The pivot's anchor must hold a CellValue::Array (the spill
        // anchor); spill cells must be filled (not Empty). If the
        // loader had failed to skip, the anchor would be Number/Text
        // (parsed from persisted text) rather than Array.
        match reloaded.get_value((4, 0)) {
            CellValue::Array(out) => assert!(!out.is_empty()),
            other => panic!("expected Array at anchor after reload, got {other:?}"),
        }
        // A spill-fill cell at the first body-row's value column
        // should look like a real number, not a Text("...") (which
        // is what set_cell would have written from the persisted
        // display string). With the page-field filter row at top
        // (1 filter in make_pivot()), the value-label header lives
        // at row 1 and the first body row's value cell lives at
        // (5, 2).
        match reloaded.get_value((5, 2)) {
            CellValue::Number(_) => {} // expected
            other => panic!("expected Number at spill cell, got {other:?}"),
        }
    }

    #[test]
    fn foreign_source_pivot_round_trips() {
        // Foreign sources must round-trip the doc_id + sheet_name +
        // range fields. The eval path is empty (Foreign returns []
        // until phase 3 wires the cross-doc snapshot lookup), so we
        // assert on the deserialized struct, not on output cells.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        eng.add_pivot(PivotTable {
            anchor: (1, 1),
            source: SourceRange::Foreign {
                doc_id: "doc-α".into(),
                sheet_name: "Sheet2".into(),
                range_a1: "B2:D10".into(),
            },
            rows: vec![],
            cols: vec![],
            values: vec![],
            filters: vec![],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Compact,
            grand_totals: GrandTotals::None,
            subtotals_position: SubtotalsPos::Below,
        });

        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);

        let p = reloaded.get_pivot((1, 1)).expect("pivot lost on reload");
        match &p.source {
            SourceRange::Foreign { doc_id, sheet_name, range_a1 } => {
                assert_eq!(doc_id, "doc-α");
                assert_eq!(sheet_name, "Sheet2");
                assert_eq!(range_a1, "B2:D10");
            }
            other => panic!("expected Foreign source, got {other:?}"),
        }
    }

    #[test]
    fn non_finite_numeric_bin_serializes_to_null_no_panic() {
        // serde_json's Number::from_f64 rejects NaN/±Inf; the
        // serializer maps them to JSON null instead of panicking
        // inside the json! macro. The pivot field becomes None on
        // reload (deserializer's as_f64 fails on null) and the
        // affected pivot drops cleanly.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "x");
        eng.add_pivot(PivotTable {
            anchor: (2, 2),
            source: SourceRange::Local { range_a1: "A1:B2".into() },
            rows: vec![PivotGroup {
                source_col: 0,
                kind: PivotGroupKind::NumericBin {
                    width: f64::NAN,
                    start: Some(f64::INFINITY),
                },
                ..Default::default()
            }],
            cols: vec![],
            values: vec![],
            filters: vec![PivotFilterSpec {
                source_col: 0,
                condition: PivotFilterCondition::NumberGreater(f64::NEG_INFINITY),
            }],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Compact,
            grand_totals: GrandTotals::None,
            subtotals_position: SubtotalsPos::Below,
        });

        // No panic during serialize.
        let table = build_table_from_engine(&eng, 4, 4, "Sheet1");
        // Document loads cleanly; the pivot with non-finite fields
        // either drops or arrives with the offending entries
        // discarded — both outcomes are acceptable.
        let state = EditorState::create_default(doc_from_table(table));
        let mut reloaded = SpreadsheetEngine::new();
        sync_engine_from_doc_sheet(&mut reloaded, &state, 0);
        let _ = reloaded.get_pivot((2, 2));
    }

    #[test]
    fn remove_pivot_recalculates_dependent_formulas() {
        // A formula =SUM(<anchor>) must update when the pivot at
        // <anchor> is removed. Without recalculate_from in
        // remove_pivot, the formula held the pre-remove sum.
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));
        // The pivot at (4, 0) spills a 2-col output. Sum of the
        // value column (col 5 = anchor.col + 1) hits the per-region
        // values + grand total. Just assert the formula is non-empty
        // before remove and zero after.
        eng.set_cell((6, 0), "=SUM(F1:F10)");
        let pre = eng.get_value((6, 0)).clone();
        match pre {
            CellValue::Number(n) => assert!(n > 0.0, "expected positive sum, got {n}"),
            other => panic!("expected pre-remove sum, got {other:?}"),
        }
        eng.remove_pivot((4, 0));
        match eng.get_value((6, 0)) {
            CellValue::Number(n) => assert_eq!(*n, 0.0,
                "SUM should recompute to 0 after pivot removal"),
            other => panic!("expected post-remove sum=0, got {other:?}"),
        }
    }

    #[test]
    fn chained_pivots_propagate_when_outer_source_edits() {
        // Pivot A summarizes A1:C6 → spills at E1.
        // Pivot B summarizes E1:F4 (covering A's output) → spills at H1.
        // Editing a source cell of A must propagate through to B.
        let mut eng = SpreadsheetEngine::new();
        small_pivot_dataset(&mut eng);
        eng.add_pivot(make_pivot((4, 0)));    // A at E1
        // Pivot B over A's output: source range covers A's spill
        // area. Group by the row-label column (F1 onward = the SUM
        // values column). Sum the value column.
        eng.add_pivot(PivotTable {
            anchor: (7, 0),                   // H1
            source: SourceRange::Local { range_a1: "E1:F10".into() },
            rows: vec![PivotGroup { source_col: 0, ..Default::default() }],
            cols: vec![],
            values: vec![PivotValue {
                source_col: 1,
                summarize_fn: SummarizeFn::Sum,
                display_name: None,
            }],
            filters: vec![],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Tabular,
            grand_totals: GrandTotals::Rows,
            subtotals_position: SubtotalsPos::Below,
        });

        // Capture B's grand total before & after editing A's source.
        let total_before = match eng.get_value((7, 0)) {
            CellValue::Array(out) => match out.last().and_then(|r| r.last()) {
                Some(CellValue::Number(n)) => *n,
                other => panic!("unexpected B grand total: {other:?}"),
            },
            other => panic!("expected B output, got {other:?}"),
        };
        // Mutate a source cell of A: row 5 Revenue 50 → 500.
        eng.set_cell((2, 5), "500");

        let total_after = match eng.get_value((7, 0)) {
            CellValue::Array(out) => match out.last().and_then(|r| r.last()) {
                Some(CellValue::Number(n)) => *n,
                other => panic!("unexpected B grand total: {other:?}"),
            },
            other => panic!("expected B output, got {other:?}"),
        };
        assert_ne!(total_before, total_after,
            "B's total should change when A's source changes");
    }

    // ─── Markdown-table paste parser ─────────────────────────

    #[test]
    fn parse_markdown_table_basic() {
        let md = "\
            | Name | Age |\n\
            |------|-----|\n\
            | Alice | 30 |\n\
            | Bob | 25 |";
        let rows = parse_markdown_table(md).expect("expected Some");
        assert_eq!(rows.len(), 3, "header + 2 data rows; separator dropped");
        assert_eq!(rows[0], vec!["Name".to_string(), "Age".to_string()]);
        assert_eq!(rows[1], vec!["Alice".to_string(), "30".to_string()]);
        assert_eq!(rows[2], vec!["Bob".to_string(), "25".to_string()]);
    }

    #[test]
    fn parse_markdown_table_handles_alignment_separators() {
        // GFM allows :--, --:, :-: alignment markers in the separator.
        let md = "\
            | A | B | C |\n\
            | :--- | :---: | ---: |\n\
            | 1 | 2 | 3 |";
        let rows = parse_markdown_table(md).expect("expected Some");
        assert_eq!(rows[0], vec!["A", "B", "C"]);
        assert_eq!(rows[1], vec!["1", "2", "3"]);
    }

    #[test]
    fn parse_markdown_table_strips_leading_trailing_pipes() {
        // #54 item 2 (behavior change): borderless pipe tables now parse.
        // This previously returned None — the old rule required a leading
        // `|` on row 1 to avoid false positives. That guard moved to the
        // separator row (the genuinely strong signal: `---|---` never
        // appears in Excel/TSV paste), so a Pandoc simple table without
        // outer pipes is now recognized.
        let md = "\
            A | B\n\
            ---|---\n\
            1 | 2";
        let rows = parse_markdown_table(md).expect("borderless table now parses");
        assert_eq!(rows[0], vec!["A", "B"]);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn parse_markdown_table_honors_escaped_pipes_in_cells() {
        // #54.1: a cell with a literal pipe is written `\|`. The old
        // unconditional split mangled `a \| b` into two cells; it must
        // now stay one cell with an unescaped pipe.
        let md = "\
            | Expr | Note |\n\
            |------|------|\n\
            | a \\| b | or-pattern |\n\
            | x | plain |";
        let rows = parse_markdown_table(md).expect("expected Some");
        assert_eq!(rows[0], vec!["Expr", "Note"]);
        assert_eq!(
            rows[1],
            vec!["a | b".to_string(), "or-pattern".to_string()],
            "escaped pipe must be one cell, unescaped to a literal |"
        );
        assert_eq!(rows[2], vec!["x", "plain"]);
    }

    #[test]
    fn parse_markdown_row_escape_edge_cases() {
        // Trailing escaped pipe before the structural border stays put.
        assert_eq!(parse_markdown_row("| ends with \\| |"), vec!["ends with |"]);
        // Escaped backslash is literal; the following pipe still splits.
        assert_eq!(parse_markdown_row("| a\\\\ | b |"), vec!["a\\", "b"]);
        // A lone trailing escaped pipe with no structural border is kept.
        assert_eq!(parse_markdown_row("| a \\|"), vec!["a |"]);
        // Empty edge cells are preserved (structural borders only).
        assert_eq!(parse_markdown_row("| | b |"), vec!["", "b"]);
    }

    #[test]
    fn parse_markdown_table_rejects_tsv_with_leading_pipe() {
        // A TSV row whose first cell happens to be "|something" must
        // not be misclassified. The second line lacks a separator.
        let md = "| weird\tcell\n| another\trow";
        assert!(parse_markdown_table(md).is_none());
    }

    #[test]
    fn parse_markdown_table_rejects_single_line() {
        // No second line → can't be a table.
        assert!(parse_markdown_table("| only one row |").is_none());
    }

    #[test]
    fn parse_markdown_table_drops_empty_cells_outside_pipes() {
        // The user's sample format has both leading and trailing `|`.
        let md = "\
            | x | y | z |\n\
            |---|---|---|\n\
            | a | b | c |";
        let rows = parse_markdown_table(md).expect("expected Some");
        assert_eq!(rows[0].len(), 3, "should not have phantom empty cells");
        assert_eq!(rows[0], vec!["x", "y", "z"]);
    }

    #[test]
    fn parse_markdown_table_handles_user_sales_example() {
        // The 7-column sales example from the user — verify the
        // header + first data row come through exactly.
        let md = "\
            | Date | Region | Category | Product | Salesperson | Units | Revenue |\n\
            |------|--------|----------|---------|-------------|-------|---------|\n\
            | 2025-01-05 | North | Electronics | Laptop | Alice | 3 | 3600 |";
        let rows = parse_markdown_table(md).expect("expected Some");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 7);
        assert_eq!(rows[0][0], "Date");
        assert_eq!(rows[0][6], "Revenue");
        assert_eq!(rows[1][0], "2025-01-05");
        assert_eq!(rows[1][6], "3600");
    }

    #[test]
    fn parse_markdown_table_separator_without_trailing_pipe() {
        // Regression for an off-by-one in `is_markdown_separator`'s
        // strip_suffix fallback. A leading-pipe-only separator like
        // `|---|---` (no trailing pipe) was misclassified because
        // the fallback used the original (still leading-piped) string,
        // producing a phantom empty first cell that failed the
        // `!is_empty()` predicate.
        let md = "\
            | A | B |\n\
            |---|---\n\
            | 1 | 2 |";
        let rows = parse_markdown_table(md).expect("valid GFM-ish table");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["A", "B"]);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn parse_markdown_table_rejects_separator_with_only_dashes_no_pipes() {
        // A line of just `---` (a markdown thematic break) is not a valid
        // table separator — it carries no `|`, so it can't delimit columns.
        let md = "\
            | A | B |\n\
            ---\n\
            | 1 | 2 |";
        assert!(parse_markdown_table(md).is_none());
    }

    #[test]
    fn parse_markdown_table_accepts_pandoc_without_outer_pipes() {
        // #54 item 2: a Pandoc "simple pipe table" omits the leading and
        // trailing border. The separator row is the signal, so this parses
        // identically to a bordered GFM table.
        let md = "\
            A | B | C\n\
            ---|---|---\n\
            1 | 2 | 3";
        let rows = parse_markdown_table(md).expect("borderless pipe table");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["A", "B", "C"]);
        assert_eq!(rows[1], vec!["1", "2", "3"]);
    }

    #[test]
    fn parse_markdown_table_borderless_separator_with_alignment() {
        // Alignment colons on a borderless separator still classify it.
        let md = "\
            Name | Qty\n\
            :--- | ---:\n\
            Apples | 5";
        let rows = parse_markdown_table(md).expect("borderless aligned table");
        assert_eq!(rows[0], vec!["Name", "Qty"]);
        assert_eq!(rows[1], vec!["Apples", "5"]);
    }

    #[test]
    fn parse_markdown_table_borderless_rejects_tsv_without_separator() {
        // Two borderless lines that each carry a stray `|` but no separator
        // row must NOT be read as a table — the separator is the only thing
        // separating a real table from pipe-bearing prose, so without it we
        // bail. (Falls through to the TSV/plain paste path.)
        let md = "a | b\nc | d";
        assert!(parse_markdown_table(md).is_none());
    }

    // ─── #54 item 3: HTML-table paste ──────────────────────────────

    #[test]
    fn parse_html_table_basic_header_and_rows() {
        let html = "<table><tr><th>A</th><th>B</th></tr>\
                    <tr><td>1</td><td>2</td></tr></table>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["A", "B"]);
        assert_eq!(rows[1], vec!["1", "2"]);
    }

    #[test]
    fn parse_html_table_ignores_attrs_and_strips_inline_tags() {
        // Excel/Sheets cells carry style/class attrs and inline formatting;
        // attributes are ignored and inner tags stripped to plain text.
        let html = "<table border=\"1\">\
            <tr><td class=\"x\" style=\"color:red\"><b>Bold</b></td>\
                <td>plain<span> tail</span></td></tr></table>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows[0], vec!["Bold", "plain tail"]);
    }

    #[test]
    fn parse_html_table_transparent_thead_tbody_and_entities() {
        let html = "<table><thead><tr><th>Name</th><th>Q&amp;A</th></tr></thead>\
            <tbody><tr><td>Ben &amp; Co</td><td>3 &lt; 5</td></tr></tbody></table>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows[0], vec!["Name", "Q&A"]);
        assert_eq!(rows[1], vec!["Ben & Co", "3 < 5"]);
    }

    #[test]
    fn parse_html_table_collapses_internal_whitespace() {
        let html = "<table><tr><td>  lots   of\n  space  </td></tr></table>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows[0], vec!["lots of space"]);
    }

    #[test]
    fn parse_html_table_case_insensitive_tags() {
        // Word emits upper/mixed-case tags.
        let html = "<TABLE><TR><TD>x</TD><TD>y</TD></TR></TABLE>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows[0], vec!["x", "y"]);
    }

    #[test]
    fn parse_html_table_none_without_table() {
        assert!(parse_html_table("<div>not a table</div>").is_none());
        assert!(parse_html_table("plain text").is_none());
        // A table element with no cells yields no rows.
        assert!(parse_html_table("<table></table>").is_none());
    }

    #[test]
    fn parse_html_table_decodes_amp_last_so_escaped_entities_survive() {
        // `&amp;lt;` is an escaped `&lt;` — it must decode to the literal
        // `&lt;`, not to `<` (no double-decode).
        let html = "<table><tr><td>&amp;lt;</td></tr></table>";
        let rows = parse_html_table(html).expect("html table");
        assert_eq!(rows[0], vec!["&lt;"]);
    }
}
