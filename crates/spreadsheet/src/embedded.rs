// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Sparse inline-workbook persistence and bounded evaluation. Coordinate keys
//! are row/column; engine addresses are column/row. No displayed values are
//! persisted, so browser and export use the same formulas and number formats.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::eval::{CellAddr, CellStyle, CellValue, SpreadsheetEngine};
use crate::parser::{Expr, RangeRef, SpreadsheetError, parse_formula};

pub const DEFAULT_ROWS: usize = 12;
pub const DEFAULT_COLS: usize = 6;
pub const MAX_ROWS: usize = 1000;
pub const MAX_COLS: usize = 256;
pub const MAX_SHEETS: usize = 32;
pub const MAX_STORED_CELLS: usize = 10_000;
/// Maximum UTF-8 bytes per cell, including source formulas and computed text.
pub const MAX_CELL_CHARS: usize = 4096;
pub const MAX_FORMAT_CHARS: usize = 128;
const MAX_REFERENCE_WORK: usize = 100_000;
const MAX_FORMULA_NODES: usize = 512;

pub(crate) type WorkbookValues = Arc<Mutex<HashMap<String, HashMap<CellAddr, CellValue>>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetData {
    pub id: String,
    pub name: String,
    pub rows: usize,
    pub cols: usize,
    pub cells: BTreeMap<CellAddr, String>,
    pub formats: BTreeMap<CellAddr, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SheetError {
    pub field: String,
    pub reason: String,
}
impl std::fmt::Display for SheetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.reason)
    }
}
impl std::error::Error for SheetError {}
fn invalid(field: &str, reason: &str) -> SheetError {
    SheetError {
        field: field.into(),
        reason: reason.into(),
    }
}

pub fn cell_key(row: usize, col: usize) -> String {
    format!("cell:{row}:{col}")
}
pub fn format_key(row: usize, col: usize) -> String {
    format!("format:{row}:{col}")
}

fn required<'a>(attrs: &'a HashMap<String, String>, key: &str) -> Result<&'a str, SheetError> {
    attrs
        .get(key)
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid(key, "required nonempty attribute"))
}
fn dimension(attrs: &HashMap<String, String>, key: &str, max: usize) -> Result<usize, SheetError> {
    let text = required(attrs, key)?;
    let n = text
        .parse::<usize>()
        .map_err(|_| invalid(key, "expected canonical positive integer"))?;
    if n == 0 || n > max || text != n.to_string() {
        return Err(invalid(key, "dimension exceeds limits or is not canonical"));
    }
    Ok(n)
}

/// Decode untrusted attributes without allocating a dense grid. Empty sparse
/// attributes are accepted as clears because ordinary editor undo uses them.
pub fn decode_sheet(attrs: &HashMap<String, String>) -> Result<SheetData, SheetError> {
    let id = required(attrs, "blockId")?;
    let name = required(attrs, "sheetName")?;
    if id.len() > 128 || id.chars().any(char::is_control) {
        return Err(invalid("blockId", "identity exceeds limits"));
    }
    if name.len() > 128 || name.trim() != name || name.chars().any(char::is_control) {
        return Err(invalid(
            "sheetName",
            "sheet name exceeds limits or has surrounding whitespace",
        ));
    }
    let rows = dimension(attrs, "gridRows", MAX_ROWS)?;
    let cols = dimension(attrs, "gridCols", MAX_COLS)?;
    let mut sheet = SheetData {
        id: id.into(),
        name: name.into(),
        rows,
        cols,
        cells: BTreeMap::new(),
        formats: BTreeMap::new(),
    };
    // Bound even empty tombstones, not just live cells.
    if attrs.len() > MAX_STORED_CELLS * 2 + 4 {
        return Err(invalid("cells", "too many sparse attributes"));
    }
    for (key, value) in attrs {
        if matches!(
            key.as_str(),
            "blockId" | "sheetName" | "gridRows" | "gridCols"
        ) {
            continue;
        }
        let mut parts = key.split(':');
        let kind = parts.next().unwrap_or("");
        let row = parts.next().and_then(|s| s.parse::<usize>().ok());
        let col = parts.next().and_then(|s| s.parse::<usize>().ok());
        let (Some(row), Some(col)) = (row, col) else {
            return Err(invalid(key, "unknown sheet attribute"));
        };
        if parts.next().is_some()
            || !matches!(kind, "cell" | "format")
            || key != &format!("{kind}:{row}:{col}")
        {
            return Err(invalid(key, "expected canonical sparse coordinate"));
        }
        if row >= rows || col >= cols {
            return Err(invalid(key, "coordinate outside sheet dimensions"));
        }
        let limit = if kind == "cell" {
            MAX_CELL_CHARS
        } else {
            MAX_FORMAT_CHARS
        };
        if value.len() > limit {
            return Err(invalid(key, "value exceeds byte limit"));
        }
        if !value.is_empty() {
            if kind == "cell" {
                sheet.cells.insert((col, row), value.clone());
            } else {
                sheet.formats.insert((col, row), value.clone());
            }
        }
    }
    if sheet.cells.len() > MAX_STORED_CELLS || sheet.formats.len() > MAX_STORED_CELLS {
        return Err(invalid("cells", "too many stored cells or formats"));
    }
    Ok(sheet)
}

pub fn validate_workbook(sheets: &[SheetData]) -> Result<(), SheetError> {
    if sheets.is_empty() || sheets.len() > MAX_SHEETS {
        return Err(invalid(
            "sheets",
            "workbook must contain between 1 and 32 sheets",
        ));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut count = 0usize;
    for s in sheets {
        if !ids.insert(&s.id) {
            return Err(invalid("blockId", "duplicate sheet identity"));
        }
        if !names.insert(s.name.to_ascii_lowercase()) {
            return Err(invalid(
                "sheetName",
                "duplicate sheet name (case insensitive)",
            ));
        }
        if s.id.is_empty()
            || s.name.trim().is_empty()
            || s.rows == 0
            || s.cols == 0
            || s.rows > MAX_ROWS
            || s.cols > MAX_COLS
        {
            return Err(invalid("sheets", "invalid sheet metadata"));
        }
        count = count
            .saturating_add(s.cells.len())
            .saturating_add(s.formats.len());
        if count > MAX_STORED_CELLS {
            return Err(invalid(
                "cells",
                "workbook exceeds 10000 sparse values and formats",
            ));
        }
        for (&(col, row), value) in &s.cells {
            if col >= s.cols || row >= s.rows || value.len() > MAX_CELL_CHARS {
                return Err(invalid(
                    "cells",
                    "cell outside dimensions or exceeds byte limit",
                ));
            }
        }
        for (&(col, row), value) in &s.formats {
            if col >= s.cols || row >= s.rows || value.len() > MAX_FORMAT_CHARS {
                return Err(invalid(
                    "formats",
                    "format outside dimensions or exceeds byte limit",
                ));
            }
        }
    }
    Ok(())
}

// Inline export evaluates untrusted persisted expressions on the server. Only
// audited bounded scalar and aggregate functions are enabled here. Standalone
// spreadsheet evaluation is unchanged. Unsupported functions keep their source
// and display #NAME?; pathological reference work displays #NUM!.
fn bounded_function(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "SUM"
            | "AVERAGE"
            | "MIN"
            | "MAX"
            | "COUNT"
            | "COUNTA"
            | "COUNTBLANK"
            | "ABS"
            | "ROUND"
            | "ROUNDUP"
            | "ROUNDDOWN"
            | "INT"
            | "MOD"
            | "POWER"
            | "SQRT"
            | "PI"
            | "CEILING"
            | "FLOOR"
            | "RAND"
            | "RANDBETWEEN"
            | "PRODUCT"
            | "SIGN"
            | "LN"
            | "LOG"
            | "LOG10"
            | "EXP"
            | "TRUNC"
            | "SUMIF"
            | "COUNTIF"
            | "SUMIFS"
            | "COUNTIFS"
            | "AVERAGEIF"
            | "AVERAGEIFS"
            | "SUMSQ"
            | "SIN"
            | "COS"
            | "TAN"
            | "ASIN"
            | "ACOS"
            | "ATAN"
            | "ATAN2"
            | "SINH"
            | "COSH"
            | "TANH"
            | "RADIANS"
            | "DEGREES"
            | "IF"
            | "IFERROR"
            | "IFNA"
            | "AND"
            | "OR"
            | "NOT"
            | "XOR"
            | "TRUE"
            | "FALSE"
            | "ISBLANK"
            | "ISNUMBER"
            | "ISTEXT"
            | "ISERROR"
            | "ISNA"
            | "LEN"
            | "LEFT"
            | "RIGHT"
            | "MID"
            | "UPPER"
            | "LOWER"
            | "TRIM"
            | "VALUE"
            | "FIND"
            | "SEARCH"
            | "CHAR"
            | "CODE"
            | "EXACT"
            | "CLEAN"
            | "TODAY"
            | "NOW"
            | "DATE"
            | "TIME"
            | "YEAR"
            | "MONTH"
            | "DAY"
            | "HOUR"
            | "MINUTE"
            | "SECOND"
            | "WEEKDAY"
            | "ISODD"
            | "ISEVEN"
            | "NA"
            | "N"
            | "T"
            | "CHOOSE"
            | "INDEX"
            | "MATCH"
            | "VLOOKUP"
            | "HLOOKUP"
    )
}

type Address = (usize, usize, usize); // sheet, column, row
fn references(
    expr: &Expr,
    sheet: usize,
    sheets: &[SheetData],
    names: &HashMap<String, usize>,
    work: &mut usize,
) -> Result<HashSet<Address>, SpreadsheetError> {
    let mut pending = vec![expr];
    let mut refs = HashSet::new();
    let mut nodes = 0usize;
    while let Some(e) = pending.pop() {
        nodes += 1;
        if nodes > MAX_FORMULA_NODES {
            return Err(SpreadsheetError::Num);
        }
        let mut range: Option<(usize, RangeRef)> = None;
        match e {
            Expr::CellRef(c) => {
                range = Some((
                    sheet,
                    RangeRef {
                        start: c.clone(),
                        end: c.clone(),
                    },
                ))
            }
            Expr::Range(r) => range = Some((sheet, r.clone())),
            Expr::SheetCellRef { sheet: name, cell } => {
                let target = *names
                    .get(&name.to_ascii_lowercase())
                    .ok_or(SpreadsheetError::Ref)?;
                range = Some((
                    target,
                    RangeRef {
                        start: cell.clone(),
                        end: cell.clone(),
                    },
                ));
            }
            Expr::SheetRange {
                sheet: name,
                range: r,
            } => {
                let target = *names
                    .get(&name.to_ascii_lowercase())
                    .ok_or(SpreadsheetError::Ref)?;
                range = Some((target, r.clone()));
            }
            Expr::BinOp { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            Expr::UnaryNeg(inner) | Expr::Percent(inner) => pending.push(inner),
            Expr::FuncCall { name, args } => {
                if !bounded_function(name) {
                    return Err(SpreadsheetError::Name);
                }
                pending.extend(args);
            }
            _ => {}
        }
        if let Some((index, r)) = range {
            let max_col = r.start.col.max(r.end.col);
            let max_row = r.start.row.max(r.end.row);
            if max_col >= sheets[index].cols || max_row >= sheets[index].rows {
                return Err(SpreadsheetError::Ref);
            }
            let width = max_col - r.start.col.min(r.end.col) + 1;
            let height = max_row - r.start.row.min(r.end.row) + 1;
            *work = work.saturating_add(width.saturating_mul(height));
            if *work > MAX_REFERENCE_WORK {
                return Err(SpreadsheetError::Num);
            }
            for row in r.start.row.min(r.end.row)..=max_row {
                for col in r.start.col.min(r.end.col)..=max_col {
                    refs.insert((index, col, row));
                }
            }
        }
    }
    Ok(refs)
}

/// Evaluate an entire sparse workbook in dependency order and return one sheet.
/// Cross-sheet references carry typed computed values, not formatted strings.
/// Cycles and their downstream dependents display #CIRCULAR! deterministically.
/// No foreign-document requests are issued by this entry point.
pub fn engine_for_sheet(active: &SheetData, sheets: &[SheetData]) -> SpreadsheetEngine {
    let invalid = || {
        let mut engine = SpreadsheetEngine::new();
        for (&addr, raw) in active.cells.iter().take(MAX_STORED_CELLS) {
            engine.set_embedded_error(addr, raw, SpreadsheetError::Ref);
        }
        engine
    };
    if validate_workbook(sheets).is_err() {
        return invalid();
    }
    let Some(active_index) = sheets.iter().position(|s| s.id == active.id) else {
        return invalid();
    };
    let values: WorkbookValues = Arc::new(Mutex::new(
        sheets
            .iter()
            .map(|s| (s.name.to_ascii_lowercase(), HashMap::new()))
            .collect(),
    ));
    let names: HashMap<_, _> = sheets
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.to_ascii_lowercase(), i))
        .collect();
    let mut engines: Vec<_> = sheets
        .iter()
        .map(|s| {
            let mut engine = SpreadsheetEngine::new();
            engine.set_active_sheet_name(s.name.clone());
            engine.set_embedded_values(values.clone());
            for (&addr, fmt) in &s.formats {
                engine.set_style(
                    addr,
                    CellStyle {
                        number_format: Some(fmt.clone()),
                        ..CellStyle::default()
                    },
                );
            }
            engine
        })
        .collect();
    // BTreeMap makes invalid/oversized workbook behavior independent of hash iteration.
    let mut deps: BTreeMap<Address, HashSet<Address>> = BTreeMap::new();
    let mut errors = HashMap::new();
    let mut work = 0usize;
    for (si, s) in sheets.iter().enumerate() {
        for (&(col, row), raw) in &s.cells {
            let address = (si, col, row);
            let refs = raw
                .strip_prefix('=')
                .map(|formula| {
                    let expr = parse_formula(formula).map_err(|_| SpreadsheetError::Name)?;
                    references(&expr, si, sheets, &names, &mut work)
                })
                .unwrap_or_else(|| Ok(HashSet::new()));
            match refs {
                Ok(mut refs) => {
                    refs.retain(|&(i, c, r)| sheets[i].cells.contains_key(&(c, r)));
                    deps.insert(address, refs);
                }
                Err(error) => {
                    deps.insert(address, HashSet::new());
                    errors.insert(address, error);
                }
            }
        }
    }
    let mut dependents: HashMap<Address, Vec<Address>> = HashMap::new();
    let mut counts: HashMap<Address, usize> = HashMap::new();
    let mut ready = VecDeque::new();
    for (&address, refs) in &deps {
        counts.insert(address, refs.len());
        if refs.is_empty() {
            ready.push_back(address);
        }
        for &r in refs {
            dependents.entry(r).or_default().push(address);
        }
    }
    while let Some((si, col, row)) = ready.pop_front() {
        let address = (si, col, row);
        let raw = &sheets[si].cells[&(col, row)];
        let engine = &mut engines[si];
        if let Some(error) = errors.remove(&address) {
            engine.set_embedded_error((col, row), raw, error);
        } else {
            engine.set_cell((col, row), raw);
        }
        let value = engine.get_value((col, row)).clone();
        values
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get_mut(&sheets[si].name.to_ascii_lowercase())
            .expect("known sheet")
            .insert((col, row), value);
        counts.remove(&address);
        if let Some(children) = dependents.get(&address) {
            for child in children {
                if let Some(count) = counts.get_mut(child) {
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(*child);
                    }
                }
            }
        }
    }
    for &(si, col, row) in counts.keys() {
        engines[si].set_embedded_error(
            (col, row),
            &sheets[si].cells[&(col, row)],
            SpreadsheetError::Circular,
        );
    }
    engines.swap_remove(active_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attrs() -> HashMap<String, String> {
        [
            ("blockId", "sheet-1"),
            ("sheetName", "Sheet1"),
            ("gridRows", "12"),
            ("gridCols", "6"),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect()
    }
    fn sheet(name: &str, entries: &[((usize, usize), &str)]) -> SheetData {
        let mut a = attrs();
        a.insert("blockId".into(), name.into());
        a.insert("sheetName".into(), name.into());
        for &((col, row), v) in entries {
            a.insert(cell_key(row, col), v.into());
        }
        decode_sheet(&a).unwrap()
    }
    #[test]
    fn sparse_decode_rejects_bad_keys_dimensions_and_oversize_but_supports_clears() {
        let mut a = attrs();
        a.insert(cell_key(1, 2), "12".into());
        a.insert(format_key(1, 2), "currency".into());
        let s = decode_sheet(&a).unwrap();
        assert_eq!(s.cells[&(2, 1)], "12");
        a.insert("cell:01:2".into(), "x".into());
        assert!(decode_sheet(&a).is_err());
        a.remove("cell:01:2");
        a.insert(cell_key(12, 0), "".into());
        assert!(decode_sheet(&a).is_err());
        a.remove(&cell_key(12, 0));
        a.insert(cell_key(1, 2), "".into());
        a.insert(format_key(1, 2), "".into());
        assert!(decode_sheet(&a).unwrap().cells.is_empty());
        a.insert(cell_key(0, 0), "x".repeat(MAX_CELL_CHARS + 1));
        assert!(decode_sheet(&a).is_err());
    }
    #[test]
    fn computed_cross_sheet_chains_preserve_types_formats_and_source() {
        let a = sheet(
            "Sheet1",
            &[((0, 0), "=Sheet2!A1+1"), ((1, 0), "=IF(Sheet2!B1,7,9)")],
        );
        let mut b = sheet("Sheet2", &[((0, 0), "=Sheet3!A1*2"), ((1, 0), "TRUE")]);
        b.formats.insert((0, 0), "currency".into());
        let c = sheet("Sheet3", &[((0, 0), "4")]);
        let sheets = [a.clone(), b.clone(), c];
        let e = engine_for_sheet(&a, &sheets);
        assert_eq!(e.get_display((0, 0)), "9");
        assert_eq!(e.get_display((1, 0)), "7");
        let e = engine_for_sheet(&b, &sheets);
        assert_eq!(e.get_display((0, 0)), "$8.00");
        assert_eq!(e.get_raw((0, 0)), "=Sheet3!A1*2");
    }
    #[test]
    fn cross_sheet_cycle_and_errors_remain_errors() {
        let a = sheet("Sheet1", &[((0, 0), "=Sheet2!A1")]);
        let b = sheet("Sheet2", &[((0, 0), "=Sheet1!A1")]);
        assert_eq!(
            engine_for_sheet(&a, &[a.clone(), b]).get_display((0, 0)),
            "#CIRCULAR!"
        );
        let b = sheet("Sheet2", &[((0, 0), "=1/0")]);
        assert_eq!(
            engine_for_sheet(&a, &[a.clone(), b]).get_display((0, 0)),
            "#DIV/0!"
        );
    }
    #[test]
    fn untrusted_allocation_and_reference_work_are_bounded_without_losing_raw() {
        let a = sheet(
            "Sheet1",
            &[
                ((0, 0), "=REPT(\"x\",1e99)"),
                ((1, 0), "=SUM(A1:XFD1048576)"),
                ((3, 0), "=ZZZZZZZZZZZZZZZZZZZZZZZZZZ1"),
                ((4, 0), "=A999999999999999999999999999999"),
                ((2, 0), "=SUM(A2:A3)"),
                ((0, 1), "2"),
                ((0, 2), "3"),
            ],
        );
        let e = engine_for_sheet(&a, std::slice::from_ref(&a));
        assert_eq!(e.get_display((0, 0)), "#NAME?");
        assert_eq!(e.get_raw((0, 0)), "=REPT(\"x\",1e99)");
        assert_eq!(e.get_display((1, 0)), "#REF!");
        assert_eq!(e.get_display((3, 0)), "#REF!");
        assert_eq!(e.get_display((4, 0)), "#REF!");
        assert_eq!(e.get_display((2, 0)), "5");
    }
    #[test]
    fn duplicate_names_and_ids_are_never_silently_discarded() {
        let a = sheet("Sheet1", &[]);
        let mut b = sheet("sheet1", &[]);
        assert!(validate_workbook(&[a.clone(), b.clone()]).is_err());
        b.name = "Other".into();
        b.id = a.id.clone();
        assert!(validate_workbook(&[a, b]).is_err());
    }
}
