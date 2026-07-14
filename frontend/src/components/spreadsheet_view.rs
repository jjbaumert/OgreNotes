// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod cell_comment;
mod context_menu;
mod filter_dropdown;
mod find_replace_bar;
mod foreign_consent;
mod persistence;
mod pivot_editor;
mod sheet_tabs;
mod sort_dialog;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::state::EditorState;
use crate::spreadsheet::eval::{CellValue, SpreadsheetEngine, ValidationRule};
use super::formula_keyboard::{FormulaKeyboard, KeyboardMode};
use super::spreadsheet_chart::render_chart_svg;
use crate::spreadsheet::parser::col_to_letters;
use crate::spreadsheet::translate::Axis;
use crate::touch::{LongPressTracker, LONG_PRESS_MS, TOUCH_MOVE_THRESHOLD_PX};
use persistence::{
    DEFAULT_SHEET_NAME,
    build_doc_with_sheets,
    build_doc_dropping_sheet,
    extract_sheet_names,
    parse_html_table,
    parse_markdown_table,
    snapshot_foreign_doc,
    sync_engine_from_doc_sheet,
};
use context_menu::{ContextMenuDeps, render_context_menu};

/// Phase-5 threaded-cell-comments — payload the spreadsheet hands
/// up to its hosting page when a user wants to open the comment
/// popup on a cell. The hosting page wires the carried fields to
/// its existing CommentPopup state (popup_thread_id, popup_left,
/// popup_top, popup_block_id, popup_is_new) and the popup opens.
///
/// `thread_id` is always Some at this point — the spreadsheet
/// itself creates the thread synchronously before firing this
/// callback (via `comments::create_thread`) and writes the new id
/// into the cell's `CellStyle.comment_thread_id`. That keeps the
/// popup in Thread-mode immediately and avoids the round-trip
/// where the popup creates the thread and the spreadsheet needs a
/// write-back path. `block_id` is the same string as `thread_id`
/// (the `cell-<10-alphanumerics>` shape).
#[derive(Clone)]
pub struct CellCommentOpen {
    pub thread_id: String,
    pub block_id: String,
    pub left: f64,
    pub top: f64,
}

pub(crate) use cell_comment::parse_cell_block_id;

/// Target for deep-linking a comment notification to a specific cell
/// (issue #50). The hosting page sets this (parsed from the thread's
/// `cell-…` block id); SpreadsheetView watches it, switches to the sheet,
/// selects + scrolls the cell into view, and opens its comment popup.
#[derive(Clone, Debug, PartialEq)]
pub struct CellFocus {
    pub sheet: usize,
    pub row: usize,
    pub col: usize,
    pub thread_id: String,
}

/// Preview metadata for one cell-anchored comment thread, supplied by
/// the hosting page from its `list_threads` fetch. The spreadsheet keys
/// these by `block_id` (`cell-s<sheet>r<row>c<col>`) to render a
/// thread-aware hover preview on commented cells — the opening message
/// plus a reply count — so a threaded cell reads as a conversation, not
/// a static one-author note. The page already loads these threads for
/// the document's inline-comment highlights, so wiring them here adds no
/// extra request.
#[derive(Clone, Debug, PartialEq)]
pub struct CellThreadInfo {
    /// Deterministic cell block id this thread is anchored to.
    pub block_id: String,
    pub thread_id: String,
    /// Opening message preview (already truncated server-side). `None`
    /// for a freshly created, still-empty thread.
    pub first_message: Option<String>,
    /// Number of replies after the opening message (`message_count - 1`,
    /// floored at 0).
    pub reply_count: u32,
}
use filter_dropdown::render_filter_dropdown;
use find_replace_bar::render_find_replace_bar;
use foreign_consent::render_foreign_consent;
use pivot_editor::render_pivot_editor;
use sheet_tabs::render_sheet_tab_bar;
use sort_dialog::{render_sort_dialog, SortDialogContext};

/// Read `document.activeElement` and downcast to `Element`. Returns `None`
/// when there's no focused element or the cast fails (the latter shouldn't
/// happen in practice — included for safety against unusual DOM shapes).
fn document_active_element(doc: &web_sys::Document) -> Option<web_sys::Element> {
    doc.active_element()
}

/// Function names and descriptions for the formula autocomplete
/// popover. Generated mechanically from the dispatch table in
/// `frontend/src/spreadsheet/functions.rs`; well-known names carry
/// detailed prose, the rest get a one-word category tag (Math/trig,
/// Statistical, Financial, Database, Engineering, …).
///
/// When adding a new function to the dispatch, also add it here so
/// it surfaces in the `=NAME(` autocomplete dropdown — issue #22.
///
/// `COMMON_FUNCTIONS` below pins the autocomplete ranking for
/// high-demand functions — without it, alphabetical FUNCTION_LIST
/// order puts SUBSTITUTE / SUBTOTAL above SUM for partial `SU`.
/// Mirror const in formula_keyboard.rs::COMMON_FUNCTIONS; the two
/// must stay in sync.
const COMMON_FUNCTIONS: &[&str] = &[
    "SUM", "AVERAGE", "IF", "COUNT", "MIN", "MAX", "VLOOKUP", "SUMIF",
];

const FUNCTION_LIST: &[(&str, &str)] = &[
    ("ABS", "Absolute value"),
    ("ACCRINT", "Financial"),
    ("ACCRINTM", "Financial"),
    ("ACOS", "Math/trig"),
    ("ACOSH", "Math/trig"),
    ("ACOT", "Math/trig"),
    ("ACOTH", "Math/trig"),
    ("AND", "TRUE if all args TRUE"),
    ("ARABIC", "Math/trig"),
    ("ASIN", "Math/trig"),
    ("ASINH", "Math/trig"),
    ("ATAN", "Math/trig"),
    ("ATANH", "Math/trig"),
    ("AVEDEV", "Statistical"),
    ("AVERAGE", "Arithmetic mean"),
    ("AVERAGEIF", "Statistical"),
    ("AVERAGEIFS", "Statistical"),
    ("AVG", "Statistical"),
    ("BASE", "Math/trig"),
    ("BESSELI", "Bessel function"),
    ("BESSELJ", "Bessel function"),
    ("BESSELK", "Bessel function"),
    ("BESSELY", "Bessel function"),
    ("BETADIST", "Statistical distribution"),
    ("BETAINV", "Statistical distribution"),
    ("BINOMDIST", "Statistical distribution"),
    ("BITAND", "Bitwise"),
    ("BITLSHIFT", "Bitwise"),
    ("BITOR", "Bitwise"),
    ("BITRSHIFT", "Bitwise"),
    ("BITXOR", "Bitwise"),
    ("CEILING.MATH", "Math/trig"),
    ("CHAR", "Character from code"),
    ("CHIDIST", "Statistical distribution"),
    ("CHIINV", "Statistical distribution"),
    ("CHISQ.DIST", "Statistical distribution"),
    ("CHISQ.INV", "Statistical distribution"),
    ("CHISQ.TEST", "Statistical test"),
    ("CHITEST", "Statistical test"),
    ("CHOOSE", "Value by index"),
    ("CLEAN", "Text/date"),
    ("CODE", "Code of first char"),
    ("COLUMN", "Column number"),
    ("COLUMNS", "Number of columns"),
    ("COMBIN", "Math/trig"),
    ("COMBINA", "Math/trig"),
    ("COMPLEX", "Engineering"),
    ("CONCAT", "Join text"),
    ("CONFIDENCE.NORM", "Statistical"),
    ("CONFIDENCE.T", "Statistical"),
    ("COS", "Math/trig"),
    ("COSH", "Math/trig"),
    ("COT", "Math/trig"),
    ("COTH", "Math/trig"),
    ("COUNT", "Count numbers"),
    ("COUNTA", "Count non-empty"),
    ("COUNTBLANK", "Count blanks"),
    ("COUNTIF", "Conditional count"),
    ("COUNTIFS", "Statistical"),
    ("CRITBINOM", "Statistical distribution"),
    ("CSC", "Math/trig"),
    ("CSCH", "Math/trig"),
    ("CUMIPMT", "Financial"),
    ("CUMPRINC", "Financial"),
    ("DATE", "Create date"),
    ("DATEDIF", "Date/time"),
    ("DATEVALUE", "Date/time"),
    ("DAVERAGE", "Database"),
    ("DAY", "Day of month"),
    ("DAYS", "Date/time"),
    ("DB", "Financial"),
    ("DCOUNT", "Database"),
    ("DCOUNTA", "Database"),
    ("DDB", "Financial"),
    ("DECIMAL", "Math/trig"),
    ("DEGREES", "Math/trig"),
    ("DELTA", "Engineering"),
    ("DEVSQ", "Statistical"),
    ("DGET", "Database"),
    ("DISC", "Financial"),
    ("DMAX", "Database"),
    ("DMIN", "Database"),
    ("DOLLAR", "Financial"),
    ("DOLLARDE", "Financial"),
    ("DOLLARFR", "Financial"),
    ("DPRODUCT", "Database"),
    ("DSTDEV", "Database"),
    ("DSTDEVP", "Database"),
    ("DSUM", "Database"),
    ("DURATION", "Financial"),
    ("DVAR", "Database"),
    ("DVARP", "Database"),
    ("EDATE", "Date/time"),
    ("EFFECT", "Financial"),
    ("EOMONTH", "Date/time"),
    ("ERF.PRECISE", "Engineering"),
    ("ERFC.PRECISE", "Engineering"),
    ("ERROR.TYPE", "Lookup/info"),
    ("EVEN", "Math/trig"),
    ("EXACT", "Text/date"),
    ("EXP", "e to power"),
    ("EXPONDIST", "Statistical distribution"),
    ("F.DIST", "Statistical distribution"),
    ("F.INV", "Statistical distribution"),
    ("F.TEST", "Statistical test"),
    ("FACT", "Math/trig"),
    ("FACTDOUBLE", "Math/trig"),
    ("FALSE", "Logical FALSE"),
    ("FDIST", "Statistical distribution"),
    ("FILTER", "Lookup/info"),
    ("FIND", "Find text (case-sensitive)"),
    ("FINV", "Statistical distribution"),
    ("FIXED", "Text/date"),
    ("FTEST", "Statistical test"),
    ("FLOOR.MATH", "Math/trig"),
    ("FLOOR.PRECISE", "Math/trig"),
    ("FORECAST", "Statistical"),
    ("FORECAST.LINEAR", "Statistical"),
    ("FV", "Financial"),
    ("FVSCHEDULE", "Financial"),
    ("GAMMA", "Statistical distribution"),
    ("GAMMADIST", "Statistical distribution"),
    ("GAMMAINV", "Statistical distribution"),
    ("GAMMALN.PRECISE", "Statistical distribution"),
    ("GCD", "Math/trig"),
    ("GEOMEAN", "Statistical"),
    ("GESTEP", "Engineering"),
    ("GROWTH", "Statistical regression"),
    ("HARMEAN", "Statistical"),
    ("HLOOKUP", "Horizontal lookup"),
    ("HOUR", "Date/time"),
    ("HYPGEOMDIST", "Statistical distribution"),
    ("IF", "Conditional"),
    ("IFERROR", "Handle errors"),
    ("IFNA", "Handle #N/A"),
    ("IFS", "Multiple conditions"),
    ("IMABS", "Complex number"),
    ("IMAGINARY", "Complex number"),
    ("IMARGUMENT", "Complex number"),
    ("IMCONJUGATE", "Complex number"),
    ("IMCOS", "Complex number"),
    ("IMCOSH", "Complex number"),
    ("IMDIV", "Complex number"),
    ("IMEXP", "Complex number"),
    ("IMLN", "Complex number"),
    ("IMPOWER", "Complex number"),
    ("IMPRODUCT", "Complex number"),
    ("IMREAL", "Complex number"),
    ("IMSIN", "Complex number"),
    ("IMSINH", "Complex number"),
    ("IMSQRT", "Complex number"),
    ("IMSUB", "Complex number"),
    ("IMSUM", "Complex number"),
    ("IMTAN", "Complex number"),
    ("INDEX", "Value by position"),
    ("INT", "Round to integer"),
    ("INTERCEPT", "Statistical"),
    ("INTRATE", "Financial"),
    ("IPMT", "Financial"),
    ("IRR", "Financial"),
    ("ISBLANK", "Test blank"),
    ("ISERR", "Lookup/info"),
    ("ISEVEN", "Math/trig"),
    ("ISFORMULA", "Lookup/info"),
    ("ISLOGICAL", "Lookup/info"),
    ("ISNA", "Test #N/A"),
    ("ISNUMBER", "Test number"),
    ("ISO.CEILING", "Math/trig"),
    ("ISODD", "Math/trig"),
    ("ISOWEEKNUM", "Date/time"),
    ("ISREF", "Lookup/info"),
    ("ISTEXT", "Test text"),
    ("LARGE", "Statistical"),
    ("LCM", "Math/trig"),
    ("LEFT", "Leftmost chars"),
    ("LEN", "Character count"),
    ("LINEST", "Statistical regression"),
    ("LN", "Natural log"),
    ("LOG", "Logarithm"),
    ("LOGEST", "Statistical regression"),
    ("LOGINV", "Statistical distribution"),
    ("LOGNORMDIST", "Statistical distribution"),
    ("LOWER", "To lowercase"),
    ("MATCH", "Position in array"),
    ("MAX", "Maximum"),
    ("MAXA", "Statistical"),
    ("MAXIFS", "Statistical"),
    ("MDETERM", "Math/trig"),
    ("MDURATION", "Financial"),
    ("MEDIAN", "Statistical"),
    ("MID", "Middle chars"),
    ("MIN", "Minimum"),
    ("MINA", "Statistical"),
    ("MINIFS", "Statistical"),
    ("MINUTE", "Date/time"),
    ("MINVERSE", "Math/trig"),
    ("MIRR", "Financial"),
    ("MMULT", "Math/trig"),
    ("MOD", "Remainder"),
    ("MODE.SNGL", "Statistical"),
    ("MONTH", "Month number"),
    ("MROUND", "Math/trig"),
    ("MULTINOMIAL", "Math/trig"),
    ("MUNIT", "Math/trig"),
    ("NEGBINOM.DIST", "Statistical distribution"),
    ("NEGBINOMDIST", "Statistical distribution"),
    ("NETWORKDAYS", "Date/time"),
    ("NETWORKDAYS.INTL", "Date/time"),
    ("NOMINAL", "Financial"),
    ("NORMDIST", "Statistical distribution"),
    ("NORMINV", "Statistical distribution"),
    ("NORMSDIST", "Statistical distribution"),
    ("NORMSINV", "Statistical distribution"),
    ("NOT", "Reverse logical"),
    ("NOW", "Current date/time"),
    ("NPER", "Financial"),
    ("NPV", "Financial"),
    ("ODD", "Math/trig"),
    ("OR", "TRUE if any TRUE"),
    ("PDURATION", "Financial"),
    ("PEARSON", "Statistical"),
    ("PERCENTILE.EXC", "Statistical"),
    ("PERCENTILE.INC", "Statistical"),
    ("PERMUT", "Math/trig"),
    ("PERMUTATIONA", "Math/trig"),
    ("PI", "Value of pi"),
    ("PMT", "Financial"),
    ("POISSON", "Statistical distribution"),
    ("POWER", "Exponentiation"),
    ("PPMT", "Financial"),
    ("PRICE", "Financial"),
    ("PRICEDISC", "Financial"),
    ("PRICEMAT", "Financial"),
    ("PRODUCT", "Multiply all"),
    ("PROPER", "Text/date"),
    ("PV", "Financial"),
    ("QUARTILE.EXC", "Statistical"),
    ("QUARTILE.INC", "Statistical"),
    ("QUOTIENT", "Math/trig"),
    ("RADIANS", "Math/trig"),
    ("RAND", "Random 0-1"),
    ("RANDARRAY", "Lookup/info"),
    ("RANDBETWEEN", "Random in range"),
    ("RANK.AVG", "Statistical"),
    ("RANK.EQ", "Statistical"),
    ("RATE", "Financial"),
    ("RECEIVED", "Financial"),
    ("REFERENCERANGE", "Spreadsheet ext"),
    ("REFERENCESHEET", "Spreadsheet ext"),
    ("REPLACEB", "Text/date"),
    ("REPT", "Repeat text"),
    ("RIGHT", "Rightmost chars"),
    ("ROMAN", "Math/trig"),
    ("ROUND", "Round to digits"),
    ("ROUNDDOWN", "Round toward zero"),
    ("ROUNDUP", "Round away from zero"),
    ("ROW", "Row number"),
    ("ROWS", "Number of rows"),
    ("RRI", "Financial"),
    ("RSQ", "Statistical"),
    ("SEARCH", "Find text (case-insensitive)"),
    ("SEC", "Math/trig"),
    ("SECH", "Math/trig"),
    ("SECOND", "Date/time"),
    ("SEQUENCE", "Lookup/info"),
    ("SERIESSUM", "Math/trig"),
    ("SIGN", "Sign of number"),
    ("SIN", "Math/trig"),
    ("SINH", "Math/trig"),
    ("SLN", "Financial"),
    ("SLOPE", "Statistical"),
    ("SMALL", "Statistical"),
    ("SORT", "Lookup/info"),
    ("SQRT", "Square root"),
    ("SQRTPI", "Math/trig"),
    ("STDEV.S", "Statistical"),
    ("STDEVP", "Statistical"),
    ("STEYX", "Statistical"),
    ("SUBSTITUTE", "Replace text"),
    ("SUBTOTAL", "Math/trig"),
    ("SUM", "Sum values"),
    ("SUMIF", "Conditional sum"),
    ("SUMIFS", "Statistical"),
    ("SUMPRODUCT", "Statistical"),
    ("SUMSQ", "Statistical"),
    ("SWITCH", "Match value list"),
    ("SYD", "Financial"),
    ("T.DIST", "Statistical distribution"),
    ("T.INV", "Statistical distribution"),
    ("T.TEST", "Statistical test"),
    ("TAN", "Math/trig"),
    ("TANH", "Math/trig"),
    ("TBILLEQ", "Financial"),
    ("TBILLPRICE", "Financial"),
    ("TBILLYIELD", "Financial"),
    ("TDIST", "Statistical distribution"),
    ("TEXT", "Format as text"),
    ("TEXTJOIN", "Text/date"),
    ("TIME", "Date/time"),
    ("TIMEVALUE", "Date/time"),
    ("TINV", "Statistical distribution"),
    ("TRANSPOSE", "Lookup/info"),
    ("TREND", "Statistical regression"),
    ("TRIM", "Remove extra spaces"),
    ("TRIMMEAN", "Statistical"),
    ("TRUE", "Logical TRUE"),
    ("TRUNC", "Truncate"),
    ("TTEST", "Statistical test"),
    ("TYPE", "Value type"),
    ("UNICHAR", "Text/date"),
    ("UNICODE", "Text/date"),
    ("UNIQUE", "Lookup/info"),
    ("UPPER", "To uppercase"),
    ("VALUE", "Text to number"),
    ("VAR.S", "Statistical"),
    ("VARP", "Statistical"),
    ("VDB", "Financial"),
    ("VLOOKUP", "Vertical lookup"),
    ("WEEKDAY", "Date/time"),
    ("WEIBULL", "Statistical distribution"),
    ("WORKDAY", "Date/time"),
    ("WORKDAY.INTL", "Date/time"),
    ("XIRR", "Financial"),
    ("XLOOKUP", "Lookup/info"),
    ("XMATCH", "Lookup/info"),
    ("XNPV", "Financial"),
    ("XOR", "Exclusive OR"),
    ("YEAR", "Year from date"),
    ("YIELD", "Financial"),
    ("YIELDDISC", "Financial"),
    ("YIELDMAT", "Financial"),
    ("ZTEST", "Statistical"),
];

/// Ordered selection bounds.
pub(super) fn sel_bounds(r1: usize, c1: usize, r2: usize, c2: usize) -> (usize, usize, usize, usize) {
    (r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2))
}

/// True when the cell at (row, col) holds non-empty raw content.
/// `CellAddr` is (col, row), so the arguments are swapped on the way in.
pub(super) fn cell_is_used(engine: &SpreadsheetEngine, row: usize, col: usize) -> bool {
    !engine.get_raw((col, row)).is_empty()
}

/// The last used cell as (row, col): the max occupied row and max occupied
/// column taken independently — Excel's "used range" bottom-right corner,
/// which may itself be empty. (0, 0) for an empty sheet. Backs Ctrl+End.
pub(super) fn last_used_cell(engine: &SpreadsheetEngine) -> (usize, usize) {
    let mut max_r = 0;
    let mut max_c = 0;
    for ((c, r), raw) in engine.iter_raw() {
        if !raw.is_empty() {
            max_r = max_r.max(r);
            max_c = max_c.max(c);
        }
    }
    (max_r, max_c)
}

/// The rightmost used column in `row`, or `None` when the row is entirely
/// empty. Backs the plain `End` key (jump to the end of the row's data).
/// Returns `None` rather than a sentinel `0` so the caller can tell "empty
/// row — don't move" apart from "the rightmost data is in column 0".
pub(super) fn last_used_col_in_row(
    engine: &SpreadsheetEngine,
    row: usize,
    max_col: usize,
) -> Option<usize> {
    let mut last = None;
    for c in 0..=max_col {
        if cell_is_used(engine, row, c) {
            last = Some(c);
        }
    }
    last
}

/// Excel-style Ctrl+Arrow jump from (row, col) in direction (dr, dc) where
/// exactly one of dr/dc is ±1. Returns the destination cell (row, col):
///   * non-empty cell with a non-empty neighbor → the last cell of the
///     contiguous run before the next blank (the run's edge);
///   * non-empty cell with a blank neighbor, or a blank starting cell →
///     the next non-empty cell across the gap;
///   * if no further data exists, the last grid cell in that direction.
/// The search is bounded by `max_row`/`max_col` (the current grid extent)
/// so a jump through empty space halts at the grid edge instead of looping.
pub(super) fn data_edge(
    engine: &SpreadsheetEngine,
    row: usize,
    col: usize,
    dr: i32,
    dc: i32,
    max_row: usize,
    max_col: usize,
) -> (usize, usize) {
    let in_bounds = |r: i32, c: i32| {
        r >= 0 && c >= 0 && r as usize <= max_row && c as usize <= max_col
    };
    let used = |r: i32, c: i32| in_bounds(r, c) && cell_is_used(engine, r as usize, c as usize);

    let mut r = row as i32;
    let mut c = col as i32;

    if used(r, c) && used(r + dr, c + dc) {
        // Walk to the far end of the contiguous run.
        while used(r + dr, c + dc) {
            r += dr;
            c += dc;
        }
    } else {
        // Skip any blanks and land on the first used cell; if none, stop at
        // the last in-bounds cell (the grid edge).
        let mut nr = r + dr;
        let mut nc = c + dc;
        while in_bounds(nr, nc) && !used(nr, nc) {
            nr += dr;
            nc += dc;
        }
        if in_bounds(nr, nc) {
            r = nr;
            c = nc;
        } else {
            r = nr - dr;
            c = nc - dc;
        }
    }

    (r.max(0) as usize, c.max(0) as usize)
}

/// Decompose `rect` (r1,c1,r2,c2 inclusive, normalized) into the sub-rects
/// that remain after removing the single cell (row, col). Returns the rect
/// unchanged when the cell is outside it, an empty Vec when the rect was a
/// 1x1 covering exactly that cell, or up to four bands otherwise. Backs the
/// Ctrl-click toggle-off of a cell from a multi-region selection (#73).
pub(super) fn subtract_cell(
    rect: (usize, usize, usize, usize),
    row: usize,
    col: usize,
) -> Vec<(usize, usize, usize, usize)> {
    let (r1, c1, r2, c2) = rect;
    if row < r1 || row > r2 || col < c1 || col > c2 {
        return vec![rect];
    }
    let mut out = Vec::new();
    if row > r1 {
        out.push((r1, c1, row - 1, c2)); // full band above the cell
    }
    if row < r2 {
        out.push((row + 1, c1, r2, c2)); // full band below the cell
    }
    if col > c1 {
        out.push((row, c1, row, col - 1)); // same row, left of the cell
    }
    if col < c2 {
        out.push((row, col + 1, row, c2)); // same row, right of the cell
    }
    out
}

/// Direction for Ctrl+D / Ctrl+R fill.
#[derive(Copy, Clone, PartialEq, Eq)]
enum FillDir {
    Down,
    Right,
}

/// Apply a fill-down (Ctrl+D) or fill-right (Ctrl+R) over the selection
/// rectangle. Each target cell's value is the source cell's raw content
/// translated by the per-cell delta so formulas shift their relative
/// references (e.g. `=A1` filled down becomes `=A2`, `=A3`, ...).
fn apply_fill(
    eng: &mut crate::spreadsheet::eval::SpreadsheetEngine,
    sel: (usize, usize, usize, usize),
    dir: FillDir,
    bounds: (usize, usize),
) {
    let (r1, c1, r2, c2) = sel;
    match dir {
        FillDir::Down => {
            if r1 >= r2 {
                return;
            }
            for c in c1..=c2 {
                let src = eng.get_raw((c, r1)).to_string();
                for r in (r1 + 1)..=r2 {
                    let delta = (0i32, (r as i32) - (r1 as i32));
                    let translated = crate::spreadsheet::translate::translate_formula(
                        &src, delta, bounds,
                    );
                    eng.set_cell((c, r), &translated);
                }
            }
        }
        FillDir::Right => {
            if c1 >= c2 {
                return;
            }
            for r in r1..=r2 {
                let src = eng.get_raw((c1, r)).to_string();
                for c in (c1 + 1)..=c2 {
                    let delta = ((c as i32) - (c1 as i32), 0i32);
                    let translated = crate::spreadsheet::translate::translate_formula(
                        &src, delta, bounds,
                    );
                    eng.set_cell((c, r), &translated);
                }
            }
        }
    }
}

/// Rewrite every formula cell in `engine` (within the rectangle
/// `(rows, cols)`) so its references reflect a row or column
/// insertion / deletion. Closes the issue #3 finding "row/column
/// insertion doesn't rewrite formula references" — without this,
/// inserting a row above `=A5` would leave the formula pointing
/// at the now-shifted cell rather than the original target.
///
/// `delta = +1` is insert; `delta = -1` is delete. Refs whose
/// coord crosses the deleted line become `#REF!`. Cells that don't
/// hold a formula are skipped.
fn rewrite_formulas_after_axis_shift(
    engine: &mut crate::spreadsheet::eval::SpreadsheetEngine,
    rows: usize,
    cols: usize,
    axis: Axis,
    threshold: usize,
    delta: i32,
) {
    use crate::spreadsheet::translate::translate_for_axis_shift;
    for r in 0..rows {
        for c in 0..cols {
            let raw = engine.get_raw((c, r)).to_string();
            if !raw.starts_with('=') { continue; }
            let translated = translate_for_axis_shift(&raw, axis, threshold, delta);
            if translated != raw {
                engine.set_cell((c, r), &translated);
            }
        }
    }
}

/// Compare two rows by a chain of `(column, ascending)` sort keys.
/// Tries numeric comparison first per key (so `"10"` sorts after
/// `"9"`); falls back to case-insensitive textual comparison when
/// either cell isn't parseable as `f64`. Earlier keys dominate;
/// later keys are tiebreakers. Empty chain returns `Ordering::Equal`,
/// preserving caller-provided order.
fn compare_rows_by_keys(
    a: &[(String, Option<crate::spreadsheet::eval::CellStyle>)],
    b: &[(String, Option<crate::spreadsheet::eval::CellStyle>)],
    keys: &[(usize, bool)],
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for &(col, ascending) in keys {
        let av = a.get(col).map(|(s, _)| s.as_str()).unwrap_or("");
        let bv = b.get(col).map(|(s, _)| s.as_str()).unwrap_or("");
        // Excel parity: blank cells always sort to the BOTTOM,
        // regardless of asc/desc direction. (`"" < "1"` lexicographically
        // would otherwise stack empty rows at the top of an ascending
        // sort and shove the populated rows down — visible as
        // doctor-scenario `sortReorderedRows` failing on a sparse grid.)
        // The intra-direction reverse below is gated on a non-blank
        // ordering so blanks-last stays stable in both directions.
        let cmp = match (av.is_empty(), bv.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,   // blank > populated → blank goes last
            (false, true) => Ordering::Less,
            (false, false) => match (av.parse::<f64>(), bv.parse::<f64>()) {
                // Both sides parse as finite numbers — compare numerically.
                // NaN is intentionally rejected here: `"NaN".parse::<f64>()`
                // succeeds, but `f64::NAN.partial_cmp(...)` returns `None`,
                // and falling through to `Equal` would scatter NaN-valued
                // rows arbitrarily. Falling through to text instead at
                // least keeps `"NaN"` and `"nan"` in the same equivalence
                // class.
                (Ok(an), Ok(bn)) if !an.is_nan() && !bn.is_nan() => {
                    an.partial_cmp(&bn).unwrap_or(Ordering::Equal)
                }
                _ => av.to_lowercase().cmp(&bv.to_lowercase()),
            },
        };
        // Don't flip the blank-comparison branches when descending —
        // blanks always go last. Only flip when both sides are populated.
        let cmp = match (av.is_empty(), bv.is_empty()) {
            (true, _) | (_, true) => cmp,
            _ => if ascending { cmp } else { cmp.reverse() },
        };
        if cmp != Ordering::Equal { return cmp; }
    }
    Ordering::Equal
}

/// Build the TSV + per-row cell vector for a rectangular selection.
/// Returns `(tsv_string, cells_by_row)` where `tsv_string` is the OS-
/// clipboard payload (tab-separated, newline-terminated rows) and
/// `cells_by_row` is the same data sliced for the in-app
/// `SheetClipboard::cells` field. Both Cut and Copy emit identical
/// shapes — only the resulting `ClipMode` differs.
fn selection_to_tsv(
    eng: &SpreadsheetEngine,
    bounds: (usize, usize, usize, usize),
) -> (String, Vec<Vec<String>>) {
    let (r1, c1, r2, c2) = bounds;
    let mut tsv = String::new();
    let mut cells: Vec<Vec<String>> = Vec::with_capacity(r2 - r1 + 1);
    for r in r1..=r2 {
        let mut row_cells: Vec<String> = Vec::with_capacity(c2 - c1 + 1);
        for c in c1..=c2 {
            if c > c1 { tsv.push('\t'); }
            let raw = eng.get_raw((c, r));
            tsv.push_str(raw);
            row_cells.push(raw.to_string());
        }
        tsv.push('\n');
        cells.push(row_cells);
    }
    (tsv, cells)
}

/// #54: emit the selection as a GitHub-flavored markdown table — the
/// inverse of `parse_markdown_table`. The first selected row becomes the
/// header (GFM requires a header + a `---` separator row). Literal pipes
/// in cell text are backslash-escaped (`|` → `\|`) so the round-trip back
/// through `parse_markdown_table` (which honors `\|`, #54 item 1) is
/// lossless.
fn selection_to_markdown(
    eng: &SpreadsheetEngine,
    bounds: (usize, usize, usize, usize),
) -> String {
    let (r1, c1, r2, c2) = bounds;
    let ncols = c2 - c1 + 1;
    let row_md = |r: usize| -> String {
        let mut s = String::from("|");
        for c in c1..=c2 {
            s.push(' ');
            s.push_str(&eng.get_raw((c, r)).replace('|', "\\|"));
            s.push_str(" |");
        }
        s
    };
    let mut out = String::new();
    out.push_str(&row_md(r1)); // header row
    out.push('\n');
    out.push('|');
    for _ in 0..ncols {
        out.push_str(" --- |");
    }
    out.push('\n');
    for r in (r1 + 1)..=r2 {
        out.push_str(&row_md(r));
        out.push('\n');
    }
    out
}

/// #75: copy and cut operate on a single rectangular range. When the
/// selection is non-contiguous (the user Ctrl-clicked extra regions),
/// silently copying just the primary rectangle is a data surprise —
/// the user sees several highlighted regions but only one travels. Excel
/// refuses the op in this case; we do the same. Returns true when extra
/// regions are present and the clipboard op must be blocked.
fn multi_region_blocks_clipboard(extra_regions: &[(usize, usize, usize, usize)]) -> bool {
    !extra_regions.is_empty()
}

/// Fire-and-forget write of `text` to the OS clipboard via
/// `navigator.clipboard.writeText`. Reflect-based to avoid a hard
/// dependency on the `Clipboard` web-sys binding (which isn't
/// universally available across browsers we target). All error paths
/// are silently swallowed — the in-app clipboard already holds the
/// authoritative copy.
fn write_text_to_os_clipboard(text: String) {
    let Some(window) = web_sys::window() else { return };
    let nav = window.navigator();
    let Ok(clip) = js_sys::Reflect::get(&nav, &"clipboard".into()) else { return };
    let Ok(write) = js_sys::Reflect::get(&clip, &"writeText".into()) else { return };
    let Ok(write_fn) = write.dyn_into::<js_sys::Function>() else { return };
    let _ = write_fn.call1(&clip, &text.into());
}

/// Kick off an HTTP fetch for a foreign document referenced by a
/// REFERENCERANGE / REFERENCESHEET formula. On completion, decode
/// the CRDT bytes into a `ForeignDocSnapshot` and write it into the
/// engine's foreign-doc cache; on failure, write the typed error.
/// Either way, bump `grid_version` so dependent formulas re-evaluate.
///
/// On a successful fetch, also invoke `on_subscribe_foreign` so the
/// page-level WS client subscribes this connection to live updates
/// from the foreign doc — server-side push then drives invalidation
/// without waiting for a manual recompute.
///
/// `fetched_ids` is a session-scoped guard that keeps a second
/// concurrent recompute from spawning a duplicate request for the
/// same id while the first is still in flight.
fn spawn_foreign_doc_fetch(
    engine: &'static Mutex<crate::spreadsheet::eval::SpreadsheetEngine>,
    fetched_ids: &'static Mutex<std::collections::HashSet<String>>,
    set_grid_version: WriteSignal<u32>,
    grid_version: ReadSignal<u32>,
    on_subscribe_foreign: Callback<String>,
    id: String,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    {
        let mut guard = fetched_ids.lock().unwrap();
        if guard.contains(&id) { return; }
        guard.insert(id.clone());
    }
    leptos::task::spawn_local(async move {
        use crate::spreadsheet::eval::ForeignFetchError;
        let result = crate::api::documents::get_content(&id).await;
        // Bail out if the SpreadsheetView has dropped while we were
        // awaiting the HTTP fetch — `engine` and `fetched_ids` have
        // been freed by on_cleanup and any further lock() would
        // dereference dangling memory. The atomic load is on the
        // Arc-backed flag which stays valid past component drop.
        if !alive.load(std::sync::atomic::Ordering::SeqCst) { return; }
        let mut eng = engine.lock().unwrap();
        let mut subscribe_after = false;
        match result {
            Ok(bytes) => {
                match crate::editor::yrs_bridge::ydoc_bytes_to_doc(&bytes) {
                    Ok(doc) => {
                        let snap = persistence::snapshot_foreign_doc(&doc);
                        eng.set_foreign_doc_snapshot(id.clone(), snap);
                        subscribe_after = true;
                    }
                    Err(_) => eng.set_foreign_doc_error(id.clone(), ForeignFetchError::Decode),
                }
            }
            Err(err) => {
                use crate::api::client::ApiClientError;
                let mapped = match err {
                    ApiClientError::Http(404, _) => ForeignFetchError::NotFound,
                    ApiClientError::Http(401 | 403, _) | ApiClientError::Unauthorized => {
                        ForeignFetchError::Forbidden
                    }
                    // Other 4xx/5xx and transport / decode errors stay
                    // transient — the formula keeps `#LOADING!` so a
                    // future retry (WS push or manual refresh) can
                    // recover.
                    _ => ForeignFetchError::Network,
                };
                eng.set_foreign_doc_error(id.clone(), mapped);
            }
        }
        drop(eng);
        // Allow this id to be re-fetched on a future recompute
        // (e.g. WS push or manual refresh).
        fetched_ids.lock().unwrap().remove(&id);
        // Subscribe to live pushes from the foreign doc — only on a
        // successful initial fetch. Errors don't subscribe (a 404
        // doesn't generate updates anyway, and a Network failure
        // means our initial decode failed — we'll retry from the
        // recompute path instead of opening a doomed subscription).
        if subscribe_after {
            on_subscribe_foreign.run(id.clone());
        }
        set_grid_version.set(grid_version.get_untracked().wrapping_add(1));
    });
}

// ─── Formula reference pick helpers ────────────────────────────

/// True when the caret is in a position where Excel-style formula entry
/// expects the user to pick a cell reference: just after `=`, `(`, `,`,
/// or one of the binary operators. v1 restricts picks to end-of-string —
/// mid-string picks need caret tracking we don't have yet.
fn is_ref_context(edit_value: &str, caret: usize) -> bool {
    if !edit_value.starts_with('=') {
        return false;
    }
    if caret == 0 || caret != edit_value.len() {
        return false;
    }
    // Last char safe to unwrap: caret > 0 and we've indexed to caret.
    let last = edit_value[..caret].chars().last().unwrap();
    matches!(
        last,
        '=' | '(' | ',' | '+' | '-' | '*' | '/' | '^' | '&' | ':' | '<' | '>'
    )
}

/// Format a single cell address as "A1", "B4", "Z10", "AA100".
fn cell_label(addr: (usize, usize)) -> String {
    let (c, r) = addr;
    format!("{}{}", col_to_letters(c), r + 1)
}

/// Format a range as "A1:B3". When start == end, degenerate to a single
/// cell label. Always normalizes so the start is the top-left corner —
/// Excel's convention regardless of drag direction.
fn range_label(start: (usize, usize), end: (usize, usize)) -> String {
    if start == end {
        return cell_label(start);
    }
    let (sc, sr) = start;
    let (ec, er) = end;
    let tl = (sc.min(ec), sr.min(er));
    let br = (sc.max(ec), sr.max(er));
    format!("{}:{}", cell_label(tl), cell_label(br))
}

/// Replace the tail of `edit_value` (from `insert_at` onward) with `label`.
/// Used to update the ref we're building as the user drags or arrows.
fn splice_ref(edit_value: &str, insert_at: usize, label: &str) -> String {
    let head = edit_value.get(..insert_at).unwrap_or(edit_value);
    format!("{head}{label}")
}

/// True when the device is touch-primary (no hover capability). Used to
/// gate the in-page formula keyboard so desktop users editing `=...` cells
/// don't suddenly see a mobile keyboard. Read once on first call, but the
/// matchMedia result is stable enough across a session that we don't need
/// to track changes — desktops with touch screens are the only edge case
/// and behave reasonably either way.
fn is_touch_primary() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(hover: none)").ok().flatten())
        .map(|m| m.matches())
        .unwrap_or(false)
}

/// Infer the appropriate `inputmode` hint for a cell input based on the
/// column's existing data. Returns `"decimal"` when ≥80% of the first
/// `max_rows` non-empty cells in that column parse as `f64`, otherwise `""`
/// (which omits the attribute and falls back to the default OS keyboard).
///
/// Requires at least 3 non-empty cells before latching numeric so a
/// brand-new column with one typed digit doesn't suppress the alphanumeric
/// keyboard.
fn infer_column_inputmode(engine: &SpreadsheetEngine, col: usize, max_rows: usize) -> &'static str {
    let mut numeric = 0usize;
    let mut total = 0usize;
    for row in 0..max_rows {
        let raw = engine.get_raw((col, row));
        if raw.is_empty() {
            continue;
        }
        total += 1;
        if raw.parse::<f64>().is_ok() {
            numeric += 1;
        }
    }
    if total >= 3 && numeric * 5 >= total * 4 {
        "decimal"
    } else {
        ""
    }
}

/// For a formula string, count how many closing parens are missing. Parens
/// inside double-quoted string literals don't count (so `="he(llo"` returns
/// 0). Non-formula strings return 0. An over-balanced formula (more `)`
/// than `(`) also returns 0 — we only *add* missing closes on commit; we
/// don't rewrite user-typed content.
fn missing_close_parens(formula: &str) -> usize {
    if !formula.starts_with('=') {
        return 0;
    }
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    for c in formula.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            match c {
                '\\' => escape_next = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth.max(0) as usize
}

/// State for an in-progress reference pick.
#[derive(Clone, Debug, PartialEq)]
struct RefPick {
    start: (usize, usize),   // anchor cell (col, row)
    end: (usize, usize),     // current end (== start for a single cell)
    insert_at: usize,        // byte offset into edit_value where the ref begins
}

impl RefPick {
    fn label(&self) -> String {
        range_label(self.start, self.end)
    }

    /// True if (c, r) falls inside the normalized bounding rect.
    fn contains(&self, c: usize, r: usize) -> bool {
        let (sc, sr) = self.start;
        let (ec, er) = self.end;
        let (c1, r1) = (sc.min(ec), sr.min(er));
        let (c2, r2) = (sc.max(ec), sr.max(er));
        c >= c1 && c <= c2 && r >= r1 && r <= r2
    }
}

/// Parse a spreadsheet-cell block id of the shape
/// `ss:<sheet>:c:<row>:<col>` into its components. Block ids that
/// don't match this shape (document blocks, comment threads, etc.)
/// return None. Used by the remote-cursor overlay to filter
/// awareness updates down to cells on the active sheet.
pub(super) fn parse_ss_block_id(s: &str) -> Option<(String, usize, usize)> {
    let rest = s.strip_prefix("ss:")?;
    let mut parts = rest.rsplitn(3, ':');
    let col: usize = parts.next()?.parse().ok()?;
    let row: usize = parts.next()?.parse().ok()?;
    // The middle marker is always "c" today; tolerate other markers
    // (`r` for row addresses, `h` for headers, etc.) since they
    // might land in awareness payloads from older clients — they
    // just won't match any cell and the overlay will skip them.
    let head = parts.next()?;
    // head is like "<sheet>:c" — split off the trailing marker.
    let sheet = head.rsplitn(2, ':').nth(1)?.to_string();
    Some((sheet, row, col))
}

/// Clamp a context-menu's preferred (left, top) so the menu stays
/// inside the viewport. Uses conservative menu-size estimates and
/// flips the menu up / left when the natural placement would push
/// it past the right or bottom edge. The shared menu chrome
/// (`components::menu`) clamps again from its own size estimate, so
/// this trigger-side clamp is a harmless first pass that keeps the
/// stored (x, y) sane for anything else that reads it.
pub(super) fn clamp_menu_position(x: f64, y: f64) -> (f64, f64) {
    let window = web_sys::window();
    let vw = window.as_ref()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(1024.0);
    let vh = window.as_ref()
        .and_then(|w| w.inner_height().ok())
        .and_then(|v| v.as_f64())
        .unwrap_or(768.0);
    clamp_menu_position_in_viewport(x, y, vw, vh)
}

/// Apply a toolbar formatting command to every cell across one or
/// more selection rects. Returns `true` if the engine was touched,
/// `false` for commands that don't map to a cell style (document-
/// only commands like `SetParagraph`, `InsertHorizontalRule`, etc.).
///
/// Toggle commands (Bold / Italic / Underline / Strike) compute
/// `all` across the *union* of every rect — so a multi-region
/// selection toggles consistently rather than per-region. This
/// matters once Excel-style non-contiguous selection (#59) is in
/// play: ctrl-clicking three cells and pressing Ctrl+B should set
/// all three bold (or clear all three if all three were already
/// bold), not per-region toggle.
///
/// Color and format commands (TextColor / Highlight / NumberFormat)
/// treat an empty string as "clear back to default" — the picker's
/// "clear" actions send `String::new()`.
pub(super) fn apply_toolbar_command_to_selection(
    eng: &mut crate::spreadsheet::eval::SpreadsheetEngine,
    cmd: &crate::components::toolbar::ToolbarCommand,
    sels: &[(usize, usize, usize, usize)],
) -> bool {
    use crate::components::toolbar::ToolbarCommand;
    fn all_with<F: Fn(&crate::spreadsheet::eval::CellStyle) -> bool>(
        eng: &crate::spreadsheet::eval::SpreadsheetEngine,
        sels: &[(usize, usize, usize, usize)],
        pred: F,
    ) -> bool {
        sels.iter().all(|&(r1, c1, r2, c2)| {
            (r1..=r2).all(|r| (c1..=c2).all(|c| {
                eng.get_style((c, r)).map_or(false, |s| pred(s))
            }))
        })
    }
    fn for_each<F: FnMut(&mut crate::spreadsheet::eval::CellStyle)>(
        eng: &mut crate::spreadsheet::eval::SpreadsheetEngine,
        sels: &[(usize, usize, usize, usize)],
        mut apply: F,
    ) {
        for &(r1, c1, r2, c2) in sels {
            for r in r1..=r2 {
                for c in c1..=c2 { apply(eng.style_mut((c, r))); }
            }
        }
    }
    match cmd {
        ToolbarCommand::ToggleBold => {
            let all = all_with(eng, sels, |s| s.bold);
            for_each(eng, sels, |s| s.bold = !all);
            true
        }
        ToolbarCommand::ToggleItalic => {
            let all = all_with(eng, sels, |s| s.italic);
            for_each(eng, sels, |s| s.italic = !all);
            true
        }
        ToolbarCommand::ToggleUnderline => {
            let all = all_with(eng, sels, |s| s.underline);
            for_each(eng, sels, |s| s.underline = !all);
            true
        }
        ToolbarCommand::ToggleStrike => {
            let all = all_with(eng, sels, |s| s.strike);
            for_each(eng, sels, |s| s.strike = !all);
            true
        }
        ToolbarCommand::ToggleTextColor(hex) => {
            let val = if hex.is_empty() { None } else { Some(hex.clone()) };
            for_each(eng, sels, |s| s.text_color = val.clone());
            true
        }
        ToolbarCommand::ToggleHighlight(hex) => {
            let val = if hex.is_empty() { None } else { Some(hex.clone()) };
            for_each(eng, sels, |s| s.bg_color = val.clone());
            true
        }
        ToolbarCommand::SetNumberFormat(key) => {
            let val = if key.is_empty() { None } else { Some(key.clone()) };
            for_each(eng, sels, |s| s.number_format = val.clone());
            true
        }
        _ => false, // document-only commands: ignore
    }
}

/// Pure-math half of `clamp_menu_position`: given a desired
/// `(x, y)` anchor and a viewport `(vw, vh)`, return the clamped
/// anchor. Extracted so it's unit-testable without the wasm-bindgen
/// shim that `web_sys::window()` requires (cargo-test outside WASM
/// can't resolve those JS imports).
pub(super) fn clamp_menu_position_in_viewport(
    x: f64, y: f64, vw: f64, vh: f64,
) -> (f64, f64) {
    const MENU_W: f64 = 250.0;
    const MENU_H: f64 = 320.0;
    const EDGE: f64 = 4.0;
    // Flip left when the natural right edge would clip; clamp to
    // EDGE so a flip past the left edge doesn't push the menu
    // out the other side either.
    let cx = if x + MENU_W > vw - EDGE { (x - MENU_W).max(EDGE) } else { x };
    let cy = if y + MENU_H > vh - EDGE { (y - MENU_H).max(EDGE) } else { y };
    (cx, cy)
}

/// Walk up from a mouse event's target to the nearest data cell and
/// read its `(row, col)`. Returns `None` when the event did not land on
/// a `.spreadsheet-cell` (header gutter, formula bar, blank canvas) — or
/// when a descendant that `stop_propagation()`'d kept the event from
/// reaching the delegated wrapper handler at all. Mirrors the
/// `closest("[data-row][data-col]")` idiom the touch handlers already
/// use; the data cell render stamps both attributes (#72: the four cell
/// mouse handlers are delegated to the wrapper, so per-cell render no
/// longer wires ~4 listeners × thousands of cells).
fn cell_rc_from_mouse_event(e: &web_sys::MouseEvent) -> Option<(usize, usize)> {
    let target = e.target()?.dyn_into::<web_sys::Element>().ok()?;
    let el = target.closest("[data-row][data-col]").ok().flatten()?;
    let r = el.get_attribute("data-row")?.parse::<usize>().ok()?;
    let c = el.get_attribute("data-col")?.parse::<usize>().ok()?;
    Some((r, c))
}

// ─── Component ─────────────────────────────────────────────────

#[component]
pub fn SpreadsheetView(
    editor_state: ReadSignal<Option<EditorState>>,
    on_state_change: Callback<EditorState>,
    /// Content-changed ping, fired after `on_state_change` has published
    /// the new state. The parent decides whether/when to serialize —
    /// #121: the yrs encode used to happen here per commit and was
    /// discarded whenever the WebSocket was handling persistence, so the
    /// consumer now encodes lazily from `editor_state` at save time.
    on_change: Callback<()>,
    /// The id of the document this view is editing. Plumbed into
    /// the engine so REFERENCERANGE / REFERENCESHEET can short-
    /// circuit self-references and so the fetch loop can skip the
    /// current doc when batching cross-doc fetches.
    doc_id: String,
    /// Invoked once per foreign doc id this view starts using —
    /// the page wires this to `CollabClient::subscribe_foreign_doc`
    /// over the existing WebSocket so server-pushed updates from
    /// the foreign doc invalidate this view's cache.
    on_subscribe_foreign: Callback<String>,
    /// Invoked when a foreign doc id is no longer referenced by
    /// any formula (GC). Wires to `CollabClient::unsubscribe_foreign_doc`.
    on_unsubscribe_foreign: Callback<String>,
    /// Pulses with the foreign doc id whose CRDT just advanced via
    /// the WS push channel. The view invalidates its engine cache
    /// for that id, which triggers a refetch on the next recompute.
    foreign_doc_invalidate: ReadSignal<Option<String>>,
    /// Toolbar command pulses (Bold / Italic / Underline / colors,
    /// etc.). The page-level signal is shared with the document
    /// editor; this view applies the formatting subset that maps to
    /// cell styles, ignoring document-only commands like
    /// `SetHeading`. Cleared after consumption so the same command
    /// can fire again.
    toolbar_command: ReadSignal<Option<crate::components::toolbar::ToolbarCommand>>,
    set_toolbar_command: WriteSignal<Option<crate::components::toolbar::ToolbarCommand>>,
    /// Fired whenever the active cell moves. The page-level handler
    /// translates `(sheet, row, col)` into a presence-awareness
    /// `cursor_block` (synthetic block-id `ss:<sheet>:c:<r>:<c>`)
    /// and dispatches it through `CollabClient::send_awareness`,
    /// so other connected clients can render this user's cell
    /// position. Wired separately from the document editor's
    /// presence path because the spreadsheet view doesn't write
    /// the selection back into `editor_state`.
    on_cell_cursor: Callback<(String, usize, usize)>,
    /// Remote users' cursors as published over WebSocket awareness.
    /// The spreadsheet view filters to entries whose `cursor_block`
    /// matches the active sheet and renders an outline + name tag
    /// on the matching cell.
    remote_cursors: ReadSignal<Vec<crate::collab::ws_client::RemoteCursor>>,
    /// When false, remote cursors are not rendered (View → Show Cursors
    /// toggle, #99). Mirrors the document editor's CursorOverlay gate.
    #[prop(into)] cursors_enabled: Signal<bool>,
    /// Threaded cell-comment opener. The spreadsheet pre-creates
    /// the thread (and migrates legacy `comment` text into the
    /// first message when present), then fires this so the page
    /// opens its CommentPopup at the cell's screen position.
    /// See `CellCommentOpen` for the payload shape.
    on_open_cell_comment: Callback<CellCommentOpen>,
    /// Cell-anchored comment threads for the open document, keyed by the
    /// page from its `list_threads` fetch. Drives the thread-aware hover
    /// preview (opening message + reply count) and the click-to-open
    /// marker on commented cells. Refreshes with the page's
    /// `comments_dirty` signal, so a peer's reply updates the preview
    /// without a reload. See `CellThreadInfo`.
    cell_threads: ReadSignal<Vec<CellThreadInfo>>,
    /// Deep-link target from a comment notification. When set, the view
    /// switches to the named sheet, selects + scrolls the cell into view,
    /// and opens its comment popup. See `CellFocus`.
    focus_cell: ReadSignal<Option<CellFocus>>,
) -> impl IntoView {
    // Active cell (cursor)
    let (active_row, set_active_row) = signal(0usize);
    let (active_col, set_active_col) = signal(0usize);
    // Selection anchor (start of drag/shift-select)
    let (sel_row, set_sel_row) = signal(0usize);
    let (sel_col, set_sel_col) = signal(0usize);
    // Excel-style non-contiguous selection (issue #59). Each entry is
    // a `(r1, c1, r2, c2)` rect in addition to the primary rect
    // defined by (sel_row, sel_col) ↔ (active_row, active_col).
    // Ctrl-click on a cell saves the current primary rect into this
    // list and starts a fresh single-cell primary; a plain or
    // shift-click clears it. Cell-level operations that should treat
    // the whole multi-region as a unit (delete, formatting) walk
    // both primary + extras; range-level ones that don't have a
    // sensible multi-region semantic (sort, copy/paste, charts) keep
    // operating on just the primary rect.
    let (extra_sel_regions, set_extra_sel_regions) =
        signal::<Vec<(usize, usize, usize, usize)>>(Vec::new());
    // Whether mouse is dragging a selection
    let (dragging, set_dragging) = signal(false);

    let (editing, set_editing) = signal(false);
    let (edit_value, set_edit_value) = signal(String::new());
    // Mobile tap-and-hold range selection. True after a long-press fires;
    // the next touch-synthesized mousedown then becomes the range-end tap.
    let (touch_anchor_active, set_touch_anchor_active) = signal(false);
    // True between touchstart and touchend; used to suppress the desktop
    // context menu when the OS dispatches a `contextmenu` event from the
    // same long-press gesture that arms our anchor.
    let (touch_in_progress, set_touch_in_progress) = signal(false);
    // True once a touchmove has crossed into a different cell — switches
    // the gesture from "potential tap/long-press" to "drag-extend
    // selection" and suppresses iOS mouse-event synthesis via
    // preventDefault so the touchend doesn't reset our selection.
    let (touch_dragging, set_touch_dragging) = signal(false);
    // Touch-primary devices get the in-page mobile keyboard whenever a
    // cell is being edited (M-P3 piece C). matchMedia is read once —
    // desktop with a touchscreen is the only edge case and is fine
    // either way. The mode signal is the single source of truth for
    // (a) which layout `FormulaKeyboard` renders and (b) whether the
    // cell input suppresses the OS soft keyboard via `inputmode="none"`.
    let touch_primary = is_touch_primary();
    let (kb_mode, set_kb_mode) = signal(KeyboardMode::Standard);
    let formula_keyboard_visible = Signal::derive(move || {
        touch_primary && editing.get()
    });
    let effective_kb_mode = Signal::derive(move || {
        KeyboardMode::auto_for(&edit_value.get()).unwrap_or_else(|| kb_mode.get())
    });
    let (grid_version, set_grid_version) = signal(0u32);
    // #129: set true by `persist()` immediately before it emits its
    // `editor_state` change, consumed (and cleared) by the very next run
    // of the engine-resync Effect. `persist()` builds the doc *from* the
    // engine, so re-parsing it back in is a redundant O(rows×cols) pass;
    // this flag lets that one Effect invocation skip it. Remote updates
    // and sheet switches never set the flag, so they still resync.
    let (persist_origin, set_persist_origin) = signal(false);
    let cell_input_ref = NodeRef::<leptos::html::Input>::new();
    // While building a formula reference via mouse or keyboard. None when
    // not in pick mode; Some when the user has typed a trigger character
    // (=, (, ,, operator) and started selecting a cell/range.
    let (ref_pick, set_ref_pick) = signal::<Option<RefPick>>(None);
    let (grid_rows, set_grid_rows) = signal(10usize);
    let (grid_cols, set_grid_cols) = signal(10usize);
    let wrapper_ref = NodeRef::<leptos::html::Div>::new();
    // Per-column widths (matches initial grid_cols)
    let (col_widths, set_col_widths) = signal::<Vec<f64>>(vec![80.0; 10]);
    // Context menu state
    let (ctx_menu_visible, set_ctx_menu_visible) = signal(false);
    let (ctx_menu_x, set_ctx_menu_x) = signal(0.0f64);
    let (ctx_menu_y, set_ctx_menu_y) = signal(0.0f64);
    // Freeze state: number of frozen rows/cols
    let (frozen_rows, set_frozen_rows) = signal(0usize);
    let (frozen_cols, set_frozen_cols) = signal(0usize);
    // Sheet state
    let (active_sheet, set_active_sheet) = signal(0usize);
    let (sheet_names, set_sheet_names) = signal::<Vec<String>>(vec![DEFAULT_SHEET_NAME.to_string()]);
    // Sort state
    // Multi-column sort. Empty chain = no active sort. Each entry is
    // `(col_index, ascending)`. Earlier entries are higher-priority
    // (primary key first; ties broken by subsequent keys).
    let (sort_keys, set_sort_keys) = signal::<Vec<(usize, bool)>>(Vec::new());
    // Filter state: per-column set of hidden values
    let (filter_col, set_filter_col) = signal::<Option<usize>>(None);
    let (hidden_rows, set_hidden_rows) = signal::<std::collections::HashSet<usize>>(std::collections::HashSet::new());
    // Find/replace state
    let (find_visible, set_find_visible) = signal(false);
    let (find_query, set_find_query) = signal(String::new());
    let (find_matches, set_find_matches) = signal::<Vec<(usize, usize)>>(Vec::new());
    let (find_index, set_find_index) = signal(0usize);
    let (replace_text, set_replace_text) = signal(String::new());
    // Pivot editor state. None when closed; Some(anchor) when the
    // editor is open for the pivot anchored at that cell. Set by
    // the "Insert Pivot Table" context-menu action; cleared by the
    // editor's close button or the Delete-pivot action.
    let (pivot_editor_open, set_pivot_editor_open) = signal::<Option<(usize, usize)>>(None);
    // Which filter chip's popover is expanded inside the pivot editor.
    // Lifted to spreadsheet-view scope so the cell-click path (when
    // the user clicks a page-field filter cell in the pivot output)
    // can open the editor AND auto-expand the relevant filter
    // popover. The editor itself reads/writes this signal through the
    // pair we hand into render_pivot_editor.
    let (pivot_filter_popover_open, set_pivot_filter_popover_open) = signal::<Option<usize>>(None);
    // Which row/col group's "visible values" picker is open. The
    // first usize is the axis (0 = row group, 1 = col group), the
    // second is the group index within that axis. Set when the user
    // clicks a row-/col-header field-name cell in the rendered
    // pivot output (the cells with the trailing "▾" glyph).
    let (pivot_group_picker_open, set_pivot_group_picker_open) =
        signal::<Option<(u8, usize)>>(None);
    // Sort dialog state. None when closed; Some(ctx) when open with
    // the seed context (initial keys / range / has-headers). The
    // dialog itself maintains in-flight signals for the user's
    // edits; ctx is just the entry-time defaults.
    let (sort_dialog_open, set_sort_dialog_open) = signal::<Option<SortDialogContext>>(None);
    // Format painter state. `None` = idle. `Some((style, sticky))` =
    // armed; the next cell click stamps `style` onto the target. When
    // `sticky` is false the painter resets to idle after one stamp;
    // true keeps painting until Esc or another button click.
    let (format_painter, set_format_painter) =
        signal::<Option<(crate::spreadsheet::eval::CellStyle, bool)>>(None);
    // Autocomplete state
    let (ac_visible, set_ac_visible) = signal(false);
    let (ac_matches, set_ac_matches) = signal::<Vec<(&'static str, &'static str)>>(Vec::new());
    let (ac_index, set_ac_index) = signal(0usize);
    // Column resize drag state
    let (resize_col, set_resize_col) = signal::<Option<usize>>(None);
    let (resize_start_x, set_resize_start_x) = signal(0.0f64);
    let (resize_start_w, set_resize_start_w) = signal(0.0f64);
    // Row resize drag state
    let (resize_row, set_resize_row) = signal::<Option<usize>>(None);
    let (resize_start_y, set_resize_start_y) = signal(0.0f64);
    let (resize_start_h, set_resize_start_h) = signal(0.0f64);
    // Per-row heights (default 24px each)
    let (row_heights, set_row_heights) = signal::<Vec<f64>>(vec![24.0; 100]);
    // Hidden columns
    let (hidden_cols, set_hidden_cols) = signal::<std::collections::HashSet<usize>>(std::collections::HashSet::new());

    // ─── Row virtualization (#72) ───────────────────────────
    // The tbody renders only the rows near the viewport (plus frozen
    // rows and an overscan margin); spacer rows preserve the scroll
    // geometry. `scroll_top`/`viewport_h` mirror the
    // `.spreadsheet-grid-container` scroll state (updated by its
    // on:scroll handler and a mount Effect); `visible_rows` derives the
    // [start, end) window. The Memo dedupes by value, so per-pixel
    // scroll events only re-render the grid when the window actually
    // shifts by a row.
    let grid_scroll_ref = NodeRef::<leptos::html::Div>::new();
    let (scroll_top, set_scroll_top) = signal(0.0f64);
    let (viewport_h, set_viewport_h) = signal(600.0f64);
    // The table is `border-collapse: collapse`, so the browser's actual
    // row pitch differs from the styled height by a constant per-row
    // overhead (shared borders). Spacer heights computed from styled
    // heights alone drift from reality as rows move in/out of the
    // spacers, which makes the total table height change per window
    // shift — the browser then clamps scrollTop and the view "jumps".
    // `row_pitch_extra` is that measured per-row delta; the Effect
    // below derives it from the rendered rows, and all row geometry
    // (window Memo, spacers, scroll-into-view) adds it per row.
    let (row_pitch_extra, set_row_pitch_extra) = signal(0.0f64);
    /// Rows rendered beyond each edge of the viewport so small scrolls
    /// reveal already-rendered cells instead of blank spacer.
    const VIRT_OVERSCAN_ROWS: usize = 12;
    /// Snap the window edges to this granularity so the Memo (and the
    /// grid re-render it triggers) only fires once per ~8 rows of
    /// scroll, not on every row boundary. Must stay below
    /// VIRT_OVERSCAN_ROWS so the snap never exposes blank spacer.
    const VIRT_SNAP_ROWS: usize = 8;
    let visible_rows: Memo<(usize, usize)> = Memo::new(move |_| {
        let total = grid_rows.get().max(100);
        // scrollTop counts the sticky 24px thead as content above the fold.
        let top = (scroll_top.get() - 24.0).max(0.0);
        let vh = viewport_h.get().max(100.0);
        let extra = row_pitch_extra.get();
        hidden_rows.with(|hr| {
            row_heights.with(|heights| {
                let mut y = 0.0_f64;
                let mut start = total;
                let mut end = total;
                for r in 0..total {
                    let h = if hr.contains(&r) {
                        0.0
                    } else {
                        heights.get(r).copied().unwrap_or(24.0) + extra
                    };
                    if start == total && y + h > top {
                        start = r;
                    }
                    y += h;
                    if y >= top + vh {
                        end = r + 1;
                        break;
                    }
                }
                if start == total {
                    start = total.saturating_sub(1);
                }
                let s = start.saturating_sub(VIRT_OVERSCAN_ROWS);
                let e = (end + VIRT_OVERSCAN_ROWS).min(total);
                (
                    s - (s % VIRT_SNAP_ROWS),
                    (e.div_ceil(VIRT_SNAP_ROWS) * VIRT_SNAP_ROWS).min(total),
                )
            })
        })
    });
    // Seed the viewport height once the scroll container mounts (the
    // signal's 600px default only covers the pre-mount render).
    Effect::new(move |_| {
        if let Some(el) = grid_scroll_ref.get() {
            let el: web_sys::HtmlElement = el.into();
            let h = el.client_height();
            if h > 0 {
                set_viewport_h.set(h as f64);
            }
        }
    });
    // Measure the actual per-row pitch overhead from the rendered rows
    // (last data row's bottom minus first's top vs the styled sum).
    // Skips frozen rows: `position: sticky` makes their bounding rect
    // reflect the stuck position, not the flow position. Effects run
    // after the DOM patch, so the rows queried here are current. The
    // epsilon guard makes this a fixed point — once the measured value
    // is stable, the signal stops changing and nothing re-renders.
    Effect::new(move |_| {
        let _ = grid_version.get();
        let _ = visible_rows.get();
        let heights = row_heights.get();
        let hr = hidden_rows.get();
        let Some(root) = grid_scroll_ref.get() else { return };
        let root: web_sys::HtmlElement = root.into();
        let Ok(rows) = root.query_selector_all(
            ".spreadsheet-grid tbody tr[data-row]:not(.frozen-row)"
        ) else { return };
        if rows.length() < 2 { return; }
        let as_el = |i: u32| rows.item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok());
        let (Some(first), Some(last)) = (as_el(0), as_el(rows.length() - 1)) else { return };
        let row_of = |el: &web_sys::Element| -> Option<usize> {
            el.get_attribute("data-row")?.parse::<usize>().ok()
        };
        let (Some(r1), Some(r2)) = (row_of(&first), row_of(&last)) else { return };
        if r2 <= r1 { return; }
        let actual = last.get_bounding_client_rect().bottom()
            - first.get_bounding_client_rect().top();
        let visible = (r1..=r2).filter(|r| !hr.contains(r));
        let n = visible.clone().count() as f64;
        let styled: f64 = visible
            .map(|r| heights.get(r).copied().unwrap_or(24.0))
            .sum();
        if n < 2.0 || styled <= 0.0 || actual <= 0.0 {
            return;
        }
        let extra = (actual - styled) / n;
        if (extra - row_pitch_extra.get_untracked()).abs() > 0.05 {
            set_row_pitch_extra.set(extra);
        }
    });

    // Component-scoped Mutexes. `Box::leak` gives us a `&'static`
    // that's `Copy` so closures share without explicit `Arc::clone`
    // — convenient when ~70 closures across this file capture the
    // engine handle. To keep mount/dismount memory bounded (issue
    // #4 finding 1), we register an `on_cleanup` for each leaked
    // box that reclaims it via `Box::from_raw`. This is `unsafe`
    // but precise: by the time `on_cleanup` fires Leptos has
    // already dropped every signal subscription and event-handler
    // closure that could deref these pointers.
    //
    // `engine` and `fetched_ids` are also `Box::leak`'d for `Copy`
    // closure ergonomics, BUT they're now reclaimed by `on_cleanup`
    // too (issue #4 finding 1). Three `spawn_local` futures across
    // this view + `context_menu.rs` capture `engine` by raw pointer
    // and suspend across an `.await`; freeing engine before they
    // resume would be a use-after-free. The `alive` flag below
    // synchronises the cleanup with those futures: cleanup stores
    // `false` BEFORE `Box::from_raw`, and every post-await engine
    // access gates on `alive.load() == true` first. The flag's
    // backing memory lives in an `Arc<AtomicBool>` — `Arc` keeps it
    // valid even after the SpreadsheetView component drops, so the
    // post-await check is itself safe.
    let alive: std::sync::Arc<std::sync::atomic::AtomicBool> =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let engine: &'static Mutex<SpreadsheetEngine> =
        Box::leak(Box::new(Mutex::new(SpreadsheetEngine::new())));
    {
        let mut eng = engine.lock().unwrap();
        eng.set_current_doc_id(doc_id.clone());
    }
    // Cross-doc fetch state. `consent_approved` is the set of foreign
    // doc-ids the user has OK'd this session. `consent_pending` holds
    // ids awaiting the next consent prompt (it's a Vec because the
    // modal lists them in insertion order). `fetched_ids` tracks
    // doc-ids we've already kicked an HTTP fetch for so the drain
    // loop doesn't double-spawn while the first request is in
    // flight.
    let (consent_approved, set_consent_approved) =
        signal::<std::collections::HashSet<String>>(std::collections::HashSet::new());
    let (consent_pending, set_consent_pending) = signal::<Vec<String>>(Vec::new());
    let fetched_ids: &'static Mutex<std::collections::HashSet<String>> =
        Box::leak(Box::new(Mutex::new(std::collections::HashSet::new())));

    // Reclaim the engine + fetched_ids leaks at unmount, gated on
    // the `alive` flag so any pending `spawn_local` future checks
    // it and bails out before touching the freed pointer. The flag
    // store + Box::from_raw run synchronously, so once cleanup has
    // run there's no window where a resumed future sees `alive ==
    // true` but engine pointer is dangling — within a single WASM
    // task, the future's resume point can't interleave the cleanup
    // body.
    let engine_addr = engine as *const _ as usize;
    let fetched_ids_addr = fetched_ids as *const _ as usize;
    let alive_for_cleanup = std::sync::Arc::clone(&alive);
    on_cleanup(move || {
        alive_for_cleanup.store(false, std::sync::atomic::Ordering::SeqCst);
        unsafe {
            drop(Box::from_raw(engine_addr as *mut Mutex<SpreadsheetEngine>));
            drop(Box::from_raw(fetched_ids_addr as *mut Mutex<std::collections::HashSet<String>>));
        }
    });

    // Undo/redo stacks: each entry is a vec of (addr, old_value, new_value)
    type UndoEntry = Vec<((usize, usize), String, String)>;
    let undo_stack: &'static Mutex<Vec<UndoEntry>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    let redo_stack: &'static Mutex<Vec<UndoEntry>> = Box::leak(Box::new(Mutex::new(Vec::new())));
    let undo_stack_addr = undo_stack as *const _ as usize;
    let redo_stack_addr = redo_stack as *const _ as usize;
    on_cleanup(move || unsafe {
        drop(Box::from_raw(undo_stack_addr as *mut Mutex<Vec<UndoEntry>>));
        drop(Box::from_raw(redo_stack_addr as *mut Mutex<Vec<UndoEntry>>));
    });

    // In-app clipboard sidecar. Rides alongside the OS clipboard so paste
    // from our own copy can translate relative refs (which plain TSV on the
    // OS clipboard can't encode). When the TSV we read back at paste time
    // matches what we cached at copy time, we use the structured cells
    // here; otherwise we fall back to verbatim TSV paste (for content
    // coming from Excel, the browser, etc.).
    //
    // `tsv` is the exact byte-for-byte string we wrote to the OS clipboard
    // at copy time; it's the gate that distinguishes "our copy" from
    // "external copy".
    //
    // `edit_serial_at_copy` is captured at cut/copy time. At paste time we
    // compare against the current `edit_serial` — if any other edit
    // happened in between, the cut marquee is "cancelled" (Excel
    // behavior) and we degrade Cut to Copy semantics.
    #[derive(Clone)]
    enum ClipMode { Copy, Cut }
    #[derive(Clone)]
    struct SheetClipboard {
        source_top_left: (usize, usize), // (col, row) anchor used for delta math
        cells: Vec<Vec<String>>,         // raw cell strings, row-major
        mode: ClipMode,
        tsv: String,
        edit_serial_at_copy: u64,
    }
    let clipboard: &'static Mutex<Option<SheetClipboard>> =
        Box::leak(Box::new(Mutex::new(None)));
    let edit_serial: &'static Mutex<u64> = Box::leak(Box::new(Mutex::new(0)));
    let clipboard_addr = clipboard as *const _ as usize;
    let edit_serial_addr = edit_serial as *const _ as usize;
    on_cleanup(move || unsafe {
        drop(Box::from_raw(clipboard_addr as *mut Mutex<Option<SheetClipboard>>));
        drop(Box::from_raw(edit_serial_addr as *mut Mutex<u64>));
    });
    // Source rect of a pending cut, as (c1, r1, c2, r2). Rendered as a
    // dashed outline on those cells. Cleared on cut-paste complete or
    // on any edit (which also invalidates the cut itself).
    let (cut_source, set_cut_source) =
        signal::<Option<(usize, usize, usize, usize)>>(None);

    // ─── Derived helpers ───────────────────────────────────

    let cell_ref_label = move || {
        format!("{}{}", col_to_letters(active_col.get()), active_row.get() + 1)
    };

    let cell_raw = move || {
        let _v = grid_version.get();
        engine.lock().unwrap().get_raw((active_col.get(), active_row.get())).to_string()
    };

    // ─── Persist helper ────────────────────────────────────

    let persist = move || {
        let eng = engine.lock().unwrap();
        let names = sheet_names.get_untracked();
        let sheet_idx = active_sheet.get_untracked();
        // #132: borrow the existing doc instead of cloning it. The old
        // `get_untracked()` cloned the whole EditorState and then
        // `.map(|s| s.doc.clone())` cloned the doc AGAIN — two O(doc) deep
        // copies per commit, both wasted since build_doc_with_sheets only
        // reads `&Node`. `with_untracked` hands us a borrow with no clone.
        let empty = crate::editor::model::Node::empty_doc();
        let doc = editor_state.with_untracked(|st| {
            let existing = st.as_ref().map(|s| &s.doc).unwrap_or(&empty);
            build_doc_with_sheets(
                existing, &eng, sheet_idx,
                grid_rows.get_untracked(), grid_cols.get_untracked(), &names,
            )
        });
        drop(eng);
        // #129: mark the upcoming editor_state change as engine-originated
        // so the resync Effect skips re-parsing this doc back into the
        // engine (the engine already holds exactly this state). Set before
        // the emit; the Effect runs after persist() returns and clears it.
        set_persist_origin.set(true);
        on_state_change.run(EditorState::create_default(doc));
        on_change.run(());
        set_grid_version.set(grid_version.get_untracked().wrapping_add(1));
        // Any persisted edit invalidates a pending cut (Excel cancels the
        // marching-ants marquee). Bump the serial so the next paste can
        // detect that something changed between cut and paste, and clear
        // the marquee from the view.
        *edit_serial.lock().unwrap() += 1;
        if cut_source.get_untracked().is_some() {
            set_cut_source.set(None);
        }
    };

    // ─── Deep-link: focus a cell from a comment notification (#50) ──
    // The page sets `focus_cell` (parsed from a cell thread's block id)
    // when a notification is opened. Switch to the sheet, select the cell,
    // then defer a scroll-into-view + comment-popup open until the grid has
    // re-rendered for the (possibly newly switched) sheet.
    Effect::new(move |_| {
        let Some(focus) = focus_cell.get() else { return };
        let sheet_count = sheet_names.get_untracked().len().max(1);
        set_active_sheet.set(focus.sheet.min(sheet_count - 1));
        set_active_row.set(focus.row);
        set_active_col.set(focus.col);
        set_sel_row.set(focus.row);
        set_sel_col.set(focus.col);
        let row = focus.row;
        let col = focus.col;
        let thread_id = focus.thread_id.clone();
        gloo_timers::callback::Timeout::new(80, move || {
            let (left, top) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| {
                    d.query_selector(&format!("[data-row=\"{row}\"][data-col=\"{col}\"]"))
                        .ok()
                        .flatten()
                })
                .map(|el| {
                    el.scroll_into_view_with_bool(true);
                    let rect = el.get_bounding_client_rect();
                    (rect.right(), rect.top())
                })
                .unwrap_or((200.0, 200.0));
            on_open_cell_comment.run(CellCommentOpen {
                thread_id: thread_id.clone(),
                block_id: thread_id.clone(),
                left,
                top,
            });
        })
        .forget();
    });

    // ─── Outgoing cursor presence ──────────────────────────
    //
    // Fires on every active-cell move so the page-level handler can
    // publish the user's position to other connected clients via
    // the WebSocket awareness channel. Reads `active_row`,
    // `active_col`, and `active_sheet` reactively; the doc-state
    // path doesn't carry spreadsheet selection, so without this
    // Effect remote viewers never see where the user is.
    Effect::new(move |_| {
        let r = active_row.get();
        let c = active_col.get();
        let sheet_idx = active_sheet.get();
        let names = sheet_names.get_untracked();
        let sheet = names.get(sheet_idx).cloned()
            .unwrap_or_else(|| DEFAULT_SHEET_NAME.to_string());
        on_cell_cursor.run((sheet, r, c));
    });

    // ─── Toolbar command bridge ─────────────────────────────
    //
    // The shared `toolbar_command` signal carries clicks from the
    // page-level Toolbar to whichever editor is mounted (document or
    // spreadsheet). For spreadsheets we apply the formatting subset
    // (bold / italic / underline / strike / text color / highlight /
    // number format) to the current selection via the pure helper
    // `apply_toolbar_command_to_selection` (testable without the
    // signal layer). After consumption the signal is cleared so the
    // same button can fire again.
    Effect::new(move |_| {
        let Some(cmd) = toolbar_command.get() else { return };
        let primary = sel_bounds(
            sel_row.get_untracked(), sel_col.get_untracked(),
            active_row.get_untracked(), active_col.get_untracked(),
        );
        let sels: Vec<(usize, usize, usize, usize)> = std::iter::once(primary)
            .chain(extra_sel_regions.get_untracked().into_iter())
            .collect();
        let handled = {
            let mut eng = engine.lock().unwrap();
            apply_toolbar_command_to_selection(&mut eng, &cmd, &sels)
        };
        if handled { persist(); }
        // Always clear the signal — even for ignored commands — so a
        // subsequent click of the SAME command (handled or not) still
        // fires the Effect. Without this, a second click on Bold would
        // see the same Some(ToggleBold) and not retrigger.
        set_toolbar_command.set(None);
    });

    // ─── Delete-sheet helper ────────────────────────────────
    //
    // Drops the sheet at `idx` from the document tree (preserving
    // every other table + non-table child), updates `sheet_names`,
    // clamps `active_sheet`, and pushes the new doc through the
    // editor-state callbacks. The Effect that watches `active_sheet`
    // re-syncs the engine for whichever sheet ends up active. Refuses
    // to delete the last remaining sheet — every doc must keep at
    // least one table.
    let delete_sheet = move |idx: usize| {
        let names = sheet_names.get_untracked();
        if names.len() <= 1 || idx >= names.len() { return; }
        let existing = editor_state.get_untracked().map(|s| s.doc.clone())
            .unwrap_or_else(crate::editor::model::Node::empty_doc);
        let doc = build_doc_dropping_sheet(&existing, idx);
        let mut new_names = names.clone();
        new_names.remove(idx);
        let active = active_sheet.get_untracked();
        let new_active = if active == idx {
            active.min(new_names.len().saturating_sub(1))
        } else if active > idx {
            active - 1
        } else {
            active
        };
        set_sheet_names.set(new_names);
        if new_active != active {
            set_active_sheet.set(new_active);
        }
        on_state_change.run(EditorState::create_default(doc));
        on_change.run(());
        set_grid_version.update(|v| *v = v.wrapping_add(1));
    };

    // ─── Row/Col insert/delete helpers ───────────────────────

    let insert_row_at = move |at: usize| {
        let rows = grid_rows.get_untracked();
        let cols = grid_cols.get_untracked();
        let mut eng = engine.lock().unwrap();
        for ri in (at..rows).rev() {
            for ci in 0..cols {
                let val = eng.get_raw((ci, ri)).to_string();
                let style = eng.get_style((ci, ri)).cloned();
                eng.set_cell((ci, ri + 1), &val);
                if let Some(s) = style { eng.set_style((ci, ri + 1), s); }
            }
        }
        for ci in 0..cols { eng.set_cell((ci, at), ""); }
        rewrite_formulas_after_axis_shift(&mut eng, rows + 1, cols, Axis::Row, at, 1);
        drop(eng);
        set_grid_rows.set(rows + 1);
        persist();
    };

    let delete_row_at = move |at: usize| {
        let rows = grid_rows.get_untracked();
        let cols = grid_cols.get_untracked();
        if rows <= 1 { return; }
        let mut eng = engine.lock().unwrap();
        for ri in at..(rows - 1) {
            for ci in 0..cols {
                let val = eng.get_raw((ci, ri + 1)).to_string();
                let style = eng.get_style((ci, ri + 1)).cloned();
                eng.set_cell((ci, ri), &val);
                if let Some(s) = style { eng.set_style((ci, ri), s); }
                else { eng.set_style((ci, ri), Default::default()); }
            }
        }
        for ci in 0..cols { eng.set_cell((ci, rows - 1), ""); }
        rewrite_formulas_after_axis_shift(&mut eng, rows - 1, cols, Axis::Row, at, -1);
        drop(eng);
        set_grid_rows.set(rows - 1);
        persist();
    };

    let insert_col_at = move |at: usize| {
        let rows = grid_rows.get_untracked();
        let cols = grid_cols.get_untracked();
        let mut eng = engine.lock().unwrap();
        for ci in (at..cols).rev() {
            for ri in 0..rows {
                let val = eng.get_raw((ci, ri)).to_string();
                let style = eng.get_style((ci, ri)).cloned();
                eng.set_cell((ci + 1, ri), &val);
                if let Some(s) = style { eng.set_style((ci + 1, ri), s); }
            }
        }
        for ri in 0..rows { eng.set_cell((at, ri), ""); }
        rewrite_formulas_after_axis_shift(&mut eng, rows, cols + 1, Axis::Col, at, 1);
        drop(eng);
        set_grid_cols.set(cols + 1);
        set_col_widths.update(|w| w.insert(at, 80.0));
        persist();
    };

    let delete_col_at = move |at: usize| {
        let rows = grid_rows.get_untracked();
        let cols = grid_cols.get_untracked();
        if cols <= 1 { return; }
        let mut eng = engine.lock().unwrap();
        for ci in at..(cols - 1) {
            for ri in 0..rows {
                let val = eng.get_raw((ci + 1, ri)).to_string();
                let style = eng.get_style((ci + 1, ri)).cloned();
                eng.set_cell((ci, ri), &val);
                if let Some(s) = style { eng.set_style((ci, ri), s); }
                else { eng.set_style((ci, ri), Default::default()); }
            }
        }
        for ri in 0..rows { eng.set_cell((cols - 1, ri), ""); }
        rewrite_formulas_after_axis_shift(&mut eng, rows, cols - 1, Axis::Col, at, -1);
        drop(eng);
        set_grid_cols.set(cols - 1);
        set_col_widths.update(|w| { if at < w.len() { w.remove(at); } });
        persist();
    };

    // ─── Sort helper ───────────────────────────────────────

    // Apply a chain of sort keys to all rows in the active sheet.
    // Earlier keys dominate; later keys are tiebreakers. Caller is
    // responsible for `set_sort_keys.set(...)` to update the visual
    // indicator after this returns.
    let sort_by_keys = move |keys: Vec<(usize, bool)>| {
        if keys.is_empty() { return; }
        let rows = grid_rows.get_untracked();
        let cols = grid_cols.get_untracked();
        let mut eng = engine.lock().unwrap();

        let mut row_data: Vec<Vec<(String, Option<crate::spreadsheet::eval::CellStyle>)>> = (0..rows)
            .map(|r| {
                (0..cols).map(|c| {
                    (eng.get_raw((c, r)).to_string(), eng.get_style((c, r)).cloned())
                }).collect()
            })
            .collect();

        row_data.sort_by(|a, b| compare_rows_by_keys(a, b, &keys));

        for (r, row) in row_data.iter().enumerate() {
            for (c, (val, style)) in row.iter().enumerate() {
                eng.set_cell((c, r), val);
                if let Some(s) = style { eng.set_style((c, r), s.clone()); }
                else { eng.set_style((c, r), Default::default()); }
            }
        }
        drop(eng);
        persist();
    };

    // Header-click sort: replaces the chain when called without
    // `extend`, or appends/flips when called with `extend = true`.
    // Used by the column-header click handler and by the context-menu
    // single-key items, which always replace.
    //
    // The `set_sort_keys` write fires BEFORE `sort_by_keys` so the
    // grid_version bump from `persist()` re-renders with the new
    // chain in one frame. Reverse order would briefly paint the old
    // chain before the second signal write triggered another render.
    let sort_by_column = move |col: usize, ascending: bool| {
        set_sort_keys.set(vec![(col, ascending)]);
        sort_by_keys(vec![(col, ascending)]);
    };

    // Range-aware multi-key sort. Drives the new Sort dialog (the
    // dialog passes a parsed range, the chain, and the user's "data
    // has headers" toggle). Rows OUTSIDE the range stay untouched;
    // when `skip_first_row` is true the first row of the range is
    // also untouched (typical when the range's first row is a
    // header).
    //
    // `keys` carries ABSOLUTE column indices (e.g., (3, true) means
    // col D ascending), not relative-to-range. That's what the
    // dialog naturally produces from its column dropdown and what
    // `compare_rows_by_keys` already expects.
    let sort_by_keys_in_range = move |
        range: ((usize, usize), (usize, usize)),
        keys: Vec<(usize, bool)>,
        skip_first_row: bool,
    | {
        if keys.is_empty() { return; }
        let ((c1, r1), (c2, r2)) = range;
        let start_row = if skip_first_row { r1 + 1 } else { r1 };
        if start_row > r2 || c1 > c2 { return; }
        let mut eng = engine.lock().unwrap();
        // Collect rows in [start_row, r2]. Each row is the FULL
        // grid-width tuple of (raw, style) so the comparator's
        // absolute column indices Just Work — but we only write
        // back the in-range columns [c1, c2] so cells outside the
        // selection are not disturbed.
        let cols_total = grid_cols.get_untracked().max(c2 + 1);
        let mut row_data: Vec<Vec<(String, Option<crate::spreadsheet::eval::CellStyle>)>> = (start_row..=r2)
            .map(|r| {
                (0..cols_total).map(|c| {
                    (eng.get_raw((c, r)).to_string(), eng.get_style((c, r)).cloned())
                }).collect()
            })
            .collect();

        row_data.sort_by(|a, b| compare_rows_by_keys(a, b, &keys));

        for (idx, row) in row_data.iter().enumerate() {
            let r = start_row + idx;
            for c in c1..=c2 {
                let (val, style) = row.get(c).cloned().unwrap_or_default();
                eng.set_cell((c, r), &val);
                if let Some(s) = style { eng.set_style((c, r), s); }
                else { eng.set_style((c, r), Default::default()); }
            }
        }
        drop(eng);
        persist();
    };

    // ─── Re-focus the grid wrapper so keydown events keep working ───

    let refocus_wrapper = move || {
        if let Some(el) = wrapper_ref.get() {
            let html: web_sys::HtmlElement = el.into();
            let _ = html.focus();
        }
    };

    // ─── Document-level Ctrl+A ─────────────────────────────
    //
    // The wrapper-bound on:keydown only fires when the wrapper or one of
    // its descendants has focus. Focus drifts (the user clicks the
    // sidebar, scrolls, dismisses a dialog) leave the body focused, and
    // a wrapper handler can't see Ctrl+A from there — the browser's
    // default "select all page text" wins. Listening on the document
    // catches the keystroke regardless of where focus lives, then we
    // gate it on the wrapper being mounted and editing being inactive.
    //
    // INVARIANT: this Effect must run exactly once per SpreadsheetView
    // mount, otherwise we register duplicate listeners. To preserve that,
    // do NOT add any reactive `.get()` reads to the Effect body — only
    // `get_untracked()` inside the closure. Adding a tracked read here
    // would re-run the Effect on every signal change and re-register.
    Effect::new(move |_: Option<()>| {
        let Some(window) = web_sys::window() else { return };
        let Some(document) = window.document() else { return };
        let doc_for_handler = document.clone();

        let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |e: web_sys::KeyboardEvent| {
                let ctrl = e.ctrl_key() || e.meta_key();
                if !ctrl || e.shift_key() || e.alt_key() {
                    return;
                }
                if e.key().to_lowercase() != "a" {
                    return;
                }
                // While editing a cell, Ctrl+A means "select all text in the
                // input" — let the browser handle it.
                if editing.get_untracked() {
                    return;
                }
                // Only intercept when the spreadsheet wrapper is in this
                // document. Without it (we're on a non-spreadsheet page),
                // bail so other components' Ctrl+A handlers stay in charge.
                let Some(wrapper_el) = wrapper_ref.get_untracked() else { return };
                let wrapper_node: web_sys::HtmlElement = wrapper_el.into();
                // If focus sits inside an input/textarea OUTSIDE the
                // spreadsheet (find dialog, share modal, etc.), don't
                // hijack — the user is editing text there.
                if let Some(active) = document_active_element(&doc_for_handler) {
                    let is_text_input = matches!(
                        active.tag_name().to_uppercase().as_str(),
                        "INPUT" | "TEXTAREA"
                    );
                    if is_text_input && !wrapper_node.contains(Some(&active)) {
                        return;
                    }
                }
                e.prevent_default();
                let last_row = grid_rows.get_untracked().saturating_sub(1);
                let last_col = grid_cols.get_untracked().saturating_sub(1);
                set_sel_row.set(0);
                set_sel_col.set(0);
                set_active_row.set(last_row);
                set_active_col.set(last_col);
                // Ctrl+A replaces any multi-region selection (#59).
                set_extra_sel_regions.set(Vec::new());
                // Re-focus so subsequent keys (Delete, formatting) flow
                // back through the wrapper handler as expected.
                let _ = wrapper_node.focus();
            }
        ) as Box<dyn Fn(web_sys::KeyboardEvent)>);

        let cb = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        let _ = document.add_event_listener_with_callback("keydown", &cb);
        // The wasm-bindgen Closure holds a Box<dyn Fn> which is !Send, but
        // leptos's on_cleanup requires Send + 'static. We can't move the
        // Closure into the cleanup capture, so leak it via forget() to
        // keep the JS-side reference valid for as long as the listener
        // lives. removeEventListener still detaches the listener on
        // unmount; only the small heap allocation is leaked. For typical
        // session-bounded mount/unmount counts the cumulative leak is
        // sub-KB. Revisit (with send_wrapper or a StoredValue holder) if
        // this view starts mounting/unmounting at high frequency.
        closure.forget();
        on_cleanup(move || {
            let _ = document.remove_event_listener_with_callback("keydown", &cb);
        });
    });

    // ─── Record undo entry ───────────────────────────────

    let do_redo = move || {
        let entry = redo_stack.lock().unwrap().pop();
        if let Some(entry) = entry {
            let mut eng = engine.lock().unwrap();
            let mut undo_entry = Vec::new();
            for ((c, r), _old, new_val) in &entry {
                let current = eng.get_raw((*c, *r)).to_string();
                eng.set_cell((*c, *r), new_val);
                undo_entry.push(((*c, *r), current, new_val.clone()));
            }
            drop(eng);
            undo_stack.lock().unwrap().push(undo_entry);
            persist();
        }
    };

    let record_undo = move |entries: Vec<((usize, usize), String, String)>| {
        if entries.iter().all(|(_, old, new)| old == new) { return; }
        undo_stack.lock().unwrap().push(entries);
        redo_stack.lock().unwrap().clear();
    };

    // ─── Commit edit ───────────────────────────────────────

    let commit_edit = move || {
        let mut val = edit_value.get_untracked();
        // Auto-close unbalanced parens on formula commit (Excel behavior):
        // `=SUM(A1:A3` + Enter → `=SUM(A1:A3)`. Counts parens outside string
        // literals so `="Hello("` stays as-typed.
        let missing = missing_close_parens(&val);
        if missing > 0 {
            val.push_str(&")".repeat(missing));
        }
        let c = active_col.get_untracked();
        let r = active_row.get_untracked();

        // Enforce data validation rules before accepting
        {
            use crate::spreadsheet::eval::ValidationRule;
            let eng = engine.lock().unwrap();
            if let Some(style) = eng.get_style((c, r)) {
                if let Some(ref rule) = style.validation {
                    let valid = match rule {
                        ValidationRule::Number { min, max } => {
                            if val.trim().is_empty() {
                                true // allow clearing
                            } else if let Ok(n) = val.parse::<f64>() {
                                min.map_or(true, |m| n >= m) && max.map_or(true, |m| n <= m)
                            } else {
                                false
                            }
                        }
                        ValidationRule::Dropdown(options) => {
                            val.trim().is_empty() || options.iter().any(|o| o == val.trim())
                        }
                        ValidationRule::Checkbox => true, // always valid (toggled by click)
                    };
                    if !valid {
                        web_sys::console::warn_1(&format!("Validation failed for cell ({c},{r}): {val}").into());
                        set_editing.set(false);
                        refocus_wrapper();
                        return; // reject the edit
                    }
                }
            }
        }

        let old = {
            let mut eng = engine.lock().unwrap();
            let old = eng.get_raw((c, r)).to_string();
            eng.set_cell((c, r), &val);
            old
        };
        record_undo(vec![((c, r), old, val)]);
        set_editing.set(false);
        set_ref_pick.set(None); // commit clears any in-progress pick
        persist();
        refocus_wrapper();
    };

    // ─── Navigation ────────────────────────────────────────

    let scroll_active_into_view = move || {
        // #72: with row virtualization the active cell may not be in the
        // DOM at all (it could lie outside the rendered window), so the
        // old nth-child + scrollIntoView approach can't work. Compute the
        // cell's content-box position arithmetically from the row/col
        // geometry and adjust the container's scroll offsets "nearest"-
        // style: scroll only if the cell is occluded by the sticky
        // header/frozen panes or past the viewport edge. Setting
        // scrollTop fires the container's on:scroll, which shifts the
        // virtualization window and renders the cell.
        let Some(container) = grid_scroll_ref.get_untracked() else { return };
        let el: web_sys::HtmlElement = container.into();
        let r = active_row.get_untracked();
        let c = active_col.get_untracked();
        let hr = hidden_rows.get_untracked();
        let hc = hidden_cols.get_untracked();
        let heights = row_heights.get_untracked();
        let widths = col_widths.get_untracked();
        let pitch_extra = row_pitch_extra.get_untracked();
        let row_px = |rr: usize| if hr.contains(&rr) {
            0.0
        } else {
            heights.get(rr).copied().unwrap_or(24.0) + pitch_extra
        };
        let col_px = |cc: usize| if hc.contains(&cc) {
            0.0
        } else {
            widths.get(cc).copied().unwrap_or(80.0)
        };
        let frozen_n = frozen_rows.get_untracked();
        let frozen_c_n = frozen_cols.get_untracked();

        // Vertical: content y of the row top = 24px sticky thead + rows above.
        if r >= frozen_n {
            let y: f64 = 24.0 + (0..r).map(row_px).sum::<f64>();
            let h = row_px(r);
            let frozen_px: f64 = (0..frozen_n).map(row_px).sum();
            let st = el.scroll_top() as f64;
            let ch = el.client_height() as f64;
            if y < st + 24.0 + frozen_px {
                el.set_scroll_top((y - 24.0 - frozen_px).max(0.0) as i32);
            } else if y + h > st + ch {
                el.set_scroll_top(((y + h) - ch).ceil() as i32);
            }
        }
        // Horizontal: content x = 40px sticky row-header gutter + cols left.
        if c >= frozen_c_n {
            let x: f64 = 40.0 + (0..c).map(col_px).sum::<f64>();
            let w = col_px(c);
            let frozen_w: f64 = (0..frozen_c_n).map(col_px).sum();
            let sl = el.scroll_left() as f64;
            let cw = el.client_width() as f64;
            if x < sl + 40.0 + frozen_w {
                el.set_scroll_left((x - 40.0 - frozen_w).max(0.0) as i32);
            } else if x + w > sl + cw {
                el.set_scroll_left(((x + w) - cw).ceil() as i32);
            }
        }
    };

    let move_active = move |dr: i32, dc: i32, extend: bool| {
        let r = (active_row.get_untracked() as i32 + dr).max(0) as usize;
        let c = (active_col.get_untracked() as i32 + dc).max(0) as usize;
        set_active_row.set(r);
        set_active_col.set(c);
        if !extend {
            set_sel_row.set(r);
            set_sel_col.set(c);
        }
        // Arrow-key navigation collapses any multi-region selection
        // back to the primary rect (#59). Excel does the same.
        set_extra_sel_regions.set(Vec::new());
        // Auto-expand grid when cursor nears the edge
        if r + 5 >= grid_rows.get_untracked() {
            set_grid_rows.set(grid_rows.get_untracked() + 20);
        }
        if c + 3 >= grid_cols.get_untracked() {
            set_grid_cols.set(grid_cols.get_untracked() + 10);
            set_col_widths.update(|w| {
                while w.len() < grid_cols.get_untracked() { w.push(80.0); }
            });
        }
        scroll_active_into_view();
    };

    let select_cell = move |r: usize, c: usize, extend: bool| {
        set_active_row.set(r);
        set_active_col.set(c);
        if !extend {
            set_sel_row.set(r);
            set_sel_col.set(c);
        }
        // Any non-ctrl-click selection mutator clears the
        // non-contiguous extras (#59). The ctrl-click path bypasses
        // this helper and manages extras itself.
        set_extra_sel_regions.set(Vec::new());
        set_editing.set(false);
        refocus_wrapper();
    };

    // ─── Tap-and-hold tracking ──────────────────────────────────────
    // Holds the (row, col) of the cell currently being long-pressed so the
    // tracker callback (which has no access to the touchstart event) can
    // resolve the target cell when it fires. Reset on every touchstart.
    let pending_anchor_cell: Rc<RefCell<Option<(usize, usize)>>> =
        Rc::new(RefCell::new(None));

    let touch_tracker = {
        let pending = Rc::clone(&pending_anchor_cell);
        LongPressTracker::new(LONG_PRESS_MS, TOUCH_MOVE_THRESHOLD_PX, move || {
            if let Some((r, c)) = *pending.borrow() {
                select_cell(r, c, false);
                set_touch_anchor_active.set(true);
            }
        })
    };

    // ─── Auto-focus wrapper on mount so arrow keys work immediately ──

    Effect::new(move |_| {
        refocus_wrapper();
    });

    // ─── Focus cell input when editing ─────────────────────

    Effect::new(move |_| {
        if editing.get() {
            if let Some(input) = cell_input_ref.get() {
                let el: web_sys::HtmlInputElement = input.into();
                let _ = el.focus();
                let len = el.value().len() as u32;
                let _ = el.set_selection_range(len, len);
            }
        }
    });

    // ─── Sync engine + derived signals from editor_state ──
    //
    // Runs as an Effect rather than inline in the grid render closure: doing
    // a mutex lock and signal writes during a reactive render would re-enter
    // the same closure (which reads hidden_rows / grid_rows via .get()) while
    // the engine lock is still held, panicking on recursive mutex acquire.
    //
    // Deps: editor_state, active_sheet (read tracked). Intentionally NOT
    // subscribing to grid_version — we *write* it at the bottom as an
    // unconditional "engine is now fresh; re-render the grid" signal.
    // If we also read it, we'd loop.
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        let sheet = active_sheet.get();

        // #129: skip the resync for the editor_state change `persist()`
        // just emitted — the engine already holds this exact state, all
        // engine-authoritative signals (sheet_names, hidden/frozen rows
        // and cols, grid extent) were set directly by the operation that
        // called persist(), and persist() bumped grid_version itself.
        // Read untracked + clear so this consumes only the one invocation
        // the flag was set for; remote updates and sheet switches (flag
        // false) fall through to the full resync below. Both `.get()`s
        // above run first so the editor_state/active_sheet subscriptions
        // are preserved regardless of this early return.
        if persist_origin.get_untracked() {
            set_persist_origin.set(false);
            return;
        }

        let names = extract_sheet_names(&state.doc);
        if names != sheet_names.get_untracked() {
            set_sheet_names.set(names.clone());
        }

        // Snapshot every sheet in the doc so cross-sheet refs
        // (`Sheet2!B2`) can resolve. The active sheet is also in
        // here, but the engine prefers its in-memory state for
        // refs targeting its own name — see `resolve_sheet_cell`.
        let local_snap = snapshot_foreign_doc(&state.doc);
        let active_sheet_name = names.get(sheet).cloned()
            .unwrap_or_else(|| DEFAULT_SHEET_NAME.to_string());

        let (doc_rows, doc_cols, hr, hc, fr, fc) = {
            let mut eng = engine.lock().unwrap();
            let (r, c) = sync_engine_from_doc_sheet(&mut eng, &state, sheet);
            eng.set_active_sheet_name(active_sheet_name);
            // Installing the snapshot also re-evaluates formulas
            // so any `=Sheet2!B2` cells in this sheet pick up the
            // current values of the foreign sheet immediately.
            eng.set_local_sheets_snapshot(local_snap);
            (r, c, eng.hidden_rows.clone(), eng.hidden_cols.clone(),
             eng.frozen_rows, eng.frozen_cols)
        };

        if hr != hidden_rows.get_untracked() { set_hidden_rows.set(hr); }
        if hc != hidden_cols.get_untracked() { set_hidden_cols.set(hc); }
        // Mirror frozen-pane counts loaded from the doc into the UI
        // signals so the grid renders the freeze state on initial
        // mount (and on sheet-switch).
        if fr != frozen_rows.get_untracked() { set_frozen_rows.set(fr); }
        if fc != frozen_cols.get_untracked() { set_frozen_cols.set(fc); }
        if doc_rows > 0 && doc_rows.max(10) != grid_rows.get_untracked() {
            set_grid_rows.set(doc_rows.max(10));
        }
        if doc_cols > 0 && doc_cols.max(10) != grid_cols.get_untracked() {
            set_grid_cols.set(doc_cols.max(10));
        }

        // Force the grid render to re-run now that the engine is populated.
        // Without this, the initial render fires BEFORE this Effect (seeing
        // an empty engine) and never re-subscribes to anything that would
        // change, so formula cells display as blank until the user moves
        // the active cell. The render closure reads grid_version at the
        // top, so bumping it unconditionally triggers a re-render.
        set_grid_version.update(|v| *v = v.wrapping_add(1));
    });

    // ─── Cross-document subscription GC ────────────────────────
    //
    // When a formula stops referencing a foreign doc (e.g. the user
    // deletes the cell), we want to drop the WS subscription so the
    // server doesn't keep pushing updates we no longer care about.
    // Track the set of ids we've subscribed to and diff against
    // `engine.foreign_doc_ids()` after each recompute.
    let subscribed_ids: &'static Mutex<std::collections::HashSet<String>> =
        Box::leak(Box::new(Mutex::new(std::collections::HashSet::new())));
    let subscribed_ids_addr = subscribed_ids as *const _ as usize;
    on_cleanup(move || unsafe {
        drop(Box::from_raw(
            subscribed_ids_addr as *mut Mutex<std::collections::HashSet<String>>,
        ));
    });
    Effect::new(move |_| {
        let _v = grid_version.get();
        let live: std::collections::HashSet<String> =
            engine.lock().unwrap().foreign_doc_ids().into_iter().collect();
        let mut subbed = subscribed_ids.lock().unwrap();
        // Add live ids to the subscribed set (they were subscribed by
        // `spawn_foreign_doc_fetch` on success).
        for id in &live {
            subbed.insert(id.clone());
        }
        // Find ids we still hold a subscription for that are no
        // longer referenced — tear them down.
        let stale: Vec<String> = subbed
            .iter()
            .filter(|id| !live.contains(*id))
            .cloned()
            .collect();
        for id in stale {
            subbed.remove(&id);
            on_unsubscribe_foreign.run(id);
        }
    });

    // ─── Cross-document live invalidation (WS push) ───────────
    //
    // The collab WS multi-doc subscribe channel pulses the
    // `foreign_doc_invalidate` signal with a foreign doc id whenever
    // that doc's CRDT advances. v1 treats every push as "stale,
    // refetch": drop the engine's cache for that id and bump
    // `grid_version` so the recompute path re-queues the fetch
    // through the existing HTTP loop (Network and ingest paths
    // already handle retry on success).
    let doc_id_for_invalidate = doc_id.clone();
    Effect::new(move |_| {
        let Some(stale_id) = foreign_doc_invalidate.get() else { return };
        if stale_id == doc_id_for_invalidate { return; }
        engine.lock().unwrap().invalidate_foreign_doc(&stale_id);
        set_grid_version.update(|v| *v = v.wrapping_add(1));
    });

    // ─── Cross-document fetch loop ────────────────────────────
    //
    // After every recompute, REFERENCE* formulas may have queued
    // foreign-doc fetches via `engine.register_foreign_fetch`.
    // Drain the queue, batch unfamiliar ids into the consent
    // prompt, and dispatch HTTP fetches for the approved ones.
    Effect::new({
        let alive = std::sync::Arc::clone(&alive);
        let doc_id_self = doc_id.clone();
        move |_| {
        // Re-run on each grid_version bump.
        let _v = grid_version.get();
        let pending = engine.lock().unwrap().take_pending_fetches();
        if pending.is_empty() { return; }

        let approved_set = consent_approved.get_untracked();
        let mut to_fetch: Vec<String> = Vec::new();
        let mut to_prompt: Vec<String> = Vec::new();
        for id in pending {
            if id == doc_id_self { continue; } // self-ref shouldn't reach here, but be defensive
            if approved_set.contains(&id) {
                to_fetch.push(id);
            } else {
                to_prompt.push(id);
            }
        }

        for id in to_fetch {
            spawn_foreign_doc_fetch(
                engine, fetched_ids,
                set_grid_version, grid_version,
                on_subscribe_foreign, id,
                std::sync::Arc::clone(&alive),
            );
        }

        if !to_prompt.is_empty() {
            set_consent_pending.update(|v| {
                for id in to_prompt {
                    if !v.contains(&id) { v.push(id); }
                }
            });
        }
        }  // close `move |_| { ... }`
    });

    // ─── Keydown handler ───────────────────────────────────

    let on_keydown = {
        let alive = std::sync::Arc::clone(&alive);
        move |e: web_sys::KeyboardEvent| {
        let ctrl = e.ctrl_key() || e.meta_key();
        let shift = e.shift_key();

        // When editing, only intercept specific keys
        if editing.get_untracked() { return; }

        match e.key().as_str() {
            // Painter Esc fires before touch-anchor: if both modes are
            // active, the painter is the more visually prominent state
            // and the user reaches for Esc to clear it first.
            "Escape" if format_painter.get_untracked().is_some() => {
                e.prevent_default();
                set_format_painter.set(None);
            }
            "Escape" if touch_anchor_active.get_untracked() => {
                e.prevent_default();
                set_touch_anchor_active.set(false);
            }
            // #57: Ctrl+Arrow jumps to the edge of the data region
            // (Excel parity). Shift extends the selection to the landing
            // cell. The jump is computed as an absolute target, then
            // expressed as a delta so it reuses move_active's grid-expand
            // and scroll-into-view behavior.
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" if ctrl => {
                e.prevent_default();
                let (dr, dc) = match e.key().as_str() {
                    "ArrowUp" => (-1, 0),
                    "ArrowDown" => (1, 0),
                    "ArrowLeft" => (0, -1),
                    _ => (0, 1),
                };
                let ar = active_row.get_untracked();
                let ac = active_col.get_untracked();
                let (r, c) = data_edge(
                    &engine.lock().unwrap(),
                    ar, ac, dr, dc,
                    grid_rows.get_untracked().saturating_sub(1),
                    grid_cols.get_untracked().saturating_sub(1),
                );
                move_active(r as i32 - ar as i32, c as i32 - ac as i32, shift);
            }
            "ArrowUp" => { e.prevent_default(); move_active(-1, 0, shift); }
            "ArrowDown" => { e.prevent_default(); move_active(1, 0, shift); }
            "ArrowLeft" => { e.prevent_default(); move_active(0, -1, shift); }
            "ArrowRight" => { e.prevent_default(); move_active(0, 1, shift); }
            // #57: Ctrl+Home → A1, Home → first column of the row.
            "Home" if ctrl => {
                e.prevent_default();
                let ar = active_row.get_untracked();
                let ac = active_col.get_untracked();
                move_active(-(ar as i32), -(ac as i32), shift);
            }
            "Home" => {
                e.prevent_default();
                let ac = active_col.get_untracked();
                move_active(0, -(ac as i32), shift);
            }
            // #57: Ctrl+End → last used cell, End → end of the row's data.
            "End" if ctrl => {
                e.prevent_default();
                let ar = active_row.get_untracked();
                let ac = active_col.get_untracked();
                let (r, c) = last_used_cell(&engine.lock().unwrap());
                move_active(r as i32 - ar as i32, c as i32 - ac as i32, shift);
            }
            "End" => {
                e.prevent_default();
                let ar = active_row.get_untracked();
                let ac = active_col.get_untracked();
                // Only move if the row actually has data; End on an empty row
                // is a no-op (Excel doesn't snap back to column A).
                if let Some(c) = last_used_col_in_row(
                    &engine.lock().unwrap(),
                    ar,
                    grid_cols.get_untracked().saturating_sub(1),
                ) {
                    move_active(0, c as i32 - ac as i32, shift);
                }
            }
            "Tab" => {
                e.prevent_default();
                if shift { move_active(0, -1, false); } else { move_active(0, 1, false); }
            }
            "Enter" => {
                e.prevent_default();
                if ctrl {
                    // Ctrl+Enter: toggle checkbox
                    let c = active_col.get_untracked();
                    let r = active_row.get_untracked();
                    if engine.lock().unwrap().is_checkbox((c, r)) {
                        engine.lock().unwrap().toggle_checkbox((c, r));
                        persist();
                    }
                } else if shift {
                    move_active(-1, 0, false);
                } else {
                    move_active(1, 0, false);
                }
            }
            "Delete" | "Backspace" => {
                e.prevent_default();
                let primary = sel_bounds(
                    sel_row.get_untracked(), sel_col.get_untracked(),
                    active_row.get_untracked(), active_col.get_untracked(),
                );
                // Iterate the primary rect plus every extra
                // non-contiguous region (#59). Each rect's cells
                // get cleared once; a cell present in multiple
                // regions is still cleared exactly once because
                // the inner loop short-circuits on already-empty
                // cells (set_cell("") of an empty cell is a no-op
                // at the engine level).
                let mut regions: Vec<(usize, usize, usize, usize)> =
                    std::iter::once(primary)
                        .chain(extra_sel_regions.get_untracked().into_iter())
                        .collect();
                // `Vec::dedup` only removes CONSECUTIVE duplicates, which
                // is mostly a no-op for arbitrary region order. Sort
                // first so identical rects collapse to one. Functionally
                // unnecessary — the empty-string skip below makes a
                // double-clear idempotent — but it keeps the undo-entry
                // count honest and saves a few `get_raw` calls.
                regions.sort_unstable();
                regions.dedup();
                let mut entries = Vec::new();
                {
                    let mut eng = engine.lock().unwrap();
                    for (r1, c1, r2, c2) in regions {
                        for r in r1..=r2 {
                            for c in c1..=c2 {
                                let old = eng.get_raw((c, r)).to_string();
                                if old.is_empty() { continue; }
                                eng.set_cell((c, r), "");
                                entries.push(((c, r), old, String::new()));
                            }
                        }
                    }
                }
                record_undo(entries);
                persist();
            }
            "F2" => {
                e.prevent_default();
                let is_locked = engine.lock().unwrap()
                    .get_style((active_col.get_untracked(), active_row.get_untracked()))
                    .map_or(false, |s| s.locked);
                if !is_locked {
                    set_edit_value.set(cell_raw());
                    set_editing.set(true);
                }
            }
            key if key.len() == 1 && !ctrl => {
                e.prevent_default();
                let is_locked = engine.lock().unwrap()
                    .get_style((active_col.get_untracked(), active_row.get_untracked()))
                    .map_or(false, |s| s.locked);
                if !is_locked {
                    set_edit_value.set(key.to_string());
                    set_editing.set(true);
                }
            }
            _ if ctrl && !shift => {
                match e.key().to_lowercase().as_str() {
                    // ─── Undo ──────────────────────────────
                    "z" => {
                        e.prevent_default();
                        let entry = undo_stack.lock().unwrap().pop();
                        if let Some(entry) = entry {
                            let mut eng = engine.lock().unwrap();
                            let mut redo_entry = Vec::new();
                            for ((c, r), old, new_val) in &entry {
                                let current = eng.get_raw((*c, *r)).to_string();
                                eng.set_cell((*c, *r), old);
                                redo_entry.push(((*c, *r), old.clone(), current));
                            }
                            drop(eng);
                            redo_stack.lock().unwrap().push(redo_entry);
                            persist();
                        }
                    }
                    // ─── Cut ───────────────────────────────
                    // Same OS-clipboard write as Copy; the difference is
                    // mode=Cut on the in-app clipboard, which causes the
                    // paste handler to move instead of translate.
                    "x" => {
                        e.prevent_default();
                        // #75: refuse a non-contiguous selection rather than
                        // silently cutting only the primary rectangle.
                        if multi_region_blocks_clipboard(&extra_sel_regions.get_untracked()) {
                            if let Some(w) = web_sys::window() {
                                let _ = w.alert_with_message(&crate::t!("ss-multi-region-copy"));
                            }
                            return;
                        }
                        let bounds = sel_bounds(
                            sel_row.get_untracked(), sel_col.get_untracked(),
                            active_row.get_untracked(), active_col.get_untracked(),
                        );
                        let (r1, c1, r2, c2) = bounds;
                        let (tsv, cells) = {
                            let eng = engine.lock().unwrap();
                            selection_to_tsv(&eng, bounds)
                        };

                        *clipboard.lock().unwrap() = Some(SheetClipboard {
                            source_top_left: (c1, r1),
                            cells,
                            mode: ClipMode::Cut,
                            tsv: tsv.clone(),
                            edit_serial_at_copy: *edit_serial.lock().unwrap(),
                        });
                        set_cut_source.set(Some((c1, r1, c2, r2)));
                        write_text_to_os_clipboard(tsv);
                    }
                    // ─── Copy ──────────────────────────────
                    "c" => {
                        e.prevent_default();
                        // #75: refuse a non-contiguous selection rather than
                        // silently copying only the primary rectangle.
                        if multi_region_blocks_clipboard(&extra_sel_regions.get_untracked()) {
                            if let Some(w) = web_sys::window() {
                                let _ = w.alert_with_message(&crate::t!("ss-multi-region-copy"));
                            }
                            return;
                        }
                        let bounds = sel_bounds(
                            sel_row.get_untracked(), sel_col.get_untracked(),
                            active_row.get_untracked(), active_col.get_untracked(),
                        );
                        let (r1, c1, _, _) = bounds;
                        let (tsv, cells) = {
                            let eng = engine.lock().unwrap();
                            selection_to_tsv(&eng, bounds)
                        };

                        // Stash the in-app clipboard with the same TSV we're
                        // about to write to the OS. At paste time we compare
                        // TSV strings to tell our own copy from an external
                        // one.
                        *clipboard.lock().unwrap() = Some(SheetClipboard {
                            source_top_left: (c1, r1),
                            cells,
                            mode: ClipMode::Copy,
                            tsv: tsv.clone(),
                            edit_serial_at_copy: *edit_serial.lock().unwrap(),
                        });
                        // A new Copy cancels any pending cut marquee.
                        if cut_source.get_untracked().is_some() {
                            set_cut_source.set(None);
                        }
                        write_text_to_os_clipboard(tsv);
                    }
                    // Paste is handled by the native `paste` event on the
                    // wrapper (see on:paste below). Going through the event
                    // gives us clipboardData synchronously — no async
                    // readText(), no clipboard-permission prompt.
                    // ─── Fill down ─────────────────────────
                    "d" => {
                        e.prevent_default();
                        let sel = sel_bounds(
                            sel_row.get_untracked(), sel_col.get_untracked(),
                            active_row.get_untracked(), active_col.get_untracked(),
                        );
                        let (_, _, r2, c2) = sel;
                        let bounds = (
                            grid_cols.get_untracked().max(c2 + 1),
                            grid_rows.get_untracked().max(r2 + 1),
                        );
                        let mut eng = engine.lock().unwrap();
                        apply_fill(&mut eng, sel, FillDir::Down, bounds);
                        drop(eng);
                        persist();
                    }
                    // ─── Fill right ────────────────────────
                    "r" => {
                        e.prevent_default();
                        let sel = sel_bounds(
                            sel_row.get_untracked(), sel_col.get_untracked(),
                            active_row.get_untracked(), active_col.get_untracked(),
                        );
                        let (_, _, r2, c2) = sel;
                        let bounds = (
                            grid_cols.get_untracked().max(c2 + 1),
                            grid_rows.get_untracked().max(r2 + 1),
                        );
                        let mut eng = engine.lock().unwrap();
                        apply_fill(&mut eng, sel, FillDir::Right, bounds);
                        drop(eng);
                        persist();
                    }
                    // ─── Bold toggle ──────────────────────
                    "b" => {
                        e.prevent_default();
                        let (r1, c1, r2, c2) = sel_bounds(
                            sel_row.get_untracked(), sel_col.get_untracked(),
                            active_row.get_untracked(), active_col.get_untracked(),
                        );
                        let mut eng = engine.lock().unwrap();
                        // Toggle: if any cell in selection is not bold, make all bold; else unbold all
                        let all_bold = (r1..=r2).all(|r| (c1..=c2).all(|c| {
                            eng.get_style((c, r)).map_or(false, |s| s.bold)
                        }));
                        for r in r1..=r2 {
                            for c in c1..=c2 {
                                eng.style_mut((c, r)).bold = !all_bold;
                            }
                        }
                        drop(eng);
                        persist();
                    }
                    // ─── Insert row ───────────────────────
                    "i" => {
                        e.prevent_default();
                        insert_row_at(active_row.get_untracked());
                    }
                    // ─── Find ──────────────────────────────
                    "f" => {
                        e.prevent_default();
                        set_find_visible.set(true);
                    }
                    // ─── Find & Replace ────────────────────
                    "h" => {
                        e.prevent_default();
                        set_find_visible.set(true);
                    }
                    // ─── Redo (Ctrl+Y, Excel/Windows convention)
                    // Ctrl+Shift+Z is the other binding for the same
                    // action; both share the `do_redo` closure.
                    "y" => {
                        e.prevent_default();
                        do_redo();
                    }
                    _ => {}
                }
            }
            _ if ctrl && shift => {
                // Ctrl+Shift+V — Paste Values Only
                if e.key().to_lowercase() == "v" {
                    e.prevent_default();
                    let r0 = active_row.get_untracked();
                    let c0 = active_col.get_untracked();
                    let alive_for_paste = std::sync::Arc::clone(&alive);
                    leptos::task::spawn_local(async move {
                        let alive = alive_for_paste;
                        if let Some(window) = web_sys::window() {
                            let nav = window.navigator();
                            let clip = js_sys::Reflect::get(&nav, &"clipboard".into()).ok();
                            if let Some(clip) = clip {
                                let read = js_sys::Reflect::get(&clip, &"readText".into())
                                    .ok()
                                    .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
                                if let Some(read) = read {
                                    if let Ok(promise) = read.call0(&clip) {
                                        let promise: js_sys::Promise = promise.into();
                                        if let Ok(text_js) = wasm_bindgen_futures::JsFuture::from(promise).await {
                                            // Bail out if the component unmounted
                                            // during the clipboard read — engine
                                            // has been freed by on_cleanup and
                                            // the lock() below would dereference
                                            // dangling memory.
                                            if !alive.load(std::sync::atomic::Ordering::SeqCst) { return; }
                                            let text = text_js.as_string().unwrap_or_default();
                                            let mut eng = engine.lock().unwrap();
                                            // Paste computed values, not formulas
                                            for (ri, line) in text.lines().enumerate() {
                                                if line.is_empty() { continue; }
                                                for (ci, val) in line.split('\t').enumerate() {
                                                    // Strip leading = to prevent formula interpretation
                                                    let clean = if val.starts_with('=') {
                                                        val.trim_start_matches('=')
                                                    } else {
                                                        val
                                                    };
                                                    eng.set_cell((c0 + ci, r0 + ri), clean);
                                                }
                                            }
                                            drop(eng);
                                            persist();
                                        }
                                    }
                                }
                            }
                        }
                    });
                }
                // Ctrl+Shift+Z — Redo (shares do_redo with Ctrl+Y)
                if e.key().to_lowercase() == "z" {
                    e.prevent_default();
                    do_redo();
                }
                if e.key() == "-" || e.key() == "_" {
                    e.prevent_default();
                    delete_row_at(active_row.get_untracked());
                }
            }
            _ => {}
        }
        }  // close the `move |e: …| { ... }` closure body
    };  // close the `let on_keydown = { … }` block

    // ─── Paste via native ClipboardEvent ───────────────────
    //
    // Using the paste event (instead of handling Ctrl+V in keydown +
    // navigator.clipboard.readText()) gives us the data synchronously,
    // without the browser's clipboard permission prompt. When the user
    // is editing a cell (focus on input), we let the input handle paste
    // natively instead of intercepting here.
    let on_paste = move |e: web_sys::Event| {
        if editing.get_untracked() {
            return;
        }
        // `paste` events are ClipboardEvents in practice; cast so we can
        // reach clipboardData. Leptos's `on:paste` binding forwards a
        // generic `Event`, but the runtime instance is a ClipboardEvent.
        let Ok(ce) = e.dyn_into::<web_sys::ClipboardEvent>() else {
            return;
        };
        let Some(data) = ce.clipboard_data() else { return };
        let text = data.get_data("text/plain").unwrap_or_default();
        // #54 item 3: browsers/Excel/Sheets/Word also expose a `text/html`
        // representation; an HTML `<table>` is parsed in preference to the
        // text heuristic for a literal paste (see the fallback chain below).
        let html = data.get_data("text/html").unwrap_or_default();
        ce.prevent_default();

        let r0 = active_row.get_untracked();
        let c0 = active_col.get_untracked();

        // Dispatch table — identical to the old keydown path, just
        // synchronous now.
        //   - TSV mismatch                → literal TSV paste
        //   - TSV match, Copy             → copy-paste (translate)
        //   - TSV match, Cut, serial ==   → cut-paste (move + rewrite)
        //   - TSV match, Cut, serial !=   → degrade to copy-paste
        //                                   (Excel: cancelled marquee)
        let stash = clipboard.lock().unwrap().clone();
        let current_serial = *edit_serial.lock().unwrap();
        enum PasteMode { Copy, Cut, Literal }
        let paste_mode = match &stash {
            Some(c) if c.tsv == text => match c.mode {
                ClipMode::Copy => PasteMode::Copy,
                ClipMode::Cut if c.edit_serial_at_copy == current_serial => {
                    PasteMode::Cut
                }
                ClipMode::Cut => PasteMode::Copy,
            },
            _ => PasteMode::Literal,
        };

        let mut eng = engine.lock().unwrap();
        let mut max_r = grid_rows.get_untracked();
        let mut max_c = grid_cols.get_untracked();

        if !matches!(paste_mode, PasteMode::Literal) {
            let clip = stash.as_ref().unwrap();
            let paste_rows = clip.cells.len();
            let paste_cols = clip.cells.first().map(|r| r.len()).unwrap_or(0);
            let bounds = (
                grid_cols.get_untracked().max(c0 + paste_cols),
                grid_rows.get_untracked().max(r0 + paste_rows),
            );
            match paste_mode {
                PasteMode::Cut => {
                    crate::spreadsheet::translate::apply_cut_paste(
                        &mut eng,
                        &clip.cells,
                        clip.source_top_left,
                        (c0, r0),
                        bounds,
                    );
                    // Cut is one-shot.
                    *clipboard.lock().unwrap() = None;
                }
                _ => {
                    crate::spreadsheet::translate::apply_copy_paste(
                        &mut eng,
                        &clip.cells,
                        clip.source_top_left,
                        (c0, r0),
                        bounds,
                    );
                }
            }
            max_c = max_c.max(c0 + paste_cols);
            max_r = max_r.max(r0 + paste_rows);
        } else if let Some(rows) = parse_html_table(&html) {
            // #54 item 3: HTML-table paste (Excel / Sheets / Word / web
            // tables). Preferred over the markdown/TSV heuristic because the
            // `<table>` structure is unambiguous. Cells are already stripped
            // and trimmed by the parser.
            for (ri, row) in rows.iter().enumerate() {
                for (ci, val) in row.iter().enumerate() {
                    eng.set_cell((c0 + ci, r0 + ri), val);
                    max_c = max_c.max(c0 + ci + 1);
                }
                max_r = max_r.max(r0 + ri + 1);
            }
        } else if let Some(rows) = parse_markdown_table(&text) {
            // Markdown-table paste. The first non-separator row is
            // the header; the separator row is dropped. Cells are
            // already trimmed by the parser.
            for (ri, row) in rows.iter().enumerate() {
                for (ci, val) in row.iter().enumerate() {
                    eng.set_cell((c0 + ci, r0 + ri), val);
                    max_c = max_c.max(c0 + ci + 1);
                }
                max_r = max_r.max(r0 + ri + 1);
            }
        } else {
            for (ri, line) in text.lines().enumerate() {
                if line.is_empty() {
                    continue;
                }
                for (ci, val) in line.split('\t').enumerate() {
                    eng.set_cell((c0 + ci, r0 + ri), val);
                    max_c = max_c.max(c0 + ci + 1);
                }
                max_r = max_r.max(r0 + ri + 1);
            }
        }
        set_grid_rows.set(max_r.max(10));
        set_grid_cols.set(max_c.max(10));
        drop(eng);
        persist();
    };

    // ─── View ──────────────────────────────────────────────

    // Touch handlers — wrapper-level so a single LongPressTracker covers
    // the whole grid. Each handler walks up the event target to find the
    // nearest <td> with data-row / data-col, set in the cell render below.
    let on_touchstart = {
        let pending = Rc::clone(&pending_anchor_cell);
        let tracker = Rc::clone(&touch_tracker);
        move |ev: web_sys::TouchEvent| {
            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            let cell = target
                .and_then(|el| el.closest("[data-row][data-col]").ok().flatten())
                .and_then(|el| {
                    let r = el.get_attribute("data-row")?.parse::<usize>().ok()?;
                    let c = el.get_attribute("data-col")?.parse::<usize>().ok()?;
                    Some((r, c))
                });
            *pending.borrow_mut() = cell;
            if cell.is_some() {
                set_touch_in_progress.set(true);
                set_touch_dragging.set(false);
                tracker.on_start(&ev);
            }
        }
    };
    let on_touchmove = {
        let tracker = Rc::clone(&touch_tracker);
        let pending = Rc::clone(&pending_anchor_cell);
        move |ev: web_sys::TouchEvent| {
            tracker.on_move(&ev);
            // Drag-to-extend: once the finger crosses into a different
            // cell, transition to drag mode. preventDefault suppresses
            // iOS mouse-event synthesis so the upcoming touchend doesn't
            // re-fire the start cell's mousedown and reset the range.
            let touches = ev.touches();
            if touches.length() != 1 {
                return;
            }
            let Some(t) = touches.item(0) else { return };
            let (x, y) = (t.client_x() as f64, t.client_y() as f64);
            let cell = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.element_from_point(x as f32, y as f32))
                .and_then(|el| el.closest("[data-row][data-col]").ok().flatten())
                .and_then(|el| {
                    let r = el.get_attribute("data-row")?.parse::<usize>().ok()?;
                    let c = el.get_attribute("data-col")?.parse::<usize>().ok()?;
                    Some((r, c))
                });
            let Some((r, c)) = cell else { return };
            let start = *pending.borrow();
            if start.is_none() || start == Some((r, c)) {
                if !touch_dragging.get_untracked() {
                    return;
                }
                // Already dragging back to the start cell — keep it that way.
            }
            ev.prevent_default();
            if !touch_dragging.get_untracked() {
                if let Some((sr, sc)) = start {
                    // Anchor the selection to the touchstart cell on the
                    // first cross-cell move; subsequent moves only update
                    // the cursor end.
                    set_sel_row.set(sr);
                    set_sel_col.set(sc);
                }
                set_touch_dragging.set(true);
            }
            set_active_row.set(r);
            set_active_col.set(c);
        }
    };
    let make_touchend_handler = || {
        let tracker = Rc::clone(&touch_tracker);
        let pending = Rc::clone(&pending_anchor_cell);
        move |ev: web_sys::TouchEvent| {
            tracker.on_end();
            *pending.borrow_mut() = None;
            // Multitouch: only release the in-progress flag when the last
            // remaining finger lifts. `TouchEvent.touches` reflects the
            // post-event state.
            if ev.touches().length() == 0 {
                set_touch_in_progress.set(false);
                set_touch_dragging.set(false);
            }
        }
    };
    let on_touchend = make_touchend_handler();
    let on_touchcancel = make_touchend_handler();

    // #72: the four data-cell mouse handlers (mousedown / mousemove /
    // dblclick / contextmenu) used to be wired on every `<td>` — ~4
    // listeners × thousands of cells, re-created on every grid rebuild
    // (the `addEventListener` / `makeMutClosure` churn that dominated
    // the profile). They're hoisted here as single closures and bound
    // once on the stable `.spreadsheet-wrapper` below; each derives its
    // `(row, col)` from the event target via `cell_rc_from_mouse_event`.
    // Bodies are verbatim from the former per-cell handlers, with `r`/`c`
    // now parameters (and dblclick re-reading the cell's raw text from the
    // engine instead of capturing it from the render pass).
    let on_cell_mousedown = {
        let select_cell = select_cell.clone();
        let commit_edit = commit_edit.clone();
        let persist = persist.clone();
        let refocus_wrapper = refocus_wrapper.clone();
        move |r: usize, c: usize, e: web_sys::MouseEvent| {
            // Right-click is for the context menu. Don't let
            // mousedown's selection logic collapse an existing
            // range selection before `on:contextmenu` fires —
            // the contextmenu handler decides whether to
            // preserve the selection (right-click inside the
            // range) or move the active cell (right-click
            // outside).
            if e.button() == 2 { return; }
            // Format-painter apply: if armed, stamp the captured
            // style onto this cell, consume the click, and (if not
            // sticky) disarm. Runs first so it overrides ref-pick
            // and selection behavior. If the user was mid-edit on
            // some other cell, commit that edit first so its typed
            // text doesn't get silently dropped by the persist()
            // below.
            if let Some((source_style, sticky)) = format_painter.get_untracked() {
                e.prevent_default();
                if editing.get_untracked() {
                    commit_edit();
                    set_editing.set(false);
                }
                let mut eng = engine.lock().unwrap();
                let target = eng.get_style((c, r)).cloned().unwrap_or_default();
                eng.set_style((c, r), target.with_visual_style_from(&source_style));
                drop(eng);
                if !sticky { set_format_painter.set(None); }
                persist();
                return;
            }
            // Touch-synthesized click as the second tap of a
            // tap-and-hold range. Only triggers outside edit mode
            // so the formula ref-pick branch below stays intact.
            if touch_anchor_active.get_untracked()
                && !editing.get_untracked()
            {
                select_cell(r, c, /* extend */ true);
                set_touch_anchor_active.set(false);
                return;
            }

            let extend = e.shift_key();
            let modify_multi = e.ctrl_key() || e.meta_key();

            // Ref-pick mode: if we're editing and the caret is in a
            // position that expects a reference (or we've already started
            // a pick), clicking a cell inserts its reference into the
            // formula rather than committing the edit.
            if editing.get_untracked() {
                let val = edit_value.get_untracked();
                let caret = val.len();
                let current_pick = ref_pick.get_untracked();
                if current_pick.is_some() || is_ref_context(&val, caret) {
                    e.prevent_default(); // keep input focused
                    let new_pick = match (&current_pick, extend) {
                        (Some(p), true) => RefPick {
                            start: p.start,
                            end: (c, r),
                            insert_at: p.insert_at,
                        },
                        (Some(p), false) => RefPick {
                            start: (c, r),
                            end: (c, r),
                            insert_at: p.insert_at,
                        },
                        (None, _) => RefPick {
                            start: (c, r),
                            end: (c, r),
                            insert_at: val.len(),
                        },
                    };
                    let new_val = splice_ref(&val, new_pick.insert_at, &new_pick.label());
                    set_edit_value.set(new_val);
                    set_ref_pick.set(Some(new_pick));
                    // Arm drag-to-extend.
                    set_dragging.set(true);
                    // Caret at end of new value.
                    if let Some(input) = cell_input_ref.get() {
                        let el: web_sys::HtmlInputElement = input.into();
                        let len = el.value().len() as u32;
                        let _ = el.set_selection_range(len, len);
                    }
                    return;
                }
            }

            // Ctrl-click (Cmd-click on Mac) extends the
            // multi-region selection introduced in #59:
            // save the current primary rect into the
            // extras list, then start a fresh single-cell
            // primary at the clicked cell. Shift+Ctrl is
            // treated as Ctrl for now — shift-extending an
            // arbitrary extra region would need a
            // "last-clicked anchor" the engine doesn't
            // track yet.
            if modify_multi && !editing.get_untracked() {
                let (cr1, cc1, cr2, cc2) = sel_bounds(
                    sel_row.get_untracked(),
                    sel_col.get_untracked(),
                    active_row.get_untracked(),
                    active_col.get_untracked(),
                );
                // #73: Ctrl-click toggles. If the cell is
                // already part of a non-contiguous extra
                // region, remove it (Excel parity) by
                // splitting that region around the cell,
                // rather than re-adding a duplicate. Primary
                // anchor is left untouched.
                let in_extra = extra_sel_regions
                    .get_untracked()
                    .iter()
                    .any(|&(er1, ec1, er2, ec2)| {
                        r >= er1 && r <= er2 && c >= ec1 && c <= ec2
                    });
                if in_extra {
                    set_extra_sel_regions.update(|v| {
                        *v = v
                            .iter()
                            .flat_map(|&rect| subtract_cell(rect, r, c))
                            .collect();
                    });
                    refocus_wrapper();
                    set_dragging.set(false);
                    return;
                }
                // Re-clicking the lone primary anchor is a
                // no-op: nothing to add, and the anchor
                // (active cell) can't be removed.
                let same_as_primary =
                    cr1 == cr2 && cc1 == cc2 && cr1 == r && cc1 == c;
                if same_as_primary {
                    refocus_wrapper();
                    set_dragging.set(false);
                    return;
                }
                // New cell: bank the current primary rect
                // into the extras and make the click the
                // new primary.
                set_extra_sel_regions.update(|v| {
                    v.push((cr1, cc1, cr2, cc2));
                });
                set_active_row.set(r);
                set_active_col.set(c);
                set_sel_row.set(r);
                set_sel_col.set(c);
                set_editing.set(false);
                refocus_wrapper();
                // #74: arm dragging so a Ctrl-DRAG (not just a
                // Ctrl-click) extends this freshly-banked anchor
                // into a rect. The per-cell mousemove extends the
                // primary via active_row/col without touching the
                // extras we just pushed, so the dragged rect joins
                // the non-contiguous selection. A Ctrl-click with
                // no movement still ends as a single cell once
                // mouseup clears the flag.
                set_dragging.set(true);
                // Skip the click-into-spill/pivot logic
                // below — ctrl-click is a selection-only
                // gesture and shouldn't open editors.
                return;
            }
            // Plain / shift-click: reset extras so a fresh
            // selection isn't shadowed by lingering
            // ctrl-click regions from before.
            set_extra_sel_regions.set(Vec::new());

            select_cell(r, c, extend);
            if !extend { set_dragging.set(true); }

            // Click-into-spill: if the clicked cell belongs to a
            // pivot's spill area (anchor or fill cell), pop the
            // editor open on that pivot. The editor itself stays
            // user-dismissible via its ✕; we only auto-open, not
            // auto-close.
            //
            // Special case for page-field filter rows: the topmost
            // `n_filters` rows of every pivot's spill render the
            // filter dropdowns. A click on those cells should also
            // expand the filter popover for the matching index, so
            // the user can pick values without first hunting through
            // the sidebar's Filters zone.
            // Inspect the clicked cell against each known pivot's
            // anchor to determine which header dropdown (if any)
            // it represents:
            //   - filter_idx_to_open: page-field row, col == anchor.0
            //   - row_group_idx_to_open: value-label header row,
            //     col in [anchor.0, anchor.0 + n_row_groups)
            //   - col_group_idx_to_open: col-group level-0 row,
            //     col == anchor.0 (only level 0)
            let (
                pivot_anchor,
                filter_idx_to_open,
                row_group_idx_to_open,
                col_group_idx_to_open,
            ) = {
                let eng = engine.lock().unwrap();
                let candidate = eng.spill_anchor((c, r)).unwrap_or((c, r));
                if let Some(p) = eng.get_pivot(candidate) {
                    let dr = r.saturating_sub(candidate.1);
                    let dc = c.saturating_sub(candidate.0);
                    let n_filters = p.filters.len();
                    let n_col_groups = p.cols.len();
                    let n_values = p.values.len();
                    let on_anchor_col = c == candidate.0;
                    let filter_idx = if on_anchor_col && dr < n_filters {
                        Some(dr)
                    } else { None };
                    // Col-group level-0 row: row index = anchor + n_filters,
                    // and the leftmost cell carries the "<col field> ▾" glyph.
                    let col_group_idx = if on_anchor_col
                        && n_col_groups > 0
                        && dr == n_filters
                    {
                        Some(0_usize)
                    } else { None };
                    // Value-label header row: row index = anchor +
                    // n_filters + n_col_groups (only when values exist;
                    // otherwise no value-label row gets emitted).
                    let row_group_idx = if n_values > 0
                        && dr == n_filters + n_col_groups
                        && dc < p.rows.len().max(1)
                    {
                        // Compact stacks all row-groups into 1 click target,
                        // mapped to row group 0; Outline/Tabular separate
                        // them across columns.
                        let layout_compact = matches!(
                            p.layout_style,
                            crate::spreadsheet::pivot::LayoutStyle::Compact
                        );
                        if !p.rows.is_empty() {
                            Some(if layout_compact { 0 } else { dc })
                        } else { None }
                    } else { None };
                    (Some(candidate), filter_idx, row_group_idx, col_group_idx)
                } else {
                    (None, None, None, None)
                }
            };
            if let Some(a) = pivot_anchor {
                if pivot_editor_open.get_untracked() != Some(a) {
                    set_pivot_editor_open.set(Some(a));
                }
                if let Some(idx) = filter_idx_to_open {
                    if pivot_filter_popover_open.get_untracked() != Some(idx) {
                        set_pivot_filter_popover_open.set(Some(idx));
                    }
                }
                if let Some(idx) = row_group_idx_to_open {
                    set_pivot_group_picker_open.set(Some((0, idx)));
                } else if let Some(idx) = col_group_idx_to_open {
                    set_pivot_group_picker_open.set(Some((1, idx)));
                }
            }
        }
    };
    let on_cell_mousemove = move |r: usize, c: usize| {
        if !dragging.get_untracked() {
            return;
        }
        // Drag in ref-pick mode extends the range endpoint.
        if let Some(pick) = ref_pick.get_untracked() {
            if pick.end == (c, r) {
                return; // no-op if pointer didn't cross a cell boundary
            }
            let val = edit_value.get_untracked();
            let new_pick = RefPick {
                start: pick.start,
                end: (c, r),
                insert_at: pick.insert_at,
            };
            let new_val = splice_ref(&val, pick.insert_at, &new_pick.label());
            set_edit_value.set(new_val);
            set_ref_pick.set(Some(new_pick));
            return;
        }
        // Normal drag — extend the grid selection.
        set_active_row.set(r);
        set_active_col.set(c);
    };
    let on_cell_dblclick = {
        let select_cell = select_cell.clone();
        move |r: usize, c: usize| {
            select_cell(r, c, false);
            // #72: the former per-cell handler captured the cell's `raw`
            // text from the render pass; the delegated handler re-reads
            // it from the engine for the resolved (row, col). One lock
            // scope for both the locked-style check and the raw read.
            let (is_locked, raw) = {
                let eng = engine.lock().unwrap();
                let locked = eng.get_style((c, r)).map_or(false, |s| s.locked);
                (locked, eng.get_raw((c, r)).to_string())
            };
            if !is_locked {
                set_edit_value.set(raw);
                set_editing.set(true);
            }
        }
    };
    let on_cell_contextmenu = {
        let select_cell = select_cell.clone();
        move |r: usize, c: usize, e: web_sys::MouseEvent| {
            e.prevent_default();
            // Suppress the desktop context menu when the event was
            // dispatched from a touch long-press — that gesture is
            // already reserved for arming the tap-and-hold range
            // anchor on mobile.
            if touch_in_progress.get_untracked()
                || touch_anchor_active.get_untracked()
            {
                return;
            }
            // Excel-parity: right-click inside the existing range
            // preserves the selection so menu actions like
            // "Format cells..." apply to the whole range. Right-
            // click outside moves the active cell to the click
            // target before opening the menu.
            let (sr1, sc1, sr2, sc2) = sel_bounds(
                sel_row.get_untracked(), sel_col.get_untracked(),
                active_row.get_untracked(), active_col.get_untracked(),
            );
            let inside = c >= sc1 && c <= sc2 && r >= sr1 && r <= sr2;
            if !inside {
                select_cell(r, c, false);
            }
            let (mx, my) = clamp_menu_position(
                e.client_x() as f64,
                e.client_y() as f64,
            );
            set_ctx_menu_x.set(mx);
            set_ctx_menu_y.set(my);
            set_ctx_menu_visible.set(true);
        }
    };

    view! {
        <div class="spreadsheet-wrapper"
            node_ref=wrapper_ref
            on:keydown=on_keydown
            on:paste=on_paste
            on:mousedown=move |e: web_sys::MouseEvent| {
                // #72: delegated data-cell mousedown. On a real cell, run the
                // hoisted handler (selection / ref-pick / format-painter /
                // pivot-open). Off-cell taps (formula bar, header, resize
                // handle, blank canvas) clear a stuck tap-anchor — formerly
                // this relied on the per-cell handler having already cleared
                // it via bubbling; now the on-cell branch does that itself.
                if let Some((r, c)) = cell_rc_from_mouse_event(&e) {
                    on_cell_mousedown(r, c, e);
                } else if touch_anchor_active.get_untracked() {
                    set_touch_anchor_active.set(false);
                }
            }
            on:mouseup=move |_| {
                set_dragging.set(false);
                set_resize_col.set(None);
                set_resize_row.set(None);
            }
            on:mousemove=move |e: web_sys::MouseEvent| {
                if let Some(col) = resize_col.get_untracked() {
                    let dx = e.client_x() as f64 - resize_start_x.get_untracked();
                    let new_w = (resize_start_w.get_untracked() + dx).max(30.0);
                    set_col_widths.update(|widths| {
                        if col < widths.len() { widths[col] = new_w; }
                    });
                }
                if let Some(row) = resize_row.get_untracked() {
                    let dy = e.client_y() as f64 - resize_start_y.get_untracked();
                    let new_h = (resize_start_h.get_untracked() + dy).max(16.0);
                    set_row_heights.update(|heights| {
                        while heights.len() <= row { heights.push(24.0); }
                        heights[row] = new_h;
                    });
                }
                // #72: delegated cell drag-select (formerly per-cell
                // on:mousemove). No-op unless a cell drag is armed
                // (`dragging`), so it never fights the resize branches above.
                if let Some((r, c)) = cell_rc_from_mouse_event(&e) {
                    on_cell_mousemove(r, c);
                }
            }
            on:dblclick=move |e: web_sys::MouseEvent| {
                // #72: delegated cell dblclick (open the editor).
                if let Some((r, c)) = cell_rc_from_mouse_event(&e) {
                    on_cell_dblclick(r, c);
                }
            }
            on:contextmenu=move |e: web_sys::MouseEvent| {
                // #72: delegated cell contextmenu. Off-cell right-clicks fall
                // through to the browser's default menu — the former per-cell
                // handler only existed on data cells.
                if let Some((r, c)) = cell_rc_from_mouse_event(&e) {
                    on_cell_contextmenu(r, c, e);
                }
            }
            on:touchstart=on_touchstart
            on:touchmove=on_touchmove
            on:touchend=on_touchend
            on:touchcancel=on_touchcancel
            tabindex="0"
        >
            <div class="spreadsheet-formula-bar">
                <span class="formula-bar-cell-ref">{cell_ref_label}</span>
                <div class="formula-bar-separator"></div>
                {move || {
                    if editing.get() {
                        view! {
                            <span class="formula-bar-value formula-bar-editing">
                                {move || edit_value.get()}
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            <span class="formula-bar-value"
                                on:click=move |_| {
                                    let is_locked = engine.lock().unwrap()
                                        .get_style((active_col.get_untracked(), active_row.get_untracked()))
                                        .map_or(false, |s| s.locked);
                                    if !is_locked {
                                        set_edit_value.set(cell_raw());
                                        set_editing.set(true);
                                    }
                                }
                            >{cell_raw}</span>
                        }.into_any()
                    }
                }}
            </div>

            <div class="spreadsheet-toolbar">
                <button
                    class="ss-tool-btn"
                    class:active=move || format_painter.get().is_some()
                    title=crate::t!("ss-format-painter-title")
                    on:click=move |e: web_sys::MouseEvent| {
                        if format_painter.get_untracked().is_some() {
                            // Already armed — second click cancels.
                            set_format_painter.set(None);
                            return;
                        }
                        let style = engine.lock().unwrap()
                            .get_style((active_col.get_untracked(), active_row.get_untracked()))
                            .cloned()
                            .unwrap_or_default();
                        set_format_painter.set(Some((style, e.shift_key())));
                    }
                >"\u{1F58C}"</button>
                <button
                    class="ss-tool-btn"
                    title=crate::t!("ss-sort-tooltip")
                    on:click=move |_| {
                        // #75: refuse on a non-contiguous selection, matching
                        // the context-menu Sort guard and Excel.
                        if !extra_sel_regions.get_untracked().is_empty() {
                            if let Some(w) = web_sys::window() {
                                let _ = w.alert_with_message(&crate::t!("ss-multi-region-op"));
                            }
                            return;
                        }
                        let rows = grid_rows.get_untracked().max(1);
                        let cols = grid_cols.get_untracked().max(1);
                        let range_a1 = format!(
                            "A1:{}{}",
                            col_to_letters(cols - 1),
                            rows,
                        );
                        let prior = sort_keys.get_untracked();
                        let initial_keys = if prior.is_empty() {
                            vec![(active_col.get_untracked(), true)]
                        } else {
                            prior
                        };
                        set_sort_dialog_open.set(Some(SortDialogContext {
                            initial_keys,
                            initial_range_a1: range_a1,
                            initial_has_headers: false,
                        }));
                    }
                >"\u{21F5}"</button>
                {move || {
                    if let Some((_, sticky)) = format_painter.get() {
                        let label = if sticky {
                            crate::t!("ss-format-painter-status-sticky")
                        } else {
                            crate::t!("ss-format-painter-status")
                        };
                        view! { <span class="ss-tool-status">{label}</span> }.into_any()
                    } else {
                        view! { <span></span> }.into_any()
                    }
                }}
            </div>

            <div class="spreadsheet-grid-container"
                class:fmt-painter-armed=move || format_painter.get().is_some()
                node_ref=grid_scroll_ref
                // #72: feed the row-virtualization window. Fires per scroll
                // event, but the downstream `visible_rows` Memo dedupes, so
                // the grid only re-renders when the window shifts by a row.
                on:scroll=move |_| {
                    if let Some(el) = grid_scroll_ref.get_untracked() {
                        let el: web_sys::HtmlElement = el.into();
                        let st = el.scroll_top() as f64;
                        set_scroll_top.set(st);
                        let h = el.client_height();
                        if h > 0 {
                            set_viewport_h.set(h as f64);
                        }
                        // Wheel/scrollbar parity with keyboard navigation:
                        // move_active grows the grid by 20 rows when the
                        // cursor nears the edge, but a wheel scroll used to
                        // dead-end at the bottom. When the scrollbar bottoms
                        // out, extend by the same 20 rows (against the
                        // rendered floor, since display_rows = max(100)).
                        let sh = el.scroll_height() as f64;
                        if sh > h as f64 && st + h as f64 >= sh - 2.0 {
                            let cur = grid_rows.get_untracked().max(100);
                            set_grid_rows.set(cur + 20);
                        }
                    }
                }
            >
                {
                    // #72: derive the `(col,row) -> (name,color)` remote-cursor
                    // map OUTSIDE the table-render closure, as a Memo. Reading
                    // `remote_cursors` here (rather than inside the grid
                    // closure) means a peer cursor move recomputes only this
                    // small map and updates the affected cells' outline/badge
                    // (the per-cell `remote_by_cell.with(...)` reads below) —
                    // instead of re-running the whole ~2600-cell render.
                    // Created once (this block runs once); the Memo is Copy, so
                    // the grid closure and the per-cell closures capture it.
                    let remote_by_cell: Memo<std::collections::HashMap<(usize, usize), (String, String)>> =
                        Memo::new(move |_| {
                            // #99: View → Show Cursors toggle. When off, draw
                            // nothing (data is still tracked elsewhere).
                            if !cursors_enabled.get() {
                                return std::collections::HashMap::new();
                            }
                            let names_for_remote = sheet_names.get();
                            let sheet_idx = active_sheet.get();
                            let current_sheet = names_for_remote
                                .get(sheet_idx)
                                .cloned()
                                .unwrap_or_else(|| DEFAULT_SHEET_NAME.to_string());
                            remote_cursors
                                .get()
                                .into_iter()
                                .filter_map(|cursor| {
                                    let (block_id, _) = cursor.cursor_block.as_ref()?;
                                    let (sheet, r, c) = parse_ss_block_id(block_id)?;
                                    if !sheet.eq_ignore_ascii_case(&current_sheet) {
                                        return None;
                                    }
                                    Some(((c, r), (cursor.name.clone(), cursor.color.clone())))
                                })
                                .collect()
                        });
                    // #72: `sel_rect` is the normalized (top, left, bottom,
                    // right) of the primary selection, derived once as a Memo so
                    // the column/row header `class:active` closures can read it
                    // reactively (cheap — only ~126 headers). Created here in the
                    // once-block; the Memo is Copy.
                    let sel_rect: Memo<(usize, usize, usize, usize)> =
                        Memo::new(move |_| {
                            sel_bounds(
                                sel_row.get(), sel_col.get(),
                                active_row.get(), active_col.get(),
                            )
                        });
                    // #72: O(1) selection highlight. The cursor / selection /
                    // tap-anchor classes used to be reactive closures on EVERY
                    // cell, so a single keystroke woke ~O(cells) effects — the
                    // dominant cursoring cost in the profile. Instead, this one
                    // Effect imperatively toggles those same CSS classes on only
                    // the cells that change. Visuals are byte-identical (same
                    // classes, same CSS); moving the cursor now costs a handful
                    // of classList writes instead of thousands of effect runs.
                    // Reading `grid_version` re-applies the classes after a
                    // data-edit rebuild recreates the cells.
                    Effect::new(move |_| {
                        let ar = active_row.get();
                        let ac = active_col.get();
                        let (sr1, sc1, sr2, sc2) = sel_rect.get();
                        let extras = extra_sel_regions.get();
                        let tap = touch_anchor_active.get();
                        let _ = grid_version.get(); // re-apply after a rebuild
                        // The filter dropdown hides rows/cols WITHOUT bumping
                        // grid_version (the grid rebuilds via its own tracked
                        // hidden_rows/hidden_cols reads), so subscribe to both
                        // here too or the freshly-rendered cells would be born
                        // without cursor/selection classes after a filter.
                        let _ = hidden_rows.get();
                        let _ = hidden_cols.get();
                        // #72 virtualization: scrolling renders new rows, which
                        // need their highlight re-applied. Also used to clamp
                        // the selected-rect loop below to rows that exist.
                        let (virt_s, virt_e) = visible_rows.get();
                        let frozen_keep = frozen_rows.get();

                        let Some(root) = wrapper_ref.get() else { return };
                        let root: web_sys::HtmlElement = root.into();

                        // Strip the three dynamic classes from whatever cells
                        // currently carry them (one query per class, not per cell).
                        for cls in ["cursor", "selected", "tap-anchor"] {
                            if let Ok(list) = root.query_selector_all(
                                &format!(".spreadsheet-cell.{cls}")
                            ) {
                                for i in 0..list.length() {
                                    if let Some(el) = list.item(i)
                                        .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                                    {
                                        let _ = el.class_list().remove_1(cls);
                                    }
                                }
                            }
                        }

                        let find = |r: usize, c: usize| {
                            root.query_selector(&format!(
                                ".spreadsheet-cell[data-row=\"{r}\"][data-col=\"{c}\"]"
                            )).ok().flatten()
                        };

                        // Cursor (+ tap-anchor) on the active cell.
                        if let Some(el) = find(ar, ac) {
                            let _ = el.class_list().add_1("cursor");
                            if tap {
                                let _ = el.class_list().add_1("tap-anchor");
                            }
                        }

                        // Selection: primary rectangle + any ctrl-click extras.
                        // Rows outside the rendered window (frozen rows + the
                        // virtualization window) have no DOM cells — skip them
                        // rather than issuing a guaranteed-miss query per cell
                        // (a full-column selection would otherwise probe every
                        // logical row).
                        for (r1, c1, r2, c2) in
                            std::iter::once((sr1, sc1, sr2, sc2)).chain(extras)
                        {
                            for r in r1..=r2 {
                                if !(r < frozen_keep || (r >= virt_s && r < virt_e)) {
                                    continue;
                                }
                                for c in c1..=c2 {
                                    if let Some(el) = find(r, c) {
                                        let _ = el.class_list().add_1("selected");
                                    }
                                }
                            }
                        }
                    });
                    // Cloned once for the reactive grid closure so the
                    // per-cell comment marker can spawn the open-or-create
                    // flow without moving the component-level `doc_id` /
                    // `alive` (both reused later for the context menu etc.).
                    let doc_id_for_grid = doc_id.clone();
                    let alive_for_grid = std::sync::Arc::clone(&alive);
                    move || {
                    let _v = grid_version.get();
                    if editor_state.get().is_none() {
                        return view! { <div class="spreadsheet-empty">{crate::t!("ss-empty")}</div> }.into_any();
                    }

                    // Engine + derived signals are synced by the Effect above.
                    // This closure only reads — no signal writes, no mutex locks
                    // spanning signal writes (would re-enter and panic).
                    let display_rows = grid_rows.get().max(100);
                    let display_cols = grid_cols.get().max(26);

                    // #72 virtualization: the tbody window (tracked — a scroll
                    // that shifts the window re-renders the grid). Extended for
                    // merged regions after the engine lock below.
                    let (virt_s0, virt_e0) = visible_rows.get();

                    // #72: read the active cell UNTRACKED so this render closure
                    // no longer re-runs on every cursor move / range change — the
                    // cursor/selection highlight is applied imperatively by the
                    // selection Effect above, and the header highlights read the
                    // `sel_rect` Memo. The untracked `ar`
                    // / `ac` snapshot only seeds `is_editing_this` (the edit-input
                    // swap); that stays correct because the closure remains
                    // subscribed to `editing`, so entering/leaving edit re-renders
                    // with a fresh active cell. The grid still rebuilds on real
                    // data edits (grid_version), cut/pick, and structural changes.
                    let ar = active_row.get_untracked();
                    let ac = active_col.get_untracked();
                    // #72: read `editing` UNCONDITIONALLY here, not inside the
                    // per-cell loop. The per-cell use is `is_cursor &&
                    // editing.get()`, and with row virtualization the active
                    // cell can be outside the rendered window during a render
                    // pass (e.g. after scrolling away) — every `is_cursor` is
                    // then false, the short-circuit skips the read, and the
                    // closure silently UNSUBSCRIBES from `editing`. The next
                    // set_editing(true) (click + type on a freshly scrolled-in
                    // cell) would no longer re-render, so the edit input never
                    // mounted: no live characters, and no blur-commit.
                    let editing_now = editing.get();
                    // Read every reactive signal we need BEFORE
                    // acquiring the engine mutex. Holding the engine
                    // lock across `.get()` calls is risky: any
                    // closure-time re-entry (e.g. a derived signal
                    // that itself reads the engine) would deadlock
                    // since `std::sync::Mutex` is not reentrant. CSR
                    // is single-threaded so this is mostly a
                    // defensive ordering — but it prevents future
                    // refactors from introducing a re-entry path.
                    // (Issue #4 finding 2.)
                    let hr = hidden_rows.get();
                    let hc = hidden_cols.get();
                    let widths = col_widths.get();
                    let heights = row_heights.get();
                    let frozen_r = frozen_rows.get();
                    let frozen_c = frozen_cols.get();
                    // #72: extra (ctrl-click) selection regions are no longer
                    // snapshotted here — the reactive `class:selected` closure
                    // reads `extra_sel_regions` per cell, so multi-region
                    // changes update only the affected cells instead of
                    // re-running this render.
                    // #72: the `(col,row) -> (name,color)` remote-cursor map is
                    // now the `remote_by_cell` Memo created above the closure,
                    // so this render no longer subscribes to `remote_cursors`.
                    // Each `<td>` reads it per-cell (outline style + name badge)
                    // via `remote_by_cell.with(...)`, applied inline so the
                    // geometry follows the cell's actual rendered box — the
                    // approach #70 established to avoid the pre-#70
                    // absolute-overlay pixel-math misalignment.
                    // Active-sheet index + a `thread_id → preview` map for
                    // the cell-comment hover preview. Keyed by thread id
                    // (not the position-derived block id) so the preview
                    // follows the cell through row/column inserts — the
                    // cell's `comment_thread_id` travels with its style,
                    // while the server thread keeps its original block id.
                    // Built once per render pass; the per-cell closures
                    // below borrow it (they're non-`move`). Reading
                    // `cell_threads` here makes the grid re-render when a
                    // peer's reply bumps the page's thread list.
                    let comment_sheet_idx = active_sheet.get();
                    let cell_thread_by_id: std::collections::HashMap<String, CellThreadInfo> =
                        cell_threads.get()
                            .into_iter()
                            .map(|t| (t.thread_id.clone(), t))
                            .collect();
                    let eng = engine.lock().unwrap();

                    // #72 virtualization: extend the window so a merged region
                    // straddling its edge is rendered whole — its anchor cell
                    // (the only one with content and the rowspan) may sit above
                    // the window, and a partially-rendered merge would leave a
                    // hole. Regions are (col, row, col_span, row_span).
                    let (virt_start, virt_end) = {
                        let (mut s, mut e) = (virt_s0, virt_e0);
                        // While editing, the window must include the active
                        // row even if the user scrolls away mid-edit —
                        // unmounting the edit input would discard the
                        // uncommitted value (blur doesn't reliably fire on
                        // DOM removal, so commit_edit would never run).
                        if editing_now && ar < display_rows {
                            s = s.min(ar);
                            e = e.max(ar + 1);
                        }
                        for &(_, mr, _, mrs) in eng.get_merged_regions() {
                            if mr < e && mr + mrs > s {
                                s = s.min(mr);
                                e = e.max(mr + mrs);
                            }
                        }
                        (s, e.min(display_rows))
                    };
                    // Spacer heights replacing the unrendered rows above/below
                    // the window (frozen rows stay rendered; hidden rows were
                    // never in the layout).
                    let frozen_keep = frozen_r.min(display_rows);
                    // Measured per-row pitch overhead (collapsed borders);
                    // keeps spacer geometry equal to what the browser
                    // actually lays out — see `row_pitch_extra`.
                    let pitch_extra = row_pitch_extra.get();
                    let virt_row_px = |r: usize| if hr.contains(&r) {
                        0.0
                    } else {
                        heights.get(r).copied().unwrap_or(24.0) + pitch_extra
                    };
                    let virt_top_px: f64 =
                        (frozen_keep..virt_start.min(display_rows)).map(virt_row_px).sum();
                    let virt_bot_px: f64 =
                        (virt_end..display_rows).map(virt_row_px).sum();

                    view! {
                        <table class="spreadsheet-grid">
                            <thead>
                                <tr>
                                    <th class="spreadsheet-corner"
                                        on:click=move |_| {
                                            // Select all
                                            set_sel_row.set(0);
                                            set_sel_col.set(0);
                                            set_active_row.set(display_rows.saturating_sub(1));
                                            set_active_col.set(display_cols.saturating_sub(1));
                                            // Replaces any multi-region selection (#59).
                                            set_extra_sel_regions.set(Vec::new());
                                        }
                                    ></th>
                                    {(0..display_cols).filter(|c| !hc.contains(c)).map(|c| {
                                        let label = col_to_letters(c);
                                        // Read from the local `widths` snapshot taken
                                        // before the engine lock; see comment above.
                                        let w = widths.get(c).copied().unwrap_or(80.0);
                                        let width_style = format!("width:{}px;min-width:{}px;max-width:{}px;", w, w, w);
                                        // Click-to-sort + ▲/▼ indicator removed: sorting now goes
                                        // through the Sort dialog (toolbar / context menu). The
                                        // sort_keys signal still tracks the last-applied chain so
                                        // the dialog can preselect it.
                                        view! {
                                            <th
                                                class="spreadsheet-col-header"
                                                // #72: reactive column-selection highlight.
                                                class:active=move || {
                                                    let (_, sc1, _, sc2) = sel_rect.get();
                                                    c >= sc1 && c <= sc2
                                                }
                                                style=width_style
                                                on:mousedown=move |e: web_sys::MouseEvent| {
                                                    // Right-click is for the context menu —
                                                    // see the `on:contextmenu` handler below.
                                                    if e.button() == 2 { return; }
                                                    // Plain / Ctrl-click selects this column;
                                                    // Shift-click extends the existing sel_col
                                                    // anchor to this column (multi-column range).
                                                    // Mirrors the row-header handler's pattern.
                                                    if !e.shift_key() {
                                                        set_sel_col.set(c);
                                                    }
                                                    set_sel_row.set(0);
                                                    set_active_row.set(display_rows.saturating_sub(1));
                                                    set_active_col.set(c);
                                                    set_editing.set(false);
                                                    // Column-header selection replaces multi-region (#59).
                                                    set_extra_sel_regions.set(Vec::new());
                                                }
                                                on:contextmenu=move |e: web_sys::MouseEvent| {
                                                    e.prevent_default();
                                                    // If the right-clicked column is INSIDE the
                                                    // current selection's column range, preserve
                                                    // the selection. Otherwise select the entire
                                                    // column c.
                                                    let (_, sc1, _, sc2) = sel_bounds(
                                                        sel_row.get_untracked(), sel_col.get_untracked(),
                                                        active_row.get_untracked(), active_col.get_untracked(),
                                                    );
                                                    if c < sc1 || c > sc2 {
                                                        set_sel_row.set(0);
                                                        set_sel_col.set(c);
                                                        set_active_row.set(display_rows.saturating_sub(1));
                                                        set_active_col.set(c);
                                                        set_editing.set(false);
                                                    }
                                                    let (mx, my) = clamp_menu_position(
                                                        e.client_x() as f64,
                                                        e.client_y() as f64,
                                                    );
                                                    set_ctx_menu_x.set(mx);
                                                    set_ctx_menu_y.set(my);
                                                    set_ctx_menu_visible.set(true);
                                                }
                                            >
                                                {label.clone()}
                                                <span
                                                    class="col-filter-icon"
                                                    on:click=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        if filter_col.get_untracked() == Some(c) {
                                                            set_filter_col.set(None);
                                                        } else {
                                                            set_filter_col.set(Some(c));
                                                        }
                                                    }
                                                >"\u{25BD}"</span>
                                                <div
                                                    class="col-resize-handle"
                                                    on:mousedown=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        e.prevent_default();
                                                        set_resize_col.set(Some(c));
                                                        set_resize_start_x.set(e.client_x() as f64);
                                                        set_resize_start_w.set(
                                                            col_widths.get_untracked().get(c).copied().unwrap_or(80.0)
                                                        );
                                                    }
                                                ></div>
                                            </th>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tr>
                            </thead>
                            <tbody>
                                {
                                let mut tr_rows = (0..display_rows)
                                    // #72 virtualization: render frozen rows
                                    // (always — they're sticky) plus the
                                    // visible window; spacers below stand in
                                    // for the rest.
                                    .filter(|&r| !hr.contains(&r)
                                        && (r < frozen_keep
                                            || (r >= virt_start && r < virt_end)))
                                    .map(|r| {
                                    // Clamp to the visible grid: a stale or
                                    // hand-edited frozen count past the data
                                    // extent must not turn every rendered row
                                    // into a sticky one.
                                    let is_frozen_row = r < frozen_r.min(display_rows);
                                    let rh = heights.get(r).copied().unwrap_or(24.0);
                                    let row_style = format!("height:{}px;", rh);
                                    view! {
                                        <tr class:frozen-row=is_frozen_row style=row_style data-row=r>
                                            <td
                                                class="spreadsheet-row-header"
                                                // #72: reactive row-selection highlight.
                                                class:active=move || {
                                                    let (sr1, _, sr2, _) = sel_rect.get();
                                                    r >= sr1 && r <= sr2
                                                }
                                                on:mousedown=move |e: web_sys::MouseEvent| {
                                                    // Right-click is for the context menu — skip
                                                    // selection logic so an existing multi-row
                                                    // range isn't collapsed before contextmenu fires.
                                                    if e.button() == 2 { return; }
                                                    // Shift+click extends the selection from the
                                                    // existing sel_row anchor to this row (full
                                                    // column span). Plain click resets the anchor.
                                                    if !e.shift_key() {
                                                        set_sel_row.set(r);
                                                    }
                                                    set_sel_col.set(0);
                                                    set_active_row.set(r);
                                                    set_active_col.set(display_cols.saturating_sub(1));
                                                    set_editing.set(false);
                                                    // Row-header selection replaces multi-region (#59).
                                                    set_extra_sel_regions.set(Vec::new());
                                                }
                                                on:contextmenu=move |e: web_sys::MouseEvent| {
                                                    e.prevent_default();
                                                    // If the right-clicked row is INSIDE the current
                                                    // selection's row range, preserve the selection.
                                                    // Otherwise select the entire row r before
                                                    // opening the menu.
                                                    let (sr1, _, sr2, _) = sel_bounds(
                                                        sel_row.get_untracked(), sel_col.get_untracked(),
                                                        active_row.get_untracked(), active_col.get_untracked(),
                                                    );
                                                    if r < sr1 || r > sr2 {
                                                        set_sel_row.set(r);
                                                        set_sel_col.set(0);
                                                        set_active_row.set(r);
                                                        set_active_col.set(display_cols.saturating_sub(1));
                                                        set_editing.set(false);
                                                    }
                                                    let (mx, my) = clamp_menu_position(
                                                        e.client_x() as f64,
                                                        e.client_y() as f64,
                                                    );
                                                    set_ctx_menu_x.set(mx);
                                                    set_ctx_menu_y.set(my);
                                                    set_ctx_menu_visible.set(true);
                                                }
                                            >
                                                {r + 1}
                                                <div
                                                    class="row-resize-handle"
                                                    on:mousedown=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        e.prevent_default();
                                                        set_resize_row.set(Some(r));
                                                        set_resize_start_y.set(e.client_y() as f64);
                                                        set_resize_start_h.set(
                                                            row_heights.get_untracked().get(r).copied().unwrap_or(24.0)
                                                        );
                                                    }
                                                ></div>
                                            </td>
                                            {(0..display_cols).filter(|c| !hc.contains(c)).map(|c| {
                                                let display = eng.get_display((c, r));
                                                let raw = eng.get_raw((c, r)).to_string();
                                                let style = eng.get_style((c, r)).cloned();
                                                let mut cell_css = style.as_ref().map(|s| s.to_inline_css()).unwrap_or_default();
                                                // Override bg with conditional format if applicable.
                                                // `background-color` (NOT the `background` shorthand)
                                                // because the shorthand resets `background-image:
                                                // none !important` as a side-effect, which would
                                                // wipe out any DataBar gradient appended below.
                                                if let Some(bg) = eng.get_effective_bg((c, r)) {
                                                    cell_css.push_str(&format!("background-color:{bg} !important;"));
                                                }
                                                // Data bar (M-S2 step 4): paint a horizontal fill
                                                // proportional to the cell's value within the
                                                // rule's range. Stacks on top of any background
                                                // color via `background-image` so a ColorScale +
                                                // DataBar combo still shows the gradient bg
                                                // through the unfilled portion.
                                                if let Some((bar_color, ratio)) = eng.get_data_bar((c, r)) {
                                                    let pct = (ratio * 100.0).round();
                                                    cell_css.push_str(&format!(
                                                        "background-image:linear-gradient(to right, \
                                                         {bar_color} {pct}%, transparent {pct}%);",
                                                    ));
                                                }
                                                // Skip cells hidden by merge (render nothing)
                                                if eng.is_merged_hidden(c, r) {
                                                    return view! { }.into_any();
                                                }
                                                let (merge_cs, merge_rs) = eng.get_merge_span(c, r);
                                                let is_checkbox = eng.is_checkbox((c, r));
                                                let dropdown_opts: Option<Vec<String>> = style.as_ref()
                                                    .and_then(|s| s.validation.as_ref())
                                                    .and_then(|v| if let ValidationRule::Dropdown(opts) = v { Some(opts.clone()) } else { None });
                                                let is_dropdown = dropdown_opts.is_some();
                                                let comment_text: Option<String> = style.as_ref()
                                                    .and_then(|s| s.comment.as_ref())
                                                    .filter(|c| !c.is_empty())
                                                    .cloned();
                                                // Corner-triangle indicator fires for either the
                                                // legacy single-user note or the new threaded
                                                // comment id — both surfaces are "this cell has
                                                // a comment attached" from the user's view.
                                                let has_comment_thread = style.as_ref()
                                                    .and_then(|s| s.comment_thread_id.as_ref())
                                                    .is_some_and(|t| !t.is_empty());
                                                let has_comment = comment_text.is_some() || has_comment_thread;
                                                // Thread preview for this cell, looked up by the
                                                // cell's own `comment_thread_id` (which travels with
                                                // the cell's style through row/column inserts). The
                                                // `has_comment_thread` check above already gates on
                                                // the same id, so a commented cell whose preview is
                                                // still loading falls back to the placeholder rather
                                                // than dropping the marker. Skipped entirely when the
                                                // sheet has no cell threads (the common case).
                                                let thread_preview: Option<CellThreadInfo> =
                                                    if cell_thread_by_id.is_empty() {
                                                        None
                                                    } else {
                                                        style.as_ref()
                                                            .and_then(|s| s.comment_thread_id.as_deref())
                                                            .and_then(|id| cell_thread_by_id.get(id))
                                                            .cloned()
                                                    };
                                                let has_comment = has_comment || thread_preview.is_some();
                                                // Clickable comment indicator. Opens the cell's
                                                // thread (or migrates a legacy note / creates one)
                                                // via the shared open-or-create flow — the same
                                                // path the right-click "Comment" item uses. Only
                                                // built for commented cells so non-commented cells
                                                // pay no per-cell clone.
                                                let comment_marker = has_comment.then(|| {
                                                    let doc_id_click = doc_id_for_grid.clone();
                                                    let alive_click = std::sync::Arc::clone(&alive_for_grid);
                                                    let doc_id_kb = doc_id_for_grid.clone();
                                                    let alive_kb = std::sync::Arc::clone(&alive_for_grid);
                                                    view! {
                                                        <span
                                                            class="ss-comment-marker"
                                                            role="button"
                                                            tabindex="0"
                                                            title=crate::t!("ss-ctx-open-comment")
                                                            aria-label=crate::t!("ss-ctx-open-comment")
                                                            on:mousedown=move |e: web_sys::MouseEvent| {
                                                                // Keep the grid's selection /
                                                                // context-menu handlers off the
                                                                // marker — it's its own target.
                                                                e.stop_propagation();
                                                            }
                                                            on:click=move |e: web_sys::MouseEvent| {
                                                                e.stop_propagation();
                                                                cell_comment::open_or_create_cell_comment(
                                                                    engine,
                                                                    doc_id_click.clone(),
                                                                    comment_sheet_idx,
                                                                    c, r,
                                                                    e.client_x() as f64,
                                                                    e.client_y() as f64,
                                                                    persist,
                                                                    on_open_cell_comment,
                                                                    std::sync::Arc::clone(&alive_click),
                                                                );
                                                            }
                                                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                                                if e.key() != "Enter" && e.key() != " " { return; }
                                                                e.prevent_default();
                                                                // No pointer coords on keyboard
                                                                // activation; anchor the popup to
                                                                // the marker's box instead.
                                                                let (left, top) = e.target()
                                                                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                                                    .map(|el| {
                                                                        let rect = el.get_bounding_client_rect();
                                                                        (rect.left(), rect.bottom())
                                                                    })
                                                                    .unwrap_or((0.0, 0.0));
                                                                cell_comment::open_or_create_cell_comment(
                                                                    engine,
                                                                    doc_id_kb.clone(),
                                                                    comment_sheet_idx,
                                                                    c, r,
                                                                    left, top,
                                                                    persist,
                                                                    on_open_cell_comment,
                                                                    std::sync::Arc::clone(&alive_kb),
                                                                );
                                                            }
                                                        ></span>
                                                    }.into_any()
                                                });
                                                // Hover preview. Thread-aware (opening message +
                                                // reply count) when the page has loaded a matching
                                                // thread; otherwise the legacy single-string note,
                                                // or a placeholder for a thread whose preview
                                                // hasn't arrived yet.
                                                let comment_preview = has_comment.then(|| {
                                                    if let Some(info) = thread_preview.clone() {
                                                        let first = info.first_message.unwrap_or_else(
                                                            || crate::t!("ss-comment-preview-empty"));
                                                        let replies = match info.reply_count {
                                                            0 => crate::t!("ss-comment-replies-none"),
                                                            1 => crate::t!("ss-comment-replies-one"),
                                                            n => crate::t!("ss-comment-replies-many", count = n.to_string()),
                                                        };
                                                        view! {
                                                            <div class="ss-comment-popup">
                                                                <div class="ss-comment-popup-msg">{first}</div>
                                                                <div class="ss-comment-popup-meta">{replies}</div>
                                                            </div>
                                                        }.into_any()
                                                    } else if let Some(t) = comment_text.clone() {
                                                        view! { <div class="ss-comment-popup">{t}</div> }.into_any()
                                                    } else {
                                                        view! {
                                                            <div class="ss-comment-popup">
                                                                {crate::t!("ss-comment-preview-empty")}
                                                            </div>
                                                        }.into_any()
                                                    }
                                                });
                                                // IconSet conditional-format glyph, if any rule
                                                // covers this cell. Rendered as a prefix span
                                                // before the cell value so the icon doesn't
                                                // displace alignment.
                                                let cf_icon: Option<&'static str> = eng.get_icon((c, r));
                                                // #72: `is_cursor` is kept (untracked) only to seed
                                                // `is_editing_this` (the edit-input swap) below; the
                                                // visible cursor/selection highlight is applied
                                                // imperatively by the selection Effect (O(1) per move).
                                                let is_cursor = r == ar && c == ac;
                                                let is_formula = raw.starts_with('=');
                                                // Spill-fill rendering (M-S1a step 4): cells whose
                                                // value comes from a dynamic-array formula anchored
                                                // elsewhere get the `spill-fill` class so users can
                                                // see at a glance which cells aren't independently
                                                // editable. The styled rule is in the spreadsheet
                                                // CSS; the engine query is collision-free with
                                                // `is_formula` (a spill-filled cell has no formula
                                                // and no raw text of its own).
                                                let is_spill_fill = eng.is_spill_fill((c, r));
                                                // Frozen-column rendering (M-S2). Each frozen
                                                // column's `<td>` needs its own `left` offset =
                                                // row-header-width + Σ widths of the columns to
                                                // its left. Computed here from `col_widths` so a
                                                // resized column shifts every column to its right.
                                                // 40px is the row-header column width
                                                // (`.spreadsheet-corner` / `.spreadsheet-row-header`
                                                // in spreadsheet.css) — kept in sync with the CSS.
                                                let is_frozen_col = c < frozen_c.min(display_cols);
                                                if is_frozen_col {
                                                    let widths = col_widths.get();
                                                    let mut offset = 40.0_f64;
                                                    for prev in 0..c {
                                                        if !hc.contains(&prev) {
                                                            offset += widths.get(prev).copied().unwrap_or(80.0);
                                                        }
                                                    }
                                                    cell_css.push_str(&format!("left:{offset}px;"));
                                                }
                                                let is_editing_this = is_cursor && editing_now && !is_checkbox;
                                                let in_cut = cut_source.get().map_or(false, |(c1, r1, c2, r2)| {
                                                    c >= c1 && c <= c2 && r >= r1 && r <= r2
                                                });
                                                let in_pick = ref_pick.get().map_or(false, |p| p.contains(c, r));
                                                // Remote-cursor outline + badge for this cell, if
                                                // any peer's awareness cursor lands here. Applied
                                                // inline so the geometry follows the cell's actual
                                                // rendered box (border + padding included) — the
                                                // pre-#70 absolute-positioned overlay computed
                                                // pixels from `col_widths.get()` and accumulated
                                                // a per-column offset because cells default to
                                                // `box-sizing: content-box`.
                                                // #72: the remote-cursor outline + name badge are now
                                                // applied REACTIVELY per cell (the `style` closure and
                                                // the badge child below read the `remote_by_cell` Memo),
                                                // so a peer cursor move updates only the affected cells
                                                // instead of re-running this whole render. `static_css`
                                                // holds the non-cursor style built once for this render
                                                // pass; the reactive `style` closure appends the
                                                // cursor outline (and the `position:relative;z-index:1`
                                                // the badge anchors to / the frozen-col override needs).
                                                let static_css = cell_css;
                                                view! {
                                                    <td
                                                        class="spreadsheet-cell"
                                                        // #72: cursor / selected / tap-anchor are applied
                                                        // imperatively by the single selection Effect above
                                                        // (O(1) per move), not as per-cell reactive classes.
                                                        class:cut-marquee=in_cut
                                                        class:reference-pick=in_pick
                                                        class:formula-cell=is_formula
                                                        class:spill-fill=is_spill_fill
                                                        class:frozen-col=is_frozen_col
                                                        class:has-comment=has_comment
                                                        style=move || {
                                                            // #72: reactive — re-evaluated only when the
                                                            // remote_by_cell Memo changes (peer cursor
                                                            // move), updating this cell's style attr in
                                                            // place rather than rebuilding the grid.
                                                            // `.with` borrows the map (no full clone) and
                                                            // pulls just this cell's colour.
                                                            let mut s = static_css.clone();
                                                            if let Some(color) =
                                                                remote_by_cell.with(|m| m.get(&(c, r)).map(|(_, col)| col.clone()))
                                                            {
                                                                // z-index:1 + position:relative match the
                                                                // pre-reactive inline rule: keep the outline
                                                                // above adjacent frozen-col (sticky, z:2)
                                                                // cells and give the name badge a
                                                                // positioned ancestor.
                                                                s.push_str(&format!(
                                                                    "outline:2px solid {color};outline-offset:-1px;position:relative;z-index:1;"
                                                                ));
                                                            }
                                                            s
                                                        }
                                                        colspan=merge_cs
                                                        rowspan=merge_rs
                                                        data-row=r
                                                        data-col=c
                                                    >
                                                        {if is_checkbox {
                                                            let checked = display.to_uppercase() == "TRUE";
                                                            view! {
                                                                <input
                                                                    type="checkbox"
                                                                    class="spreadsheet-checkbox"
                                                                    prop:checked=checked
                                                                    on:change=move |_| {
                                                                        engine.lock().unwrap().toggle_checkbox((c, r));
                                                                        persist();
                                                                    }
                                                                />
                                                            }.into_any()
                                                        } else if is_dropdown && is_cursor && editing_now {
                                                            let opts = dropdown_opts.unwrap_or_default();
                                                            let current_val = display.clone();
                                                            view! {
                                                                <select
                                                                    class="spreadsheet-cell-select"
                                                                    on:change=move |e| {
                                                                        let val = event_target_value(&e);
                                                                        set_edit_value.set(val.clone());
                                                                        engine.lock().unwrap().set_cell((c, r), &val);
                                                                        set_editing.set(false);
                                                                        persist();
                                                                        refocus_wrapper();
                                                                    }
                                                                    on:blur=move |_| set_editing.set(false)
                                                                >
                                                                    <option value="" selected=current_val.is_empty()>"—"</option>
                                                                    {opts.iter().map(|opt| {
                                                                        let selected = *opt == current_val;
                                                                        let val = opt.clone();
                                                                        let val2 = val.clone();
                                                                        view! { <option value=val selected=selected>{val2}</option> }
                                                                    }).collect::<Vec<_>>()}
                                                                </select>
                                                            }.into_any()
                                                        } else if is_editing_this {
                                                            view! {
                                                                <div class="spreadsheet-cell-edit-wrapper">
                                                                    <input
                                                                        type="text"
                                                                        class="spreadsheet-cell-input"
                                                                        inputmode=move || {
                                                                            // Suppress the OS soft keyboard whenever our
                                                                            // in-page keyboard is mounted AND its current
                                                                            // mode owns the entry surface (Numeric or
                                                                            // Formula). Standard mode defers to the OS
                                                                            // keyboard, so hint based on column data.
                                                                            if formula_keyboard_visible.get()
                                                                                && effective_kb_mode.get().suppresses_os_keyboard()
                                                                            {
                                                                                "none"
                                                                            } else {
                                                                                infer_column_inputmode(
                                                                                    &engine.lock().unwrap(),
                                                                                    active_col.get(),
                                                                                    50,
                                                                                )
                                                                            }
                                                                        }
                                                                        prop:value=move || edit_value.get()
                                                                        on:input=move |e| {
                                                                            let val = event_target_value(&e);
                                                                            set_edit_value.set(val.clone());
                                                                            // Any user-typed character clears an in-progress ref
                                                                            // pick. If the typed char was an operator/paren/comma,
                                                                            // a subsequent arrow or click will re-enter pick mode
                                                                            // via `is_ref_context`; otherwise the user has moved
                                                                            // on to literal text.
                                                                            if ref_pick.get_untracked().is_some() {
                                                                                set_ref_pick.set(None);
                                                                            }
                                                                            // Autocomplete: extract partial function name after =
                                                                            if val.starts_with('=') {
                                                                                let after_eq = &val[1..];
                                                                                // Find the last token start (after (, , or start)
                                                                                let partial = after_eq.rsplit(|c: char| c == '(' || c == ',' || c == '+' || c == '-' || c == '*' || c == '/' || c == ' ')
                                                                                    .next().unwrap_or("");
                                                                                let partial_upper = partial.to_uppercase();
                                                                                if !partial_upper.is_empty() && partial.chars().all(|c| c.is_ascii_alphabetic() || c == '.') {
                                                                                    // Stable-partition by COMMON_FUNCTIONS
                                                                                    // priority so high-demand functions
                                                                                    // (SUM, AVERAGE, IF, COUNT, MIN, MAX,
                                                                                    // VLOOKUP, SUMIF) rank above same-
                                                                                    // prefix peers. Without this the
                                                                                    // pure-alphabetical order returns
                                                                                    // SUBSTITUTE then SUBTOTAL above SUM
                                                                                    // for partial SU — surprising for the
                                                                                    // user. Mirror const in formula_keyboard.rs.
                                                                                    let mut matches: Vec<(&str, &str)> = FUNCTION_LIST.iter()
                                                                                        .filter(|(name, _)| name.starts_with(&partial_upper))
                                                                                        .copied()
                                                                                        .collect();
                                                                                    matches.sort_by_key(|(name, _)| {
                                                                                        COMMON_FUNCTIONS.iter()
                                                                                            .position(|&c| c == *name)
                                                                                            .unwrap_or(usize::MAX)
                                                                                    });
                                                                                    if !matches.is_empty() {
                                                                                        set_ac_matches.set(matches);
                                                                                        set_ac_index.set(0);
                                                                                        set_ac_visible.set(true);
                                                                                    } else {
                                                                                        set_ac_visible.set(false);
                                                                                    }
                                                                                } else {
                                                                                    set_ac_visible.set(false);
                                                                                }
                                                                            } else {
                                                                                set_ac_visible.set(false);
                                                                            }
                                                                        }
                                                                        on:keydown=move |e: web_sys::KeyboardEvent| {
                                                                            if ac_visible.get_untracked() {
                                                                                match e.key().as_str() {
                                                                                    "ArrowDown" => {
                                                                                        e.prevent_default();
                                                                                        e.stop_propagation();
                                                                                        let len = ac_matches.get_untracked().len();
                                                                                        if len > 0 { set_ac_index.set((ac_index.get_untracked() + 1) % len); }
                                                                                        return;
                                                                                    }
                                                                                    "ArrowUp" => {
                                                                                        e.prevent_default();
                                                                                        e.stop_propagation();
                                                                                        let len = ac_matches.get_untracked().len();
                                                                                        if len > 0 { set_ac_index.set(ac_index.get_untracked().checked_sub(1).unwrap_or(len - 1)); }
                                                                                        return;
                                                                                    }
                                                                                    "Enter" | "Tab" => {
                                                                                        e.prevent_default();
                                                                                        e.stop_propagation();
                                                                                        let matches = ac_matches.get_untracked();
                                                                                        let idx = ac_index.get_untracked();
                                                                                        if let Some((name, _)) = matches.get(idx) {
                                                                                            // Replace the partial with the full function name + (
                                                                                            let val = edit_value.get_untracked();
                                                                                            let after_eq = &val[1..];
                                                                                            let last_delim = after_eq.rfind(|c: char| c == '(' || c == ',' || c == '+' || c == '-' || c == '*' || c == '/' || c == ' ');
                                                                                            let prefix = match last_delim {
                                                                                                Some(pos) => format!("={}{}", &after_eq[..=pos], name),
                                                                                                None => format!("={}", name),
                                                                                            };
                                                                                            set_edit_value.set(format!("{}(", prefix));
                                                                                        }
                                                                                        set_ac_visible.set(false);
                                                                                        return;
                                                                                    }
                                                                                    "Escape" => {
                                                                                        e.stop_propagation();
                                                                                        set_ac_visible.set(false);
                                                                                        return;
                                                                                    }
                                                                                    _ => {}
                                                                                }
                                                                            }
                                                                            if e.key() == "Enter" {
                                                                                e.prevent_default();
                                                                                e.stop_propagation();
                                                                                set_ac_visible.set(false);
                                                                                commit_edit();
                                                                                move_active(1, 0, false);
                                                                            } else if e.key() == "Tab" {
                                                                                e.prevent_default();
                                                                                e.stop_propagation();
                                                                                set_ac_visible.set(false);
                                                                                commit_edit();
                                                                                move_active(0, 1, false);
                                                                            } else if e.key() == "Escape" {
                                                                                e.stop_propagation();
                                                                                set_ac_visible.set(false);
                                                                                // Escape rolls back a pending pick but keeps
                                                                                // the user in edit mode (Excel behavior).
                                                                                // Only the second Escape exits edit.
                                                                                if let Some(pick) = ref_pick.get_untracked() {
                                                                                    let val = edit_value.get_untracked();
                                                                                    let truncated = val
                                                                                        .get(..pick.insert_at)
                                                                                        .unwrap_or(&val)
                                                                                        .to_string();
                                                                                    set_edit_value.set(truncated);
                                                                                    set_ref_pick.set(None);
                                                                                } else {
                                                                                    set_editing.set(false);
                                                                                }
                                                                            } else if matches!(e.key().as_str(), "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight") {
                                                                                // In ref-pick context, arrows build/move a
                                                                                // cell reference into the formula instead of
                                                                                // committing and moving the active cell.
                                                                                let val = edit_value.get_untracked();
                                                                                let caret = val.len(); // v1: caret assumed at end
                                                                                let current_pick = ref_pick.get_untracked();
                                                                                let in_pick_ctx =
                                                                                    current_pick.is_some() || is_ref_context(&val, caret);

                                                                                if in_pick_ctx {
                                                                                    e.prevent_default();
                                                                                    e.stop_propagation();
                                                                                    set_ac_visible.set(false);

                                                                                    let (dr, dc): (isize, isize) = match e.key().as_str() {
                                                                                        "ArrowUp" => (-1, 0),
                                                                                        "ArrowDown" => (1, 0),
                                                                                        "ArrowLeft" => (0, -1),
                                                                                        "ArrowRight" => (0, 1),
                                                                                        _ => (0, 0),
                                                                                    };
                                                                                    let max_cols = grid_cols.get_untracked();
                                                                                    let max_rows = grid_rows.get_untracked();
                                                                                    let shift = e.shift_key();

                                                                                    // Base position for the new end:
                                                                                    //   first arrow — adjacent to the editing cell
                                                                                    //   subsequent — previous pick's end
                                                                                    let (base_c, base_r) = match &current_pick {
                                                                                        Some(p) => p.end,
                                                                                        None => (
                                                                                            active_col.get_untracked(),
                                                                                            active_row.get_untracked(),
                                                                                        ),
                                                                                    };
                                                                                    let new_c = ((base_c as isize + dc).max(0)
                                                                                        .min(max_cols.saturating_sub(1) as isize))
                                                                                        as usize;
                                                                                    let new_r = ((base_r as isize + dr).max(0)
                                                                                        .min(max_rows.saturating_sub(1) as isize))
                                                                                        as usize;

                                                                                    let new_pick = match (&current_pick, shift) {
                                                                                        (Some(p), true) => RefPick {
                                                                                            start: p.start,
                                                                                            end: (new_c, new_r),
                                                                                            insert_at: p.insert_at,
                                                                                        },
                                                                                        (Some(p), false) => RefPick {
                                                                                            start: (new_c, new_r),
                                                                                            end: (new_c, new_r),
                                                                                            insert_at: p.insert_at,
                                                                                        },
                                                                                        (None, _) => RefPick {
                                                                                            start: (new_c, new_r),
                                                                                            end: (new_c, new_r),
                                                                                            insert_at: val.len(),
                                                                                        },
                                                                                    };
                                                                                    let new_val = splice_ref(&val, new_pick.insert_at, &new_pick.label());
                                                                                    set_edit_value.set(new_val);
                                                                                    set_ref_pick.set(Some(new_pick));
                                                                                    // Keep the caret parked at end of the
                                                                                    // input so the input value and our
                                                                                    // insert_at stay consistent.
                                                                                    if let Some(input) = cell_input_ref.get() {
                                                                                        let el: web_sys::HtmlInputElement = input.into();
                                                                                        let len = el.value().len() as u32;
                                                                                        let _ = el.set_selection_range(len, len);
                                                                                    }
                                                                                } else {
                                                                                    // Not in pick context — commit + move.
                                                                                    e.prevent_default();
                                                                                    e.stop_propagation();
                                                                                    set_ac_visible.set(false);
                                                                                    commit_edit();
                                                                                    match e.key().as_str() {
                                                                                        "ArrowUp" => move_active(-1, 0, false),
                                                                                        "ArrowDown" => move_active(1, 0, false),
                                                                                        "ArrowLeft" => move_active(0, -1, false),
                                                                                        "ArrowRight" => move_active(0, 1, false),
                                                                                        _ => {}
                                                                                    }
                                                                                }
                                                                            }
                                                                        }
                                                                        on:blur=move |_| {
                                                                            set_ac_visible.set(false);
                                                                            // If the blur was caused by a cell mousedown in
                                                                            // pick mode, ref_pick will still be Some — in
                                                                            // that case, don't commit. The mousedown handler
                                                                            // already called preventDefault to keep focus,
                                                                            // but some browsers blur anyway; this guards it.
                                                                            if ref_pick.get_untracked().is_none() {
                                                                                commit_edit();
                                                                            }
                                                                        }
                                                                        node_ref=cell_input_ref
                                                                    />
                                                                    {move || {
                                                                        if !ac_visible.get() { return view! { <span></span> }.into_any(); }
                                                                        let matches = ac_matches.get();
                                                                        let idx = ac_index.get();
                                                                        // M-P3 piece D: re-anchor the dropdown above
                                                                        // the on-screen keyboard whenever the keyboard
                                                                        // is mounted in Formula mode. Below the cell
                                                                        // would otherwise vanish behind the keyboard
                                                                        // for any cell in the lower half of the
                                                                        // viewport.
                                                                        let above_keyboard = formula_keyboard_visible.get()
                                                                            && effective_kb_mode.get() == KeyboardMode::Formula;
                                                                        view! {
                                                                            <div
                                                                                class="ss-autocomplete"
                                                                                class:is-above-keyboard=above_keyboard
                                                                            >
                                                                                {matches.iter().enumerate().map(|(i, (name, desc))| {
                                                                                    let is_active = i == idx;
                                                                                    let name_s = name.to_string();
                                                                                    let desc_s = desc.to_string();
                                                                                    view! {
                                                                                        <div
                                                                                            class="ss-autocomplete-item"
                                                                                            class:active=is_active
                                                                                        >
                                                                                            <span class="ss-ac-name">{name_s}</span>
                                                                                            <span class="ss-ac-desc">{desc_s}</span>
                                                                                        </div>
                                                                                    }
                                                                                }).collect::<Vec<_>>()}
                                                                            </div>
                                                                        }.into_any()
                                                                    }}
                                                                </div>
                                                            }.into_any()
                                                        } else {
                                                            view! {
                                                                <span>
                                                                    {cf_icon.map(|g| view! {
                                                                        <span class="ss-cf-icon">{g}</span>
                                                                    })}
                                                                    {display}
                                                                </span>
                                                            }.into_any()
                                                        }}
                                                        {comment_marker}
                                                        {comment_preview}
                                                        // Remote-cursor name badge (#63 + #70).
                                                        // The cell already carries an inline
                                                        // outline; this floats the user's name
                                                        // tag just above its top edge via the
                                                        // existing `.ss-remote-cell-label` rule
                                                        // (which is `position: absolute` and
                                                        // anchors to the now `position: relative`
                                                        // cell).
                                                        // #72: reactive — Leptos swaps just this badge
                                                        // in/out when the peer cursor enters/leaves this
                                                        // cell, without re-rendering the grid.
                                                        {move || remote_by_cell.with(|m| m.get(&(c, r)).cloned()).map(|(name, color)| view! {
                                                            <span class="ss-remote-cell-label"
                                                                style=format!("background:{};", color)>
                                                                {name}
                                                            </span>
                                                        })}
                                                    </td>
                                                }.into_any()
                                            }).collect::<Vec<_>>()}
                                        </tr>
                                    }.into_any()
                                }).collect::<Vec<_>>();
                                // #72 virtualization: spacer rows preserve the
                                // scroll geometry of the unrendered rows. The
                                // top spacer sits after the (always-rendered)
                                // frozen rows; one borderless full-width td
                                // keeps the table layout intact.
                                let spacer = |px: f64| view! {
                                    <tr class="ss-virt-spacer"
                                        style=format!("height:{px}px;")>
                                        <td colspan=display_cols + 1></td>
                                    </tr>
                                }.into_any();
                                if virt_bot_px > 0.0 {
                                    tr_rows.push(spacer(virt_bot_px));
                                }
                                if virt_top_px > 0.0 {
                                    let n_frozen_rendered = (0..frozen_keep.min(virt_start))
                                        .filter(|r| !hr.contains(r))
                                        .count();
                                    tr_rows.insert(n_frozen_rendered, spacer(virt_top_px));
                                }
                                tr_rows
                                }
                            </tbody>
                        </table>
                    }.into_any()
                }}

                // Remote-cursor outline + name tag is rendered
                // per-cell inside the table render above; see
                // `remote_by_cell` + the inline outline / badge
                // appended to each affected `<td>`. The earlier
                // absolute-positioned overlay block lived here and
                // suffered from pixel misalignment because cells
                // are `box-sizing: content-box` by default — see
                // issue #70.
            </div>

            // ─── Sheet Tab Bar ─────────────────────────────
            {render_sheet_tab_bar(
                sheet_names, set_sheet_names,
                active_sheet, set_active_sheet,
                grid_version, set_grid_version,
                engine, persist, delete_sheet,
            )}

            // ─── Selection status bar (Excel-style) ────────
            // Aggregates numeric values in the current selection and
            // surfaces them at the bottom-right. Hidden when the
            // selection is a single cell — matches Excel's visual
            // "only worth showing for ranges" rule.
            {move || {
                let _v = grid_version.get();
                let (r1, c1, r2, c2) = sel_bounds(
                    sel_row.get(), sel_col.get(),
                    active_row.get(), active_col.get(),
                );
                if r1 == r2 && c1 == c2 {
                    return view! { <span></span> }.into_any();
                }
                let mut count = 0_usize;
                let mut sum = 0.0_f64;
                let mut numeric = 0_usize;
                let mut min = f64::INFINITY;
                let mut max = f64::NEG_INFINITY;
                {
                    let eng = engine.lock().unwrap();
                    for r in r1..=r2 {
                        for c in c1..=c2 {
                            match eng.get_value((c, r)) {
                                CellValue::Number(n) if n.is_finite() => {
                                    count += 1;
                                    numeric += 1;
                                    sum += *n;
                                    if *n < min { min = *n; }
                                    if *n > max { max = *n; }
                                }
                                CellValue::Empty => {}
                                _ => { count += 1; }
                            }
                        }
                    }
                }
                // Format numeric stats with at most 4 fractional digits,
                // trimming trailing zeros so integers render as "5"
                // not "5.0000". Locale-independent — keeps the bar
                // compact and predictable for screenshots/tests.
                let fmt = |n: f64| -> String {
                    if n == n.trunc() && n.abs() < 1e15 {
                        format!("{}", n as i64)
                    } else {
                        let s = format!("{:.4}", n);
                        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
                        s
                    }
                };
                let avg = if numeric > 0 { Some(sum / numeric as f64) } else { None };
                view! {
                    <div class="ss-status-bar">
                        <span class="ss-status-stat">{crate::t!("ss-status-count", value = count.to_string())}</span>
                        {(numeric > 0).then(|| view! {
                            <>
                                <span class="ss-status-sep">"|"</span>
                                <span class="ss-status-stat">{crate::t!("ss-status-sum", value = fmt(sum))}</span>
                                <span class="ss-status-sep">"|"</span>
                                <span class="ss-status-stat">{crate::t!("ss-status-avg", value = fmt(avg.unwrap()))}</span>
                                <span class="ss-status-sep">"|"</span>
                                <span class="ss-status-stat">{crate::t!("ss-status-min", value = fmt(min))}</span>
                                <span class="ss-status-sep">"|"</span>
                                <span class="ss-status-stat">{crate::t!("ss-status-max", value = fmt(max))}</span>
                            </>
                        })}
                    </div>
                }.into_any()
            }}

            // ─── Charts ────────────────────────────────────
            {move || {
                let _v = grid_version.get();
                let eng = engine.lock().unwrap();
                let charts = eng.charts.clone();
                if charts.is_empty() { return view! { <span></span> }.into_any(); }
                view! {
                    <div class="ss-charts-area">
                        {charts.iter().enumerate().map(|(i, chart)| {
                            let svg = render_chart_svg(chart, &eng);
                            let title = chart.title.clone();
                            view! {
                                <div class="ss-chart">
                                    <div class="ss-chart-inner" inner_html=svg></div>
                                    <button class="ss-chart-remove" on:click=move |_| {
                                        engine.lock().unwrap().charts.remove(i);
                                        persist();
                                    }>"\u{2715}"</button>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>
                }.into_any()
            }}

            // ─── Context Menu ──────────────────────────────
            {render_context_menu(ContextMenuDeps {
                engine,
                alive: std::sync::Arc::clone(&alive),
                ctx_menu_visible, set_ctx_menu_visible,
                ctx_menu_x, ctx_menu_y,
                active_row, active_col,
                sel_row, sel_col,
                extra_sel_regions,
                // #54: copy the primary selection as a GFM markdown table.
                copy_as_markdown: Callback::new(move |_| {
                    let bounds = sel_bounds(
                        sel_row.get_untracked(), sel_col.get_untracked(),
                        active_row.get_untracked(), active_col.get_untracked(),
                    );
                    let md = {
                        let eng = engine.lock().unwrap();
                        selection_to_markdown(&eng, bounds)
                    };
                    write_text_to_os_clipboard(md);
                }),
                frozen_rows, set_frozen_rows,
                frozen_cols, set_frozen_cols,
                set_hidden_rows, set_hidden_cols,
                set_grid_rows, set_grid_cols, set_col_widths,
                set_pivot_editor_open,
                set_sort_dialog_open,
                sort_keys,
                grid_rows,
                grid_cols,
                doc_id: doc_id.clone(),
                active_sheet,
                on_open_cell_comment,
                persist, record_undo, sort_by_column,
                insert_row_at, insert_col_at,
                delete_row_at, delete_col_at,
            })}

            // ─── Filter Dropdown ───────────────────────────
            {render_filter_dropdown(
                filter_col, set_filter_col,
                grid_rows,
                hidden_rows, set_hidden_rows,
                engine,
            )}

            // ─── Cross-Document Consent Prompt ─────────────
            {render_foreign_consent(
                consent_pending, set_consent_pending,
                set_consent_approved,
                {
                    let alive = std::sync::Arc::clone(&alive);
                    move |ids: Vec<String>| {
                        // On approve: dispatch a fetch for each id.
                        for id in ids {
                            spawn_foreign_doc_fetch(
                                engine, fetched_ids,
                                set_grid_version, grid_version,
                                on_subscribe_foreign, id,
                                std::sync::Arc::clone(&alive),
                            );
                        }
                    }
                },
                move |ids: Vec<String>| {
                    // On deny: cache each as Denied so REFERENCE*
                    // surfaces #REF! and the engine doesn't re-queue
                    // a fetch.
                    use crate::spreadsheet::eval::ForeignFetchError;
                    let mut eng = engine.lock().unwrap();
                    for id in ids {
                        eng.set_foreign_doc_error(id, ForeignFetchError::Denied);
                    }
                    drop(eng);
                    set_grid_version.update(|v| *v = v.wrapping_add(1));
                },
            )}

            // ─── Pivot Editor Sidebar ──────────────────────
            {render_pivot_editor(
                pivot_editor_open, set_pivot_editor_open,
                grid_version,
                pivot_filter_popover_open, set_pivot_filter_popover_open,
                pivot_group_picker_open, set_pivot_group_picker_open,
                engine, persist,
            )}

            // ─── Sort Dialog ────────────────────────────────
            {render_sort_dialog(
                sort_dialog_open, set_sort_dialog_open,
                set_sort_keys, grid_cols, engine,
                sort_by_keys_in_range,
            )}

            // ─── Find/Replace Bar ──────────────────────────
            {render_find_replace_bar(
                find_visible, set_find_visible,
                find_query, set_find_query,
                find_matches, set_find_matches,
                find_index, set_find_index,
                replace_text, set_replace_text,
                grid_rows, grid_cols,
                engine, persist,
                select_cell, scroll_active_into_view, refocus_wrapper,
            )}

            <FormulaKeyboard
                edit_value=edit_value
                set_edit_value=set_edit_value
                cell_input_ref=cell_input_ref
                visible=formula_keyboard_visible
                mode=kb_mode
                set_mode=set_kb_mode
                on_commit=Callback::new(move |()| {
                    commit_edit();
                    set_editing.set(false);
                })
                on_cancel=Callback::new(move |()| {
                    set_editing.set(false);
                })
            />
        </div>
    }
}

// ─── Tests for ref-pick helpers ────────────────────────────────

#[cfg(test)]
mod block_id_tests {
    use super::*;

    #[test]
    fn parses_simple_cell_block_id() {
        assert_eq!(
            parse_ss_block_id("ss:Sheet1:c:5:3"),
            Some(("Sheet1".to_string(), 5, 3)),
        );
    }

    #[test]
    fn parses_zero_row_zero_col() {
        assert_eq!(
            parse_ss_block_id("ss:Sheet2:c:0:0"),
            Some(("Sheet2".to_string(), 0, 0)),
        );
    }

    #[test]
    fn rejects_non_spreadsheet_block_id() {
        assert!(parse_ss_block_id("block_abc123").is_none());
        assert!(parse_ss_block_id("p:para:1").is_none());
        assert!(parse_ss_block_id("").is_none());
    }

    #[test]
    fn rejects_malformed_indices() {
        assert!(parse_ss_block_id("ss:Sheet1:c:five:3").is_none());
        assert!(parse_ss_block_id("ss:Sheet1:c:5").is_none());
    }
}

#[cfg(test)]
mod menu_clamp_tests {
    use super::*;

    // These tests exercise the pure-math half of `clamp_menu_position`,
    // `clamp_menu_position_in_viewport`, so they can run under plain
    // `cargo test` without a wasm-bindgen JS context.
    // Menu-size constants encoded in the helper: MENU_W = 250,
    // MENU_H = 320, EDGE = 4. Viewport in these tests is fixed at
    // 1024×768 to match the production fallback.

    const VW: f64 = 1024.0;
    const VH: f64 = 768.0;

    #[test]
    fn well_inside_viewport_returns_unchanged() {
        // (100, 100): right edge would be 350 < 1020, bottom 420 < 764.
        // No flip on either axis.
        assert_eq!(
            clamp_menu_position_in_viewport(100.0, 100.0, VW, VH),
            (100.0, 100.0),
        );
    }

    #[test]
    fn flips_left_when_natural_right_edge_overflows() {
        // x=900 → 900+250=1150 > 1020 → flip to 900-250=650 (>= EDGE).
        let (cx, cy) = clamp_menu_position_in_viewport(900.0, 100.0, VW, VH);
        assert_eq!(cx, 650.0);
        assert_eq!(cy, 100.0);
    }

    #[test]
    fn flips_up_when_natural_bottom_overflows() {
        // y=700 → 700+320=1020 > 764 → flip to 700-320=380.
        let (cx, cy) = clamp_menu_position_in_viewport(100.0, 700.0, VW, VH);
        assert_eq!(cx, 100.0);
        assert_eq!(cy, 380.0);
    }

    #[test]
    fn flips_both_axes_when_both_overflow() {
        let (cx, cy) = clamp_menu_position_in_viewport(1100.0, 800.0, VW, VH);
        assert_eq!(cx, 850.0);  // 1100 - 250
        assert_eq!(cy, 480.0);  // 800 - 320
    }

    #[test]
    fn corner_past_both_edges_still_flips_toward_edge() {
        // The helper's "conservative estimate" doesn't recursively
        // clamp — flipping a far-overflowing click just moves the
        // menu one menu-size leftward / upward. We only assert it
        // moved at all.
        let (cx, cy) = clamp_menu_position_in_viewport(1500.0, 1500.0, VW, VH);
        assert!(cx < 1500.0, "expected flip-left, got x={cx}");
        assert!(cy < 1500.0, "expected flip-up, got y={cy}");
    }

    #[test]
    fn flip_target_below_edge_clamps_to_edge_floor() {
        // Construct viewports + clicks so `x - MENU_W` would be
        // negative: tiny viewport (300×200) + click near right edge.
        // The helper guarantees the returned coords are >= EDGE (= 4).
        for &(x, y) in &[
            (200.0_f64, 100.0_f64), // 200+250 > 296 → flip to -50 → floor at 4
            (10.0, 250.0),          // 250+320 > 196 → flip to -70 → floor at 4
        ] {
            let (cx, cy) = clamp_menu_position_in_viewport(x, y, 300.0, 200.0);
            assert!(cx >= 4.0, "cx={cx} below EDGE for ({x},{y})");
            assert!(cy >= 4.0, "cy={cy} below EDGE for ({x},{y})");
        }
    }

    #[test]
    fn flipped_coords_floor_at_edge_for_all_overflowing_inputs() {
        // For every input where natural placement would overflow,
        // the flipped output lands at or above EDGE. (Inputs that
        // don't trigger a flip are skipped — those return the
        // input unchanged, which is allowed to be 0.)
        for &x in &[1500.0_f64, 5000.0] {  // both overflow on 1024
            for &y in &[1500.0_f64, 5000.0] {  // both overflow on 768
                let (cx, cy) = clamp_menu_position_in_viewport(x, y, VW, VH);
                assert!(cx >= 4.0, "x out of bounds at ({x},{y}): cx={cx}");
                assert!(cy >= 4.0, "y out of bounds at ({x},{y}): cy={cy}");
            }
        }
    }
}

#[cfg(test)]
mod toolbar_command_tests {
    use super::*;
    use crate::components::toolbar::ToolbarCommand;
    use crate::spreadsheet::eval::SpreadsheetEngine;

    fn engine_with_styles(cells: &[(usize, usize)]) -> SpreadsheetEngine {
        let mut e = SpreadsheetEngine::new();
        for &(c, r) in cells {
            e.set_cell((c, r), "x");
        }
        e
    }

    #[test]
    fn toggle_bold_sets_when_none_have_it() {
        let mut e = engine_with_styles(&[(0, 0), (1, 0), (0, 1), (1, 1)]);
        let handled = apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleBold, &[(0, 0, 1, 1)],
        );
        assert!(handled);
        for r in 0..=1 {
            for c in 0..=1 {
                assert!(e.get_style((c, r)).map_or(false, |s| s.bold),
                    "expected bold at ({c},{r})");
            }
        }
    }

    #[test]
    fn toggle_bold_clears_when_all_have_it() {
        let mut e = engine_with_styles(&[(0, 0), (0, 1)]);
        // Pre-set bold on every cell in the target range
        // (col 0, rows 0 and 1).
        for r in 0..=1 { e.style_mut((0, r)).bold = true; }
        // sel tuple is (r1, c1, r2, c2): select col 0 across rows 0..=1.
        let handled = apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleBold, &[(0, 0, 1, 0)],
        );
        assert!(handled);
        for r in 0..=1 {
            assert!(!e.get_style((0, r)).map_or(false, |s| s.bold));
        }
    }

    #[test]
    fn toggle_italic_unaffected_by_other_marks() {
        let mut e = engine_with_styles(&[(0, 0)]);
        e.style_mut((0, 0)).bold = true;
        e.style_mut((0, 0)).underline = true;
        apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleItalic, &[(0, 0, 0, 0)],
        );
        let s = e.get_style((0, 0)).unwrap();
        assert!(s.italic);
        assert!(s.bold, "ToggleItalic must not clear pre-existing bold");
        assert!(s.underline, "ToggleItalic must not clear pre-existing underline");
    }

    #[test]
    fn toggle_underline_independent_of_strike() {
        let mut e = engine_with_styles(&[(0, 0)]);
        e.style_mut((0, 0)).strike = true;
        apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleUnderline, &[(0, 0, 0, 0)],
        );
        let s = e.get_style((0, 0)).unwrap();
        assert!(s.underline);
        assert!(s.strike, "ToggleUnderline must not touch strike");
    }

    #[test]
    fn toggle_text_color_with_hex_sets_color() {
        let mut e = engine_with_styles(&[(0, 0), (1, 0)]);
        // Select cols 0..=1 of row 0: (r1=0, c1=0, r2=0, c2=1).
        apply_toolbar_command_to_selection(
            &mut e,
            &ToolbarCommand::ToggleTextColor("#ff0000".into()),
            &[(0, 0, 0, 1)],
        );
        assert_eq!(e.get_style((0, 0)).unwrap().text_color.as_deref(), Some("#ff0000"));
        assert_eq!(e.get_style((1, 0)).unwrap().text_color.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn toggle_text_color_with_empty_clears() {
        let mut e = engine_with_styles(&[(0, 0)]);
        e.style_mut((0, 0)).text_color = Some("#abcdef".into());
        apply_toolbar_command_to_selection(
            &mut e,
            &ToolbarCommand::ToggleTextColor(String::new()),
            &[(0, 0, 0, 0)],
        );
        assert!(e.get_style((0, 0)).unwrap().text_color.is_none());
    }

    #[test]
    fn toggle_highlight_sets_bg_color_across_range() {
        let mut e = engine_with_styles(&[(0, 0), (1, 0), (0, 1), (1, 1)]);
        apply_toolbar_command_to_selection(
            &mut e,
            &ToolbarCommand::ToggleHighlight("#fff176".into()),
            &[(0, 0, 1, 1)],
        );
        for r in 0..=1 {
            for c in 0..=1 {
                assert_eq!(
                    e.get_style((c, r)).unwrap().bg_color.as_deref(),
                    Some("#fff176"),
                );
            }
        }
    }

    #[test]
    fn set_number_format_currency_sets_key() {
        let mut e = engine_with_styles(&[(0, 0)]);
        apply_toolbar_command_to_selection(
            &mut e,
            &ToolbarCommand::SetNumberFormat("currency".into()),
            &[(0, 0, 0, 0)],
        );
        assert_eq!(
            e.get_style((0, 0)).unwrap().number_format.as_deref(),
            Some("currency"),
        );
    }

    #[test]
    fn set_number_format_empty_clears() {
        let mut e = engine_with_styles(&[(0, 0)]);
        e.style_mut((0, 0)).number_format = Some("percent".into());
        apply_toolbar_command_to_selection(
            &mut e,
            &ToolbarCommand::SetNumberFormat(String::new()),
            &[(0, 0, 0, 0)],
        );
        assert!(e.get_style((0, 0)).unwrap().number_format.is_none());
    }

    #[test]
    fn toggle_bold_across_multi_region_treats_union_as_one_selection() {
        // Non-contiguous selection (#59): three separate 1×1 rects
        // for A1, A2, A3. Toggling bold once should set all three;
        // toggling again should clear all three. The per-region
        // toggle implementation would flip each region independently
        // and never reach a consistent state.
        let mut e = engine_with_styles(&[(0, 0), (0, 1), (0, 2)]);
        let regions = [
            (0, 0, 0, 0),
            (1, 0, 1, 0),
            (2, 0, 2, 0),
        ];
        apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleBold, &regions,
        );
        for r in 0..=2 {
            assert!(e.get_style((0, r)).unwrap().bold, "row {r} should be bold");
        }
        // Re-toggle clears all three.
        apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleBold, &regions,
        );
        for r in 0..=2 {
            assert!(!e.get_style((0, r)).unwrap().bold, "row {r} should be cleared");
        }
    }

    #[test]
    fn toggle_bold_mixed_region_state_sets_all() {
        // Some regions already bold, some not. The union has at
        // least one non-bold cell → all = false → toggle sets all.
        let mut e = engine_with_styles(&[(0, 0), (0, 1)]);
        e.style_mut((0, 0)).bold = true;
        // (0, 1) is plain.
        let regions = [(0, 0, 0, 0), (1, 0, 1, 0)];
        apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::ToggleBold, &regions,
        );
        assert!(e.get_style((0, 0)).unwrap().bold);
        assert!(e.get_style((0, 1)).unwrap().bold);
    }

    #[test]
    fn unhandled_command_returns_false_and_leaves_engine_alone() {
        let mut e = engine_with_styles(&[(0, 0)]);
        e.style_mut((0, 0)).bold = true;
        let handled = apply_toolbar_command_to_selection(
            &mut e, &ToolbarCommand::InsertHorizontalRule, &[(0, 0, 0, 0)],
        );
        assert!(!handled);
        // Pre-existing state unchanged.
        assert!(e.get_style((0, 0)).unwrap().bold);
    }

    #[test]
    fn multi_region_blocks_clipboard_only_when_extras_present() {
        // #75: a plain contiguous selection (no extra regions) copies
        // normally; once the user Ctrl-clicks extra regions, copy/cut
        // must be blocked rather than silently using the primary rect.
        assert!(!multi_region_blocks_clipboard(&[]));
        assert!(multi_region_blocks_clipboard(&[(1, 0, 1, 0)]));
        assert!(multi_region_blocks_clipboard(&[(1, 0, 1, 0), (3, 2, 4, 5)]));
    }

    // ─ #54: Copy as markdown ─

    #[test]
    fn selection_to_markdown_emits_gfm_table() {
        let mut eng = crate::spreadsheet::eval::SpreadsheetEngine::new();
        eng.set_cell((0, 0), "A");
        eng.set_cell((1, 0), "B");
        eng.set_cell((0, 1), "1");
        eng.set_cell((1, 1), "2");
        // First row is the header; a `---` separator follows.
        assert_eq!(
            selection_to_markdown(&eng, (0, 0, 1, 1)),
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n",
        );
    }

    #[test]
    fn selection_to_markdown_escapes_pipes_and_round_trips() {
        let mut eng = crate::spreadsheet::eval::SpreadsheetEngine::new();
        eng.set_cell((0, 0), "h1");
        eng.set_cell((1, 0), "h2");
        eng.set_cell((0, 1), "a|b"); // literal pipe in a cell
        eng.set_cell((1, 1), "c");
        let md = selection_to_markdown(&eng, (0, 0, 1, 1));
        assert!(md.contains("a\\|b"), "pipe must be escaped in the emitted markdown");
        // Lossless round-trip back through the paste parser (header + data).
        let rows = super::persistence::parse_markdown_table(&md).expect("parses");
        assert_eq!(
            rows,
            vec![
                vec!["h1".to_string(), "h2".to_string()],
                vec!["a|b".to_string(), "c".to_string()],
            ],
        );
    }
}

#[cfg(test)]
mod ref_pick_tests {
    use super::*;

    // ─ is_ref_context ─

    #[test]
    fn ctx_after_open_paren() {
        assert!(is_ref_context("=SUM(", 5));
    }

    #[test]
    fn ctx_after_comma() {
        assert!(is_ref_context("=SUM(A1,", 8));
    }

    #[test]
    fn ctx_after_operator() {
        assert!(is_ref_context("=A1+", 4));
        assert!(is_ref_context("=A1*", 4));
        assert!(is_ref_context("=A1/", 4));
        assert!(is_ref_context("=A1-", 4));
        assert!(is_ref_context("=A1^", 4));
        assert!(is_ref_context("=A1&", 4));
    }

    #[test]
    fn no_ctx_after_digit() {
        assert!(!is_ref_context("=SUM(A1", 7));
    }

    #[test]
    fn no_ctx_after_letter() {
        assert!(!is_ref_context("=SUM(A", 6));
    }

    #[test]
    fn no_ctx_without_equals() {
        assert!(!is_ref_context("hello", 5));
        assert!(!is_ref_context("SUM(", 4));
    }

    #[test]
    fn no_ctx_empty() {
        assert!(!is_ref_context("", 0));
        assert!(!is_ref_context("=", 0));
    }

    #[test]
    fn no_ctx_caret_not_at_end() {
        // v1 restriction: caret must be at end-of-string
        assert!(!is_ref_context("=SUM(A1)", 5));
    }

    #[test]
    fn ctx_with_just_equals() {
        // =<caret> is a valid ref context
        assert!(is_ref_context("=", 1));
    }

    // ─ cell_label ─

    #[test]
    fn label_single_cell() {
        assert_eq!(cell_label((0, 0)), "A1");
        assert_eq!(cell_label((1, 3)), "B4");
        assert_eq!(cell_label((25, 9)), "Z10");
        assert_eq!(cell_label((26, 0)), "AA1");
    }

    // ─ range_label ─

    #[test]
    fn range_single_cell_degenerate() {
        assert_eq!(range_label((0, 0), (0, 0)), "A1");
    }

    #[test]
    fn range_normalized_top_left_first() {
        assert_eq!(range_label((0, 0), (1, 2)), "A1:B3");
        // Reversed drag direction — still normalized.
        assert_eq!(range_label((1, 2), (0, 0)), "A1:B3");
    }

    #[test]
    fn range_row() {
        assert_eq!(range_label((0, 2), (3, 2)), "A3:D3");
    }

    #[test]
    fn range_column() {
        assert_eq!(range_label((0, 0), (0, 4)), "A1:A5");
    }

    // ─ splice_ref ─

    #[test]
    fn splice_at_end() {
        assert_eq!(splice_ref("=SUM(", 5, "A1:B3"), "=SUM(A1:B3");
    }

    #[test]
    fn splice_replaces_tail() {
        // Used when updating an in-progress pick — the old label after
        // insert_at is thrown away.
        assert_eq!(splice_ref("=SUM(A1", 5, "B2:C3"), "=SUM(B2:C3");
    }

    #[test]
    fn splice_at_beginning() {
        assert_eq!(splice_ref("SOME", 0, "X"), "X");
    }

    #[test]
    fn splice_past_end_clamps() {
        // Out-of-bounds insert_at shouldn't panic — just appends.
        assert_eq!(splice_ref("=SUM(", 999, "A1"), "=SUM(A1");
    }

    // ─ RefPick::contains ─

    #[test]
    fn pick_contains_single_cell() {
        let p = RefPick { start: (2, 2), end: (2, 2), insert_at: 0 };
        assert!(p.contains(2, 2));
        assert!(!p.contains(1, 2));
        assert!(!p.contains(3, 2));
    }

    #[test]
    fn pick_contains_range_normalized() {
        let p = RefPick { start: (3, 5), end: (1, 2), insert_at: 0 };
        // Range normalizes to (1..=3, 2..=5).
        assert!(p.contains(1, 2));
        assert!(p.contains(3, 5));
        assert!(p.contains(2, 3));
        assert!(!p.contains(0, 3));
        assert!(!p.contains(4, 3));
        assert!(!p.contains(2, 1));
        assert!(!p.contains(2, 6));
    }

    #[test]
    fn pick_label_roundtrip() {
        let p = RefPick { start: (0, 0), end: (2, 4), insert_at: 5 };
        assert_eq!(p.label(), "A1:C5");
    }

    // ─ missing_close_parens ─

    #[test]
    fn missing_one_close() {
        assert_eq!(missing_close_parens("=SUM(A1:A3"), 1);
    }

    #[test]
    fn balanced_formula_needs_none() {
        assert_eq!(missing_close_parens("=SUM(A1:A3)"), 0);
        assert_eq!(missing_close_parens("=A1+B2"), 0);
    }

    #[test]
    fn nested_missing() {
        assert_eq!(missing_close_parens("=IF(A1>0,SUM(B1:B3"), 2);
    }

    #[test]
    fn over_balanced_returns_zero() {
        // We never strip extra closes; leave those for the parser to flag.
        assert_eq!(missing_close_parens("=A1)"), 0);
        assert_eq!(missing_close_parens("=SUM(A1))"), 0);
    }

    #[test]
    fn non_formula_returns_zero() {
        assert_eq!(missing_close_parens("hello("), 0);
        assert_eq!(missing_close_parens("(((("), 0);
    }

    #[test]
    fn parens_inside_string_literal_ignored() {
        // The `(` inside quotes is literal text, not a formula paren.
        assert_eq!(missing_close_parens("=\"he(llo\""), 0);
        // Open paren outside + string literal = still need one close.
        assert_eq!(missing_close_parens("=SUM(\"he(llo\""), 1);
    }

    #[test]
    fn escaped_quote_stays_in_string_mode() {
        // `\"` inside a string doesn't terminate the string.
        assert_eq!(missing_close_parens("=\"a\\\"b(\""), 0);
    }

    #[test]
    fn empty_and_equals_only() {
        assert_eq!(missing_close_parens(""), 0);
        assert_eq!(missing_close_parens("="), 0);
    }

    #[test]
    fn user_pick_scenario() {
        // After user types =SUM( and picks A1:A3 via arrows or mouse,
        // edit_value = "=SUM(A1:A3". Enter should auto-close to 1 paren.
        assert_eq!(missing_close_parens("=SUM(A1:A3"), 1);
    }
}

#[cfg(test)]
mod fill_tests {
    //! Regression tests for Ctrl+D / Ctrl+R fill. The original bug was that
    //! fill copied the source cell's raw string verbatim to every target,
    //! leaving formula references unchanged (`=A1` stayed `=A1` in every
    //! filled row). Fill must translate relative refs per-cell.

    use super::*;
    use crate::spreadsheet::eval::SpreadsheetEngine;

    fn set(eng: &mut SpreadsheetEngine, addr: &str, val: &str) {
        let col = (addr.as_bytes()[0] - b'A') as usize;
        let row: usize = addr[1..].parse::<usize>().unwrap() - 1;
        eng.set_cell((col, row), val);
    }
    fn raw(eng: &SpreadsheetEngine, addr: &str) -> String {
        let col = (addr.as_bytes()[0] - b'A') as usize;
        let row: usize = addr[1..].parse::<usize>().unwrap() - 1;
        eng.get_raw((col, row)).to_string()
    }

    #[test]
    fn fill_down_shifts_relative_row_refs() {
        // A1=10, A2=20, A3=30. B1==A1. Fill-down B1:B3 should give
        // B2==A2 and B3==A3.
        let mut eng = SpreadsheetEngine::new();
        set(&mut eng, "A1", "10");
        set(&mut eng, "A2", "20");
        set(&mut eng, "A3", "30");
        set(&mut eng, "B1", "=A1");

        apply_fill(&mut eng, (0, 1, 2, 1), FillDir::Down, (4, 4));
        assert_eq!(raw(&eng, "B2"), "=A2");
        assert_eq!(raw(&eng, "B3"), "=A3");
    }

    #[test]
    fn fill_down_preserves_absolute_refs() {
        // =$A$1 stays =$A$1 when filled down.
        let mut eng = SpreadsheetEngine::new();
        set(&mut eng, "A1", "10");
        set(&mut eng, "B1", "=$A$1");

        apply_fill(&mut eng, (0, 1, 2, 1), FillDir::Down, (4, 4));
        assert_eq!(raw(&eng, "B2"), "=$A$1");
        assert_eq!(raw(&eng, "B3"), "=$A$1");
    }

    #[test]
    fn fill_right_shifts_relative_col_refs() {
        // A1==B1 filled right across A1:C1 should give B1==C1, C1==D1.
        let mut eng = SpreadsheetEngine::new();
        set(&mut eng, "B1", "1");
        set(&mut eng, "C1", "2");
        set(&mut eng, "D1", "3");
        set(&mut eng, "A1", "=B1");

        apply_fill(&mut eng, (0, 0, 0, 2), FillDir::Right, (5, 2));
        assert_eq!(raw(&eng, "B1"), "=C1");
        assert_eq!(raw(&eng, "C1"), "=D1");
    }

    #[test]
    fn fill_down_literal_stays_literal() {
        // Non-formula content copies verbatim (no translation).
        let mut eng = SpreadsheetEngine::new();
        set(&mut eng, "A1", "hello");

        apply_fill(&mut eng, (0, 0, 2, 0), FillDir::Down, (1, 3));
        assert_eq!(raw(&eng, "A2"), "hello");
        assert_eq!(raw(&eng, "A3"), "hello");
    }

    #[test]
    fn fill_down_single_row_is_noop() {
        // r1 == r2 — nothing to fill; must not panic.
        let mut eng = SpreadsheetEngine::new();
        set(&mut eng, "A1", "=B1");
        apply_fill(&mut eng, (0, 0, 0, 0), FillDir::Down, (2, 2));
        assert_eq!(raw(&eng, "A1"), "=B1");
    }
}

#[cfg(test)]
mod inputmode_tests {
    //! Tests for `infer_column_inputmode` — the per-column numeric heuristic
    //! that decides whether to surface the iOS/Android numeric keyboard.

    use super::*;
    use crate::spreadsheet::eval::SpreadsheetEngine;

    fn engine_with_col(col: usize, values: &[&str]) -> SpreadsheetEngine {
        let mut eng = SpreadsheetEngine::new();
        for (row, v) in values.iter().enumerate() {
            eng.set_cell((col, row), v);
        }
        eng
    }

    #[test]
    fn all_numeric_returns_decimal() {
        let eng = engine_with_col(0, &["1", "2.5", "3", "-4.7", "100"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "decimal");
    }

    #[test]
    fn all_text_returns_empty() {
        let eng = engine_with_col(0, &["alpha", "beta", "gamma", "delta"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "");
    }

    #[test]
    fn eighty_twenty_mix_returns_decimal() {
        // 4 numeric + 1 text = 80% numeric → decimal
        let eng = engine_with_col(0, &["1", "2", "3", "4", "label"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "decimal");
    }

    #[test]
    fn below_threshold_returns_empty() {
        // 3 numeric + 2 text = 60% numeric → not enough
        let eng = engine_with_col(0, &["1", "2", "3", "x", "y"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "");
    }

    #[test]
    fn fewer_than_three_entries_returns_empty() {
        // Latch threshold prevents brand-new column with one digit from
        // suppressing the alphanumeric keyboard.
        let eng = engine_with_col(0, &["7"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "");
        let eng = engine_with_col(0, &["7", "8"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "");
    }

    #[test]
    fn empty_cells_skipped() {
        // Only counts non-empty cells; gaps don't dilute the ratio.
        let mut eng = SpreadsheetEngine::new();
        eng.set_cell((0, 0), "1");
        eng.set_cell((0, 5), "2");
        eng.set_cell((0, 10), "3");
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "decimal");
    }

    #[test]
    fn formula_strings_are_not_numeric() {
        // Formulas store their raw text starting with `=`; raw `parse::<f64>`
        // rejects them, so a formula-heavy column stays alphanumeric.
        let eng = engine_with_col(0, &["=1+1", "=A1*2", "=SUM(B:B)", "=C1"]);
        assert_eq!(infer_column_inputmode(&eng, 0, 50), "");
    }
}

#[cfg(test)]
mod sort_tests {
    //! Multi-column sort comparator. The grid render passes a chain of
    //! `(col, ascending)` keys; earlier keys dominate.

    use super::*;
    use std::cmp::Ordering;

    fn row(cells: &[&str]) -> Vec<(String, Option<crate::spreadsheet::eval::CellStyle>)> {
        cells.iter().map(|s| (s.to_string(), None)).collect()
    }

    #[test]
    fn empty_chain_is_equal() {
        let a = row(&["1"]);
        let b = row(&["2"]);
        assert_eq!(compare_rows_by_keys(&a, &b, &[]), Ordering::Equal);
    }

    #[test]
    fn single_numeric_key_ascending() {
        let a = row(&["10"]);
        let b = row(&["9"]);
        // Numeric: 10 > 9 → a sorts after b
        assert_eq!(compare_rows_by_keys(&a, &b, &[(0, true)]), Ordering::Greater);
    }

    #[test]
    fn single_numeric_key_descending_flips() {
        let a = row(&["10"]);
        let b = row(&["9"]);
        assert_eq!(compare_rows_by_keys(&a, &b, &[(0, false)]), Ordering::Less);
    }

    #[test]
    fn falls_back_to_text_when_non_numeric() {
        let a = row(&["banana"]);
        let b = row(&["apple"]);
        assert_eq!(compare_rows_by_keys(&a, &b, &[(0, true)]), Ordering::Greater);
    }

    #[test]
    fn text_compare_is_case_insensitive() {
        let a = row(&["BANANA"]);
        let b = row(&["apple"]);
        assert_eq!(compare_rows_by_keys(&a, &b, &[(0, true)]), Ordering::Greater);
    }

    #[test]
    fn second_key_breaks_ties_on_first() {
        let a = row(&["X", "10"]);
        let b = row(&["X", "5"]);
        // Equal on col 0 → break with col 1 ascending: 10 > 5
        assert_eq!(
            compare_rows_by_keys(&a, &b, &[(0, true), (1, true)]),
            Ordering::Greater,
        );
    }

    #[test]
    fn second_key_can_be_descending() {
        let a = row(&["X", "10"]);
        let b = row(&["X", "5"]);
        // Equal on col 0 → break with col 1 descending: 10 < 5 in desc
        assert_eq!(
            compare_rows_by_keys(&a, &b, &[(0, true), (1, false)]),
            Ordering::Less,
        );
    }

    #[test]
    fn third_key_only_consulted_when_first_two_tie() {
        let a = row(&["X", "5", "z"]);
        let b = row(&["X", "5", "a"]);
        assert_eq!(
            compare_rows_by_keys(&a, &b, &[(0, true), (1, true), (2, true)]),
            Ordering::Greater,
        );
    }

    #[test]
    fn nan_string_falls_back_to_text_comparison() {
        // "NaN".parse::<f64>() succeeds (yields f64::NAN) but
        // partial_cmp returns None for NaN. Without the explicit
        // is_nan guard, NaN-valued rows would silently get a
        // free Ordering::Equal pass against any peer, scattering
        // them randomly in stable_sort. Falling back to text
        // keeps "NaN" sorting deterministically.
        let a = row(&["NaN"]);
        let b = row(&["100"]);
        // Text fallback: "nan" > "100" lexicographically.
        assert_eq!(compare_rows_by_keys(&a, &b, &[(0, true)]), Ordering::Greater);
    }

    #[test]
    fn missing_column_treated_as_empty_string() {
        let a = row(&["A"]);                  // only col 0
        let b = row(&["A", "B"]);             // col 0 + col 1
        // Equal on col 0; `a` has no col 1 → "" vs "b". With Excel-
        // parity blanks-last semantics, the row with the blank cell
        // at the tiebreaker key sorts AFTER the populated row, so
        // `a > b` regardless of asc/desc direction.
        assert_eq!(
            compare_rows_by_keys(&a, &b, &[(0, true), (1, true)]),
            Ordering::Greater,
        );
    }

    #[test]
    fn blanks_sort_last_in_ascending() {
        // Excel parity: empty cells always sort to the BOTTOM. Without
        // this the doctor scenario `sortReorderedRows` failed because
        // a sparse grid (3 populated rows + 7 empty) ascending-sorted
        // by col 0 left A1 = "" instead of "1".
        let populated = row(&["1"]);
        let empty = row(&[""]);
        // Ascending: populated < empty.
        assert_eq!(
            compare_rows_by_keys(&populated, &empty, &[(0, true)]),
            Ordering::Less,
        );
    }

    #[test]
    fn blanks_sort_last_in_descending_too() {
        // Same invariant under descending — blanks STILL go last,
        // even though the populated values flip order.
        let populated = row(&["1"]);
        let empty = row(&[""]);
        assert_eq!(
            compare_rows_by_keys(&populated, &empty, &[(0, false)]),
            Ordering::Less,
        );
    }

    #[test]
    fn populated_rows_still_descend_around_blanks() {
        // Three rows: "1", "3", and blank. Sorted descending by col 0,
        // expected order is "3", "1", "". Blank goes last.
        let r1 = row(&["1"]);
        let r3 = row(&["3"]);
        let blank = row(&[""]);
        // Descending: 3 > 1.
        assert_eq!(
            compare_rows_by_keys(&r3, &r1, &[(0, false)]),
            Ordering::Less,
        );
        // Blank still after populated descending.
        assert_eq!(
            compare_rows_by_keys(&r1, &blank, &[(0, false)]),
            Ordering::Less,
        );
    }
}

#[cfg(test)]
mod nav_tests {
    //! #57 (Ctrl+Arrow / Home / End jumps) and #73 (Ctrl-click toggle).
    use super::{data_edge, last_used_cell, last_used_col_in_row, subtract_cell};
    use crate::spreadsheet::eval::SpreadsheetEngine;

    // Fixture: a 10x10 grid (max index 9) with
    //   col0 rows 0..=3 filled, then a gap, col0 row 6 filled;
    //   row0 also has a cell at col5.
    fn fixture() -> SpreadsheetEngine {
        let mut e = SpreadsheetEngine::new();
        for r in 0..=3 {
            e.set_cell((0, r), "x"); // (col, row)
        }
        e.set_cell((0, 6), "x");
        e.set_cell((5, 0), "x");
        e
    }

    // ── Ctrl+Arrow data-region edges (#57) ──

    #[test]
    fn ctrl_down_walks_to_end_of_run() {
        let e = fixture();
        // From (0,0): cell and neighbor used → stop at last of the run (row 3).
        assert_eq!(data_edge(&e, 0, 0, 1, 0, 9, 9), (3, 0));
    }

    #[test]
    fn ctrl_down_jumps_across_gap_to_next_used() {
        let e = fixture();
        // From (3,0): neighbor blank → skip the gap, land on row 6.
        assert_eq!(data_edge(&e, 3, 0, 1, 0, 9, 9), (6, 0));
    }

    #[test]
    fn ctrl_down_past_last_data_stops_at_grid_edge() {
        let e = fixture();
        // From (6,0): nothing below → halt at the last grid row.
        assert_eq!(data_edge(&e, 6, 0, 1, 0, 9, 9), (9, 0));
    }

    #[test]
    fn ctrl_up_jumps_across_gap() {
        let e = fixture();
        // From (6,0): neighbor blank → skip gap up to the run, land on row 3.
        assert_eq!(data_edge(&e, 6, 0, -1, 0, 9, 9), (3, 0));
    }

    #[test]
    fn ctrl_arrow_from_blank_lands_on_first_used() {
        let e = fixture();
        // Blank (5,0): Ctrl+Down skips to the next used cell (row 6).
        assert_eq!(data_edge(&e, 5, 0, 1, 0, 9, 9), (6, 0));
        // Blank (5,0): Ctrl+Up skips up to the run end (row 3).
        assert_eq!(data_edge(&e, 5, 0, -1, 0, 9, 9), (3, 0));
    }

    #[test]
    fn ctrl_right_skips_gap_then_grid_edge() {
        let e = fixture();
        // (0,0): neighbor blank → skip to the col-5 cell.
        assert_eq!(data_edge(&e, 0, 0, 0, 1, 9, 9), (0, 5));
        // (0,5): nothing further right → halt at the last grid column.
        assert_eq!(data_edge(&e, 0, 5, 0, 1, 9, 9), (0, 9));
    }

    // ── Ctrl+End / End targets (#57) ──

    #[test]
    fn last_used_cell_is_used_range_corner() {
        let e = fixture();
        // max row = 6 (col0), max col = 5 (row0), taken independently.
        assert_eq!(last_used_cell(&e), (6, 5));
    }

    #[test]
    fn last_used_cell_empty_sheet_is_origin() {
        assert_eq!(last_used_cell(&SpreadsheetEngine::new()), (0, 0));
    }

    #[test]
    fn last_used_col_in_row_finds_rightmost() {
        let e = fixture();
        assert_eq!(last_used_col_in_row(&e, 0, 9), Some(5)); // row 0 spans to col 5
        assert_eq!(last_used_col_in_row(&e, 1, 9), Some(0)); // row 1 only col 0
        // Empty row → None, so the `End` handler leaves the cursor put rather
        // than snapping it left to column 0.
        assert_eq!(last_used_col_in_row(&e, 8, 9), None);
    }

    // ── Ctrl-click toggle: subtract_cell (#73) ──

    fn cells_of(rects: &[(usize, usize, usize, usize)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &(r1, c1, r2, c2) in rects {
            for r in r1..=r2 {
                for c in c1..=c2 {
                    out.push((r, c));
                }
            }
        }
        out.sort_unstable();
        out
    }

    #[test]
    fn subtract_cell_removes_one_by_one() {
        // A 1x1 region collapses to nothing — the cell is deselected.
        assert!(subtract_cell((4, 4, 4, 4), 4, 4).is_empty());
    }

    #[test]
    fn subtract_cell_outside_is_unchanged() {
        assert_eq!(subtract_cell((0, 0, 2, 2), 5, 5), vec![(0, 0, 2, 2)]);
    }

    #[test]
    fn subtract_cell_center_splits_into_four_bands() {
        let parts = subtract_cell((0, 0, 2, 2), 1, 1);
        let cells = cells_of(&parts);
        // The 3x3 minus the center = 8 cells, none of them (1,1).
        assert_eq!(cells.len(), 8);
        assert!(!cells.contains(&(1, 1)));
        // No duplicates / overlap across the bands.
        let mut unique = cells.clone();
        unique.dedup();
        assert_eq!(unique.len(), 8);
    }

    #[test]
    fn subtract_cell_edge_of_row_strip() {
        // Removing the left end of a 1x3 strip leaves the right two cells.
        assert_eq!(subtract_cell((0, 0, 0, 2), 0, 0), vec![(0, 1, 0, 2)]);
    }
}
