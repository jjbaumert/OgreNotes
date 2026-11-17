// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Formula translation for copy/paste.
//!
//! Excel semantics: when a formula is **copied** from cell `S` and pasted into
//! cell `D`, each relative cell reference in the formula shifts by the delta
//! `D - S`. References anchored with `$` on a given axis are fixed on that
//! axis. If a shift lands out of bounds, that reference becomes `#REF!` and
//! the containing range — if any endpoint fails — also becomes `#REF!`.
//!
//! Cut-paste uses different semantics (references stay verbatim, other
//! cells that point at the moved range get rewritten to follow it). That
//! is handled in a separate step; this module only implements the
//! copy-paste "shift relative refs" transform and a round-trip printer
//! for the parser AST.

use super::eval::SpreadsheetEngine;
use super::parser::{parse_formula, BinOp, CellRef, Expr, RangeRef, SpreadsheetError};

// ─── Public API ────────────────────────────────────────────────

/// Translate a cell's raw content string by a (d_col, d_row) delta.
///
/// - Strings that don't start with `=` are returned unchanged (literals).
/// - If the formula can't be parsed, it's returned unchanged (preserving
///   whatever the user typed — we don't destroy unknown content).
/// - A zero delta short-circuits to the original string.
/// - `bounds` is `(num_cols, num_rows)`; refs that would land outside the
///   grid become `#REF!`.
pub fn translate_formula(formula: &str, delta: (i32, i32), bounds: (usize, usize)) -> String {
    if delta == (0, 0) {
        return formula.to_string();
    }
    let Some(body) = formula.strip_prefix('=') else {
        return formula.to_string();
    };
    let Ok(expr) = parse_formula(body) else {
        return formula.to_string();
    };
    let shifted = shift_expr(&expr, delta, bounds);
    let mut out = String::from("=");
    write_expr(&shifted, &mut out, 0);
    out
}

/// Apply a copy-paste to `engine`, translating relative refs from
/// `source_top_left` to `dest_top_left`. Each source cell's raw string
/// is run through [`translate_formula`] before being written. Used by
/// the view layer's Ctrl+V handler and by integration tests.
pub fn apply_copy_paste(
    engine: &mut SpreadsheetEngine,
    source_cells: &[Vec<String>],
    source_top_left: (usize, usize),
    dest_top_left: (usize, usize),
    bounds: (usize, usize),
) {
    let (sc, sr) = source_top_left;
    let (dc, dr) = dest_top_left;
    let delta = (dc as i32 - sc as i32, dr as i32 - sr as i32);
    for (ri, row) in source_cells.iter().enumerate() {
        for (ci, raw) in row.iter().enumerate() {
            let translated = translate_formula(raw, delta, bounds);
            engine.set_cell((dc + ci, dr + ri), &translated);
        }
    }
}

/// Apply a cut-paste to `engine`. Excel semantics:
///
/// 1. The source cells are copied to the destination **verbatim** — their
///    own internal refs do not shift (it's a move, not a transform).
/// 2. Every other cell whose formula references a cell inside the source
///    rect is rewritten so the reference follows the physical move. `$`
///    anchors are ignored for the shift amount (the target *moved*), but
///    preserved in the output so `$A$1` → `$B$1` when A1 moves to B1.
/// 3. Source cells that don't overlap the destination rect are cleared.
///
/// Ranges: shifted only when **both** endpoints fall inside the source
/// rect. Partial overlap leaves the range alone (Excel-ambiguous; safest
/// not to mangle).
pub fn apply_cut_paste(
    engine: &mut SpreadsheetEngine,
    source_cells: &[Vec<String>],
    source_top_left: (usize, usize),
    dest_top_left: (usize, usize),
    bounds: (usize, usize),
) {
    let (sc, sr) = source_top_left;
    let (dc, dr) = dest_top_left;
    let h = source_cells.len();
    let w = source_cells.first().map(|r| r.len()).unwrap_or(0);
    if w == 0 || h == 0 {
        return;
    }
    let delta = (dc as i32 - sc as i32, dr as i32 - sr as i32);
    let source_rect = (
        (sc, sr),
        (sc + w - 1, sr + h - 1),
    );
    let dest_rect = (
        (dc, dr),
        (dc + w - 1, dr + h - 1),
    );

    // 1. Reverse-rewrite other cells BEFORE we touch the source or dest.
    //    We skip cells inside the source or dest rects (they'll be
    //    cleared or overwritten in the next steps, so no point rewriting).
    let mut updates: Vec<((usize, usize), String)> = Vec::new();
    for (addr, raw) in engine.iter_raw() {
        if in_rect(addr, source_rect) || in_rect(addr, dest_rect) {
            continue;
        }
        if !raw.starts_with('=') {
            continue;
        }
        let new_formula = rewrite_formula_for_cut(raw, source_rect, delta, bounds);
        if new_formula != raw {
            updates.push((addr, new_formula));
        }
    }
    for (addr, new_formula) in updates {
        engine.set_cell(addr, &new_formula);
    }

    // 2. Clear source cells outside the destination rect. (Cells that
    //    overlap the destination would be cleared and then overwritten
    //    — skip clearing them so we don't churn dependencies twice.)
    for ri in 0..h {
        for ci in 0..w {
            let addr = (sc + ci, sr + ri);
            if !in_rect(addr, dest_rect) {
                engine.set_cell(addr, "");
            }
        }
    }

    // 3. Write destination cells verbatim.
    for (ri, row) in source_cells.iter().enumerate() {
        for (ci, raw) in row.iter().enumerate() {
            engine.set_cell((dc + ci, dr + ri), raw);
        }
    }
}

// ─── Cut reverse-rewrite helpers ────────────────────────────────

fn in_rect(cell: (usize, usize), rect: ((usize, usize), (usize, usize))) -> bool {
    let (c, r) = cell;
    let ((c1, r1), (c2, r2)) = rect;
    c >= c1 && c <= c2 && r >= r1 && r <= r2
}

fn cellref_in_rect(
    cell: &CellRef,
    rect: ((usize, usize), (usize, usize)),
) -> bool {
    in_rect((cell.col, cell.row), rect)
}

/// Shift a CellRef's physical position by `delta`, preserving its `$`
/// markers. Used for cut-paste where the physical cell moved — anchors
/// follow the move instead of pinning against it.
fn shift_physical(
    r: &CellRef,
    delta: (i32, i32),
    bounds: (usize, usize),
) -> Option<CellRef> {
    let new_col = r.col as i32 + delta.0;
    let new_row = r.row as i32 + delta.1;
    if new_col < 0 || new_row < 0 {
        return None;
    }
    let new_col = new_col as usize;
    let new_row = new_row as usize;
    if new_col >= bounds.0 || new_row >= bounds.1 {
        return None;
    }
    Some(CellRef {
        col: new_col,
        row: new_row,
        abs_col: r.abs_col,
        abs_row: r.abs_row,
    })
}

/// Rewrite a single formula so any reference pointing into `source_rect`
/// follows the move by `delta`. Parse failures return the input unchanged.
pub fn rewrite_formula_for_cut(
    formula: &str,
    source_rect: ((usize, usize), (usize, usize)),
    delta: (i32, i32),
    bounds: (usize, usize),
) -> String {
    let Some(body) = formula.strip_prefix('=') else {
        return formula.to_string();
    };
    let Ok(expr) = parse_formula(body) else {
        return formula.to_string();
    };
    let shifted = rewrite_expr_for_cut(&expr, source_rect, delta, bounds);
    let mut out = String::from("=");
    write_expr(&shifted, &mut out, 0);
    out
}

fn rewrite_expr_for_cut(
    e: &Expr,
    source_rect: ((usize, usize), (usize, usize)),
    delta: (i32, i32),
    bounds: (usize, usize),
) -> Expr {
    match e {
        Expr::CellRef(r) => {
            if !cellref_in_rect(r, source_rect) {
                return Expr::CellRef(r.clone());
            }
            match shift_physical(r, delta, bounds) {
                Some(shifted) => Expr::CellRef(shifted),
                None => Expr::Error(SpreadsheetError::Ref),
            }
        }
        Expr::Range(r) => {
            let s_in = cellref_in_rect(&r.start, source_rect);
            let e_in = cellref_in_rect(&r.end, source_rect);
            if s_in && e_in {
                let new_start = shift_physical(&r.start, delta, bounds);
                let new_end = shift_physical(&r.end, delta, bounds);
                match (new_start, new_end) {
                    (Some(s), Some(en)) => Expr::Range(RangeRef { start: s, end: en }),
                    _ => Expr::Error(SpreadsheetError::Ref),
                }
            } else {
                // Partial or no overlap — leave the range alone to match
                // Excel's behavior for ambiguous splits.
                Expr::Range(r.clone())
            }
        }
        Expr::BinOp { op, left, right } => Expr::BinOp {
            op: op.clone(),
            left: Box::new(rewrite_expr_for_cut(left, source_rect, delta, bounds)),
            right: Box::new(rewrite_expr_for_cut(right, source_rect, delta, bounds)),
        },
        Expr::UnaryNeg(inner) => Expr::UnaryNeg(Box::new(rewrite_expr_for_cut(
            inner, source_rect, delta, bounds,
        ))),
        Expr::Percent(inner) => Expr::Percent(Box::new(rewrite_expr_for_cut(
            inner, source_rect, delta, bounds,
        ))),
        Expr::FuncCall { name, args } => Expr::FuncCall {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| rewrite_expr_for_cut(a, source_rect, delta, bounds))
                .collect(),
        },
        // Named ranges aren't translated by cut/paste/fill — the
        // alias points at fixed addresses regardless of the formula's
        // own location.
        // Sheet-qualified refs target a different sheet's cells, so
        // they never shift on local row/col inserts or paste
        // rewrites — clone through unchanged.
        Expr::Name(_)
        | Expr::SheetCellRef { .. } | Expr::SheetRange { .. }
        | Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) => e.clone(),
    }
}

// ─── Conditional axis shift (for row/col insert + delete) ────

/// Which axis a structural shift applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis { Row, Col }

/// Translate a formula's cell references after a row or column has
/// been inserted or deleted. Unlike `translate_formula` (which
/// shifts every ref unconditionally for fill/cut), this only shifts
/// refs whose coordinate on the named axis crosses `threshold`:
///
/// * **Insert** (`delta = +1`): refs with coord `>= threshold` shift
///   by +1; refs below `threshold` are unchanged.
/// * **Delete** (`delta = -1`): refs with coord `> threshold` shift
///   by -1; refs equal to `threshold` (the deleted row/column)
///   become `#REF!`.
///
/// Absolute axes (`$row` or `$col`) shift the same as relative ones —
/// inserts/deletes are structural changes that affect every reference
/// uniformly, regardless of `$` prefixes.
///
/// Returns the rewritten formula text (without leading `=`) or the
/// original input if parsing fails (so a non-formula or malformed
/// cell isn't corrupted).
pub fn translate_for_axis_shift(
    formula: &str,
    axis: Axis,
    threshold: usize,
    delta: i32,
) -> String {
    let Some(body) = formula.strip_prefix('=') else {
        return formula.to_string();
    };
    let Ok(parsed) = parse_formula(body) else {
        return formula.to_string();
    };
    let shifted = shift_expr_for_axis(&parsed, axis, threshold, delta);
    let mut out = String::from("=");
    write_expr(&shifted, &mut out, 0);
    out
}

fn shift_expr_for_axis(e: &Expr, axis: Axis, threshold: usize, delta: i32) -> Expr {
    match e {
        Expr::CellRef(r) => match shift_cell_ref_for_axis(r, axis, threshold, delta) {
            Some(shifted) => Expr::CellRef(shifted),
            None => Expr::Error(SpreadsheetError::Ref),
        },
        Expr::Range(r) => {
            let start = shift_cell_ref_for_axis(&r.start, axis, threshold, delta);
            let end = shift_cell_ref_for_axis(&r.end, axis, threshold, delta);
            match (start, end) {
                (Some(s), Some(en)) => Expr::Range(RangeRef { start: s, end: en }),
                _ => Expr::Error(SpreadsheetError::Ref),
            }
        }
        Expr::BinOp { op, left, right } => Expr::BinOp {
            op: op.clone(),
            left: Box::new(shift_expr_for_axis(left, axis, threshold, delta)),
            right: Box::new(shift_expr_for_axis(right, axis, threshold, delta)),
        },
        Expr::UnaryNeg(inner) =>
            Expr::UnaryNeg(Box::new(shift_expr_for_axis(inner, axis, threshold, delta))),
        Expr::Percent(inner) =>
            Expr::Percent(Box::new(shift_expr_for_axis(inner, axis, threshold, delta))),
        Expr::FuncCall { name, args } => Expr::FuncCall {
            name: name.clone(),
            args: args.iter()
                .map(|a| shift_expr_for_axis(a, axis, threshold, delta))
                .collect(),
        },
        // Sheet-qualified refs target a different sheet's cells, so
        // they never shift on local row/col inserts or paste
        // rewrites — clone through unchanged.
        Expr::Name(_)
        | Expr::SheetCellRef { .. } | Expr::SheetRange { .. }
        | Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) => e.clone(),
    }
}

fn shift_cell_ref_for_axis(
    r: &CellRef,
    axis: Axis,
    threshold: usize,
    delta: i32,
) -> Option<CellRef> {
    let coord = match axis { Axis::Row => r.row, Axis::Col => r.col };
    let should_shift = match delta.signum() {
        1 => coord >= threshold,            // insert: shift refs at or below the new line
        -1 => coord > threshold,            // delete: shift refs strictly below the deleted line
        _ => false,
    };
    let crosses_deleted = delta < 0 && coord == threshold;
    if crosses_deleted { return None; }
    if !should_shift {
        return Some(r.clone());
    }
    let new_coord = (coord as i32 + delta).max(0) as usize;
    Some(match axis {
        Axis::Row => CellRef { row: new_coord, ..r.clone() },
        Axis::Col => CellRef { col: new_coord, ..r.clone() },
    })
}

// ─── Shifting ──────────────────────────────────────────────────

/// Shift a single cell reference by (d_col, d_row). Axes marked absolute
/// with `$` are fixed. Returns `None` if the shifted position falls outside
/// the grid.
fn shift_cell_ref(r: &CellRef, delta: (i32, i32), bounds: (usize, usize)) -> Option<CellRef> {
    let (d_col, d_row) = delta;
    let (max_cols, max_rows) = bounds;
    let new_col = if r.abs_col { r.col as i32 } else { r.col as i32 + d_col };
    let new_row = if r.abs_row { r.row as i32 } else { r.row as i32 + d_row };
    if new_col < 0 || new_row < 0 {
        return None;
    }
    let new_col = new_col as usize;
    let new_row = new_row as usize;
    if new_col >= max_cols || new_row >= max_rows {
        return None;
    }
    Some(CellRef {
        col: new_col,
        row: new_row,
        abs_col: r.abs_col,
        abs_row: r.abs_row,
    })
}

/// Shift both endpoints of a range. If either endpoint is out of bounds,
/// the whole range is invalid — callers convert that to `#REF!`.
fn shift_range(r: &RangeRef, delta: (i32, i32), bounds: (usize, usize)) -> Option<RangeRef> {
    let start = shift_cell_ref(&r.start, delta, bounds)?;
    let end = shift_cell_ref(&r.end, delta, bounds)?;
    Some(RangeRef { start, end })
}

/// Walk the AST, shifting every cell ref. Non-ref expressions pass through
/// untouched except for their children (which recurse).
fn shift_expr(e: &Expr, delta: (i32, i32), bounds: (usize, usize)) -> Expr {
    match e {
        Expr::CellRef(r) => match shift_cell_ref(r, delta, bounds) {
            Some(shifted) => Expr::CellRef(shifted),
            None => Expr::Error(SpreadsheetError::Ref),
        },
        Expr::Range(r) => match shift_range(r, delta, bounds) {
            Some(shifted) => Expr::Range(shifted),
            None => Expr::Error(SpreadsheetError::Ref),
        },
        Expr::BinOp { op, left, right } => Expr::BinOp {
            op: op.clone(),
            left: Box::new(shift_expr(left, delta, bounds)),
            right: Box::new(shift_expr(right, delta, bounds)),
        },
        Expr::UnaryNeg(inner) => Expr::UnaryNeg(Box::new(shift_expr(inner, delta, bounds))),
        Expr::Percent(inner) => Expr::Percent(Box::new(shift_expr(inner, delta, bounds))),
        Expr::FuncCall { name, args } => Expr::FuncCall {
            name: name.clone(),
            args: args.iter().map(|a| shift_expr(a, delta, bounds)).collect(),
        },
        // Named ranges aren't translated by cut/paste/fill — the
        // alias points at fixed addresses regardless of the formula's
        // own location.
        // Sheet-qualified refs target a different sheet's cells, so
        // they never shift on local row/col inserts or paste
        // rewrites — clone through unchanged.
        Expr::Name(_)
        | Expr::SheetCellRef { .. } | Expr::SheetRange { .. }
        | Expr::Number(_) | Expr::Text(_) | Expr::Bool(_) | Expr::Error(_) => e.clone(),
    }
}

// ─── AST printer ───────────────────────────────────────────────

/// Precedence level for an expression. Atoms (cells, numbers, function
/// calls) get the maximum so they never take parens. Binary operators
/// follow the grammar in `parser.rs`.
fn precedence(e: &Expr) -> u8 {
    match e {
        Expr::BinOp { op, .. } => match op {
            BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => 1,
            BinOp::Concat => 2,
            BinOp::Add | BinOp::Sub => 3,
            BinOp::Mul | BinOp::Div => 4,
            BinOp::Pow => 5,
        },
        Expr::Percent(_) => 6,
        Expr::UnaryNeg(_) => 7,
        _ => u8::MAX,
    }
}

fn is_right_associative(e: &Expr) -> bool {
    matches!(e, Expr::BinOp { op: BinOp::Pow, .. })
}

fn binop_str(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Pow => "^",
        BinOp::Concat => "&",
        BinOp::Eq => "=",
        BinOp::Neq => "<>",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Lte => "<=",
        BinOp::Gte => ">=",
    }
}

/// Print an expression, inserting parentheses only when required by
/// `parent_prec`. Top-level callers pass `0`.
fn write_expr(e: &Expr, out: &mut String, parent_prec: u8) {
    let p = precedence(e);
    let needs_paren = p < parent_prec;
    if needs_paren {
        out.push('(');
    }
    match e {
        Expr::Number(n) => {
            // `{}` on f64 produces the minimal round-trippable form in
            // modern Rust (ryu under the hood) — "42" for 42.0, "3.14" for
            // 3.14, etc. Re-parses to the same value.
            out.push_str(&format!("{n}"));
        }
        Expr::Text(s) => {
            out.push('"');
            for c in s.chars() {
                if c == '"' || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        }
        Expr::Bool(b) => out.push_str(if *b { "TRUE" } else { "FALSE" }),
        Expr::Error(err) => out.push_str(&err.to_string()),
        Expr::CellRef(r) => out.push_str(&r.label()),
        Expr::Range(r) => {
            out.push_str(&r.start.label());
            out.push(':');
            out.push_str(&r.end.label());
        }
        Expr::SheetCellRef { sheet, cell } => {
            out.push_str(sheet);
            out.push('!');
            out.push_str(&cell.label());
        }
        Expr::SheetRange { sheet, range } => {
            out.push_str(sheet);
            out.push('!');
            out.push_str(&range.start.label());
            out.push(':');
            out.push_str(&range.end.label());
        }
        Expr::Name(name) => out.push_str(name),
        Expr::BinOp { op, left, right } => {
            // For left-associative operators (most), the right child needs
            // parens on same-precedence sub-expressions (so `A-B-C` is
            // printed as `A-B-C`, but `A-(B-C)` keeps the parens).
            // Pow is right-associative: the left child needs the bump.
            let (lp, rp) = if is_right_associative(e) {
                (p + 1, p)
            } else {
                (p, p + 1)
            };
            write_expr(left, out, lp);
            out.push_str(binop_str(op));
            write_expr(right, out, rp);
        }
        Expr::UnaryNeg(inner) => {
            out.push('-');
            write_expr(inner, out, p);
        }
        Expr::Percent(inner) => {
            write_expr(inner, out, p);
            out.push('%');
        }
        Expr::FuncCall { name, args } => {
            out.push_str(name);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_expr(a, out, 0);
            }
            out.push(')');
        }
    }
    if needs_paren {
        out.push(')');
    }
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: (usize, usize) = (26, 100);

    // ─── Single-ref shifts ────────────────────────────────────

    #[test]
    fn relative_col_shift() {
        assert_eq!(translate_formula("=A1", (1, 0), BOUNDS), "=B1");
    }

    #[test]
    fn relative_row_shift() {
        assert_eq!(translate_formula("=A1", (0, 1), BOUNDS), "=A2");
    }

    #[test]
    fn absolute_col_ignores_col_shift() {
        assert_eq!(translate_formula("=$A1", (1, 0), BOUNDS), "=$A1");
    }

    #[test]
    fn absolute_col_still_shifts_row() {
        assert_eq!(translate_formula("=$A1", (0, 1), BOUNDS), "=$A2");
    }

    #[test]
    fn absolute_row_ignores_row_shift() {
        assert_eq!(translate_formula("=A$1", (0, 1), BOUNDS), "=A$1");
    }

    #[test]
    fn absolute_row_still_shifts_col() {
        assert_eq!(translate_formula("=A$1", (1, 0), BOUNDS), "=B$1");
    }

    #[test]
    fn fully_absolute_pinned() {
        assert_eq!(translate_formula("=$A$1", (3, 3), BOUNDS), "=$A$1");
    }

    // ─── Ranges ───────────────────────────────────────────────

    #[test]
    fn range_shifts_both_endpoints() {
        assert_eq!(translate_formula("=A1:B3", (1, 1), BOUNDS), "=B2:C4");
    }

    #[test]
    fn range_with_absolute_start() {
        assert_eq!(translate_formula("=$A$1:C3", (1, 1), BOUNDS), "=$A$1:D4");
    }

    // ─── The user's reported scenario ────────────────────────

    #[test]
    fn sum_range_column_shift_user_scenario() {
        // Copy A4==SUM(A1:A3), paste to B4 → delta (1, 0).
        assert_eq!(
            translate_formula("=SUM(A1:A3)", (1, 0), BOUNDS),
            "=SUM(B1:B3)"
        );
    }

    #[test]
    fn sum_range_row_shift() {
        assert_eq!(
            translate_formula("=SUM(A1:A3)", (0, 1), BOUNDS),
            "=SUM(A2:A4)"
        );
    }

    // ─── Multi-arg funcs + mixed absolutes ────────────────────

    #[test]
    fn multi_arg_with_mixed_absolute() {
        assert_eq!(
            translate_formula("=SUM(A1,A2,$A$3)", (0, 1), BOUNDS),
            "=SUM(A2,A3,$A$3)"
        );
    }

    #[test]
    fn arithmetic_preserves_precedence() {
        // A1 + A2*2, shift rows by 1
        assert_eq!(translate_formula("=A1+A2*2", (0, 1), BOUNDS), "=A2+A3*2");
    }

    #[test]
    fn nested_function_calls() {
        assert_eq!(
            translate_formula("=IF(A1>0,B1,C1)", (1, 0), BOUNDS),
            "=IF(B1>0,C1,D1)"
        );
    }

    // ─── Out-of-bounds → #REF! ───────────────────────────────

    #[test]
    fn negative_col_becomes_ref_error() {
        assert_eq!(translate_formula("=A1", (-1, 0), BOUNDS), "=#REF!");
    }

    #[test]
    fn range_endpoint_out_of_bounds_whole_range_ref() {
        // A1:B3 shifted by (0,-2) → row -2..1, start out of bounds.
        assert_eq!(translate_formula("=A1:B3", (0, -2), BOUNDS), "=#REF!");
    }

    #[test]
    fn off_right_edge_becomes_ref_error() {
        // Z1 (col 25) + 1 = col 26, off the 26-wide grid.
        assert_eq!(translate_formula("=Z1", (1, 0), BOUNDS), "=#REF!");
    }

    // ─── Non-formula / unparseable passthrough ────────────────

    #[test]
    fn non_formula_unchanged() {
        assert_eq!(translate_formula("hello", (1, 1), BOUNDS), "hello");
        assert_eq!(translate_formula("42", (1, 1), BOUNDS), "42");
        assert_eq!(translate_formula("", (1, 1), BOUNDS), "");
    }

    #[test]
    fn unparseable_formula_unchanged() {
        // Unknown token / broken formula — keep the user's text intact
        // rather than corrupting it.
        assert_eq!(translate_formula("=A1+", (1, 0), BOUNDS), "=A1+");
    }

    #[test]
    fn zero_delta_is_identity() {
        assert_eq!(translate_formula("=SUM(A1:A3)", (0, 0), BOUNDS), "=SUM(A1:A3)");
    }

    #[test]
    fn literal_number_formula() {
        // `=123` — no refs, but is a formula; should round-trip.
        assert_eq!(translate_formula("=123", (1, 1), BOUNDS), "=123");
    }

    // ─── Round-trip invariant ─────────────────────────────────

    #[test]
    fn translate_by_delta_then_neg_delta_is_identity_when_in_bounds() {
        let cases = &[
            "=A5",
            "=B3+C4",
            "=SUM(A1:C10)",
            "=IF(B2>5,C3*2,D4)",
            "=A1+$B$2*C$3-$D4",
        ];
        for formula in cases {
            let forward = translate_formula(formula, (2, 2), BOUNDS);
            assert!(!forward.contains("#REF!"), "setup error, {formula} went out of bounds");
            let back = translate_formula(&forward, (-2, -2), BOUNDS);
            assert_eq!(
                &back, formula,
                "round-trip failed for {formula}: forward={forward}, back={back}"
            );
        }
    }

    // ─── Operators ────────────────────────────────────────────

    #[test]
    fn concat_operator() {
        assert_eq!(
            translate_formula("=A1&\" \"&B1", (0, 1), BOUNDS),
            "=A2&\" \"&B2"
        );
    }

    #[test]
    fn comparison_operators() {
        assert_eq!(translate_formula("=A1<>B1", (0, 1), BOUNDS), "=A2<>B2");
        assert_eq!(translate_formula("=A1<=B1", (0, 1), BOUNDS), "=A2<=B2");
    }

    #[test]
    fn percent_operator() {
        assert_eq!(translate_formula("=A1%", (0, 1), BOUNDS), "=A2%");
    }

    #[test]
    fn unary_neg() {
        assert_eq!(translate_formula("=-A1", (1, 0), BOUNDS), "=-B1");
    }

    #[test]
    fn power_right_associative() {
        // A1^B1^C1 = A1^(B1^C1) — parser builds the tree right-associatively.
        // The reprint must preserve that meaning.
        let src = "=A1^B1^C1";
        let shifted = translate_formula(src, (1, 0), BOUNDS);
        assert_eq!(shifted, "=B1^C1^D1");
    }

    // ─── Bounds ───────────────────────────────────────────────

    #[test]
    fn tight_bounds_allow_just_inside() {
        // 2-wide, 2-tall grid: only A1, A2, B1, B2. A2 + (0,0) → A2, valid.
        assert_eq!(translate_formula("=A1", (1, 1), (2, 2)), "=B2");
    }

    #[test]
    fn tight_bounds_reject_just_outside() {
        // Same grid, shift A1 by (2,0) → col 2, out of a 2-wide grid.
        assert_eq!(translate_formula("=A1", (2, 0), (2, 2)), "=#REF!");
    }

    // ─── Integration: apply_copy_paste against a real engine ──

    use super::super::eval::{CellValue, SpreadsheetEngine};

    /// Build the scenario from the bug report: A1=1, A2=2, A3=3,
    /// A4==SUM(A1:A3), B1=3, B2=4, B3=5.
    fn user_scenario_engine() -> SpreadsheetEngine {
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");
        eng.set_cell((0, 3), "=SUM(A1:A3)");
        eng.set_cell((1, 0), "3");
        eng.set_cell((1, 1), "4");
        eng.set_cell((1, 2), "5");
        eng
    }

    fn as_number(v: &CellValue) -> f64 {
        match v {
            CellValue::Number(n) => *n,
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn user_scenario_copy_a4_to_b4_translates_and_evaluates() {
        let mut eng = user_scenario_engine();
        // Copy the single cell A4, paste to B4.
        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_copy_paste(&mut eng, &src_cells, (0, 3), (1, 3), BOUNDS);

        assert_eq!(eng.get_raw((1, 3)), "=SUM(B1:B3)");
        // Eval: SUM(B1:B3) = 3 + 4 + 5 = 12
        assert_eq!(as_number(eng.get_value((1, 3))), 12.0);
        // A4 is untouched by copy
        assert_eq!(eng.get_raw((0, 3)), "=SUM(A1:A3)");
        assert_eq!(as_number(eng.get_value((0, 3))), 6.0);
    }

    #[test]
    fn copy_preserves_absolute_refs() {
        let mut eng = user_scenario_engine();
        eng.set_cell((0, 3), "=SUM($A$1:$A$3)");

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_copy_paste(&mut eng, &src_cells, (0, 3), (1, 3), BOUNDS);

        // Absolute ref — column shift should not move it.
        assert_eq!(eng.get_raw((1, 3)), "=SUM($A$1:$A$3)");
        assert_eq!(as_number(eng.get_value((1, 3))), 6.0);
    }

    #[test]
    fn copy_mixed_absolute_shifts_unanchored_axis_only() {
        let mut eng = user_scenario_engine();
        // $A1 — absolute col, relative row
        eng.set_cell((0, 3), "=SUM($A1:$A3)");

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_copy_paste(&mut eng, &src_cells, (0, 3), (1, 3), BOUNDS);

        // Column is anchored; row delta is 0 (same row B4).
        assert_eq!(eng.get_raw((1, 3)), "=SUM($A1:$A3)");
        assert_eq!(as_number(eng.get_value((1, 3))), 6.0);
    }

    #[test]
    fn copy_row_shift_produces_expected_formula() {
        let mut eng = user_scenario_engine();
        // Copy A1 (=1, literal) and A4 (=SUM(A1:A3)) down one row.
        // Simulates copy A4 → paste A5.
        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_copy_paste(&mut eng, &src_cells, (0, 3), (0, 4), BOUNDS);

        // Row +1 shift → SUM(A2:A4).
        assert_eq!(eng.get_raw((0, 4)), "=SUM(A2:A4)");
        // Note: A5 now references A4 which itself references A1:A3, so
        // A5 = 2 + 3 + SUM(A1:A3) = 2 + 3 + 6 = 11. Not a circular ref.
        assert_eq!(as_number(eng.get_value((0, 4))), 11.0);
    }

    #[test]
    fn multi_cell_copy_translates_each() {
        let mut eng = SpreadsheetEngine::new();
        // A1=1, A2=2, A3==A1+A2
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "=A1+A2");

        // Copy A1:A3 as a block, paste at B1.
        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_copy_paste(&mut eng, &src_cells, (0, 0), (1, 0), BOUNDS);

        assert_eq!(eng.get_raw((1, 0)), "1");
        assert_eq!(eng.get_raw((1, 1)), "2");
        assert_eq!(eng.get_raw((1, 2)), "=B1+B2");
        assert_eq!(as_number(eng.get_value((1, 2))), 3.0);
    }

    #[test]
    fn zero_delta_paste_is_identity() {
        let mut eng = user_scenario_engine();
        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        // Paste A4 back onto A4 — no translation, same formula.
        apply_copy_paste(&mut eng, &src_cells, (0, 3), (0, 3), BOUNDS);
        assert_eq!(eng.get_raw((0, 3)), "=SUM(A1:A3)");
        assert_eq!(as_number(eng.get_value((0, 3))), 6.0);
    }

    // ─── Cut-paste integration ────────────────────────────────

    #[test]
    fn cut_paste_moves_formula_verbatim_and_rewrites_refs() {
        // A4==SUM(A1:A3), C1==A4*2. Cut A4 → paste D4.
        // Expected: D4 = =SUM(A1:A3) (verbatim, Excel move), A4 empty,
        // C1 = =D4*2 (ref follows the move).
        let mut eng = user_scenario_engine();
        eng.set_cell((2, 0), "=A4*2"); // C1

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_cut_paste(&mut eng, &src_cells, (0, 3), (3, 3), BOUNDS);

        assert_eq!(eng.get_raw((3, 3)), "=SUM(A1:A3)");
        assert_eq!(as_number(eng.get_value((3, 3))), 6.0);
        assert_eq!(eng.get_raw((0, 3)), ""); // A4 cleared
        assert_eq!(eng.get_raw((2, 0)), "=D4*2"); // reference rewritten
        assert_eq!(as_number(eng.get_value((2, 0))), 12.0);
    }

    #[test]
    fn cut_paste_absolute_ref_still_follows_move() {
        // Excel: $A$4 is absolute for COPY, but a CUT physically relocates
        // the target, so refs — even absolute ones — follow the move.
        let mut eng = user_scenario_engine();
        eng.set_cell((2, 0), "=$A$4"); // C1 = $A$4

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_cut_paste(&mut eng, &src_cells, (0, 3), (1, 3), BOUNDS);

        // C1 ref follows A4 → B4, $ preserved.
        assert_eq!(eng.get_raw((2, 0)), "=$B$4");
    }

    #[test]
    fn cut_paste_external_refs_untouched() {
        // Cells outside the source rect shouldn't change.
        let mut eng = user_scenario_engine();
        eng.set_cell((2, 0), "=B1+B2"); // C1 points at B-column, not A4

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_cut_paste(&mut eng, &src_cells, (0, 3), (1, 3), BOUNDS);

        assert_eq!(eng.get_raw((2, 0)), "=B1+B2");
    }

    #[test]
    fn cut_paste_multi_cell_range_refs_follow() {
        // SUM(A1:A3) — entire range is inside a cut of A1:A3. After cut,
        // the range ref should shift to the new location.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");
        eng.set_cell((2, 0), "=SUM(A1:A3)"); // C1

        // Cut A1:A3 → paste B1:B3.
        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_cut_paste(&mut eng, &src_cells, (0, 0), (1, 0), BOUNDS);

        assert_eq!(eng.get_raw((1, 0)), "1"); // B1
        assert_eq!(eng.get_raw((1, 1)), "2");
        assert_eq!(eng.get_raw((1, 2)), "3");
        assert_eq!(eng.get_raw((0, 0)), ""); // A1 cleared
        assert_eq!(eng.get_raw((0, 1)), "");
        assert_eq!(eng.get_raw((0, 2)), "");
        assert_eq!(eng.get_raw((2, 0)), "=SUM(B1:B3)"); // range ref follows
        assert_eq!(as_number(eng.get_value((2, 0))), 6.0);
    }

    #[test]
    fn cut_paste_partial_range_overlap_leaves_range_alone() {
        // =SUM(A1:A5) — only A1:A3 is cut (partial overlap). Excel's
        // treatment of this is ambiguous; we leave the range as-is.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");
        eng.set_cell((0, 3), "4");
        eng.set_cell((0, 4), "5");
        eng.set_cell((2, 0), "=SUM(A1:A5)");

        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_cut_paste(&mut eng, &src_cells, (0, 0), (5, 0), BOUNDS);

        assert_eq!(eng.get_raw((2, 0)), "=SUM(A1:A5)");
    }

    #[test]
    fn cut_paste_to_same_location_is_identity() {
        // Cut A4, paste to A4. No reverse-rewrite fires, source ==
        // destination, and the clear-then-write preserves the cell.
        let mut eng = user_scenario_engine();
        eng.set_cell((2, 0), "=A4");

        let src_cells = vec![vec![eng.get_raw((0, 3)).to_string()]];
        apply_cut_paste(&mut eng, &src_cells, (0, 3), (0, 3), BOUNDS);

        assert_eq!(eng.get_raw((0, 3)), "=SUM(A1:A3)");
        assert_eq!(eng.get_raw((2, 0)), "=A4");
    }

    // ─── Overlap (source rect and dest rect intersect) ────────

    #[test]
    fn copy_paste_source_dest_overlap_shifts_refs() {
        // Copy A1:A3 (literals 1,2,3) to A2. Destination A2:A4 overlaps
        // source. Paste overwrites; after the paste the three literals
        // live at A2, A3, A4 (A1 retains its original 1).
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");

        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_copy_paste(&mut eng, &src_cells, (0, 0), (0, 1), BOUNDS);

        assert_eq!(eng.get_raw((0, 0)), "1"); // unchanged
        assert_eq!(eng.get_raw((0, 1)), "1"); // overwritten
        assert_eq!(eng.get_raw((0, 2)), "2");
        assert_eq!(eng.get_raw((0, 3)), "3");
    }

    #[test]
    fn copy_paste_overlap_with_formula_translates_each_cell() {
        // Source cells contain formulas referencing themselves.
        // A1=1, A2==A1+1, A3==A2+1. Copy A2:A3, paste to A3 (overlap).
        // Each source cell snapshots its ORIGINAL formula; translation is
        // by delta (0, 1). So:
        //   Source A2==A1+1 → dest A3 gets translated to =A2+1
        //   Source A3==A2+1 → dest A4 gets translated to =A3+1
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "=A1+1");
        eng.set_cell((0, 2), "=A2+1");

        let src_cells = vec![
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_copy_paste(&mut eng, &src_cells, (0, 1), (0, 2), BOUNDS);

        assert_eq!(eng.get_raw((0, 0)), "1");
        assert_eq!(eng.get_raw((0, 1)), "=A1+1"); // unchanged
        assert_eq!(eng.get_raw((0, 2)), "=A2+1"); // overwritten with translation
        assert_eq!(eng.get_raw((0, 3)), "=A3+1");
        // Evaluate: A1=1, A2=2, A3=3, A4=4
        assert_eq!(as_number(eng.get_value((0, 3))), 4.0);
    }

    #[test]
    fn cut_paste_overlap_does_not_clear_overlapping_source() {
        // Cut A1:A3 (=1,2,3), paste to A2. Source {A1,A2,A3}, dest
        // {A2,A3,A4}. Only A1 is outside dest and gets cleared; A2/A3 are
        // overwritten by the paste, not cleared-then-written.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");

        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_cut_paste(&mut eng, &src_cells, (0, 0), (0, 1), BOUNDS);

        assert_eq!(eng.get_raw((0, 0)), ""); // cleared (outside dest)
        assert_eq!(eng.get_raw((0, 1)), "1"); // dest row 0 from source
        assert_eq!(eng.get_raw((0, 2)), "2");
        assert_eq!(eng.get_raw((0, 3)), "3");
    }

    #[test]
    fn cut_paste_overlap_rewrites_external_refs_once() {
        // External reference to A1 should follow the move by (0,1), not
        // double-hop even though source and dest overlap.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 1), "2");
        eng.set_cell((0, 2), "3");
        eng.set_cell((2, 0), "=A1"); // C1 points at A1

        let src_cells = vec![
            vec![eng.get_raw((0, 0)).to_string()],
            vec![eng.get_raw((0, 1)).to_string()],
            vec![eng.get_raw((0, 2)).to_string()],
        ];
        apply_cut_paste(&mut eng, &src_cells, (0, 0), (0, 1), BOUNDS);

        // A1 moved to A2, so C1's =A1 → =A2
        assert_eq!(eng.get_raw((2, 0)), "=A2");
    }

    #[test]
    fn cut_rewrite_formula_unit() {
        // Isolated test of the rewrite function — source rect covers A4
        // only. Ref to A4 shifts; ref to A5 doesn't.
        let rect = ((0, 3), (0, 3));
        let delta = (1, 0);
        assert_eq!(
            rewrite_formula_for_cut("=A4*2+A5", rect, delta, BOUNDS),
            "=B4*2+A5"
        );
        assert_eq!(
            rewrite_formula_for_cut("=$A$4", rect, delta, BOUNDS),
            "=$B$4"
        );
        assert_eq!(
            rewrite_formula_for_cut("=B2", rect, delta, BOUNDS),
            "=B2"
        );
    }

    // ─── Conditional axis shift (row/col insert + delete) ────

    #[test]
    fn axis_shift_row_insert_above_threshold_pushes_refs_down() {
        // Insert row at index 2 (Row 3 in user terms). Refs to row
        // 5 (index 4) shift to row 6 (index 5). Refs to row 1
        // (index 0) stay put.
        assert_eq!(translate_for_axis_shift("=A5", Axis::Row, 2, 1), "=A6");
        assert_eq!(translate_for_axis_shift("=A1", Axis::Row, 2, 1), "=A1");
    }

    #[test]
    fn axis_shift_row_insert_at_threshold_pushes_ref_down() {
        // Inserting at row 2 means anything at index >= 2 shifts.
        // A ref to A3 (index 2) is exactly at the boundary and shifts to A4.
        assert_eq!(translate_for_axis_shift("=A3", Axis::Row, 2, 1), "=A4");
    }

    #[test]
    fn axis_shift_row_insert_preserves_absolute_refs() {
        // Structural shifts apply uniformly — `$` only resists
        // fill/copy translation, not insert/delete.
        assert_eq!(translate_for_axis_shift("=$A$5", Axis::Row, 2, 1), "=$A$6");
    }

    #[test]
    fn axis_shift_col_insert_pushes_refs_right() {
        assert_eq!(translate_for_axis_shift("=C1", Axis::Col, 1, 1), "=D1");
        assert_eq!(translate_for_axis_shift("=A1", Axis::Col, 1, 1), "=A1");
    }

    #[test]
    fn axis_shift_row_delete_pulls_refs_up() {
        // Delete row at index 2. Refs to row 5 (index 4) shift up to row 4.
        assert_eq!(translate_for_axis_shift("=A5", Axis::Row, 2, -1), "=A4");
    }

    #[test]
    fn axis_shift_row_delete_at_threshold_yields_ref_error() {
        // A ref to the deleted row itself becomes #REF!.
        assert_eq!(translate_for_axis_shift("=A3", Axis::Row, 2, -1), "=#REF!");
    }

    #[test]
    fn axis_shift_range_with_one_endpoint_at_deleted_row_is_ref_error() {
        // Deleting row 2 (index 1). The range A1:A2 had its second
        // endpoint at the deleted row → whole range collapses to #REF!.
        assert_eq!(translate_for_axis_shift("=SUM(A1:A2)", Axis::Row, 1, -1), "=SUM(#REF!)");
    }

    #[test]
    fn axis_shift_unrelated_axis_unchanged() {
        // Row insert doesn't touch column coordinates.
        assert_eq!(translate_for_axis_shift("=B1", Axis::Row, 0, 1), "=B2");
        // Col delete doesn't touch row coordinates.
        assert_eq!(translate_for_axis_shift("=B5", Axis::Col, 5, -1), "=B5");
    }

    #[test]
    fn axis_shift_non_formula_passes_through() {
        assert_eq!(translate_for_axis_shift("hello", Axis::Row, 0, 1), "hello");
        assert_eq!(translate_for_axis_shift("42", Axis::Row, 0, 1), "42");
    }

    #[test]
    fn axis_shift_named_range_passes_through() {
        // Named ranges are independent of structural shifts (the
        // `RangeRef` they map to is updated separately by the engine
        // on row/col insert; the formula text just keeps the alias).
        assert_eq!(translate_for_axis_shift("=PROFIT", Axis::Row, 0, 1), "=PROFIT");
    }
}
