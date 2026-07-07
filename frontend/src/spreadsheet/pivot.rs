// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pivot table v2 — typed object model + evaluator.
//!
//! Replaces the v1 `=PIVOT(...)` formula function. The new shape:
//! a `PivotTable` is anchored to a cell; the engine recomputes the
//! pivot when its source range changes and installs the resulting
//! `CellValue::Array` at the anchor — the existing dynamic-array
//! spill machinery in `eval.rs` then renders the output.
//!
//! See `design/pivot-tables.md` for the full design.

use std::collections::{BTreeMap, HashSet};

use crate::spreadsheet::eval::CellValue;
use crate::spreadsheet::parser::SpreadsheetError;

// ─── Public types ─────────────────────────────────────────────────────

/// One configured pivot table. Storage shape mirrors the Sheets API
/// `PivotTable` plus the Excel-parity additions (layout style, grand
/// totals, subtotals position, group kinds).
#[derive(Clone, Debug, PartialEq)]
pub struct PivotTable {
    pub anchor: (usize, usize),
    pub source: SourceRange,
    pub rows: Vec<PivotGroup>,
    pub cols: Vec<PivotGroup>,
    pub values: Vec<PivotValue>,
    pub filters: Vec<PivotFilterSpec>,
    pub value_layout: ValueLayout,
    pub layout_style: LayoutStyle,
    pub grand_totals: GrandTotals,
    pub subtotals_position: SubtotalsPos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceRange {
    /// Range within the same sheet/engine instance. Resolved by the
    /// engine before passing source rows to `eval_pivot`.
    Local { range_a1: String },
    /// Foreign-doc range, riding on the cross-doc references plumbing.
    /// `eval_pivot` itself doesn't fetch — it receives whatever rows
    /// the engine resolved (or empty if the foreign cache is missing).
    Foreign { doc_id: String, sheet_name: String, range_a1: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotGroup {
    pub source_col: usize,
    pub sort_order: SortOrder,
    pub show_totals: bool,
    pub label: Option<String>,
    pub sort_by_value: Option<usize>,
    pub kind: PivotGroupKind,
    /// When `Some(v)`, only source rows whose value in `source_col`
    /// has bucket-label appearing in `v` survive into this group.
    /// `None` means "show every value" (the default). Mirrors Excel's
    /// per-axis row/col label filter dropdown.
    pub visible_values: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PivotGroupKind {
    Direct,
    Date(DateGranularity),
    NumericBin { width: f64, start: Option<f64> },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DateGranularity { Year, Quarter, Month, Day, Hour }

#[derive(Clone, Debug, PartialEq)]
pub struct PivotValue {
    pub source_col: usize,
    pub summarize_fn: SummarizeFn,
    pub display_name: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SummarizeFn {
    Sum, Count, CountA, Average,
    Min, Max, Median, Product,
    StdDev, StdDevP, Var, VarP,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PivotFilterSpec {
    pub source_col: usize,
    pub condition: PivotFilterCondition,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PivotFilterCondition {
    /// Discrete value picker — keep rows whose value-as-text matches
    /// any entry. Used for "Q4 only" style choosers.
    ValueIn(Vec<String>),
    NumberGreater(f64),
    NumberLess(f64),
    NumberEqual(f64),
    NumberBetween(f64, f64),
    TextContains(String),
    TextEquals(String),
    TextStartsWith(String),
    Empty,
    NotEmpty,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ValueLayout { Horizontal, Vertical }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum LayoutStyle { Compact, Outline, Tabular }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum GrandTotals { None, Rows, Cols, Both }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum SubtotalsPos { Above, Below }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum SortOrder { Asc, Desc, None }

// ─── Defaults ─────────────────────────────────────────────────────────

impl Default for PivotGroup {
    fn default() -> Self {
        Self {
            source_col: 0,
            sort_order: SortOrder::None,
            show_totals: false,
            label: None,
            sort_by_value: None,
            kind: PivotGroupKind::Direct,
            visible_values: None,
        }
    }
}

impl PivotTable {
    /// Construct an empty pivot at the given anchor with a local
    /// source. Used by the editor's "Insert Pivot Table" entry point.
    pub fn new_local_at(anchor: (usize, usize), range_a1: String) -> Self {
        Self {
            anchor,
            source: SourceRange::Local { range_a1 },
            rows: Vec::new(),
            cols: Vec::new(),
            values: Vec::new(),
            filters: Vec::new(),
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Compact,
            grand_totals: GrandTotals::Both,
            subtotals_position: SubtotalsPos::Below,
        }
    }
}

// ─── Group-key derivation ─────────────────────────────────────────────

/// Turn a source-cell value into a bucket label for one `PivotGroup`.
/// `Direct` returns the cell as text. `Date(g)` parses the cell as a
/// date (Excel serial number or YYYY-MM-DD / MM/DD/YYYY text) and
/// returns the granularity-appropriate label; values that don't parse
/// as a date fall through to `as_text()` (so a literal "Unknown" cell
/// still groups together). `NumericBin` buckets numerics into `[start
/// + n·width, start + (n+1)·width)` ranges with a "lo–hi" label;
/// non-numerics fall through to text.
fn bucket_label(kind: &PivotGroupKind, v: &CellValue) -> String {
    match kind {
        PivotGroupKind::Direct => v.as_text(),
        PivotGroupKind::Date(g) => parse_to_serial(v)
            .and_then(|serial| date_bucket_label(serial, *g))
            .unwrap_or_else(|| v.as_text()),
        PivotGroupKind::NumericBin { width, start } => match v.as_number() {
            Ok(n) if *width > 0.0 && !n.is_nan() => {
                let s = start.unwrap_or(0.0);
                // floor division to a bucket index, then label as
                // half-open interval [lo, hi).
                let idx = ((n - s) / *width).floor();
                let lo = s + idx * *width;
                let hi = lo + *width;
                format!("{}–{}", trim_zero(lo), trim_zero(hi))
            }
            _ => v.as_text(),
        },
    }
}

fn trim_zero(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Best-effort conversion of a cell to an Excel-style date serial.
/// Number → as-is. Text → try `YYYY-MM-DD` / `MM/DD/YYYY`. Anything
/// else returns None.
fn parse_to_serial(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Number(n) => Some(*n),
        CellValue::Text(s) => parse_date_text_to_serial(s.trim()),
        _ => None,
    }
}

fn parse_date_text_to_serial(s: &str) -> Option<f64> {
    let parts_ymd: Vec<&str> = s.split('-').collect();
    let parts_mdy: Vec<&str> = s.split('/').collect();
    let (y, m, d) = if parts_ymd.len() == 3 {
        let y: i32 = parts_ymd[0].parse().ok()?;
        let m: u32 = parts_ymd[1].parse().ok()?;
        let d: u32 = parts_ymd[2].parse().ok()?;
        (y, m, d)
    } else if parts_mdy.len() == 3 {
        let m: u32 = parts_mdy[0].parse().ok()?;
        let d: u32 = parts_mdy[1].parse().ok()?;
        let y: i32 = parts_mdy[2].parse().ok()?;
        (y, m, d)
    } else {
        return None;
    };
    Some(ymd_to_serial(y, m, d)?)
}

fn ymd_to_serial(y: i32, m: u32, d: u32) -> Option<f64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || y < 1900 { return None; }
    if d > days_in_month(y, m) { return None; }
    let mut serial = 0_i64;
    for yy in 1900..y { serial += if is_leap(yy) { 366 } else { 365 }; }
    for mm in 1..m { serial += days_in_month(y, mm) as i64; }
    serial += d as i64;
    if y > 1900 || (y == 1900 && (m > 2 || (m == 2 && d == 29))) {
        serial += 1;
    }
    Some(serial as f64)
}

fn serial_to_ymd(serial: f64) -> Option<(i32, u32, u32)> {
    // Inverse of ymd_to_serial. Walks forward from 1900-01-01
    // (serial 1) through whole years and months.
    //
    // Quirk: Excel preserves Lotus 1-2-3's bogus "1900-02-29" date at
    // serial 60. Forward direction emits serial 60 for that date and
    // bumps every later date by +1; inverse mirrors both halves —
    // serial 60 → (1900, 2, 29); serial >= 61 walks the remaining
    // days back through real-calendar months after subtracting 1.
    if serial < 1.0 || !serial.is_finite() { return None; }
    let s_floor = serial.floor() as i64;
    if s_floor == 60 { return Some((1900, 2, 29)); }
    let mut s = if s_floor >= 61 { s_floor - 1 } else { s_floor };
    let mut y = 1900_i32;
    loop {
        let yd = if is_leap(y) { 366 } else { 365 };
        if s > yd { s -= yd; y += 1; } else { break; }
    }
    let mut m = 1_u32;
    loop {
        let md = days_in_month(y, m) as i64;
        if s > md { s -= md; m += 1; } else { break; }
    }
    Some((y, m, s as u32))
}

fn is_leap(y: i32) -> bool { (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 }
fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1|3|5|7|8|10|12 => 31,
        4|6|9|11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 0,
    }
}

fn date_bucket_label(serial: f64, g: DateGranularity) -> Option<String> {
    let (y, m, d) = serial_to_ymd(serial)?;
    Some(match g {
        DateGranularity::Year => format!("{y}"),
        DateGranularity::Quarter => format!("{y}-Q{}", (m - 1) / 3 + 1),
        DateGranularity::Month => format!("{y}-{:02}", m),
        DateGranularity::Day => format!("{y}-{:02}-{:02}", m, d),
        DateGranularity::Hour => {
            let frac = serial - serial.floor();
            let hour = (frac * 24.0).floor() as u32;
            format!("{y}-{:02}-{:02} {:02}:00", m, d, hour)
        }
    })
}

/// Returns true iff every position in `keys` either has no
/// visibility whitelist on its corresponding `groups[i]` or the
/// position's bucket label is present in `groups[i].visible_values`.
/// `keys.len()` is expected to equal `groups.len()`; mismatched
/// inputs default to "pass".
fn group_keys_pass_visibility(keys: &[String], groups: &[PivotGroup]) -> bool {
    for (i, g) in groups.iter().enumerate() {
        let Some(allow) = g.visible_values.as_ref() else { continue };
        let Some(k) = keys.get(i) else { continue };
        if !allow.iter().any(|v| v == k) { return false; }
    }
    true
}

// ─── Filter ───────────────────────────────────────────────────────────

fn passes_filters(row: &[CellValue], filters: &[PivotFilterSpec]) -> bool {
    filters.iter().all(|f| {
        let v = row.get(f.source_col).unwrap_or(&CellValue::Empty);
        let txt = v.as_text();
        let num = v.as_number().ok();
        match &f.condition {
            PivotFilterCondition::ValueIn(vs) =>
                vs.iter().any(|s| s == &txt),
            PivotFilterCondition::NumberGreater(n) => num.is_some_and(|x| x > *n),
            PivotFilterCondition::NumberLess(n) => num.is_some_and(|x| x < *n),
            PivotFilterCondition::NumberEqual(n) => num.is_some_and(|x| (x - n).abs() < f64::EPSILON),
            PivotFilterCondition::NumberBetween(lo, hi) =>
                num.is_some_and(|x| x >= *lo && x <= *hi),
            PivotFilterCondition::TextContains(s) =>
                txt.to_lowercase().contains(&s.to_lowercase()),
            PivotFilterCondition::TextEquals(s) => txt.eq_ignore_ascii_case(s),
            PivotFilterCondition::TextStartsWith(s) =>
                txt.to_lowercase().starts_with(&s.to_lowercase()),
            PivotFilterCondition::Empty => matches!(v, CellValue::Empty)
                || (matches!(v, CellValue::Text(t) if t.is_empty())),
            PivotFilterCondition::NotEmpty => !matches!(v, CellValue::Empty)
                && !matches!(v, CellValue::Text(t) if t.is_empty()),
        }
    })
}

// ─── Aggregation ──────────────────────────────────────────────────────

/// Numeric values collected per (row_key, col_key, value_idx) cell,
/// plus a non-empty-cell counter used by COUNTA.
#[derive(Default, Clone)]
struct Bucket {
    nums: Vec<f64>,
    non_empty: usize,
}

fn fold_bucket(b: &Bucket, sf: SummarizeFn) -> CellValue {
    let nums = &b.nums;
    match sf {
        SummarizeFn::Sum => CellValue::Number(nums.iter().sum()),
        SummarizeFn::Count => CellValue::Number(nums.len() as f64),
        SummarizeFn::CountA => CellValue::Number(b.non_empty as f64),
        SummarizeFn::Average => {
            if nums.is_empty() { CellValue::Error(SpreadsheetError::Div0) }
            else { CellValue::Number(nums.iter().sum::<f64>() / nums.len() as f64) }
        }
        SummarizeFn::Min => match nums.iter().cloned().reduce(f64::min) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Error(SpreadsheetError::Value),
        },
        SummarizeFn::Max => match nums.iter().cloned().reduce(f64::max) {
            Some(n) => CellValue::Number(n),
            None => CellValue::Error(SpreadsheetError::Value),
        },
        SummarizeFn::Median => {
            if nums.is_empty() { return CellValue::Error(SpreadsheetError::Div0); }
            let mut sorted = nums.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = sorted.len();
            CellValue::Number(if n % 2 == 1 {
                sorted[n / 2]
            } else {
                (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
            })
        }
        SummarizeFn::Product => CellValue::Number(nums.iter().product()),
        SummarizeFn::StdDev => sample_stddev(nums),
        SummarizeFn::StdDevP => pop_stddev(nums),
        SummarizeFn::Var => sample_var(nums),
        SummarizeFn::VarP => pop_var(nums),
    }
}

fn mean(nums: &[f64]) -> f64 { nums.iter().sum::<f64>() / nums.len() as f64 }
fn sample_var(nums: &[f64]) -> CellValue {
    if nums.len() < 2 { return CellValue::Error(SpreadsheetError::Div0); }
    let m = mean(nums);
    let v = nums.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
    CellValue::Number(v)
}
fn sample_stddev(nums: &[f64]) -> CellValue {
    match sample_var(nums) {
        CellValue::Number(v) => CellValue::Number(v.sqrt()),
        e => e,
    }
}
fn pop_var(nums: &[f64]) -> CellValue {
    if nums.is_empty() { return CellValue::Error(SpreadsheetError::Div0); }
    let m = mean(nums);
    let v = nums.iter().map(|x| (x - m).powi(2)).sum::<f64>() / nums.len() as f64;
    CellValue::Number(v)
}
fn pop_stddev(nums: &[f64]) -> CellValue {
    match pop_var(nums) {
        CellValue::Number(v) => CellValue::Number(v.sqrt()),
        e => e,
    }
}

// ─── Evaluator ────────────────────────────────────────────────────────

/// Evaluate the pivot against `src` (row-major, including header
/// row). Returns the rendered grid; empty if the source is too small
/// or no fields are configured.
pub fn eval_pivot(pt: &PivotTable, src: &[Vec<CellValue>]) -> Vec<Vec<CellValue>> {
    if src.len() < 2 || src[0].is_empty() { return Vec::new(); }
    if pt.values.is_empty() && pt.rows.is_empty() && pt.cols.is_empty() {
        return Vec::new();
    }

    let header = &src[0];
    let data: Vec<&Vec<CellValue>> = src.iter().skip(1)
        .filter(|row| passes_filters(row, &pt.filters))
        .collect();

    // ─── Group rows by (row_key, col_key) ─────────────────────
    //
    // Use BTreeMap<Vec<String>, ...> keyed by group keys for
    // deterministic ordering. Insertion order is captured via a
    // separate Vec so we can apply per-level sort after the fact.
    let mut row_keys: Vec<Vec<String>> = Vec::new();
    let mut row_seen: HashSet<Vec<String>> = HashSet::new();
    let mut col_keys: Vec<Vec<String>> = Vec::new();
    let mut col_seen: HashSet<Vec<String>> = HashSet::new();
    let mut buckets: BTreeMap<(Vec<String>, Vec<String>, usize), Bucket> = BTreeMap::new();

    for row in &data {
        let rk: Vec<String> = pt.rows.iter()
            .map(|g| bucket_label(&g.kind, row.get(g.source_col).unwrap_or(&CellValue::Empty)))
            .collect();
        let ck: Vec<String> = pt.cols.iter()
            .map(|g| bucket_label(&g.kind, row.get(g.source_col).unwrap_or(&CellValue::Empty)))
            .collect();
        // Per-group visibility filter: skip rows whose row-group or
        // col-group bucket label is excluded by the corresponding
        // group's `visible_values`. This drives the row-/col-header
        // ▾ dropdown without polluting the global Filters zone.
        if !group_keys_pass_visibility(&rk, &pt.rows) { continue; }
        if !group_keys_pass_visibility(&ck, &pt.cols) { continue; }
        if row_seen.insert(rk.clone()) { row_keys.push(rk.clone()); }
        if col_seen.insert(ck.clone()) { col_keys.push(ck.clone()); }
        for (vi, vdef) in pt.values.iter().enumerate() {
            let cell = row.get(vdef.source_col).unwrap_or(&CellValue::Empty);
            let entry = buckets.entry((rk.clone(), ck.clone(), vi)).or_default();
            if !matches!(cell, CellValue::Empty) {
                entry.non_empty += 1;
            }
            if let CellValue::Number(n) = cell {
                if !n.is_nan() { entry.nums.push(*n); }
            }
        }
    }

    // ─── Sort row / col keys ─────────────────────────────────
    apply_sort(&mut row_keys, &pt.rows);
    apply_sort(&mut col_keys, &pt.cols);

    // ─── Emit ────────────────────────────────────────────────
    emit(pt, header, &row_keys, &col_keys, &buckets, &data)
}

fn apply_sort(keys: &mut [Vec<String>], groups: &[PivotGroup]) {
    // Multi-axis sort: stable sort by each axis right-to-left so the
    // leftmost axis dominates.
    //
    // Comparison is text-lex for `Direct` (matches Excel for string
    // groups) and numeric for `NumericBin` (so "100–120" sorts AFTER
    // "20–40", not before — lex would invert the order). `Date(_)`
    // labels are zero-padded year-month / year-Q-quarter / year-mon-
    // day strings, so lex order coincides with chronological order
    // and the text path is fine.
    for (i, g) in groups.iter().enumerate().rev() {
        if matches!(g.sort_order, SortOrder::None) { continue; }
        let asc = matches!(g.sort_order, SortOrder::Asc);
        let numeric = matches!(g.kind, PivotGroupKind::NumericBin { .. });
        keys.sort_by(|a, b| {
            let av = a.get(i).map(|s| s.as_str()).unwrap_or("");
            let bv = b.get(i).map(|s| s.as_str()).unwrap_or("");
            let ord = if numeric {
                let an = parse_bin_lead(av);
                let bn = parse_bin_lead(bv);
                an.partial_cmp(&bn).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                av.cmp(bv)
            };
            if asc { ord } else { ord.reverse() }
        });
    }
}

/// Parse the leading numeric of a `NumericBin` label (`"20–40"` →
/// 20.0). Falls back to 0.0 when the label doesn't fit the bin shape
/// (typical when bucket_label fell through to `as_text` for a
/// non-numeric source cell).
fn parse_bin_lead(label: &str) -> f64 {
    label.split('–').next()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

fn emit(
    pt: &PivotTable,
    header: &[CellValue],
    row_keys: &[Vec<String>],
    col_keys: &[Vec<String>],
    buckets: &BTreeMap<(Vec<String>, Vec<String>, usize), Bucket>,
    data: &[&Vec<CellValue>],
) -> Vec<Vec<CellValue>> {
    let n_row_groups = pt.rows.len();
    let n_col_groups = pt.cols.len();
    let n_values = pt.values.len();
    let n_col_keys = if n_col_groups == 0 { 1 } else { col_keys.len() };

    // Width of each region of the output:
    //   row_label_cols + (col_keys × values) + maybe grand-total col
    let row_label_cols = match pt.layout_style {
        LayoutStyle::Compact => if n_row_groups > 0 { 1 } else { 0 },
        LayoutStyle::Outline | LayoutStyle::Tabular => n_row_groups,
    };
    // Cells per (col_key) reserved for value output. With values
    // configured, that's `n_values`. Without values but WITH col
    // groups, we still reserve one cell per col_key so the col-group
    // header has somewhere to land. With neither, the pivot is just
    // row labels — zero value cells per col_key.
    let value_cols_per_col_key = if n_values > 0 {
        n_values
    } else if n_col_groups > 0 {
        1
    } else {
        0
    };
    let value_cols = n_col_keys * value_cols_per_col_key;
    let want_grand_col = matches!(pt.grand_totals, GrandTotals::Cols | GrandTotals::Both)
        && n_values > 0;
    let total_cols = row_label_cols + value_cols + if want_grand_col { value_cols_per_col_key } else { 0 };

    let mut out: Vec<Vec<CellValue>> = Vec::new();

    // ─── Filter rows (Excel-style "page fields") ─────────────
    //
    // One row per configured filter, emitted ABOVE the col-group /
    // value-label headers, in the leftmost cell. The text reads e.g.
    // "Region: All" or "Region: North, South" (truncated). The grid
    // layer recognises these cells (rows in [anchor_row, anchor_row +
    // n_filters)) as click targets and opens a value-picker popover;
    // the cell's text itself is purely informational.
    let n_filters = pt.filters.len();
    for f in &pt.filters {
        let mut row: Vec<CellValue> = Vec::with_capacity(total_cols);
        let header_text = header
            .get(f.source_col)
            .map(|h| h.as_text())
            .unwrap_or_default();
        let summary = filter_summary_for_display(&f.condition);
        row.push(CellValue::Text(format!("{}: {} \u{25BE}", header_text, summary)));
        while row.len() < total_cols.max(1) { row.push(CellValue::Empty); }
        out.push(row);
    }
    let _ = n_filters; // referenced indirectly via pt.filters.len() callers

    // ─── Header rows ─────────────────────────────────────────
    //
    // One header row per col-group level (if any), plus one for value
    // labels (if values.len() > 1 OR cols.len() == 0). Excel always
    // shows value labels once a value exists; we follow that.
    if n_col_groups > 0 {
        for level in 0..n_col_groups {
            let mut hdr: Vec<CellValue> = Vec::with_capacity(total_cols);
            // Leftmost cell of the col-group level-0 row carries the
            // first col-group's field name + ▾ glyph as a click
            // target for the column-header value picker. Deeper
            // levels and other layout positions stay empty.
            if level == 0 && row_label_cols > 0 && !pt.cols.is_empty() {
                let g = &pt.cols[0];
                let label = g.label.clone()
                    .or_else(|| header.get(g.source_col).map(|h| h.as_text()))
                    .unwrap_or_default();
                hdr.push(CellValue::Text(format!("{} \u{25BE}", label)));
                for _ in 1..row_label_cols { hdr.push(CellValue::Empty); }
            } else {
                for _ in 0..row_label_cols { hdr.push(CellValue::Empty); }
            }
            for ck in col_keys {
                let label = ck.get(level).cloned().unwrap_or_default();
                for _ in 0..value_cols_per_col_key {
                    hdr.push(CellValue::Text(label.clone()));
                }
            }
            if want_grand_col && level == 0 {
                for _ in 0..value_cols_per_col_key { hdr.push(CellValue::Text("Grand Total".into())); }
            } else if want_grand_col {
                for _ in 0..value_cols_per_col_key { hdr.push(CellValue::Empty); }
            }
            out.push(hdr);
        }
    }
    // Value-label header row: always emit when values exist (matches
    // Excel: even with 1 value + 0 cols, the header reads "Sum of X").
    if n_values > 0 {
        let mut hdr: Vec<CellValue> = Vec::with_capacity(total_cols);
        // Row-group column headers go in the leftmost cells. The
        // trailing ▾ glyph is the user-facing affordance: clicking
        // these cells in the rendered spill opens a value-picker
        // popover that writes back to the matching PivotGroup's
        // `visible_values`.
        match pt.layout_style {
            LayoutStyle::Compact => {
                if row_label_cols > 0 {
                    if pt.rows.is_empty() {
                        hdr.push(CellValue::Empty);
                    } else {
                        // Compact stacks all row-group keys in one
                        // column, so a single "Row Labels ▾" handle
                        // is enough — the popover behind it drives
                        // the FIRST row group's `visible_values`,
                        // which is the most user-visible level.
                        hdr.push(CellValue::Text("Row Labels \u{25BE}".into()));
                    }
                }
            }
            LayoutStyle::Outline | LayoutStyle::Tabular => {
                for g in &pt.rows {
                    let label = g.label.clone()
                        .or_else(|| header.get(g.source_col).map(|h| h.as_text()))
                        .unwrap_or_default();
                    hdr.push(CellValue::Text(format!("{} \u{25BE}", label)));
                }
            }
        }
        let value_label = |vi: usize| -> String {
            let v = &pt.values[vi];
            if let Some(name) = &v.display_name { return name.clone(); }
            let src_label = header.get(v.source_col).map(|h| h.as_text()).unwrap_or_default();
            format!("{} of {}", agg_pretty(v.summarize_fn), src_label)
        };
        for _ in 0..n_col_keys {
            for vi in 0..n_values { hdr.push(CellValue::Text(value_label(vi))); }
        }
        if want_grand_col {
            for vi in 0..n_values { hdr.push(CellValue::Text(value_label(vi))); }
        }
        out.push(hdr);
    }

    // ─── Body rows ───────────────────────────────────────────
    for rk in row_keys {
        // Subtotals-Above placement: emit a subtotal row for the
        // outermost group flagged show_totals BEFORE its data row.
        emit_subtotal_if(pt, rk, row_keys, col_keys, buckets, &mut out, total_cols, value_cols_per_col_key, SubtotalsPos::Above);

        let mut row: Vec<CellValue> = Vec::with_capacity(total_cols);
        // Row-group label columns
        match pt.layout_style {
            LayoutStyle::Compact => {
                if row_label_cols > 0 {
                    let joined = rk.join(" / ");
                    row.push(CellValue::Text(joined));
                }
            }
            LayoutStyle::Outline | LayoutStyle::Tabular => {
                for v in rk.iter().take(n_row_groups) {
                    row.push(CellValue::Text(v.clone()));
                }
                while row.len() < n_row_groups { row.push(CellValue::Empty); }
            }
        }
        // Per (col_key × value) cells. The body always emits exactly
        // `value_cols_per_col_key` cells per col_key so the output
        // stays rectangular against the headers and filter rows —
        // try_register_spill_block reads cols from array[0].len()
        // and would panic on a jagged grid otherwise.
        for ck in col_key_iter(col_keys, n_col_groups) {
            if n_values == 0 {
                for _ in 0..value_cols_per_col_key {
                    row.push(CellValue::Empty);
                }
            } else {
                for vi in 0..n_values {
                    let b = buckets.get(&(rk.clone(), ck.clone(), vi)).cloned().unwrap_or_default();
                    row.push(fold_bucket(&b, pt.values[vi].summarize_fn));
                }
            }
        }
        // Grand-total column for this row
        if want_grand_col {
            for vi in 0..n_values {
                let total = grand_total_row(buckets, rk, vi);
                row.push(fold_bucket(&total, pt.values[vi].summarize_fn));
            }
        }
        out.push(row);

        emit_subtotal_if(pt, rk, row_keys, col_keys, buckets, &mut out, total_cols, value_cols_per_col_key, SubtotalsPos::Below);
    }

    // ─── Grand-total row ─────────────────────────────────────
    if matches!(pt.grand_totals, GrandTotals::Rows | GrandTotals::Both) && n_values > 0 {
        let mut tot: Vec<CellValue> = Vec::with_capacity(total_cols);
        if row_label_cols > 0 {
            tot.push(CellValue::Text("Grand Total".into()));
            for _ in 1..row_label_cols { tot.push(CellValue::Empty); }
        }
        for ck in col_key_iter(col_keys, n_col_groups) {
            for vi in 0..n_values {
                let total = grand_total_col(buckets, &ck, vi);
                tot.push(fold_bucket(&total, pt.values[vi].summarize_fn));
            }
        }
        if want_grand_col {
            for vi in 0..n_values {
                let total = grand_total_full(buckets, vi);
                tot.push(fold_bucket(&total, pt.values[vi].summarize_fn));
            }
        }
        out.push(tot);
    }

    let _ = data; // future: per-row drilldown / show-as-percent
    out
}

#[allow(clippy::too_many_arguments)]
fn emit_subtotal_if(
    pt: &PivotTable,
    rk: &[String],
    _row_keys: &[Vec<String>],
    col_keys: &[Vec<String>],
    buckets: &BTreeMap<(Vec<String>, Vec<String>, usize), Bucket>,
    out: &mut Vec<Vec<CellValue>>,
    total_cols: usize,
    value_cols_per_col_key: usize,
    pos: SubtotalsPos,
) {
    if pt.subtotals_position != pos { return; }
    // Subtotal a single row at its OWN level only — the v2.0 design
    // ships per-row subtotals (not per-parent rollups). Each row that
    // belongs to a group with show_totals=true gets a subtotal ROW
    // emitted alongside it. v2.1 will add hierarchical rollups.
    let needs = pt.rows.iter().any(|g| g.show_totals);
    if !needs { return; }
    let n_values = pt.values.len();
    let n_col_groups = pt.cols.len();
    let mut row: Vec<CellValue> = Vec::with_capacity(total_cols);
    let row_label_cols = match pt.layout_style {
        LayoutStyle::Compact => if pt.rows.is_empty() { 0 } else { 1 },
        LayoutStyle::Outline | LayoutStyle::Tabular => pt.rows.len(),
    };
    if row_label_cols > 0 {
        row.push(CellValue::Text(format!("{} Total", rk.join(" / "))));
        for _ in 1..row_label_cols { row.push(CellValue::Empty); }
    }
    let value_cols_per_col_key = if n_values > 0 {
        n_values
    } else if n_col_groups > 0 {
        1
    } else {
        0
    };
    for ck in col_key_iter(col_keys, n_col_groups) {
        if n_values == 0 {
            for _ in 0..value_cols_per_col_key {
                row.push(CellValue::Empty);
            }
        } else {
            for vi in 0..n_values {
                let b = buckets.get(&(rk.to_vec(), ck.clone(), vi)).cloned().unwrap_or_default();
                row.push(fold_bucket(&b, pt.values[vi].summarize_fn));
            }
        }
    }
    if matches!(pt.grand_totals, GrandTotals::Cols | GrandTotals::Both) && n_values > 0 {
        for vi in 0..n_values {
            let total = grand_total_row(buckets, rk, vi);
            row.push(fold_bucket(&total, pt.values[vi].summarize_fn));
        }
    }
    let _ = value_cols_per_col_key;
    out.push(row);
}

fn col_key_iter(col_keys: &[Vec<String>], n_col_groups: usize) -> Vec<Vec<String>> {
    if n_col_groups == 0 { vec![vec![]] } else { col_keys.to_vec() }
}

fn grand_total_row(
    buckets: &BTreeMap<(Vec<String>, Vec<String>, usize), Bucket>,
    rk: &[String],
    vi: usize,
) -> Bucket {
    let mut acc = Bucket::default();
    for ((r, _c, v), b) in buckets {
        if r == rk && *v == vi {
            acc.nums.extend_from_slice(&b.nums);
            acc.non_empty += b.non_empty;
        }
    }
    acc
}

fn grand_total_col(
    buckets: &BTreeMap<(Vec<String>, Vec<String>, usize), Bucket>,
    ck: &[String],
    vi: usize,
) -> Bucket {
    let mut acc = Bucket::default();
    for ((_r, c, v), b) in buckets {
        if c == ck && *v == vi {
            acc.nums.extend_from_slice(&b.nums);
            acc.non_empty += b.non_empty;
        }
    }
    acc
}

fn grand_total_full(
    buckets: &BTreeMap<(Vec<String>, Vec<String>, usize), Bucket>,
    vi: usize,
) -> Bucket {
    let mut acc = Bucket::default();
    for ((_r, _c, v), b) in buckets {
        if *v == vi {
            acc.nums.extend_from_slice(&b.nums);
            acc.non_empty += b.non_empty;
        }
    }
    acc
}

/// Human-readable one-line summary of a filter's condition for the
/// Excel-style "page field" filter row at the top of the pivot
/// output. Designed to fit in a single cell. The chip popover (in
/// the editor sidebar / inline grid popover) is the source of truth
/// for editing — this is a status line.
fn filter_summary_for_display(c: &PivotFilterCondition) -> String {
    const MAX_LIST: usize = 2;
    match c {
        PivotFilterCondition::ValueIn(v) => {
            if v.is_empty() { return "(none)".into(); }
            if v.len() <= MAX_LIST { return v.join(", "); }
            // "first, second, +N more" rather than ellipsis so the
            // user can tell at a glance how many values are kept.
            format!("{}, +{} more", v[..MAX_LIST].join(", "), v.len() - MAX_LIST)
        }
        PivotFilterCondition::NumberGreater(n) => format!("> {n}"),
        PivotFilterCondition::NumberLess(n) => format!("< {n}"),
        PivotFilterCondition::NumberEqual(n) => format!("= {n}"),
        PivotFilterCondition::NumberBetween(lo, hi) => format!("{lo}–{hi}"),
        PivotFilterCondition::TextContains(s) => format!("contains \"{s}\""),
        PivotFilterCondition::TextEquals(s) => format!("= \"{s}\""),
        PivotFilterCondition::TextStartsWith(s) => format!("starts \"{s}\""),
        PivotFilterCondition::Empty => "empty".into(),
        PivotFilterCondition::NotEmpty => "All".into(),
    }
}

fn agg_pretty(sf: SummarizeFn) -> &'static str {
    match sf {
        SummarizeFn::Sum => "Sum",
        SummarizeFn::Count => "Count",
        SummarizeFn::CountA => "CountA",
        SummarizeFn::Average => "Average",
        SummarizeFn::Min => "Min",
        SummarizeFn::Max => "Max",
        SummarizeFn::Median => "Median",
        SummarizeFn::Product => "Product",
        SummarizeFn::StdDev => "StdDev",
        SummarizeFn::StdDevP => "StdDevP",
        SummarizeFn::Var => "Var",
        SummarizeFn::VarP => "VarP",
    }
}

// ─── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cv_n(n: f64) -> CellValue { CellValue::Number(n) }
    fn cv_t(s: &str) -> CellValue { CellValue::Text(s.into()) }

    fn sales_data() -> Vec<Vec<CellValue>> {
        // header: Region | Product | Revenue | OrderDate
        vec![
            vec![cv_t("Region"), cv_t("Product"), cv_t("Revenue"), cv_t("OrderDate")],
            vec![cv_t("West"),   cv_t("A"),       cv_n(10.0),       cv_t("2026-01-15")],
            vec![cv_t("West"),   cv_t("B"),       cv_n(20.0),       cv_t("2026-02-15")],
            vec![cv_t("East"),   cv_t("A"),       cv_n(30.0),       cv_t("2026-01-15")],
            vec![cv_t("East"),   cv_t("A"),       cv_n(40.0),       cv_t("2026-02-15")],
            vec![cv_t("East"),   cv_t("B"),       cv_n(50.0),       cv_t("2026-03-15")],
        ]
    }

    fn pivot(rows: Vec<PivotGroup>, values: Vec<PivotValue>) -> PivotTable {
        PivotTable {
            anchor: (0, 0),
            source: SourceRange::Local { range_a1: "A1:D6".into() },
            rows,
            cols: vec![],
            values,
            filters: vec![],
            value_layout: ValueLayout::Horizontal,
            layout_style: LayoutStyle::Tabular,
            grand_totals: GrandTotals::None,
            subtotals_position: SubtotalsPos::Below,
        }
    }

    fn group(col: usize) -> PivotGroup {
        PivotGroup { source_col: col, ..Default::default() }
    }
    fn value_sum(col: usize) -> PivotValue {
        PivotValue { source_col: col, summarize_fn: SummarizeFn::Sum, display_name: None }
    }

    // ─── Pure-data evaluator tests ───────────────────────────

    #[test]
    fn pivot_single_row_field_sum() {
        // Replicates the v1 SUM behavior: group by Region, sum
        // Revenue. Expect West=30, East=120.
        let pt = pivot(vec![group(0)], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        assert_eq!(out.len(), 3);                                // header + 2 rows
        assert_eq!(out[1][0], cv_t("West"));
        assert_eq!(out[1][1], cv_n(30.0));
        assert_eq!(out[2][0], cv_t("East"));
        assert_eq!(out[2][1], cv_n(120.0));
    }

    #[test]
    fn pivot_two_row_fields_nested_grouping() {
        // Region > Product. Expect 4 distinct row keys:
        // (West,A)=10, (West,B)=20, (East,A)=70, (East,B)=50.
        let pt = pivot(vec![group(0), group(1)], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        assert_eq!(out.len(), 5);
        let body: Vec<_> = out.iter().skip(1).collect();
        let find = |a: &str, b: &str| -> Option<f64> {
            body.iter().find_map(|r| {
                if r[0].as_text() == a && r[1].as_text() == b {
                    if let CellValue::Number(n) = r[2] { Some(n) } else { None }
                } else { None }
            })
        };
        assert_eq!(find("West", "A"), Some(10.0));
        assert_eq!(find("West", "B"), Some(20.0));
        assert_eq!(find("East", "A"), Some(70.0));
        assert_eq!(find("East", "B"), Some(50.0));
    }

    #[test]
    fn pivot_one_row_one_col_two_values() {
        // Rows = Region, Cols = Product, Values = [SUM(Revenue),
        // COUNT(Revenue)]. Expect interleaved value labels per col.
        let pt = PivotTable {
            cols: vec![group(1)],
            values: vec![
                value_sum(2),
                PivotValue { source_col: 2, summarize_fn: SummarizeFn::Count, display_name: None },
            ],
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        // Output layout (Tabular):
        //   row[0] = col-group header (A | A | B | B)
        //   row[1] = value-label header (Region | Sum | Count | Sum | Count)
        //   row[2] = West | 10 | 1 | 20 | 1
        //   row[3] = East | 70 | 2 | 50 | 1
        assert_eq!(out.len(), 4);
        assert_eq!(out[0][1].as_text(), "A");
        assert_eq!(out[0][3].as_text(), "B");
        let west = out.iter().find(|r| r[0].as_text() == "West").unwrap();
        assert_eq!(west[1], cv_n(10.0));
        assert_eq!(west[2], cv_n(1.0));
        assert_eq!(west[3], cv_n(20.0));
        assert_eq!(west[4], cv_n(1.0));
    }

    #[test]
    fn pivot_value_layout_horizontal_vs_vertical() {
        // For now we only have Horizontal implemented (values inside
        // each col group). Vertical is documented as a future shape;
        // assert Horizontal renders as expected.
        let pt = PivotTable {
            cols: vec![group(1)],
            values: vec![
                value_sum(2),
                PivotValue { source_col: 2, summarize_fn: SummarizeFn::Count, display_name: None },
            ],
            value_layout: ValueLayout::Horizontal,
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let h = eval_pivot(&pt, &sales_data());
        assert!(!h.is_empty());
        // Horizontal: 5 columns total (Region + 2*A + 2*B)
        assert_eq!(h[1].len(), 5);
    }

    #[test]
    fn pivot_filter_value_in_excludes_rows() {
        let pt = PivotTable {
            filters: vec![PivotFilterSpec {
                source_col: 1,
                condition: PivotFilterCondition::ValueIn(vec!["A".into()]),
            }],
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        // Only Product=A rows survive: West=10, East=70.
        let body: Vec<_> = out.iter().skip(1).collect();
        let west = body.iter().find(|r| r[0].as_text() == "West").unwrap();
        assert_eq!(west[1], cv_n(10.0));
        let east = body.iter().find(|r| r[0].as_text() == "East").unwrap();
        assert_eq!(east[1], cv_n(70.0));
    }

    #[test]
    fn pivot_filter_number_gt() {
        let pt = PivotTable {
            filters: vec![PivotFilterSpec {
                source_col: 2,
                condition: PivotFilterCondition::NumberGreater(25.0),
            }],
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        // Surviving rows: West/B=NO(20), East/A=30+40=70, East/B=50.
        // West total = 0 (filtered out completely; no row).
        let body: Vec<_> = out.iter().skip(1).collect();
        let west = body.iter().find(|r| r[0].as_text() == "West");
        let east = body.iter().find(|r| r[0].as_text() == "East").unwrap();
        assert!(west.is_none());
        assert_eq!(east[1], cv_n(120.0));
    }

    #[test]
    fn pivot_grand_totals_rows_only() {
        let pt = PivotTable {
            grand_totals: GrandTotals::Rows,
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        let last = out.last().unwrap();
        assert_eq!(last[0].as_text(), "Grand Total");
        assert_eq!(last[1], cv_n(150.0));
        // Width = 2 columns (Region + value); no grand-total column.
        assert_eq!(out[0].len(), 2);
    }

    #[test]
    fn pivot_subtotals_above_vs_below() {
        let mut g = group(0);
        g.show_totals = true;
        let above = PivotTable {
            subtotals_position: SubtotalsPos::Above,
            ..pivot(vec![g.clone()], vec![value_sum(2)])
        };
        let below = PivotTable {
            subtotals_position: SubtotalsPos::Below,
            ..pivot(vec![g], vec![value_sum(2)])
        };
        let above_out = eval_pivot(&above, &sales_data());
        let below_out = eval_pivot(&below, &sales_data());
        // Both have header + 2 region rows + 2 subtotal rows = 5.
        assert_eq!(above_out.len(), 5);
        assert_eq!(below_out.len(), 5);
        // Above: row 1 is the West subtotal; row 2 is West data.
        assert!(above_out[1][0].as_text().contains("Total"));
        assert!(!above_out[2][0].as_text().contains("Total"));
        // Below: opposite.
        assert!(!below_out[1][0].as_text().contains("Total"));
        assert!(below_out[2][0].as_text().contains("Total"));
    }

    #[test]
    fn pivot_summarize_fn_all_variants() {
        // Hit each SummarizeFn at least once to confirm the dispatch
        // arm doesn't panic. Single-row source, single value, one
        // numeric column.
        let src = vec![
            vec![cv_t("k"), cv_t("v")],
            vec![cv_t("a"), cv_n(1.0)],
            vec![cv_t("a"), cv_n(2.0)],
            vec![cv_t("a"), cv_n(3.0)],
            vec![cv_t("a"), cv_n(4.0)],
            vec![cv_t("a"), cv_n(5.0)],
        ];
        for sf in [
            SummarizeFn::Sum, SummarizeFn::Count, SummarizeFn::CountA,
            SummarizeFn::Average, SummarizeFn::Min, SummarizeFn::Max,
            SummarizeFn::Median, SummarizeFn::Product, SummarizeFn::StdDev,
            SummarizeFn::StdDevP, SummarizeFn::Var, SummarizeFn::VarP,
        ] {
            let pt = pivot(
                vec![group(0)],
                vec![PivotValue { source_col: 1, summarize_fn: sf, display_name: None }],
            );
            let out = eval_pivot(&pt, &src);
            assert_eq!(out.len(), 2, "sf={sf:?}");
            // Just a sanity check that we got *some* numeric result;
            // exact value semantics covered by SummarizeFn unit tests.
            match &out[1][1] {
                CellValue::Number(_) => {}
                CellValue::Error(_) => {}
                other => panic!("unexpected variant for {sf:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn pivot_date_grouping_by_month_buckets_correctly() {
        // OrderDate spans Jan / Feb / Mar 2026. Group rows by month;
        // sum revenue. Expect 3 buckets: 2026-01=40, 2026-02=60,
        // 2026-03=50.
        let mut g = group(3);
        g.kind = PivotGroupKind::Date(DateGranularity::Month);
        let pt = pivot(vec![g], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        let body: Vec<_> = out.iter().skip(1).collect();
        let find = |label: &str| body.iter().find_map(|r| {
            if r[0].as_text() == label { Some(r[1].clone()) } else { None }
        });
        assert_eq!(find("2026-01"), Some(cv_n(40.0)));
        assert_eq!(find("2026-02"), Some(cv_n(60.0)));
        assert_eq!(find("2026-03"), Some(cv_n(50.0)));
    }

    #[test]
    fn pivot_date_grouping_by_quarter() {
        // Same input as month test; with Quarter granularity all 5
        // rows fall in 2026-Q1, sum=150.
        let mut g = group(3);
        g.kind = PivotGroupKind::Date(DateGranularity::Quarter);
        let pt = pivot(vec![g], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        let body: Vec<_> = out.iter().skip(1).collect();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0][0].as_text(), "2026-Q1");
        assert_eq!(body[0][1], cv_n(150.0));
    }

    #[test]
    fn pivot_numeric_bin_grouping_by_width() {
        // Bin Revenue by width=20 starting at 0.
        // Buckets: [0–20)=10 (West/A), [20–40)=30+30=60? Wait —
        // West/B=20 falls in [20–40), East/A1=30 falls in [20–40),
        // East/A2=40 falls in [40–60), East/B=50 falls in [40–60).
        // So [0–20)=10, [20–40)=20+30=50, [40–60)=40+50=90.
        let mut g = PivotGroup { source_col: 2, ..Default::default() };
        g.kind = PivotGroupKind::NumericBin { width: 20.0, start: Some(0.0) };
        let pt = pivot(vec![g], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        let body: Vec<_> = out.iter().skip(1).collect();
        let find = |label: &str| body.iter().find_map(|r| {
            if r[0].as_text() == label { Some(r[1].clone()) } else { None }
        });
        assert_eq!(find("0–20"), Some(cv_n(10.0)));
        assert_eq!(find("20–40"), Some(cv_n(50.0)));
        assert_eq!(find("40–60"), Some(cv_n(90.0)));
    }

    #[test]
    fn pivot_layout_style_compact_vs_tabular() {
        // Two row groups: Compact stacks row-keys in 1 column with
        // " / " separator; Tabular spreads them across 2 columns.
        let compact = PivotTable {
            layout_style: LayoutStyle::Compact,
            ..pivot(vec![group(0), group(1)], vec![value_sum(2)])
        };
        let tabular = PivotTable {
            layout_style: LayoutStyle::Tabular,
            ..pivot(vec![group(0), group(1)], vec![value_sum(2)])
        };
        let c_out = eval_pivot(&compact, &sales_data());
        let t_out = eval_pivot(&tabular, &sales_data());
        assert_eq!(c_out[1].len(), 2);  // 1 row-label col + 1 value
        assert_eq!(t_out[1].len(), 3);  // 2 row-label cols + 1 value
        assert!(c_out[1][0].as_text().contains(" / "));
    }

    #[test]
    fn pivot_sort_by_value_descending() {
        // group on Region with Desc sort — alphabetical reversed.
        let mut g = group(0);
        g.sort_order = SortOrder::Desc;
        let pt = pivot(vec![g], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        let body: Vec<_> = out.iter().skip(1).collect();
        // Desc by region label: West > East alphabetically.
        assert_eq!(body[0][0].as_text(), "West");
        assert_eq!(body[1][0].as_text(), "East");
    }

    // ─── Regressions for the phase-1 reviewer findings ─────────

    #[test]
    fn date_bucket_handles_excel_serial_60_quirk() {
        // Excel preserves Lotus's bogus Feb 29 1900 at serial 60.
        // serial_to_ymd MUST map 60 → (1900, 2, 29) so it doesn't
        // collide with serial 61 (1900-03-01); both previously
        // produced (1900, 3, 1).
        assert_eq!(serial_to_ymd(60.0), Some((1900, 2, 29)));
        assert_eq!(serial_to_ymd(61.0), Some((1900, 3, 1)));
        assert_eq!(serial_to_ymd(59.0), Some((1900, 2, 28)));
        assert_eq!(date_bucket_label(60.0, DateGranularity::Day),
                   Some("1900-02-29".to_string()));
        assert_eq!(date_bucket_label(61.0, DateGranularity::Day),
                   Some("1900-03-01".to_string()));
    }

    #[test]
    fn date_bucket_rejects_serial_below_one() {
        // Excel serials < 1 are nonsense (epoch is 1900-01-01 = 1).
        // Previously serial=0 produced (1900, 1, 0) — invalid day.
        assert_eq!(serial_to_ymd(0.0), None);
        assert_eq!(serial_to_ymd(-5.0), None);
        assert_eq!(serial_to_ymd(f64::NAN), None);
        // bucket_label for an invalid serial falls through to the
        // raw text representation, so the user sees "0" rather than
        // a bogus "1900-01-00" group key.
        let v = CellValue::Number(0.0);
        let mut g = group(3);
        g.kind = PivotGroupKind::Date(DateGranularity::Day);
        assert_eq!(bucket_label(&g.kind, &v), "0");
    }

    #[test]
    fn numeric_bin_sort_is_numeric_not_lex() {
        // With width=20 and source values spanning 0..120, lex sort
        // would put "100–120" before "20–40". Numeric sort puts
        // them in ascending bucket-start order.
        let src: Vec<Vec<CellValue>> = std::iter::once(
            vec![cv_t("k"), cv_t("v")],
        ).chain((0..120_i64).step_by(15).map(|n| vec![cv_t("x"), cv_n(n as f64)])).collect();
        let mut g = PivotGroup { source_col: 1, ..Default::default() };
        g.kind = PivotGroupKind::NumericBin { width: 20.0, start: Some(0.0) };
        g.sort_order = SortOrder::Asc;
        let pt = pivot(vec![g], vec![value_sum(1)]);
        let out = eval_pivot(&pt, &src);
        let body: Vec<_> = out.iter().skip(1).map(|r| r[0].as_text()).collect();
        // Expect: 0–20, 20–40, 40–60, 60–80, 80–100, 100–120.
        assert_eq!(body[0], "0–20");
        assert_eq!(body[1], "20–40");
        // Crucial: 100–120 sorts LAST, not after 0–20.
        assert_eq!(body.last().map(|s| s.as_str()), Some("100–120"));
    }

    #[test]
    fn pivot_filter_only_with_no_values_no_cols_emits_rectangular() {
        // Regression for a jagged-output crash: a pivot with rows +
        // filters but neither values nor cols used to emit a
        // 2-cell-wide filter row above a 1-cell-wide body row. Now
        // value_cols_per_col_key collapses to 0 in this shape, so
        // both rows are row_label_cols wide.
        let pt = PivotTable {
            anchor: (2, 2),
            source: SourceRange::Local { range_a1: "A1:B2".into() },
            rows: vec![PivotGroup {
                source_col: 0,
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
        };
        let src = vec![
            vec![cv_t("x"), CellValue::Empty],
            vec![CellValue::Empty, CellValue::Empty],
        ];
        let out = eval_pivot(&pt, &src);
        assert!(!out.is_empty());
        let width = out[0].len();
        for (i, row) in out.iter().enumerate() {
            assert_eq!(row.len(), width, "row {i} jagged");
        }
    }

    #[test]
    fn pivot_group_visible_values_filters_row_keys() {
        // PivotGroup.visible_values is an inclusion whitelist driven
        // by the row-/col-header value-picker dropdown. Setting it
        // to ["West"] on the Region row group must drop "East" rows
        // from the output without touching `pt.filters`.
        let mut g = group(0); // Region
        g.visible_values = Some(vec!["West".into()]);
        let pt = pivot(vec![g], vec![value_sum(2)]);
        let out = eval_pivot(&pt, &sales_data());
        let body: Vec<_> = out.iter().skip(1).collect(); // skip value-label header
        // Only "West" should remain.
        let west = body.iter().find(|r| r[0].as_text() == "West");
        let east = body.iter().find(|r| r[0].as_text() == "East");
        assert!(west.is_some(), "West must survive the visible_values whitelist");
        assert!(east.is_none(), "East must be excluded by the visible_values whitelist");
    }

    #[test]
    fn pivot_col_group_visible_values_filters_col_keys() {
        // Same mechanism on the col axis.
        let mut g = group(1); // Product
        g.visible_values = Some(vec!["A".into()]);
        let pt = PivotTable {
            cols: vec![g],
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        // Col-group level 0 header row has the leftmost cell as
        // "Product ▾", followed by the col_keys. Only "A" should
        // appear (no "B"). Find any cell holding "B" — should be
        // none.
        let any_b = out.iter().any(|r| r.iter().any(|c| c.as_text() == "B"));
        assert!(!any_b, "B must be excluded by the col-group visible_values whitelist");
    }

    #[test]
    fn pivot_emits_one_page_field_filter_row_per_filter_at_top_of_output() {
        // Filters are rendered as Excel-style "page fields": one row
        // per filter, emitted before the col-group / value-label
        // headers. The leftmost cell carries a status string ending
        // in the dropdown glyph; the rest of the row is empty so the
        // grid layer can overlay (or layer-on) interactivity later.
        let pt = PivotTable {
            filters: vec![
                PivotFilterSpec {
                    source_col: 0,
                    condition: PivotFilterCondition::ValueIn(vec!["West".into(), "East".into()]),
                },
                PivotFilterSpec {
                    source_col: 1,
                    condition: PivotFilterCondition::TextEquals("A".into()),
                },
            ],
            ..pivot(vec![group(0)], vec![value_sum(2)])
        };
        let out = eval_pivot(&pt, &sales_data());
        // First two rows must be the filter rows, in order.
        assert!(out.len() >= 3, "expected filter rows + headers + body");
        let f0 = out[0][0].as_text();
        let f1 = out[1][0].as_text();
        assert!(f0.starts_with("Region:"), "row 0 leftmost cell: {f0:?}");
        assert!(f0.ends_with('\u{25BE}'), "row 0 must end with dropdown glyph: {f0:?}");
        assert!(f1.starts_with("Product:"), "row 1 leftmost cell: {f1:?}");
        // Row 2 onwards is the value-label header + body, unchanged
        // by filter-row presence. In Tabular layout (default in
        // `pivot()`) the value-label header carries the row-group
        // field name in the leftmost cell, with the ▾ glyph appended
        // as the row-header value-picker click target.
        let value_header = out[2][0].as_text();
        assert_eq!(value_header, "Region \u{25BE}");
    }

    #[test]
    fn pivot_with_rows_and_cols_but_no_values_emits_rectangular_grid() {
        // Regression: when the user is mid-build (rows and cols
        // configured, no values yet), the body must match the
        // col-group header width. Previously the body skipped
        // value-cells entirely while the header reserved one cell
        // per col-key, producing a jagged grid that crashed
        // try_register_spill_block on the spill register pass.
        //
        // Concrete repro from the live UI: drag Date → Rows, then
        // Region → Cols on a 7-col source, no Values configured →
        // index-out-of-bounds panic in eval.rs::try_register_spill_block.
        let pt = PivotTable {
            cols: vec![group(0)],   // group rows by Region (col 0 in this fixture)
            ..pivot(vec![group(1)], vec![]) // rows = Product (col 1), values = []
        };
        let out = eval_pivot(&pt, &sales_data());
        assert!(!out.is_empty(), "expected at least the col-group header row");
        let width = out[0].len();
        for (i, row) in out.iter().enumerate() {
            assert_eq!(
                row.len(), width,
                "row {i} length {} != header width {width}; output is jagged",
                row.len()
            );
        }
    }
}
