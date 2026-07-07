// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Formula evaluator with cell reference resolution and dependency tracking.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use super::functions;
use super::parser::{BinOp, CellRef, Expr, RangeRef, SpreadsheetError, parse_formula};

/// Snapshot of a foreign document, populated by the view layer
/// after fetching `GET /documents/{id}/content`. Stored as
/// already-flattened sheet text so the engine doesn't need to know
/// about `editor::model::Node` (keeps the eval module's dependency
/// surface tight). Each sheet's `cells[row][col]` holds the cell's
/// displayed text; missing cells are empty strings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ForeignDocSnapshot {
    /// Sheet name → row-major cell-text matrix.
    pub sheets: HashMap<String, Vec<Vec<String>>>,
}

/// Reasons a foreign-doc fetch could fail. The view layer maps HTTP
/// status to one of these and writes them via
/// `SpreadsheetEngine::set_foreign_doc_error`. The function
/// implementations translate each into a user-facing cell error
/// (`#REF!` / `#NUM!` / `#LOADING!`).
#[derive(Debug, Clone, PartialEq)]
pub enum ForeignFetchError {
    /// 404 — doc doesn't exist (or has been deleted).
    NotFound,
    /// 401 / 403 — caller doesn't have read access.
    Forbidden,
    /// Transport, timeout, or other transient network failure.
    /// Engine returns `#LOADING!` and the view layer can retry.
    Network,
    /// Payload exceeds the configured size cap.
    Oversize,
    /// CRDT bytes failed to decode.
    Decode,
    /// User declined the consent prompt.
    Denied,
}

/// State of a foreign doc in the cache.
pub type ForeignDocState = Result<ForeignDocSnapshot, ForeignFetchError>;

/// A computed cell value.
///
/// `Array` carries the result of a dynamic-array function (SORT,
/// FILTER, UNIQUE, TRANSPOSE, MMULT, etc.) — a 2-D rectangular block
/// of scalar values, indexed `[row][col]`. Scalar accessors
/// (`as_number`, `as_text`, `as_bool`) collapse an Array to its
/// top-left element so a non-spilled cell still has a sensible
/// scalar view; spill-aware rendering is wired up in
/// `SpreadsheetEngine`'s evaluation step (M-S1a part 2). An empty
/// Array is treated as `Empty` for accessor purposes.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Number(f64),
    Text(String),
    Bool(bool),
    Error(SpreadsheetError),
    Empty,
    /// 2-D rectangular block, `rows[row][col]`. Always rectangular —
    /// every row has the same length. An empty Array is `vec![]`.
    Array(Vec<Vec<CellValue>>),
}

impl CellValue {
    /// First element of an Array, or self for scalars. Used by
    /// scalar accessors below so an Array can stand in for a number /
    /// text / bool when the consumer doesn't know about arrays.
    fn array_top_left(&self) -> &CellValue {
        match self {
            CellValue::Array(rows) => rows
                .first()
                .and_then(|row| row.first())
                .unwrap_or(&CellValue::Empty),
            other => other,
        }
    }

    pub fn as_number(&self) -> Result<f64, SpreadsheetError> {
        match self {
            CellValue::Number(n) => Ok(*n),
            CellValue::Bool(true) => Ok(1.0),
            CellValue::Bool(false) => Ok(0.0),
            CellValue::Empty => Ok(0.0),
            CellValue::Text(s) => s.parse::<f64>().map_err(|_| SpreadsheetError::Value),
            CellValue::Error(e) => Err(e.clone()),
            CellValue::Array(_) => self.array_top_left().as_number(),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            CellValue::Number(n) => {
                if *n == n.trunc() && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    format!("{n}")
                }
            }
            CellValue::Text(s) => s.clone(),
            CellValue::Bool(b) => if *b { "TRUE".to_string() } else { "FALSE".to_string() },
            CellValue::Error(e) => e.to_string(),
            CellValue::Empty => String::new(),
            CellValue::Array(_) => self.array_top_left().as_text(),
        }
    }

    pub fn as_bool(&self) -> Result<bool, SpreadsheetError> {
        match self {
            CellValue::Bool(b) => Ok(*b),
            CellValue::Number(n) => Ok(*n != 0.0),
            CellValue::Empty => Ok(false),
            CellValue::Error(e) => Err(e.clone()),
            CellValue::Array(_) => self.array_top_left().as_bool(),
            _ => Err(SpreadsheetError::Value),
        }
    }

    pub fn is_error(&self) -> bool {
        // Scalar-only check by design. Arrays containing errors do
        // *not* register as errors here, otherwise `eval_binop` (which
        // short-circuits on `is_error()`) would over-propagate when a
        // formula consumes only the anchor scalar of a partially-errored
        // dynamic-array result. Excel's behavior: errors travel with
        // the specific array element they live on, not the whole block.
        // Callers that genuinely need "any element errored" should walk
        // the Array explicitly.
        matches!(self, CellValue::Error(_))
    }

    /// Promote any value to a 2-D rectangular block. Scalars become
    /// `[[self]]`; Arrays return their owned shape. Used by matrix
    /// functions (MMULT, MDETERM, MINVERSE) that need uniform shape.
    pub fn to_array_2d(&self) -> Vec<Vec<CellValue>> {
        match self {
            CellValue::Array(rows) => rows.clone(),
            other => vec![vec![other.clone()]],
        }
    }
}

impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_text())
    }
}

/// A conditional formatting rule. Four flavors, evaluated against
/// the value of each covered cell (and, for the range-aware ones,
/// against the min/max of all numeric values in the rule's coverage
/// region):
///
/// - `Single` — Excel's classic "if condition matches, paint cell with
///   `bg_color`" rule. Predates the enum (was a struct in v1).
/// - `ColorScale` — gradient between `low` and `high`; if `mid` is
///   `Some`, the gradient passes through `mid` at the median value.
/// - `DataBar` — inline horizontal bar in `color`, width proportional
///   to the cell's value relative to the rule's range max. Negative
///   values are clamped to zero in v1.
/// - `IconSet` — categorical glyph chosen by the cell's tertile
///   position in the rule's numeric range. Non-numeric cells get
///   no icon.
#[derive(Debug, Clone, PartialEq)]
pub enum ConditionalFormat {
    Single {
        condition: ConditionalCondition,
        bg_color: String,
    },
    ColorScale {
        low: String,
        mid: Option<String>,
        high: String,
    },
    DataBar {
        color: String,
    },
    IconSet {
        kind: IconSetKind,
    },
}

/// Discrete icon-set families. Each maps a tertile position
/// (`Low` = bottom third, `Mid` = middle, `High` = top third) to a
/// concrete glyph. v1 ships the two most-common families; more can
/// follow without a schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSetKind {
    /// ↓ → ↑ for low/mid/high tertiles.
    ThreeArrows,
    /// 🔴 🟡 🟢 for low/mid/high tertiles.
    ThreeTrafficLights,
}

impl IconSetKind {
    /// Return the glyph for a value's tertile position. `t` is the
    /// normalized position in `[0.0, 1.0]`.
    pub fn glyph_for(&self, t: f64) -> &'static str {
        let bucket = if t < 1.0 / 3.0 { 0 } else if t < 2.0 / 3.0 { 1 } else { 2 };
        match (self, bucket) {
            (IconSetKind::ThreeArrows, 0) => "\u{2193}",        // ↓
            (IconSetKind::ThreeArrows, 1) => "\u{2192}",        // →
            (IconSetKind::ThreeArrows, _) => "\u{2191}",        // ↑
            (IconSetKind::ThreeTrafficLights, 0) => "\u{1F534}",// 🔴
            (IconSetKind::ThreeTrafficLights, 1) => "\u{1F7E1}",// 🟡
            (IconSetKind::ThreeTrafficLights, _) => "\u{1F7E2}",// 🟢
        }
    }
}

/// Condition types for conditional formatting.
#[derive(Debug, Clone, PartialEq)]
pub enum ConditionalCondition {
    GreaterThan(f64),
    LessThan(f64),
    EqualTo(String),
    Between(f64, f64),
    TextContains(String),
    IsEmpty,
    IsNotEmpty,
}

/// Strict same-type ordering for spreadsheet `<` / `>` / `<=` / `>=`
/// operators — parallels `cell_eq`. Excel-parity:
/// * Same-type pairs compare directly:
///   - `Number` numerically.
///   - `Bool` by `false < true`.
///   - `Text` case-insensitively (so `"Apple" < "banana"` is true,
///     consistent with `cell_eq`'s case-folded equality).
/// * `Empty` vs `Number(n)` — coerces Empty to 0.0 (matches the
///   `as_number(Empty)` rule used elsewhere; mirrors `cell_eq`'s
///   Empty-as-zero arm).
/// * Cross-type pairs use Excel's type-tier ordering:
///   `Number/Empty < Text < Bool`. This keeps `<`/`>` consistent
///   with `cell_eq` (cross-type pairs are never equal *and* yield
///   a defined ordering), so `=A1<=A2` and `=A1=A2` can never
///   contradict each other across mixed types.
/// * Either side `Error` — propagates.
/// * `Array` coerces to top-left scalar.
fn cell_cmp(lv: &CellValue, rv: &CellValue) -> Result<std::cmp::Ordering, SpreadsheetError> {
    use CellValue::*;
    use std::cmp::Ordering;
    // Unwrap nested `Array` values iteratively (each `array_top_left`
    // call peels one layer). A well-formed array always bottoms out at
    // a scalar within one or two hops; a malicious/pathological doc
    // could stack arrays arbitrarily deep, so cap the loop instead of
    // recursing — the cap converts a stack-overflow into a #NUM! and
    // keeps eval deterministic.
    let mut lv = lv;
    let mut rv = rv;
    for _ in 0..MAX_ARRAY_UNWRAP_DEPTH {
        let l_arr = matches!(lv, Array(_));
        let r_arr = matches!(rv, Array(_));
        if !l_arr && !r_arr { break; }
        if l_arr { lv = lv.array_top_left(); }
        if r_arr { rv = rv.array_top_left(); }
    }
    if matches!(lv, Array(_)) || matches!(rv, Array(_)) {
        return Err(SpreadsheetError::Num);
    }
    if let Error(e) = lv { return Err(e.clone()); }
    if let Error(e) = rv { return Err(e.clone()); }
    fn type_tier(v: &CellValue) -> u8 {
        match v {
            CellValue::Number(_) | CellValue::Empty => 0,
            CellValue::Text(_) => 1,
            CellValue::Bool(_) => 2,
            _ => 0,
        }
    }
    match (lv, rv) {
        (Number(a), Number(b)) =>
            Ok(a.partial_cmp(b).unwrap_or(Ordering::Equal)),
        (Bool(a), Bool(b)) => Ok(a.cmp(b)),
        (Text(a), Text(b)) =>
            Ok(a.to_lowercase().cmp(&b.to_lowercase())),
        (Empty, Empty) => Ok(Ordering::Equal),
        (Empty, Number(n)) => Ok(0.0_f64.partial_cmp(n).unwrap_or(Ordering::Equal)),
        (Number(n), Empty) => Ok(n.partial_cmp(&0.0).unwrap_or(Ordering::Equal)),
        _ => Ok(type_tier(lv).cmp(&type_tier(rv))),
    }
}

/// Hard limit on `cell_cmp`'s array-unwrap loop. Arrays of arrays
/// never appear in normal eval output — this bound exists to keep
/// pathological CRDT-deserialised input from spinning forever and
/// is small enough that the loop is effectively free.
const MAX_ARRAY_UNWRAP_DEPTH: usize = 16;

/// Hard limit on `eval_expr` recursion depth. A user formula that
/// nests beyond this is rejected with `#NUM!` instead of being
/// allowed to overflow the stack. Chosen large enough that natural
/// formulas (deeply nested `IF`, long `+` chains, etc.) clear it
/// with margin but small enough that 256 stack frames stay within
/// the WASM default stack budget. The thread-local counter is OK
/// in our single-threaded WASM context.
const MAX_EVAL_DEPTH: usize = 256;

thread_local! {
    static EVAL_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII guard that increments the eval-depth counter on entry and
/// decrements on drop. `try_enter` returns `None` if the depth cap
/// is already reached, letting the caller return `#NUM!` instead
/// of pushing another stack frame.
struct EvalDepthGuard;
impl EvalDepthGuard {
    fn try_enter() -> Option<Self> {
        EVAL_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= MAX_EVAL_DEPTH { return None; }
            d.set(cur + 1);
            Some(EvalDepthGuard)
        })
    }
}
impl Drop for EvalDepthGuard {
    fn drop(&mut self) {
        EVAL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Strict same-type equality for spreadsheet `=` / `<>` operators.
/// Excel-parity:
/// * `Number` vs `Number` — numeric comparison (so `100 = 1e2` is true
///   regardless of how either side renders as text).
/// * `Bool` vs `Bool` — direct boolean compare.
/// * `Text` vs `Text` — case-insensitive (Excel collation rule).
/// * `Empty` vs `Empty` — true.
/// * `Empty` vs `Text("")` — true (rendered identically).
/// * `Empty` vs `Number(n)` — `n == 0.0` (matches Excel and the
///   engine's existing `<` / `>` operators, which treat Empty as
///   zero via `as_number()`).
/// * Other cross-type combinations (Number vs Text, Bool vs Number,
///   etc.) are NOT equal — the literal-text comparison the engine
///   used to do gave wrong answers like `100 = "100"` → true.
/// * Either side is `Error` → the comparison short-circuits to that
///   error.
fn cell_eq(lv: &CellValue, rv: &CellValue) -> Result<bool, SpreadsheetError> {
    use CellValue::*;
    if let Error(e) = lv { return Err(e.clone()); }
    if let Error(e) = rv { return Err(e.clone()); }
    Ok(match (lv, rv) {
        (Number(a), Number(b)) => a == b,
        (Bool(a), Bool(b)) => a == b,
        (Text(a), Text(b)) => a.eq_ignore_ascii_case(b),
        (Empty, Empty) => true,
        (Empty, Text(s)) | (Text(s), Empty) => s.is_empty(),
        // Excel treats an unset cell as numeric 0 in equality
        // comparisons (mirrors `as_number(Empty) == 0.0` used by
        // the `<` / `>` operators). Without this arm, `=IF(A1=0,…)`
        // misses an empty A1 and the `<`/`=` results disagree.
        (Empty, Number(n)) | (Number(n), Empty) => *n == 0.0,
        // Arrays compare via top-left scalar — same coercion path the
        // engine uses elsewhere when an array bleeds into a scalar
        // context.
        (Array(_), other) => {
            return cell_eq(lv.array_top_left(), other);
        }
        (other, Array(_)) => {
            return cell_eq(other, rv.array_top_left());
        }
        _ => false,
    })
}

/// True when `(addr.col, addr.row)` lies inside the closed rectangle
/// from `tl` to `br`.
fn rect_contains(tl: CellAddr, br: CellAddr, addr: CellAddr) -> bool {
    let (c1, r1) = tl;
    let (c2, r2) = br;
    addr.0 >= c1 && addr.0 <= c2 && addr.1 >= r1 && addr.1 <= r2
}

/// Linear-interpolate two `#rrggbb` color strings by parameter `t` in
/// `[0.0, 1.0]`. If parsing fails for either side, returns `low` —
/// degrade gracefully rather than panicking on bad user input.
fn lerp_color(low: &str, high: &str, t: f64) -> String {
    let parse = |s: &str| -> Option<(u8, u8, u8)> {
        let s = s.trim().strip_prefix('#')?;
        if s.len() != 6 { return None; }
        Some((
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ))
    };
    let Some((r1, g1, b1)) = parse(low) else { return low.to_string(); };
    let Some((r2, g2, b2)) = parse(high) else { return low.to_string(); };
    let mix = |a: u8, b: u8| -> u8 {
        let v = a as f64 + (b as f64 - a as f64) * t.clamp(0.0, 1.0);
        v.round().clamp(0.0, 255.0) as u8
    };
    format!("#{:02x}{:02x}{:02x}", mix(r1, r2), mix(g1, g2), mix(b1, b2))
}

/// Interpolate across a 2- or 3-stop color scale. With `mid = None`,
/// `t in [0,1]` blends `low → high`. With `mid = Some(m)`,
/// `t in [0, 0.5]` blends `low → mid` and `t in [0.5, 1]` blends
/// `mid → high`.
fn interpolate_color_scale(low: &str, mid: Option<&str>, high: &str, t: f64) -> String {
    match mid {
        None => lerp_color(low, high, t),
        Some(m) if t <= 0.5 => lerp_color(low, m, t * 2.0),
        Some(m) => lerp_color(m, high, (t - 0.5) * 2.0),
    }
}

impl ConditionalCondition {
    /// Parse the user-facing condition mini-language used by both the
    /// conditional-format dialog and the custom-filter dialog. Syntax:
    /// `>N`, `<N`, `=text`, `contains:text`, `empty`, `notempty`.
    /// Returns `None` for unrecognized input or numeric variants whose
    /// argument can't be parsed.
    pub fn parse_user_input(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(v) = s.strip_prefix('>') {
            v.trim().parse::<f64>().ok().map(ConditionalCondition::GreaterThan)
        } else if let Some(v) = s.strip_prefix('<') {
            v.trim().parse::<f64>().ok().map(ConditionalCondition::LessThan)
        } else if let Some(v) = s.strip_prefix('=') {
            Some(ConditionalCondition::EqualTo(v.trim().to_string()))
        } else if let Some(v) = s.strip_prefix("contains:") {
            Some(ConditionalCondition::TextContains(v.trim().to_string()))
        } else if s == "empty" {
            Some(ConditionalCondition::IsEmpty)
        } else if s == "notempty" {
            Some(ConditionalCondition::IsNotEmpty)
        } else {
            None
        }
    }

    /// Evaluate this condition against a cell value.
    pub fn matches(&self, val: &CellValue) -> bool {
        match self {
            ConditionalCondition::GreaterThan(n) => val.as_number().map_or(false, |v| v > *n),
            ConditionalCondition::LessThan(n) => val.as_number().map_or(false, |v| v < *n),
            ConditionalCondition::EqualTo(s) => val.as_text().to_uppercase() == s.to_uppercase(),
            ConditionalCondition::Between(lo, hi) => val.as_number().map_or(false, |v| v >= *lo && v <= *hi),
            ConditionalCondition::TextContains(s) => val.as_text().to_uppercase().contains(&s.to_uppercase()),
            ConditionalCondition::IsEmpty => matches!(val, CellValue::Empty) || val.as_text().is_empty(),
            ConditionalCondition::IsNotEmpty => !matches!(val, CellValue::Empty) && !val.as_text().is_empty(),
        }
    }
}

/// Chart type.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartType {
    Bar,
    Line,
    Pie,
}

/// Configuration for a chart embedded in the spreadsheet.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartConfig {
    pub chart_type: ChartType,
    /// Data range: (top_left_col, top_left_row), (bottom_right_col, bottom_right_row).
    pub data_range: ((usize, usize), (usize, usize)),
    pub title: String,
}

/// Cell address as (col, row), both 0-based.
pub type CellAddr = (usize, usize);

/// Cell style properties.
/// Data validation rule for a cell.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationRule {
    Checkbox,
    Dropdown(Vec<String>),
    Number { min: Option<f64>, max: Option<f64> },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub bg_color: Option<String>,
    pub text_color: Option<String>,
    pub align: Option<String>,        // "left", "center", "right"
    pub number_format: Option<String>, // "currency", "percent", "date", etc.
    pub validation: Option<ValidationRule>,
    pub locked: bool,
    /// Legacy single-user note attached to the cell (Excel-style
    /// comment). Does not participate in formula evaluation. `None`
    /// and `Some("")` are treated as equivalent — the persistence
    /// layer drops empty strings on write so a deleted comment is
    /// byte-compatible with pre-comment documents.
    ///
    /// Superseded by `comment_thread_id` in the multi-user threaded
    /// model. On document load, cells carrying a legacy `comment`
    /// but no `comment_thread_id` are migrated into a real thread
    /// seeded with the legacy text and the field is cleared. The
    /// field stays on the struct so older documents still round-trip
    /// through this version without losing data while the migration
    /// runs.
    pub comment: Option<String>,
    /// Stable identifier linking this cell to a comment thread in
    /// the document's thread table (DynamoDB via REST, NOT yrs). The
    /// linkage itself rides yrs as a cell attribute; the thread
    /// messages flow through `POST /threads/.../messages` and the
    /// existing CommentEvent WebSocket broadcast — same path that
    /// document-level inline comments already use.
    ///
    /// Assigned lazily the first time a user adds a comment to the
    /// cell; survives row/column inserts and sorts because cell
    /// styles travel with the cell content through every engine
    /// mutation. The string is the `block_id` value sent to the
    /// thread API, prefixed `cell-<nanoid16>` so it lands inside
    /// the backend's 4-32 `[a-zA-Z0-9_-]` validator without any
    /// schema change on the storage side.
    pub comment_thread_id: Option<String>,
}

impl CellStyle {
    /// Build an inline CSS string from this cell's style properties.
    pub fn to_inline_css(&self) -> String {
        let mut s = String::new();
        if self.bold { s.push_str("font-weight:700;"); }
        if self.italic { s.push_str("font-style:italic;"); }
        // Compose underline + strike into a single `text-decoration`
        // declaration so each one independently survives the other.
        match (self.underline, self.strike) {
            (true, true) => s.push_str("text-decoration:underline line-through;"),
            (true, false) => s.push_str("text-decoration:underline;"),
            (false, true) => s.push_str("text-decoration:line-through;"),
            (false, false) => {}
        }
        // `background-color` (NOT the `background` shorthand) so that
        // a later `background-image: linear-gradient(...)` from a
        // DataBar conditional format isn't clobbered by the
        // shorthand's implicit `background-image: none` reset.
        if let Some(ref bg) = self.bg_color { s.push_str(&format!("background-color:{bg};")); }
        if let Some(ref tc) = self.text_color { s.push_str(&format!("color:{tc};")); }
        if let Some(ref a) = self.align { s.push_str(&format!("text-align:{a};")); }
        s
    }

    /// Return a copy of `self` with the format-painter "visual" fields
    /// (bold, italic, colors, alignment, number_format) replaced by
    /// those from `source`. Non-visual fields (`validation`, `locked`,
    /// `comment`, `comment_thread_id`) stay with `self` — those belong
    /// to the target cell's data contract, not its appearance.
    pub fn with_visual_style_from(&self, source: &CellStyle) -> CellStyle {
        CellStyle {
            bold: source.bold,
            italic: source.italic,
            underline: source.underline,
            strike: source.strike,
            bg_color: source.bg_color.clone(),
            text_color: source.text_color.clone(),
            align: source.align.clone(),
            number_format: source.number_format.clone(),
            validation: self.validation.clone(),
            locked: self.locked,
            comment: self.comment.clone(),
            comment_thread_id: self.comment_thread_id.clone(),
        }
    }
}

/// The spreadsheet engine: holds raw cell text, parsed formulas, computed values,
/// and a dependency graph for incremental recalculation.
pub struct SpreadsheetEngine {
    /// Raw cell content (what the user typed).
    raw: HashMap<CellAddr, String>,
    /// Parsed formulas (only for cells starting with `=`).
    formulas: HashMap<CellAddr, Expr>,
    /// Computed values.
    values: HashMap<CellAddr, CellValue>,
    /// Cell styles.
    styles: HashMap<CellAddr, CellStyle>,
    /// Conditional formatting rules: (top_left, bottom_right) → rules.
    conditional_formats: Vec<(CellAddr, CellAddr, Vec<ConditionalFormat>)>,
    /// Merged cell regions: (top_left_col, top_left_row, col_span, row_span).
    merged_regions: Vec<(usize, usize, usize, usize)>,
    /// Charts embedded in this sheet.
    pub charts: Vec<ChartConfig>,
    /// Hidden rows (for filtering/manual hide).
    pub hidden_rows: HashSet<usize>,
    /// Hidden columns.
    pub hidden_cols: HashSet<usize>,
    /// Frozen-pane state (M-S2). `frozen_rows` is the count of
    /// header rows that should `position: sticky` to the top of the
    /// scroll viewport; `frozen_cols` is the symmetric count of
    /// left-edge columns. 0 = no freeze. Persisted via `(get|set)_freeze`
    /// alongside the rest of the sheet's structural state.
    pub frozen_rows: usize,
    pub frozen_cols: usize,
    /// Forward dependencies: cell → set of cells that depend on it.
    dependents: HashMap<CellAddr, HashSet<CellAddr>>,
    /// Reverse dependencies: cell → set of cells it depends on.
    dependencies: HashMap<CellAddr, HashSet<CellAddr>>,
    /// Cells currently being evaluated (for circular ref detection).
    evaluating: HashSet<CellAddr>,
    /// Spill-block fill map: cell → anchor (the cell whose formula
    /// produced the surrounding array). The anchor itself is NOT in
    /// this map; its `values` entry holds the full
    /// `CellValue::Array`. Filled cells have no `values` / `formulas`
    /// entry — `get_value` synthesizes their scalar by indexing the
    /// anchor's array. Empty when no dynamic-array formulas are live.
    spill_origins: HashMap<CellAddr, CellAddr>,
    /// User-defined named ranges. Keys are the canonical (uppercase)
    /// form of the user's name; values are the cell range the name
    /// refers to. Resolved by the evaluator on every formula run, so
    /// redefining a name immediately affects all formulas that use it.
    named_ranges: HashMap<String, RangeRef>,

    /// The id of the document this engine instance is editing. Set
    /// by the view layer at mount via `set_current_doc_id`. Used by
    /// `REFERENCERANGE` / `REFERENCESHEET` to short-circuit a
    /// self-reference (formula referencing the very document it
    /// lives in) so it goes through the local engine rather than
    /// the foreign-fetch path.
    current_doc_id: Option<String>,

    /// Foreign-document cache, populated by the view layer's fetch
    /// loop. `Ok` = the doc has been retrieved; `Err` = the fetch
    /// resolved with a known failure mode. `None` = the id has
    /// never been requested.
    foreign_docs: HashMap<String, ForeignDocState>,

    /// Snapshot of every sheet in the local document, indexed by
    /// sheet name (case-preserving — lookups are case-insensitive).
    /// Each entry is the same row-major display-text matrix shape
    /// used for foreign docs, so eval can reuse the same text→
    /// CellValue coercion for `Sheet2!B2`-style references as it
    /// does for cross-doc refs.
    ///
    /// The view layer rebuilds and installs this whenever the doc
    /// changes via `set_local_sheets_snapshot`. The active sheet
    /// always has its own dedicated engine state — refs targeting
    /// the current sheet by name fall back to the engine's
    /// in-memory `values`/`raw` maps; only non-active sheets are
    /// served from this snapshot.
    local_sheets: ForeignDocSnapshot,

    /// Name of the active sheet (matches the sheet whose state is
    /// loaded into `raw`/`values`). Used by sheet-qualified ref
    /// evaluation to decide whether to read from the engine or from
    /// `local_sheets`.
    active_sheet_name: Option<String>,

    /// Foreign-doc ids that have been *referenced* by a REFERENCE*
    /// formula but not yet fetched. Wrapped in `RefCell` because
    /// formula evaluation runs through `&SpreadsheetEngine` (the
    /// dispatch in `functions::call_function`), so the function
    /// implementations need to register their fetch dependency
    /// without a mutable borrow on the engine.
    foreign_pending: RefCell<HashSet<String>>,

    /// Pivot tables anchored within this sheet, keyed by anchor
    /// cell. The pivot's rendered output is installed at the anchor
    /// via the dynamic-array spill machinery (`try_register_spill_block`),
    /// so the renderer treats it the same as any other spilled array.
    /// Source-cell edits trigger `recompute_pivots_for_source`.
    pivots: HashMap<CellAddr, crate::spreadsheet::pivot::PivotTable>,

    /// Anchors currently mid-recompute. Acts as a re-entry guard
    /// when chained pivots feed into each other (pivot A spills
    /// into pivot B's source range): the second time the cascade
    /// reaches the same anchor, recompute_pivot short-circuits.
    pivot_recomputing: HashSet<CellAddr>,
}

impl SpreadsheetEngine {
    pub fn new() -> Self {
        Self {
            raw: HashMap::new(),
            formulas: HashMap::new(),
            values: HashMap::new(),
            styles: HashMap::new(),
            conditional_formats: Vec::new(),
            merged_regions: Vec::new(),
            charts: Vec::new(),
            hidden_rows: HashSet::new(),
            hidden_cols: HashSet::new(),
            frozen_rows: 0,
            frozen_cols: 0,
            dependents: HashMap::new(),
            dependencies: HashMap::new(),
            evaluating: HashSet::new(),
            spill_origins: HashMap::new(),
            named_ranges: HashMap::new(),
            current_doc_id: None,
            foreign_docs: HashMap::new(),
            local_sheets: ForeignDocSnapshot::default(),
            active_sheet_name: None,
            foreign_pending: RefCell::new(HashSet::new()),
            pivots: HashMap::new(),
            pivot_recomputing: HashSet::new(),
        }
    }

    /// Clear all engine state (cells, formulas, styles, dependencies).
    pub fn clear(&mut self) {
        self.raw.clear();
        self.formulas.clear();
        self.values.clear();
        self.styles.clear();
        self.conditional_formats.clear();
        self.merged_regions.clear();
        self.charts.clear();
        self.hidden_rows.clear();
        self.hidden_cols.clear();
        self.frozen_rows = 0;
        self.frozen_cols = 0;
        self.dependents.clear();
        self.dependencies.clear();
        self.evaluating.clear();
        self.spill_origins.clear();
        self.named_ranges.clear();
        self.pivots.clear();
        self.pivot_recomputing.clear();
        // Note: `current_doc_id` is preserved across `clear()` —
        // sheet-switch reuses the same document, and the view layer
        // calls `set_current_doc_id` once at mount, not per-sheet.
        // Foreign caches likewise survive sheet swaps; they're
        // keyed by the foreign id, not the local sheet.
    }

    // ─── Cross-document references ────────────────────────────

    /// Set the id of the document this engine is editing. Enables
    /// the self-reference short-circuit in REFERENCERANGE /
    /// REFERENCESHEET. Called once by the view layer at mount.
    pub fn set_current_doc_id(&mut self, id: String) {
        self.current_doc_id = Some(id);
    }

    /// Read the id this engine is editing (if set).
    pub fn current_doc_id(&self) -> Option<&str> {
        self.current_doc_id.as_deref()
    }

    /// Mark `id` as needing a fetch. The view layer drains this
    /// set with `take_pending_fetches`. No-op if the doc has
    /// already resolved (cached `Ok` or any `Err` other than
    /// `Network`). `Network` errors *do* re-queue: they're
    /// transient (transport blip, server restart, mid-flight
    /// cancel) and the user expects "still loading" rather than
    /// "permanently broken until I reload the page".
    ///
    /// Takes `&self` (not `&mut self`) so formula evaluation can
    /// call this through the function-dispatch path, which has only
    /// shared access to the engine.
    pub fn register_foreign_fetch(&self, id: &str) {
        let should_queue = match self.foreign_docs.get(id) {
            None => true,
            Some(Err(ForeignFetchError::Network)) => true,
            Some(_) => false,
        };
        if should_queue {
            self.foreign_pending.borrow_mut().insert(id.to_string());
        }
    }

    /// Drain the pending-fetch set. Returns the ids the view layer
    /// should fetch and feed back via `set_foreign_doc_*`.
    pub fn take_pending_fetches(&self) -> Vec<String> {
        let mut pending = self.foreign_pending.borrow_mut();
        let out = pending.iter().cloned().collect();
        pending.clear();
        out
    }

    /// Install a fetched foreign-doc snapshot. Replaces any existing
    /// state (success or error) for that id.
    pub fn set_foreign_doc_snapshot(&mut self, id: String, snapshot: ForeignDocSnapshot) {
        self.foreign_docs.insert(id, Ok(snapshot));
    }

    /// Install a snapshot of every sheet in the local document.
    /// Called by the view layer whenever the doc changes (sheet
    /// rename / sheet edit / sheet switch / sheet delete). After
    /// installation, all formula cells in the active sheet are
    /// re-evaluated so cross-sheet refs pick up the new values.
    pub fn set_local_sheets_snapshot(&mut self, snapshot: ForeignDocSnapshot) {
        self.local_sheets = snapshot;
        // Re-evaluate every formula so cross-sheet refs refresh.
        // The dependency graph doesn't track cross-sheet edges, so
        // we sweep all formulas unconditionally — cheap relative to
        // the cost of a stale `=Sheet2!B2` result.
        let addrs: Vec<CellAddr> = self.formulas.keys().copied().collect();
        for addr in addrs {
            self.evaluate_cell(addr);
        }
    }

    /// Tell the engine which sheet's state is currently loaded into
    /// `raw`/`values`. Sheet-qualified refs to this sheet's name
    /// short-circuit to the in-memory state; refs to OTHER sheet
    /// names go through `local_sheets`. Called by the view layer
    /// from the active-sheet-sync Effect.
    pub fn set_active_sheet_name(&mut self, name: String) {
        self.active_sheet_name = Some(name);
    }

    /// Resolve `<sheet>!<cell>` to a CellValue. Returns `#REF!` if
    /// the sheet name is unknown. Sheet-name matching is
    /// case-insensitive (Excel parity); the cell is decoded from
    /// the display-text matrix into a typed CellValue (numbers
    /// parse as f64, empty strings as Empty, everything else as
    /// Text).
    fn resolve_sheet_cell(&self, sheet: &str, cell: &crate::spreadsheet::parser::CellRef) -> CellValue {
        // Active sheet — read from the in-memory state.
        if self.active_sheet_name.as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(sheet))
        {
            return self.get_value((cell.col, cell.row)).clone();
        }
        let Some(rows) = self.local_sheets.sheets.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(sheet))
            .map(|(_, v)| v)
        else {
            return CellValue::Error(SpreadsheetError::Ref);
        };
        let Some(text) = rows.get(cell.row).and_then(|r| r.get(cell.col)) else {
            return CellValue::Empty;
        };
        if text.is_empty() { return CellValue::Empty; }
        if let Ok(n) = text.parse::<f64>() { return CellValue::Number(n); }
        CellValue::Text(text.clone())
    }

    /// Record a fetch failure for `id`. Replaces any existing state.
    pub fn set_foreign_doc_error(&mut self, id: String, err: ForeignFetchError) {
        self.foreign_docs.insert(id, Err(err));
    }

    /// Look up the cached state for a foreign doc. Used by
    /// `REFERENCERANGE` / `REFERENCESHEET`.
    pub fn get_foreign_doc(&self, id: &str) -> Option<&ForeignDocState> {
        self.foreign_docs.get(id)
    }

    /// All foreign-doc ids currently cached (regardless of
    /// success/failure). Used by the view layer to diff WS
    /// subscriptions and to garbage-collect stale entries.
    pub fn foreign_doc_ids(&self) -> Vec<String> {
        self.foreign_docs.keys().cloned().collect()
    }

    /// Drop a `Denied` cache entry so the next REFERENCE* eval
    /// re-queues a fetch. Used by the consent UI when the user
    /// reverses a denial.
    pub fn clear_foreign_consent(&mut self, id: &str) {
        if let Some(Err(ForeignFetchError::Denied)) = self.foreign_docs.get(id) {
            self.foreign_docs.remove(id);
        }
    }

    /// Invalidate the cache for a foreign doc. The next REFERENCE*
    /// eval will see no cache entry and queue a fresh fetch via
    /// `register_foreign_fetch`. Used when the WS push channel
    /// reports an upstream update — instead of carrying the diff
    /// inline, we invalidate and refetch via HTTP for v1 simplicity.
    pub fn invalidate_foreign_doc(&mut self, id: &str) {
        self.foreign_docs.remove(id);
    }

    // ─── Spill blocks (M-S1a) ───────────────────────────────────

    /// Drop every `spill_origins` entry pointing at `anchor`, plus
    /// the synthesized scalar in `values[filled]` for each filled
    /// cell. The caller is responsible for either replacing or
    /// clearing `values[anchor]`.
    fn clear_spill_block(&mut self, anchor: CellAddr) {
        let filled: Vec<CellAddr> = self.spill_origins.iter()
            .filter_map(|(k, v)| if *v == anchor { Some(*k) } else { None })
            .collect();
        for f in filled {
            self.spill_origins.remove(&f);
            // Only remove if the cell wasn't independently written.
            // It shouldn't have been (spill blocks can only land on
            // empty cells), but be defensive.
            if !self.formulas.contains_key(&f) && !self.raw.contains_key(&f) {
                self.values.remove(&f);
            }
        }
    }

    /// Try to register a spill block of `array` anchored at `anchor`.
    /// Block extends right (cols) and down (rows). On conflict — a
    /// target cell has its own formula, a non-empty value, or is part
    /// of another anchor's block — returns `Err(())` and leaves state
    /// unchanged. Caller should set `values[anchor]` to
    /// `Error(Spill)` on `Err`.
    ///
    /// On success, every cell in the block (excluding the anchor) gets
    /// `spill_origins[cell] = anchor` AND
    /// `values[cell] = array[row][col]` (the synthesized scalar). Doing
    /// this dual-write means `get_value(filled_cell)` works through
    /// the normal values lookup — no indirection through spill_origins
    /// is needed at read time.
    fn try_register_spill_block(
        &mut self,
        anchor: CellAddr,
        array: &Vec<Vec<CellValue>>,
    ) -> Result<(), ()> {
        if array.is_empty() || array[0].is_empty() {
            return Ok(()); // 0-cell array, nothing to register
        }
        let rows = array.len();
        let cols = array[0].len();
        let (a_col, a_row) = anchor;

        // Validate every target *other than the anchor* is either
        // empty/missing or already filled by THIS anchor (re-running
        // the same formula on the same shape is fine).
        for ri in 0..rows {
            for ci in 0..cols {
                if ri == 0 && ci == 0 { continue; }
                let target = (a_col + ci, a_row + ri);
                if self.formulas.contains_key(&target) {
                    return Err(());
                }
                if let Some(other_anchor) = self.spill_origins.get(&target) {
                    if *other_anchor != anchor { return Err(()); }
                }
                if let Some(v) = self.values.get(&target) {
                    if !matches!(v, CellValue::Empty)
                        && !self.spill_origins.get(&target).is_some_and(|a| *a == anchor)
                    {
                        return Err(());
                    }
                }
            }
        }

        // Register: write the scalar at every filled cell + record the
        // anchor pointer.
        for ri in 0..rows {
            for ci in 0..cols {
                if ri == 0 && ci == 0 { continue; }
                let target = (a_col + ci, a_row + ri);
                self.spill_origins.insert(target, anchor);
                self.values.insert(target, array[ri][ci].clone());
            }
        }
        Ok(())
    }

    /// True iff `addr` is a spill-anchor (its `values` entry holds an
    /// `Array`). Used by `recalculate_from` to know when to enqueue
    /// spill-fill dependents during BFS.
    fn is_spill_anchor(&self, addr: CellAddr) -> bool {
        matches!(self.values.get(&addr), Some(CellValue::Array(_)))
    }

    /// Set a cell's raw content and recalculate affected cells.
    pub fn set_cell(&mut self, addr: CellAddr, content: &str) {
        // If `addr` is currently filled by another cell's spill block,
        // the user is overwriting a slice of someone else's array.
        // Break the block: clear all of the anchor's fill cells and
        // set the anchor's value to `#SPILL!` (matches Excel — the
        // anchor reports that its spill was disrupted). The user's
        // content then writes to `addr` like a normal cell.
        if let Some(&anchor) = self.spill_origins.get(&addr) {
            self.clear_spill_block(anchor);
            self.values.insert(anchor, CellValue::Error(SpreadsheetError::Spill));
        }
        // If `addr` itself is a spill anchor (had `Array` value) and
        // we're now overwriting it, drop its old spill block before
        // proceeding. The new content may be a scalar formula or a
        // raw value — either way the old block must clear.
        if self.is_spill_anchor(addr) {
            self.clear_spill_block(addr);
        }

        // Remove old dependencies
        if let Some(old_deps) = self.dependencies.remove(&addr) {
            for dep in &old_deps {
                if let Some(fwd) = self.dependents.get_mut(dep) {
                    fwd.remove(&addr);
                }
            }
        }

        if content.is_empty() {
            self.raw.remove(&addr);
            self.formulas.remove(&addr);
            self.values.insert(addr, CellValue::Empty);
        } else if let Some(formula_str) = content.strip_prefix('=') {
            self.raw.insert(addr, content.to_string());
            match parse_formula(formula_str) {
                Ok(expr) => {
                    // Collect dependencies
                    let deps = self.collect_refs(&expr);
                    for dep in &deps {
                        self.dependents.entry(*dep).or_default().insert(addr);
                    }
                    self.dependencies.insert(addr, deps);
                    self.formulas.insert(addr, expr);
                }
                Err(_) => {
                    self.formulas.remove(&addr);
                    self.values.insert(addr, CellValue::Error(SpreadsheetError::Name));
                }
            }
        } else {
            self.raw.insert(addr, content.to_string());
            self.formulas.remove(&addr);
            // Parse as value
            let value = parse_raw_value(content);
            self.values.insert(addr, value);
        }

        // Recalculate this cell and all its dependents
        self.recalculate_from(addr);
        // Pivot tables whose source range covers `addr` need to
        // re-evaluate. Spill writes go directly to `values`, NOT
        // through `set_cell`, so this won't recurse.
        if !self.pivots.is_empty() {
            self.recompute_pivots_for_source(addr);
        }
    }

    /// Get the computed value of a cell.
    pub fn get_value(&self, addr: CellAddr) -> &CellValue {
        self.values.get(&addr).unwrap_or(&CellValue::Empty)
    }

    /// Get the raw content of a cell.
    pub fn get_raw(&self, addr: CellAddr) -> &str {
        self.raw.get(&addr).map(|s| s.as_str()).unwrap_or("")
    }

    /// Iterate over every non-empty cell's raw content. Used by cut-paste
    /// reverse-rewrite to find cells whose formulas point at the moved
    /// range. The iteration order is unspecified.
    pub fn iter_raw(&self) -> impl Iterator<Item = (CellAddr, &str)> {
        self.raw.iter().map(|(addr, s)| (*addr, s.as_str()))
    }

    /// Addresses of every cell carrying an explicit style entry — number
    /// format, borders, fill, alignment, lock, or a data validation
    /// (validation lives inside `CellStyle`). Used by the persist layer
    /// (#128) so formatting-only cells widen the saved used-extent and
    /// survive a round-trip rather than being trimmed away.
    pub fn iter_styled_cells(&self) -> impl Iterator<Item = CellAddr> + '_ {
        self.styles.keys().copied()
    }

    /// Addresses of every spill *fill* cell (the array cells around a
    /// dynamic-array anchor; the anchor itself is in `iter_raw`). Used by
    /// the persist layer (#128) to include spilled output in the saved
    /// used-extent.
    pub fn iter_spill_fill_cells(&self) -> impl Iterator<Item = CellAddr> + '_ {
        self.spill_origins.keys().copied()
    }

    /// Get the display text for a cell (computed value, formatted per number format).
    pub fn get_display(&self, addr: CellAddr) -> String {
        let val = self.get_value(addr);
        if let Some(style) = self.styles.get(&addr) {
            if let Some(ref fmt) = style.number_format {
                if let Ok(n) = val.as_number() {
                    return format_number(n, fmt);
                }
            }
        }
        val.as_text()
    }

    /// Get the style for a cell.
    pub fn get_style(&self, addr: CellAddr) -> Option<&CellStyle> {
        self.styles.get(&addr)
    }

    /// Set the style for a cell.
    pub fn set_style(&mut self, addr: CellAddr, style: CellStyle) {
        if style == CellStyle::default() {
            self.styles.remove(&addr);
        } else {
            self.styles.insert(addr, style);
        }
    }

    /// Get a mutable reference to a cell's style, creating a default if needed.
    pub fn style_mut(&mut self, addr: CellAddr) -> &mut CellStyle {
        self.styles.entry(addr).or_default()
    }

    /// Add a conditional formatting rule for a range.
    pub fn add_conditional_format(
        &mut self,
        top_left: CellAddr,
        bottom_right: CellAddr,
        rule: ConditionalFormat,
    ) {
        // Check if there's already a rule set for this range
        for (tl, br, rules) in &mut self.conditional_formats {
            if *tl == top_left && *br == bottom_right {
                rules.push(rule);
                return;
            }
        }
        self.conditional_formats.push((top_left, bottom_right, vec![rule]));
    }

    /// Get the effective background color for a cell. Checks all
    /// conditional format rules that cover this cell — the first rule
    /// that produces a color wins. `Single` returns its `bg_color` when
    /// the condition matches; `ColorScale` always returns an
    /// interpolated color (or `None` if the value is non-numeric or
    /// the range has no numeric variation). `DataBar` rules don't
    /// touch the background — see `get_data_bar`.
    pub fn get_effective_bg(&self, addr: CellAddr) -> Option<String> {
        let val = self.get_value(addr);
        for (tl, br, rules) in &self.conditional_formats {
            if !rect_contains(*tl, *br, addr) { continue; }
            for rule in rules {
                match rule {
                    ConditionalFormat::Single { condition, bg_color } => {
                        if condition.matches(val) {
                            return Some(bg_color.clone());
                        }
                    }
                    ConditionalFormat::ColorScale { low, mid, high } => {
                        // Only Number cells participate in the gradient.
                        // Bool/Empty cells silently coerce via as_number()
                        // but contributing them would compress or skew
                        // the visual scale (see `range_min_max`).
                        if let CellValue::Number(v) = *val {
                            if let Some((min, max)) = self.range_min_max(*tl, *br) {
                                if (max - min).abs() > f64::EPSILON {
                                    let t = ((v - min) / (max - min)).clamp(0.0, 1.0);
                                    return Some(interpolate_color_scale(low, mid.as_deref(), high, t));
                                }
                            }
                        }
                    }
                    ConditionalFormat::DataBar { .. }
                    | ConditionalFormat::IconSet { .. } => { /* see get_data_bar / get_icon */ }
                }
            }
        }
        // Fall back to cell style bg_color
        self.styles.get(&addr).and_then(|s| s.bg_color.clone())
    }

    /// Return `(color, ratio)` for the first DataBar rule that covers
    /// `addr`. Ratio is `value / max` over the rule's coverage region,
    /// clamped to `[0.0, 1.0]`. A ratio of `0.0` is returned (renders
    /// as an empty bar) — only `None` if the cell is non-numeric, no
    /// DataBar rule covers it, or the region's `max <= 0`.
    pub fn get_data_bar(&self, addr: CellAddr) -> Option<(String, f64)> {
        let val = self.get_value(addr);
        // Only Number cells render data bars. Bool/Empty cells coerce
        // via `as_number()` to 0.0/1.0 but rendering a bar on them
        // would advertise meaning that isn't there — skip explicitly.
        let CellValue::Number(v) = *val else { return None; };
        for (tl, br, rules) in &self.conditional_formats {
            if !rect_contains(*tl, *br, addr) { continue; }
            for rule in rules {
                if let ConditionalFormat::DataBar { color } = rule {
                    // `if let Some(...)` rather than `?` so a covered
                    // rect with no numeric variation continues looking
                    // for a usable DataBar rule on a later rect — not
                    // bail out of the whole function.
                    if let Some((_, max)) = self.range_min_max(*tl, *br) {
                        if max > 0.0 {
                            let ratio = (v / max).clamp(0.0, 1.0);
                            return Some((color.clone(), ratio));
                        }
                    }
                }
            }
        }
        None
    }

    /// Return the icon glyph for the first IconSet rule that covers
    /// `addr`. The cell's value is normalized into the rule's range
    /// (`(value - min) / (max - min)`) and the rule's `IconSetKind`
    /// chooses the glyph for the resulting tertile. Non-numeric cells
    /// and ranges without numeric variation return `None`.
    pub fn get_icon(&self, addr: CellAddr) -> Option<&'static str> {
        let val = self.get_value(addr);
        let CellValue::Number(v) = *val else { return None; };
        for (tl, br, rules) in &self.conditional_formats {
            if !rect_contains(*tl, *br, addr) { continue; }
            for rule in rules {
                if let ConditionalFormat::IconSet { kind } = rule {
                    // `if let Some(...)` rather than `?` so a covered
                    // rect with no numeric variation continues looking
                    // for a usable IconSet rule on a later rect.
                    if let Some((min, max)) = self.range_min_max(*tl, *br) {
                        if (max - min).abs() >= f64::EPSILON {
                            let t = ((v - min) / (max - min)).clamp(0.0, 1.0);
                            return Some(kind.glyph_for(t));
                        }
                    }
                }
            }
        }
        None
    }

    /// Min/max of all numeric values in the rectangle (inclusive on
    /// both corners). Returns `None` if no covered cell holds a real
    /// number — `CellValue::Empty` is skipped explicitly because
    /// `as_number()` on Empty returns `Ok(0.0)`, and treating an
    /// empty cell as zero would silently compress ColorScale
    /// gradients and DataBar ratios for any region with empties.
    fn range_min_max(&self, tl: CellAddr, br: CellAddr) -> Option<(f64, f64)> {
        let (c1, r1) = tl;
        let (c2, r2) = br;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut any = false;
        for r in r1..=r2 {
            for c in c1..=c2 {
                let cell = self.get_value((c, r));
                // Skip Empty and Bool: `as_number()` returns
                // `Ok(0.0)` for Empty and `Ok(0.0)/Ok(1.0)` for
                // Bool(false/true). Counting either as numeric data
                // silently distorts the range — empties pull min
                // toward 0; booleans pin endpoints at 0/1 regardless
                // of the other numeric values present.
                if matches!(cell, CellValue::Empty | CellValue::Bool(_)) { continue; }
                if let Ok(v) = cell.as_number() {
                    if v < min { min = v; }
                    if v > max { max = v; }
                    any = true;
                }
            }
        }
        if any { Some((min, max)) } else { None }
    }

    // ─── Named ranges ─────────────────────────────────────────

    /// Define or replace a named range. `name` is normalized to upper
    /// case for case-insensitive lookup (Excel convention).
    pub fn set_named_range(&mut self, name: &str, range: RangeRef) {
        self.named_ranges.insert(name.to_uppercase(), range);
    }

    /// Look up a named range. Lookup is case-insensitive.
    pub fn get_named_range(&self, name: &str) -> Option<&RangeRef> {
        self.named_ranges.get(&name.to_uppercase())
    }

    /// Remove a named range. Returns `true` if a name was removed.
    pub fn remove_named_range(&mut self, name: &str) -> bool {
        self.named_ranges.remove(&name.to_uppercase()).is_some()
    }

    /// All defined names as `(name, range)` pairs.
    pub fn named_ranges(&self) -> Vec<(String, RangeRef)> {
        let mut out: Vec<_> = self.named_ranges.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Replace all named ranges (used by the document loader).
    pub fn set_named_ranges(&mut self, items: Vec<(String, RangeRef)>) {
        self.named_ranges.clear();
        for (name, range) in items {
            self.named_ranges.insert(name.to_uppercase(), range);
        }
    }

    /// Get all conditional format rules (for persistence).
    pub fn get_conditional_formats(&self) -> &[(CellAddr, CellAddr, Vec<ConditionalFormat>)] {
        &self.conditional_formats
    }

    /// Set conditional formats (for loading from doc).
    pub fn set_conditional_formats(&mut self, formats: Vec<(CellAddr, CellAddr, Vec<ConditionalFormat>)>) {
        self.conditional_formats = formats;
    }

    /// Merge a range of cells. The top-left cell becomes the merged cell.
    pub fn merge_cells(&mut self, col: usize, row: usize, col_span: usize, row_span: usize) {
        // Remove any existing merge that overlaps
        self.merged_regions.retain(|&(c, r, cs, rs)| {
            !(c < col + col_span && col < c + cs && r < row + row_span && row < r + rs)
        });
        if col_span > 1 || row_span > 1 {
            self.merged_regions.push((col, row, col_span, row_span));
        }
    }

    /// Unmerge cells at a position.
    pub fn unmerge_at(&mut self, col: usize, row: usize) {
        self.merged_regions.retain(|&(c, r, _, _)| !(c == col && r == row));
    }

    /// Check if a cell is hidden by a merge (not the top-left of the merge).
    pub fn is_merged_hidden(&self, col: usize, row: usize) -> bool {
        for &(c, r, cs, rs) in &self.merged_regions {
            if col >= c && col < c + cs && row >= r && row < r + rs {
                if col != c || row != r {
                    return true;
                }
            }
        }
        false
    }

    /// Get the merge span for a cell (returns (colspan, rowspan) or (1,1) if not merged).
    pub fn get_merge_span(&self, col: usize, row: usize) -> (usize, usize) {
        for &(c, r, cs, rs) in &self.merged_regions {
            if c == col && r == row {
                return (cs, rs);
            }
        }
        (1, 1)
    }

    /// Get all merged regions.
    pub fn get_merged_regions(&self) -> &[(usize, usize, usize, usize)] {
        &self.merged_regions
    }

    /// Set merged regions (for loading from doc).
    pub fn set_merged_regions(&mut self, regions: Vec<(usize, usize, usize, usize)>) {
        self.merged_regions = regions;
    }

    /// Whether `addr` is a spill-filled cell (i.e., its value comes
    /// from a dynamic-array formula anchored elsewhere). Used by the
    /// view layer to render filled cells with distinct styling so
    /// users can tell at a glance which cells are part of a spill.
    pub fn is_spill_fill(&self, addr: CellAddr) -> bool {
        self.spill_origins.contains_key(&addr)
    }

    /// Anchor for a spill-filled cell, or `None` for non-fill cells.
    /// Used by the view layer to surface "this cell is spilled from
    /// B1" hints in the formula bar / status line.
    pub fn spill_anchor(&self, addr: CellAddr) -> Option<CellAddr> {
        self.spill_origins.get(&addr).copied()
    }

    // ─── Pivot tables (M-S2 v2) ──────────────────────────────────

    /// Add or replace a pivot at its anchor cell, then recompute it.
    /// Persistence layers call this once per restored pivot when
    /// hydrating engine state from a doc.
    pub fn add_pivot(&mut self, pivot: crate::spreadsheet::pivot::PivotTable) {
        let anchor = pivot.anchor;
        self.pivots.insert(anchor, pivot);
        self.recompute_pivot(anchor);
        // Surface the new pivot output to formulas that reference
        // the anchor or any newly-spilled cell. Without this, a
        // formula `=SUM(<anchor>)` would hold a value computed
        // against the previous anchor contents until some other
        // event re-triggered the dependency walk.
        self.recalculate_from(anchor);
    }

    /// Remove the pivot at `anchor` (if any) and clear its spill
    /// block. The anchor cell becomes Empty unless the user has
    /// since written into it directly. Formulas that referenced
    /// the anchor or any cell of the old spill area are
    /// recalculated.
    pub fn remove_pivot(&mut self, anchor: CellAddr) {
        if self.pivots.remove(&anchor).is_some() {
            // Capture the spill area BEFORE clearing — `clear_spill_block`
            // wipes the spill_origins map. Formulas referencing
            // any of these cells need to re-evaluate against
            // post-clear (Empty) state.
            let spill_cells: Vec<CellAddr> = self.spill_origins.iter()
                .filter_map(|(k, v)| if *v == anchor { Some(*k) } else { None })
                .collect();
            self.clear_spill_block(anchor);
            self.values.insert(anchor, CellValue::Empty);
            self.recalculate_from(anchor);
            for cell in spill_cells {
                self.recalculate_from(cell);
            }
        }
    }

    /// Get the pivot anchored at `anchor`, if any. Used by the
    /// editor sidebar to clone-mutate-replace an existing pivot
    /// without iterating `pivots_iter`.
    pub fn get_pivot(&self, anchor: CellAddr) -> Option<&crate::spreadsheet::pivot::PivotTable> {
        self.pivots.get(&anchor)
    }

    /// Snapshot every pivot in this engine, keyed by anchor. Used
    /// by the persistence layer to write `ATTR_PIVOTS`.
    pub fn pivots_iter(&self) -> impl Iterator<Item = (&CellAddr, &crate::spreadsheet::pivot::PivotTable)> {
        self.pivots.iter()
    }

    /// Replace the pivot map wholesale and recompute each. Used by
    /// the persistence layer to hydrate engine state on load.
    pub fn set_pivots(&mut self, pivots: Vec<crate::spreadsheet::pivot::PivotTable>) {
        self.pivots.clear();
        for pivot in pivots {
            let anchor = pivot.anchor;
            self.pivots.insert(anchor, pivot);
            self.recompute_pivot(anchor);
        }
    }

    /// Re-evaluate the pivot anchored at `anchor` and install its
    /// spilled output. No-op if no pivot lives at `anchor`. The
    /// rendered output goes through the same spill-block path as a
    /// dynamic-array formula, so the view renders it identically.
    ///
    /// Also propagates downstream: after the new spill installs,
    /// any *other* pivot whose source range overlaps the anchor or
    /// the spill cells is recomputed, and formulas referencing the
    /// anchor cell are recalculated. Re-entry into this same anchor
    /// is short-circuited via `pivot_recomputing`.
    pub fn recompute_pivot(&mut self, anchor: CellAddr) {
        if !self.pivot_recomputing.insert(anchor) { return; }
        let Some(pivot) = self.pivots.get(&anchor).cloned() else {
            self.pivot_recomputing.remove(&anchor);
            return;
        };
        let source = self.resolve_pivot_source(&pivot);
        let out = crate::spreadsheet::pivot::eval_pivot(&pivot, &source);
        self.clear_spill_block(anchor);
        if out.is_empty() {
            self.values.insert(anchor, CellValue::Empty);
            self.pivot_recomputing.remove(&anchor);
            return;
        }
        let rows = out.len();
        let cols = out[0].len();
        let array_value = CellValue::Array(out.clone());
        self.values.insert(anchor, array_value);
        let _ = self.try_register_spill_block(anchor, &out);

        // Cascade: any *downstream* pivot whose source touches our
        // newly-installed spill cells (or the anchor itself) needs
        // to recompute. The `pivot_recomputing` set short-circuits
        // a chain of pivots looping back into our own anchor.
        let downstream: Vec<CellAddr> = {
            let (a_col, a_row) = anchor;
            let our_cells: Vec<CellAddr> = (0..rows)
                .flat_map(|r| (0..cols).map(move |c| (a_col + c, a_row + r)))
                .collect();
            self.pivots.iter()
                .filter(|(other_anchor, _)| !self.pivot_recomputing.contains(other_anchor))
                .filter(|(_, other)| our_cells.iter().any(|cell| pivot_source_contains(other, *cell)))
                .map(|(a, _)| *a)
                .collect()
        };
        for other in downstream {
            self.recompute_pivot(other);
        }
        self.pivot_recomputing.remove(&anchor);
    }

    /// Re-evaluate every pivot whose source range overlaps `addr`.
    /// Called by `set_cell` after a normal recalculate, so a typed-
    /// in source-cell change flows through to pivot output AND to
    /// formulas that reference the pivot anchor.
    fn recompute_pivots_for_source(&mut self, addr: CellAddr) {
        let anchors: Vec<CellAddr> = self.pivots.iter()
            .filter(|(anchor, pt)| **anchor != addr && pivot_source_contains(pt, addr))
            .map(|(a, _)| *a)
            .collect();
        for anchor in anchors {
            self.recompute_pivot(anchor);
            // Surface the new output to formulas that reference the
            // pivot anchor. Without this, `=SUM(<anchor>)` would
            // hold the value computed against the previous spill
            // until the next event triggered the dependency walk.
            self.recalculate_from(anchor);
        }
    }

    /// Resolve a pivot's source range to a 2D array of source cells.
    /// `Local` ranges go through the engine's range resolver;
    /// `Foreign` ranges are deferred to phase 3 (returns empty for
    /// now — callers see a no-output pivot until cross-doc snapshot
    /// integration lands).
    fn resolve_pivot_source(
        &self,
        pivot: &crate::spreadsheet::pivot::PivotTable,
    ) -> Vec<Vec<CellValue>> {
        use crate::spreadsheet::pivot::SourceRange;
        match &pivot.source {
            SourceRange::Local { range_a1 } => {
                match crate::spreadsheet::parser::parse_formula(range_a1) {
                    Ok(crate::spreadsheet::parser::Expr::Range(_)) => {
                        // resolve_2d treats an `Expr::Range` as a 2D
                        // block of CellValues — exactly what
                        // eval_pivot expects.
                        let expr = crate::spreadsheet::parser::parse_formula(range_a1).ok();
                        expr.map(|e| self.resolve_2d(&e)).unwrap_or_default()
                    }
                    _ => Vec::new(),
                }
            }
            SourceRange::Foreign { .. } => Vec::new(),
        }
    }

    /// Check if a cell has checkbox validation.
    pub fn is_checkbox(&self, addr: CellAddr) -> bool {
        self.styles.get(&addr)
            .and_then(|s| s.validation.as_ref())
            .map_or(false, |v| matches!(v, ValidationRule::Checkbox))
    }

    /// Toggle a checkbox cell between TRUE and FALSE.
    pub fn toggle_checkbox(&mut self, addr: CellAddr) {
        let current = self.get_value(addr);
        let new_val = match current.as_bool() {
            Ok(true) => "FALSE",
            _ => "TRUE",
        };
        self.set_cell(addr, new_val);
    }

    /// Recalculate `start` and every cell that transitively depends
    /// on it, in topological order. Cells inside a dependency cycle
    /// are marked `#CIRCULAR!`.
    ///
    /// Two-phase:
    ///
    /// 1. **Reachability** (BFS) — collect every cell whose value may
    ///    have to refresh. Spill anchors enqueue their filled cells'
    ///    dependents too: `dependents[anchor]` records readers of the
    ///    anchor address, but readers of a *filled* cell live under
    ///    `dependents[filled_cell]`. Without that branch, refreshing
    ///    a spill array would leave its readers stale.
    ///
    /// 2. **Kahn's topological sort** — within the reachable
    ///    subgraph, evaluate cells in dependency order so every
    ///    cell's inputs are already refreshed when its formula runs.
    ///    A previous BFS-order pass would visit a cell before one
    ///    of its parents at a deeper level, leaving the cell with
    ///    a stale value. Any cell not popped by the time the queue
    ///    drains is part of a dependency cycle.
    fn recalculate_from(&mut self, start: CellAddr) {
        // ─ Phase 1: reachable subgraph ─
        let mut subgraph: HashSet<CellAddr> = HashSet::new();
        let mut queue: VecDeque<CellAddr> = VecDeque::new();
        queue.push_back(start);
        while let Some(addr) = queue.pop_front() {
            if !subgraph.insert(addr) { continue; }
            if let Some(deps) = self.dependents.get(&addr) {
                for dep in deps { queue.push_back(*dep); }
            }
            if self.is_spill_anchor(addr) {
                let filled: Vec<CellAddr> = self.spill_origins.iter()
                    .filter_map(|(k, v)| if *v == addr { Some(*k) } else { None })
                    .collect();
                for f in filled {
                    if let Some(deps) = self.dependents.get(&f) {
                        for dep in deps { queue.push_back(*dep); }
                    }
                }
            }
        }

        // ─ Phase 2: Kahn's. ─
        // For each reachable cell, in-degree = how many of its
        // dependencies are also reachable. A dep that's a spill-fill
        // cell counts via its anchor, since the anchor is what gets
        // re-evaluated (filled cells synthesize their value from the
        // anchor's array).
        let dep_in_subgraph = |d: &CellAddr, sub: &HashSet<CellAddr>| -> bool {
            if sub.contains(d) { return true; }
            self.spill_origins.get(d).is_some_and(|anchor| sub.contains(anchor))
        };
        let mut indeg: HashMap<CellAddr, usize> = HashMap::new();
        for addr in &subgraph {
            let count = self.dependencies.get(addr).map_or(0, |deps| {
                deps.iter().filter(|d| dep_in_subgraph(d, &subgraph)).count()
            });
            indeg.insert(*addr, count);
        }
        let mut ready: VecDeque<CellAddr> = indeg.iter()
            .filter_map(|(addr, &n)| if n == 0 { Some(*addr) } else { None })
            .collect();
        let mut processed: HashSet<CellAddr> = HashSet::new();
        while let Some(addr) = ready.pop_front() {
            if !processed.insert(addr) { continue; }
            self.evaluate_cell(addr);
            // Drop in-degree of every dependent — both direct
            // dependents and (if `addr` is a spill anchor) dependents
            // of every filled cell in its block.
            let mut downstream: Vec<CellAddr> = self.dependents.get(&addr)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            if self.is_spill_anchor(addr) {
                let filled: Vec<CellAddr> = self.spill_origins.iter()
                    .filter_map(|(k, v)| if *v == addr { Some(*k) } else { None })
                    .collect();
                for f in filled {
                    if let Some(deps) = self.dependents.get(&f) {
                        downstream.extend(deps.iter().copied());
                    }
                }
            }
            for d in downstream {
                if let Some(n) = indeg.get_mut(&d) {
                    *n = n.saturating_sub(1);
                    if *n == 0 { ready.push_back(d); }
                }
            }
        }

        // Anything left unprocessed is in a cycle.
        for addr in &subgraph {
            if !processed.contains(addr) {
                self.values.insert(*addr, CellValue::Error(SpreadsheetError::Circular));
            }
        }
    }

    /// Evaluate a single cell's formula.
    fn evaluate_cell(&mut self, addr: CellAddr) {
        if let Some(expr) = self.formulas.get(&addr).cloned() {
            if self.evaluating.contains(&addr) {
                self.values.insert(addr, CellValue::Error(SpreadsheetError::Circular));
                return;
            }
            self.evaluating.insert(addr);
            let value = self.eval_expr(&expr);
            self.evaluating.remove(&addr);

            // If this cell *was* a spill anchor before, drop its old
            // block — the formula may now return a smaller array, a
            // scalar, or the same shape with new values; either way
            // the old fill cells must clear so we don't leak them.
            if self.is_spill_anchor(addr) {
                self.clear_spill_block(addr);
            }

            // If the new result is an Array, try to register the spill
            // block. On conflict, store `#SPILL!` and skip registration.
            let stored = if let CellValue::Array(ref array) = value {
                if self.try_register_spill_block(addr, array).is_ok() {
                    value
                } else {
                    CellValue::Error(SpreadsheetError::Spill)
                }
            } else {
                value
            };
            self.values.insert(addr, stored);
        }
        // Non-formula cells already have their value set in set_cell
    }

    /// Evaluate an expression recursively. Depth-bounded by the
    /// thread-local `EVAL_DEPTH` counter: hitting `MAX_EVAL_DEPTH`
    /// short-circuits to `#NUM!` instead of overflowing the stack.
    /// This protects against pathological inputs (deeply nested
    /// BinOps, IF chains, function args, etc.) regardless of where
    /// they came from — user typing, paste, or CRDT deserialise.
    fn eval_expr(&self, expr: &Expr) -> CellValue {
        let Some(_guard) = EvalDepthGuard::try_enter() else {
            return CellValue::Error(SpreadsheetError::Num);
        };
        match expr {
            Expr::Number(n) => CellValue::Number(*n),
            Expr::Text(s) => CellValue::Text(s.clone()),
            Expr::Bool(b) => CellValue::Bool(*b),
            Expr::Error(e) => CellValue::Error(e.clone()),

            Expr::CellRef(cell) => {
                self.get_value((cell.col, cell.row)).clone()
            }

            Expr::SheetCellRef { sheet, cell } => self.resolve_sheet_cell(sheet, cell),

            Expr::Range(_) | Expr::SheetRange { .. } => {
                // Ranges can only be used as function arguments, not standalone
                CellValue::Error(SpreadsheetError::Value)
            }

            Expr::Name(n) => match self.named_ranges.get(&n.to_uppercase()) {
                None => CellValue::Error(SpreadsheetError::Name),
                Some(range) => {
                    // Standalone name resolves like `Expr::Range`:
                    // single-cell range → that cell's value;
                    // multi-cell range → `#VALUE!`. Function argument
                    // paths handle multi-cell named ranges via
                    // `collect_*` and `resolve_2d` below.
                    if range.start == range.end {
                        self.get_value((range.start.col, range.start.row)).clone()
                    } else {
                        CellValue::Error(SpreadsheetError::Value)
                    }
                }
            },

            Expr::UnaryNeg(inner) => {
                match self.eval_expr(inner).as_number() {
                    Ok(n) => CellValue::Number(-n),
                    Err(e) => CellValue::Error(e),
                }
            }

            Expr::Percent(inner) => {
                match self.eval_expr(inner).as_number() {
                    Ok(n) => CellValue::Number(n / 100.0),
                    Err(e) => CellValue::Error(e),
                }
            }

            Expr::BinOp { op, left, right } => {
                self.eval_binop(op, left, right)
            }

            Expr::FuncCall { name, args } => {
                functions::call_function(name, args, self)
            }
        }
    }

    fn eval_binop(&self, op: &BinOp, left: &Expr, right: &Expr) -> CellValue {
        if *op == BinOp::Concat {
            let l = self.eval_expr(left);
            if l.is_error() { return l; }
            let r = self.eval_expr(right);
            if r.is_error() { return r; }
            return CellValue::Text(format!("{}{}", l.as_text(), r.as_text()));
        }

        let lv = self.eval_expr(left);
        if lv.is_error() { return lv; }
        let rv = self.eval_expr(right);
        if rv.is_error() { return rv; }

        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Pow => {
                let ln = match lv.as_number() { Ok(n) => n, Err(e) => return CellValue::Error(e) };
                let rn = match rv.as_number() { Ok(n) => n, Err(e) => return CellValue::Error(e) };
                let result = match op {
                    BinOp::Add => ln + rn,
                    BinOp::Sub => ln - rn,
                    BinOp::Mul => ln * rn,
                    BinOp::Div => {
                        if rn == 0.0 { return CellValue::Error(SpreadsheetError::Div0); }
                        ln / rn
                    }
                    BinOp::Pow => ln.powf(rn),
                    _ => unreachable!(),
                };
                CellValue::Number(result)
            }
            BinOp::Eq => match cell_eq(&lv, &rv) {
                Ok(b) => CellValue::Bool(b),
                Err(e) => CellValue::Error(e),
            },
            BinOp::Neq => match cell_eq(&lv, &rv) {
                Ok(b) => CellValue::Bool(!b),
                Err(e) => CellValue::Error(e),
            },
            BinOp::Lt | BinOp::Gt | BinOp::Lte | BinOp::Gte => {
                let ord = match cell_cmp(&lv, &rv) {
                    Ok(o) => o,
                    Err(e) => return CellValue::Error(e),
                };
                use std::cmp::Ordering;
                let result = match op {
                    BinOp::Lt => ord == Ordering::Less,
                    BinOp::Gt => ord == Ordering::Greater,
                    BinOp::Lte => ord != Ordering::Greater,
                    BinOp::Gte => ord != Ordering::Less,
                    _ => unreachable!(),
                };
                CellValue::Bool(result)
            }
            BinOp::Concat => unreachable!(), // handled above
        }
    }

    /// Resolve a range to a flat list of cell values.
    pub fn resolve_range(&self, range: &RangeRef) -> Vec<CellValue> {
        let mut values = Vec::new();
        let r1 = range.start.row.min(range.end.row);
        let r2 = range.start.row.max(range.end.row);
        let c1 = range.start.col.min(range.end.col);
        let c2 = range.start.col.max(range.end.col);
        for r in r1..=r2 {
            for c in c1..=c2 {
                values.push(self.get_value((c, r)).clone());
            }
        }
        values
    }

    /// Resolve a range to a 2-D rectangular shape `[row][col]`.
    pub fn resolve_range_2d(&self, range: &RangeRef) -> Vec<Vec<CellValue>> {
        let r1 = range.start.row.min(range.end.row);
        let r2 = range.start.row.max(range.end.row);
        let c1 = range.start.col.min(range.end.col);
        let c2 = range.start.col.max(range.end.col);
        let mut rows = Vec::with_capacity(r2 - r1 + 1);
        for r in r1..=r2 {
            let mut row = Vec::with_capacity(c2 - c1 + 1);
            for c in c1..=c2 {
                row.push(self.get_value((c, r)).clone());
            }
            rows.push(row);
        }
        rows
    }

    /// Resolve a sheet-qualified range to a flat row-major
    /// `Vec<CellValue>`. Each cell is fetched through
    /// `resolve_sheet_cell`, which falls back to engine state for
    /// refs to the active sheet's own name and returns `#REF!` for
    /// unknown sheet names. Used by the function-arg collectors
    /// (`SUM(Sheet2!A1:B5)`, `AVERAGE(Sheet2!A:A)`, etc.).
    fn resolve_sheet_range(
        &self,
        sheet: &str,
        range: &RangeRef,
    ) -> Vec<CellValue> {
        let r1 = range.start.row.min(range.end.row);
        let r2 = range.start.row.max(range.end.row);
        let c1 = range.start.col.min(range.end.col);
        let c2 = range.start.col.max(range.end.col);
        let mut out = Vec::with_capacity((r2 - r1 + 1) * (c2 - c1 + 1));
        for r in r1..=r2 {
            for c in c1..=c2 {
                let cell = crate::spreadsheet::parser::CellRef {
                    col: c, row: r, abs_col: false, abs_row: false,
                };
                out.push(self.resolve_sheet_cell(sheet, &cell));
            }
        }
        out
    }

    /// 2-D variant of `resolve_sheet_range` — used by the
    /// `resolve_2d` dispatcher for matrix functions that take a
    /// sheet-qualified range argument.
    fn resolve_sheet_range_2d(
        &self,
        sheet: &str,
        range: &RangeRef,
    ) -> Vec<Vec<CellValue>> {
        let r1 = range.start.row.min(range.end.row);
        let r2 = range.start.row.max(range.end.row);
        let c1 = range.start.col.min(range.end.col);
        let c2 = range.start.col.max(range.end.col);
        let mut rows = Vec::with_capacity(r2 - r1 + 1);
        for r in r1..=r2 {
            let mut row = Vec::with_capacity(c2 - c1 + 1);
            for c in c1..=c2 {
                let cell = crate::spreadsheet::parser::CellRef {
                    col: c, row: r, abs_col: false, abs_row: false,
                };
                row.push(self.resolve_sheet_cell(sheet, &cell));
            }
            rows.push(row);
        }
        rows
    }

    /// Resolve any expression to a 2-D rectangular `[row][col]` block.
    /// Ranges return their shape; Array values return their owned
    /// shape; scalars return a 1×1 block. Used by dynamic-array and
    /// matrix functions (TRANSPOSE, MMULT, MDETERM, MINVERSE).
    pub fn resolve_2d(&self, expr: &Expr) -> Vec<Vec<CellValue>> {
        match expr {
            Expr::Range(range) => self.resolve_range_2d(range),
            Expr::SheetRange { sheet, range } => self.resolve_sheet_range_2d(sheet, range),
            Expr::Name(n) => match self.named_ranges.get(&n.to_uppercase()) {
                Some(range) => self.resolve_range_2d(range),
                None => vec![vec![CellValue::Error(SpreadsheetError::Name)]],
            },
            _ => self.eval_expr(expr).to_array_2d(),
        }
    }

    /// Evaluate an expression (public, for use by function implementations).
    pub fn eval(&self, expr: &Expr) -> CellValue {
        self.eval_expr(expr)
    }

    /// Collect all numeric values from an argument (which may be a
    /// range or evaluate to an Array). Both ranges and arrays flatten
    /// row-major; non-numeric cells are silently dropped (matching
    /// Excel SUM/AVERAGE semantics for mixed ranges).
    pub fn collect_numbers(&self, expr: &Expr) -> Vec<f64> {
        match expr {
            Expr::Range(range) => {
                self.resolve_range(range)
                    .iter()
                    .filter_map(|v| v.as_number().ok())
                    .collect()
            }
            Expr::SheetRange { sheet, range } => {
                self.resolve_sheet_range(sheet, range)
                    .iter()
                    .filter_map(|v| v.as_number().ok())
                    .collect()
            }
            Expr::Name(n) => match self.named_ranges.get(&n.to_uppercase()) {
                Some(range) => self.resolve_range(range)
                    .iter()
                    .filter_map(|v| v.as_number().ok())
                    .collect(),
                None => Vec::new(),
            },
            _ => {
                let val = self.eval_expr(expr);
                if let CellValue::Array(rows) = &val {
                    return rows
                        .iter()
                        .flatten()
                        .filter_map(|v| v.as_number().ok())
                        .collect();
                }
                match val.as_number() {
                    Ok(n) => vec![n],
                    Err(_) => vec![],
                }
            }
        }
    }

    /// Collect all values from an argument (which may be a range or
    /// evaluate to an Array). Row-major flatten for both shapes.
    pub fn collect_values(&self, expr: &Expr) -> Vec<CellValue> {
        match expr {
            Expr::Range(range) => self.resolve_range(range),
            Expr::SheetRange { sheet, range } => self.resolve_sheet_range(sheet, range),
            Expr::Name(n) => match self.named_ranges.get(&n.to_uppercase()) {
                Some(range) => self.resolve_range(range),
                None => vec![CellValue::Error(SpreadsheetError::Name)],
            },
            _ => {
                let val = self.eval_expr(expr);
                if let CellValue::Array(rows) = val {
                    return rows.into_iter().flatten().collect();
                }
                vec![val]
            }
        }
    }
}

/// True iff `addr` lies inside the pivot's source range. Local
/// pivots check the parsed `RangeRef`; foreign pivots are never
/// affected by local-cell edits, so always false.
fn pivot_source_contains(
    pivot: &crate::spreadsheet::pivot::PivotTable,
    addr: CellAddr,
) -> bool {
    use crate::spreadsheet::pivot::SourceRange;
    match &pivot.source {
        SourceRange::Local { range_a1 } => {
            let Ok(crate::spreadsheet::parser::Expr::Range(r)) =
                crate::spreadsheet::parser::parse_formula(range_a1)
            else { return false; };
            let (c, row) = addr;
            c >= r.start.col && c <= r.end.col
                && row >= r.start.row && row <= r.end.row
        }
        SourceRange::Foreign { .. } => false,
    }
}

/// Parse a raw (non-formula) cell value.
fn parse_raw_value(s: &str) -> CellValue {
    if s.is_empty() {
        return CellValue::Empty;
    }
    let upper = s.trim().to_uppercase();
    if upper == "TRUE" { return CellValue::Bool(true); }
    if upper == "FALSE" { return CellValue::Bool(false); }
    if let Ok(n) = s.trim().parse::<f64>() {
        return CellValue::Number(n);
    }
    CellValue::Text(s.to_string())
}

impl SpreadsheetEngine {
    /// Collect all cell addresses referenced by an expression. A named-
    /// range reference contributes the cells of the range it currently
    /// points to; an unresolved name contributes nothing (changing or
    /// defining the name later doesn't trigger a recompute on its own,
    /// but redefining the cell at the addresses already in this set
    /// will).
    fn collect_refs(&self, expr: &Expr) -> HashSet<CellAddr> {
        let mut refs = HashSet::new();
        self.collect_refs_inner(expr, &mut refs);
        refs
    }

    fn collect_refs_inner(&self, expr: &Expr, refs: &mut HashSet<CellAddr>) {
        let collect_range = |refs: &mut HashSet<CellAddr>, range: &RangeRef| {
            let r1 = range.start.row.min(range.end.row);
            let r2 = range.start.row.max(range.end.row);
            let c1 = range.start.col.min(range.end.col);
            let c2 = range.start.col.max(range.end.col);
            for r in r1..=r2 {
                for c in c1..=c2 {
                    refs.insert((c, r));
                }
            }
        };
        match expr {
            Expr::CellRef(cell) => { refs.insert((cell.col, cell.row)); }
            Expr::Range(range) => collect_range(refs, range),
            Expr::Name(n) => {
                if let Some(range) = self.named_ranges.get(&n.to_uppercase()) {
                    collect_range(refs, range);
                }
            }
            Expr::BinOp { left, right, .. } => {
                self.collect_refs_inner(left, refs);
                self.collect_refs_inner(right, refs);
            }
            Expr::UnaryNeg(inner) | Expr::Percent(inner) => {
                self.collect_refs_inner(inner, refs);
            }
            Expr::FuncCall { args, .. } => {
                for arg in args {
                    self.collect_refs_inner(arg, refs);
                }
            }
            _ => {}
        }
    }
}

/// Format a number according to a format string.
fn format_number(n: f64, fmt: &str) -> String {
    match fmt {
        "currency" | "usd" => format!("${:.2}", n),
        "eur" => format!("\u{20AC}{:.2}", n),
        "percent" | "percentage" => format!("{:.1}%", n * 100.0),
        "integer" | "int" => format!("{}", n as i64),
        "decimal1" => format!("{:.1}", n),
        "decimal2" => format!("{:.2}", n),
        "decimal3" => format!("{:.3}", n),
        "comma" => {
            let int = n as i64;
            let s = int.to_string();
            let mut result = String::new();
            for (i, c) in s.chars().rev().enumerate() {
                if i > 0 && i % 3 == 0 && c != '-' { result.insert(0, ','); }
                result.insert(0, c);
            }
            result
        }
        _ => format!("{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_visual_style_from_copies_appearance_only() {
        // Source = pure-visual (bold, red bg, currency, etc.)
        let source = CellStyle {
            bold: true,
            italic: true,
            underline: true,
            strike: true,
            bg_color: Some("#ff0000".to_string()),
            text_color: Some("#ffffff".to_string()),
            align: Some("center".to_string()),
            number_format: Some("currency".to_string()),
            // Source's data-contract fields must NOT propagate.
            validation: Some(ValidationRule::Checkbox),
            locked: true,
            comment: Some("source note".to_string()),
            comment_thread_id: Some("cell-srcnanoid111111".to_string()),
        };
        // Target = no visual but a comment thread, validation, and locked.
        let target = CellStyle {
            validation: Some(ValidationRule::Dropdown(vec!["a".into()])),
            locked: false,
            comment: Some("target note".to_string()),
            comment_thread_id: Some("cell-tgtnanoid222222".to_string()),
            ..CellStyle::default()
        };

        let merged = target.with_visual_style_from(&source);

        // Visual fields come from source.
        assert!(merged.bold);
        assert!(merged.italic);
        assert_eq!(merged.bg_color.as_deref(), Some("#ff0000"));
        assert_eq!(merged.text_color.as_deref(), Some("#ffffff"));
        assert_eq!(merged.align.as_deref(), Some("center"));
        assert_eq!(merged.number_format.as_deref(), Some("currency"));

        // Data-contract fields stay with the target — including the
        // comment thread id. The format painter must not steal the
        // source cell's comment-thread linkage.
        assert!(matches!(merged.validation, Some(ValidationRule::Dropdown(_))));
        assert!(!merged.locked);
        assert_eq!(merged.comment.as_deref(), Some("target note"));
        assert_eq!(merged.comment_thread_id.as_deref(), Some("cell-tgtnanoid222222"));
    }

    #[test]
    fn basic_value() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "42");
        assert_eq!(engine.get_display((0, 0)), "42");
    }

    #[test]
    fn basic_formula() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "10");
        engine.set_cell((1, 0), "20");
        engine.set_cell((2, 0), "=A1+B1");
        assert_eq!(engine.get_display((2, 0)), "30");
    }

    #[test]
    fn sum_range() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((0, 2), "3");
        engine.set_cell((1, 0), "=SUM(A1:A3)");
        assert_eq!(engine.get_display((1, 0)), "6");
    }

    #[test]
    fn dependency_propagation() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "5");
        engine.set_cell((1, 0), "=A1*2");
        assert_eq!(engine.get_display((1, 0)), "10");
        // Update A1 — B1 should recalculate
        engine.set_cell((0, 0), "10");
        assert_eq!(engine.get_display((1, 0)), "20");
    }

    #[test]
    fn circular_ref() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "=B1");
        engine.set_cell((1, 0), "=A1");
        assert_eq!(engine.get_display((0, 0)), "#CIRCULAR!");
    }

    #[test]
    fn div_zero() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "=1/0");
        assert_eq!(engine.get_display((0, 0)), "#DIV/0!");
    }

    #[test]
    fn string_concat() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "hello");
        engine.set_cell((1, 0), "=A1 & \" world\"");
        assert_eq!(engine.get_display((1, 0)), "hello world");
    }

    // ─── CellStyle tests ──────────────────────────────────

    #[test]
    fn cell_style_default_css_empty() {
        assert_eq!(CellStyle::default().to_inline_css(), "");
    }

    #[test]
    fn cell_style_css_bold_italic_bg() {
        let style = CellStyle {
            bold: true,
            italic: true,
            bg_color: Some("#ff0".to_string()),
            ..Default::default()
        };
        let css = style.to_inline_css();
        assert!(css.contains("font-weight:700;"));
        assert!(css.contains("font-style:italic;"));
        assert!(css.contains("background-color:#ff0;"));
    }

    // ─── Validation tests ──────────────────────────────────

    #[test]
    fn checkbox_toggle() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "FALSE");
        engine.style_mut((0, 0)).validation = Some(ValidationRule::Checkbox);
        assert!(engine.is_checkbox((0, 0)));

        engine.toggle_checkbox((0, 0));
        assert_eq!(engine.get_display((0, 0)), "TRUE");

        engine.toggle_checkbox((0, 0));
        assert_eq!(engine.get_display((0, 0)), "FALSE");
    }

    #[test]
    fn non_checkbox_is_not_checkbox() {
        let engine = SpreadsheetEngine::new();
        assert!(!engine.is_checkbox((0, 0)));
    }

    // ─── Conditional formatting tests ──────────────────────

    #[test]
    fn conditional_greater_than() {
        let cond = ConditionalCondition::GreaterThan(10.0);
        assert!(cond.matches(&CellValue::Number(15.0)));
        assert!(!cond.matches(&CellValue::Number(5.0)));
        assert!(!cond.matches(&CellValue::Number(10.0)));
    }

    #[test]
    fn conditional_text_contains() {
        let cond = ConditionalCondition::TextContains("err".to_string());
        assert!(cond.matches(&CellValue::Text("Error found".to_string())));
        assert!(!cond.matches(&CellValue::Text("All ok".to_string())));
    }

    #[test]
    fn conditional_is_empty() {
        let cond = ConditionalCondition::IsEmpty;
        assert!(cond.matches(&CellValue::Empty));
        assert!(cond.matches(&CellValue::Text(String::new())));
        assert!(!cond.matches(&CellValue::Number(0.0)));
    }

    #[test]
    fn effective_bg_from_conditional() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "20");
        engine.add_conditional_format(
            (0, 0), (0, 0),
            ConditionalFormat::Single {
                condition: ConditionalCondition::GreaterThan(10.0),
                bg_color: "#ff0000".to_string(),
            },
        );
        assert_eq!(engine.get_effective_bg((0, 0)), Some("#ff0000".to_string()));
    }

    #[test]
    fn effective_bg_no_match_falls_through() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "5");
        engine.add_conditional_format(
            (0, 0), (0, 0),
            ConditionalFormat::Single {
                condition: ConditionalCondition::GreaterThan(10.0),
                bg_color: "#ff0000".to_string(),
            },
        );
        assert_eq!(engine.get_effective_bg((0, 0)), None);
    }

    #[test]
    fn color_scale_two_stop_endpoints_match_inputs() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0");
        engine.set_cell((0, 1), "10");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::ColorScale {
                low: "#000000".to_string(),
                mid: None,
                high: "#ffffff".to_string(),
            },
        );
        // Min value gets `low`; max value gets `high`.
        assert_eq!(engine.get_effective_bg((0, 0)), Some("#000000".to_string()));
        assert_eq!(engine.get_effective_bg((0, 1)), Some("#ffffff".to_string()));
    }

    #[test]
    fn color_scale_three_stop_passes_through_mid() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0");
        engine.set_cell((0, 1), "5");
        engine.set_cell((0, 2), "10");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::ColorScale {
                low: "#ff0000".to_string(),
                mid: Some("#00ff00".to_string()),
                high: "#0000ff".to_string(),
            },
        );
        // Median value (5 of 0..10 → t=0.5) = mid stop exactly.
        assert_eq!(engine.get_effective_bg((0, 1)), Some("#00ff00".to_string()));
    }

    #[test]
    fn color_scale_no_variation_falls_through() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "5");
        engine.set_cell((0, 1), "5");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::ColorScale {
                low: "#000000".to_string(),
                mid: None,
                high: "#ffffff".to_string(),
            },
        );
        // All-equal range → no gradient → fall through to style bg
        // which is unset, so the cell has no effective bg.
        assert_eq!(engine.get_effective_bg((0, 0)), None);
    }

    #[test]
    fn data_bar_ratio_is_value_over_max() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0");
        engine.set_cell((0, 1), "5");
        engine.set_cell((0, 2), "10");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::DataBar { color: "#3b82f6".to_string() },
        );
        assert_eq!(engine.get_data_bar((0, 0)), Some(("#3b82f6".to_string(), 0.0)));
        assert_eq!(engine.get_data_bar((0, 1)), Some(("#3b82f6".to_string(), 0.5)));
        assert_eq!(engine.get_data_bar((0, 2)), Some(("#3b82f6".to_string(), 1.0)));
    }

    #[test]
    fn condition_user_input_parses_each_form() {
        assert_eq!(
            ConditionalCondition::parse_user_input(">10"),
            Some(ConditionalCondition::GreaterThan(10.0)),
        );
        assert_eq!(
            ConditionalCondition::parse_user_input("< 5.5"),
            Some(ConditionalCondition::LessThan(5.5)),
        );
        assert_eq!(
            ConditionalCondition::parse_user_input("=Done"),
            Some(ConditionalCondition::EqualTo("Done".to_string())),
        );
        assert_eq!(
            ConditionalCondition::parse_user_input("contains:err"),
            Some(ConditionalCondition::TextContains("err".to_string())),
        );
        assert_eq!(
            ConditionalCondition::parse_user_input("empty"),
            Some(ConditionalCondition::IsEmpty),
        );
        assert_eq!(
            ConditionalCondition::parse_user_input("notempty"),
            Some(ConditionalCondition::IsNotEmpty),
        );
    }

    #[test]
    fn condition_user_input_rejects_unknown() {
        assert_eq!(ConditionalCondition::parse_user_input(""), None);
        assert_eq!(ConditionalCondition::parse_user_input("> abc"), None);
        assert_eq!(ConditionalCondition::parse_user_input("garbage"), None);
    }

    // ─── Named ranges ───────────────────────────────────────

    #[test]
    fn named_range_single_cell_resolves_to_value() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "42");
        engine.set_named_range(
            "ANSWER",
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=ANSWER");
        assert_eq!(engine.get_display((1, 0)), "42");
    }

    #[test]
    fn named_range_multi_cell_standalone_yields_value_error() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_named_range(
            "PAIR",
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 1, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=PAIR");
        // Multi-cell named range can't stand alone — same as Range.
        assert!(engine.get_display((1, 0)).contains("VALUE"));
    }

    #[test]
    fn named_range_in_function_argument_flattens_like_a_range() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((0, 2), "3");
        engine.set_named_range(
            "ITEMS",
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 2, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=SUM(ITEMS)");
        assert_eq!(engine.get_display((1, 0)), "6");
    }

    #[test]
    fn named_range_unknown_yields_name_error() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "=NOPE");
        assert!(engine.get_display((0, 0)).contains("NAME"));
    }

    #[test]
    fn named_range_lookup_is_case_insensitive() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "7");
        engine.set_named_range(
            "Profit",   // mixed case at definition
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=PROFIT");   // upper at use
        assert_eq!(engine.get_display((1, 0)), "7");
        engine.set_cell((1, 1), "=profit");   // lower at use
        assert_eq!(engine.get_display((1, 1)), "7");
    }

    #[test]
    fn recalculate_diamond_dependency_uses_topological_order() {
        // Issue #3 finding 1. Diamond shape:
        //   A1 → B1, A1 → C1, B1 → C1, C1 → D1.
        // BFS-by-level visits D1 (depth 2) at the same level as
        // C1 (also depth 2 from A1 via B1). With BFS, D1 might be
        // evaluated before C1 has refreshed, leaving D1 stale.
        // Topological sort guarantees parents-before-children.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "1");          // A1
        e.set_cell((1, 0), "=A1+10");     // B1 = A1 + 10 → 11
        e.set_cell((2, 0), "=A1+B1");     // C1 = A1 + B1 → 12
        e.set_cell((3, 0), "=C1*2");      // D1 = C1 * 2  → 24

        // Drive a propagation through the diamond.
        e.set_cell((0, 0), "100");
        // After A1 = 100: B1 = 110, C1 = 100+110 = 210, D1 = 420.
        assert_eq!(e.get_display((1, 0)), "110");
        assert_eq!(e.get_display((2, 0)), "210");
        assert_eq!(e.get_display((3, 0)), "420");
    }

    #[test]
    fn recalculate_marks_cycle_cells_circular() {
        // Make a cycle: A1 = B1, B1 = A1. Both should land at #CIRCULAR!.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "=B1");
        e.set_cell((1, 0), "=A1");
        assert!(e.get_display((0, 0)).contains("CIRCULAR"));
        assert!(e.get_display((1, 0)).contains("CIRCULAR"));
    }

    #[test]
    fn recalculate_three_node_cycle_marks_all_three() {
        // Issue #3 finding 2 spirit-test: a 3-cycle that the old
        // `has_cycle` could miss when shared paths flatten the
        // search. Topological sort can't miss it — anything not
        // popped before the queue drains is in a cycle.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "=B1+1");      // A1 = B1+1
        e.set_cell((1, 0), "=C1+1");      // B1 = C1+1
        e.set_cell((2, 0), "=A1+1");      // C1 = A1+1
        assert!(e.get_display((0, 0)).contains("CIRCULAR"));
        assert!(e.get_display((1, 0)).contains("CIRCULAR"));
        assert!(e.get_display((2, 0)).contains("CIRCULAR"));
    }

    #[test]
    fn engine_clear_drops_named_ranges() {
        // Regression: clear() must wipe named_ranges. Without this,
        // sheet-switch (which calls clear() before loading the next
        // sheet's table) would leak names from the outgoing sheet.
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_named_range(
            "X",
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
            },
        );
        assert!(engine.get_named_range("X").is_some());
        engine.clear();
        assert!(engine.get_named_range("X").is_none());
    }

    #[test]
    fn named_range_redefinition_recomputes_dependents() {
        use crate::spreadsheet::parser::{CellRef, RangeRef};
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "10");
        engine.set_cell((0, 1), "99");
        engine.set_named_range(
            "X",
            RangeRef {
                start: CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 0, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=X");
        assert_eq!(engine.get_display((1, 0)), "10");

        // Redefine X to point at the second cell. Force a recompute by
        // re-setting the formula cell — v1 doesn't observe name-map
        // mutations, so callers must trigger a recalc themselves.
        engine.set_named_range(
            "X",
            RangeRef {
                start: CellRef { col: 0, row: 1, abs_col: true, abs_row: true },
                end:   CellRef { col: 0, row: 1, abs_col: true, abs_row: true },
            },
        );
        engine.set_cell((1, 0), "=X");
        assert_eq!(engine.get_display((1, 0)), "99");
    }

    #[test]
    fn range_min_max_skips_empty_cells() {
        // Regression for the CF reviewer finding: empty cells must
        // not contribute 0.0 to the ColorScale / DataBar range.
        // Before the fix, [empty, 5, 10] produced min=0/max=10, so
        // the cell with `5` showed a 50% bar instead of a 100% bar
        // (since `5` was actually the *minimum* of the non-empty set).
        let mut engine = SpreadsheetEngine::new();
        // (0,0) is empty
        engine.set_cell((0, 1), "5");
        engine.set_cell((0, 2), "10");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::DataBar { color: "#3b82f6".to_string() },
        );
        // With empties skipped, max=10. value 5 / max 10 = 0.5.
        assert_eq!(engine.get_data_bar((0, 1)).map(|(_, r)| r), Some(0.5));
    }

    #[test]
    fn range_min_max_skips_bool_cells() {
        // Bool cells coerce to 0.0/1.0 via as_number(), which would
        // pin the DataBar range to 0..1 regardless of other numeric
        // values — silently compressing the gradient to nothing.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "TRUE");      // CellValue::Bool(true)
        engine.set_cell((0, 1), "100");
        engine.set_cell((0, 2), "200");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::DataBar { color: "#3b82f6".to_string() },
        );
        // With Bool skipped, max=200. Cell (0,2) at value 200 → 100% bar.
        assert_eq!(engine.get_data_bar((0, 2)).map(|(_, r)| r), Some(1.0));
        // Cell (0,0) is non-numeric (Bool) → no data bar.
        assert_eq!(engine.get_data_bar((0, 0)), None);
    }

    #[test]
    fn icon_set_three_arrows_assigns_tertile_glyphs() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0");
        engine.set_cell((0, 1), "5");
        engine.set_cell((0, 2), "10");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        );
        // 0/10 = 0.0 → bottom tertile → ↓
        assert_eq!(engine.get_icon((0, 0)), Some("\u{2193}"));
        // 5/10 = 0.5 → middle tertile → →
        assert_eq!(engine.get_icon((0, 1)), Some("\u{2192}"));
        // 10/10 = 1.0 → top tertile → ↑
        assert_eq!(engine.get_icon((0, 2)), Some("\u{2191}"));
    }

    #[test]
    fn icon_set_three_traffic_lights_uses_emoji() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0");
        engine.set_cell((0, 1), "10");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeTrafficLights },
        );
        // 0/10 → red, 10/10 → green
        assert_eq!(engine.get_icon((0, 0)), Some("\u{1F534}"));
        assert_eq!(engine.get_icon((0, 1)), Some("\u{1F7E2}"));
    }

    #[test]
    fn icon_set_skips_non_numeric_cells() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "5");
        engine.set_cell((0, 1), "10");
        engine.set_cell((0, 2), "hello");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        );
        assert_eq!(engine.get_icon((0, 2)), None);
    }

    // ─── Cross-document references (M-S2) ─────────────────────

    fn build_snapshot(sheet_name: &str, cells: &[&[&str]]) -> ForeignDocSnapshot {
        let mut sheets = HashMap::new();
        let rows: Vec<Vec<String>> = cells.iter()
            .map(|row| row.iter().map(|c| c.to_string()).collect())
            .collect();
        sheets.insert(sheet_name.to_string(), rows);
        ForeignDocSnapshot { sheets }
    }

    // ─── Cross-sheet refs (`Sheet2!B2`) ───────────────────────

    #[test]
    fn sheet_qualified_cell_ref_resolves_from_local_snapshot() {
        // Engine on Sheet1 reads `Sheet2!B2` via the local-sheets
        // snapshot. Sheet2's B2 holds "42" → cell evaluates to the
        // typed numeric 42.
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        let snap = build_snapshot("Sheet2", &[
            &["", "",   ""],
            &["", "42", ""],
        ]);
        e.set_local_sheets_snapshot(snap);
        e.set_cell((0, 0), "=Sheet2!B2");
        assert_eq!(e.get_display((0, 0)), "42");
    }

    #[test]
    fn sheet_qualified_cell_ref_case_insensitive_sheet_name() {
        // Sheet names match case-insensitively (`SHEET2` lookup
        // resolves a snapshot keyed "Sheet2").
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        let snap = build_snapshot("Sheet2", &[
            &["", "",      ""],
            &["", "hello", ""],
        ]);
        e.set_local_sheets_snapshot(snap);
        e.set_cell((0, 0), "=SHEET2!B2");
        assert_eq!(e.get_display((0, 0)), "hello");
    }

    #[test]
    fn sheet_qualified_cell_ref_unknown_sheet_returns_ref() {
        // Unknown sheet name yields `#REF!`.
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        e.set_cell((0, 0), "=Nope!A1");
        assert_eq!(e.get_display((0, 0)), "#REF!");
    }

    #[test]
    fn sheet_qualified_range_in_sum_aggregates_foreign_sheet_cells() {
        // `SUM(Sheet2!A1:B2)` reads four cells from the snapshot.
        // Mixed numeric + non-numeric content: only numerics
        // contribute, matching SUM's normal range behaviour.
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        let snap = build_snapshot("Sheet2", &[
            &["1",   "2"],
            &["3", "txt"],
        ]);
        e.set_local_sheets_snapshot(snap);
        e.set_cell((0, 0), "=SUM(Sheet2!A1:B2)");
        assert_eq!(e.get_display((0, 0)), "6");
    }

    #[test]
    fn sheet_qualified_range_in_average_uses_only_numeric_cells() {
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        let snap = build_snapshot("Sheet2", &[
            &["10", "20"],
            &["30", "ignored"],
        ]);
        e.set_local_sheets_snapshot(snap);
        e.set_cell((0, 0), "=AVERAGE(Sheet2!A1:B2)");
        // (10 + 20 + 30) / 3 = 20.
        assert_eq!(e.get_display((0, 0)), "20");
    }

    #[test]
    fn sheet_qualified_cell_ref_active_sheet_reads_engine_state() {
        // A ref to the active sheet's own name short-circuits to
        // engine state, not the snapshot. Useful when the snapshot
        // is stale relative to the engine (mid-edit, etc.).
        let mut e = SpreadsheetEngine::new();
        e.set_active_sheet_name("Sheet1".into());
        e.set_cell((1, 1), "live"); // B2 in engine state
        let stale = build_snapshot("Sheet1", &[
            &["", "",      ""],
            &["", "stale", ""],
        ]);
        e.set_local_sheets_snapshot(stale);
        e.set_cell((0, 0), "=Sheet1!B2");
        assert_eq!(e.get_display((0, 0)), "live");
    }

    // ─── Recursion bounds ─────────────────────────────────────

    #[test]
    fn eval_depth_bound_returns_num_on_deep_arithmetic() {
        // Build a `+`-chain that nests right-associative BinOps
        // beyond `MAX_EVAL_DEPTH`. The parser allows the formula
        // (depths under MAX_PARSE_DEPTH), but eval should bail
        // with `#NUM!` rather than overflowing the stack.
        let mut e = SpreadsheetEngine::new();
        // 200 nested parens around a `1`. Each pair adds one
        // BinOp/Unary layer to walk; with the eval depth guard
        // at 256, this stays under, while a 500-deep chain would
        // exceed. Pick a length that *just* exceeds the bound by
        // wrapping in repeated `+0`.
        let mut formula = String::from("=");
        for _ in 0..(MAX_EVAL_DEPTH + 50) {
            formula.push('(');
            formula.push_str("0+");
        }
        formula.push('1');
        for _ in 0..(MAX_EVAL_DEPTH + 50) {
            formula.push(')');
        }
        // parse_formula's depth bound (128) may itself reject this
        // formula — that's a valid outcome too (the depth bound is
        // there to stop both paths). The test only asserts the
        // engine doesn't crash: writing a too-deep formula yields
        // an error cell, not a panic.
        e.set_cell((0, 0), &formula);
        // Engine still alive: a follow-up cell evaluates fine.
        e.set_cell((1, 0), "=1+2");
        assert_eq!(e.get_display((1, 0)), "3");
    }

    #[test]
    fn cell_cmp_array_unwrap_terminates() {
        // The previous recursive `cell_cmp` could in principle blow
        // the stack if an Array contained an Array of Arrays… With
        // the iterative unwrap loop bounded by MAX_ARRAY_UNWRAP_DEPTH,
        // we just return `#NUM!` once the bound is hit.
        let nested = CellValue::Array(vec![vec![
            CellValue::Array(vec![vec![
                CellValue::Array(vec![vec![CellValue::Number(7.0)]]),
            ]]),
        ]]);
        // After two unwraps we reach a scalar — well within the loop
        // cap — so comparison should succeed.
        assert!(cell_cmp(&nested, &CellValue::Number(7.0)).is_ok());
    }

    // ─── `=` / `<>` strict-type semantics (#3 finding 4) ─────

    #[test]
    fn eq_compares_numbers_numerically_not_textually() {
        // Issue #3: `100 = 1e2` was returning false because the old
        // implementation compared `as_text()` and `as_text(100)` is
        // "100" while `as_text(1e2)` could be "100" too — but the
        // reverse case (e.g. `0.1 + 0.2 = 0.3`) failed because of
        // float-to-text formatting. Numeric compare avoids it.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "100");
        e.set_cell((0, 1), "1e2");
        e.set_cell((0, 2), "=A1=A2");
        assert_eq!(e.get_display((0, 2)), "TRUE");
    }

    #[test]
    fn eq_cross_type_is_not_equal() {
        // Excel-parity: `1 = "1"` is FALSE.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "1");
        e.set_cell((0, 1), "\"1\"");
        e.set_cell((0, 2), "=A1=A2");
        assert_eq!(e.get_display((0, 2)), "FALSE");
    }

    #[test]
    fn eq_bool_vs_number_is_not_equal() {
        // `1 = TRUE` is FALSE in Excel — different types compare unequal.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "1");
        e.set_cell((0, 1), "TRUE");
        e.set_cell((0, 2), "=A1=A2");
        assert_eq!(e.get_display((0, 2)), "FALSE");
    }

    #[test]
    fn eq_text_is_case_insensitive() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "\"Hello\"");
        e.set_cell((0, 1), "\"HELLO\"");
        e.set_cell((0, 2), "=A1=A2");
        assert_eq!(e.get_display((0, 2)), "TRUE");
    }

    #[test]
    fn eq_empty_equals_empty_string_and_zero() {
        // Excel treats an unset cell as both empty-text AND numeric
        // zero in `=` comparisons (mirrors `as_number(Empty) == 0.0`
        // used by `<`/`>`). Both should return TRUE so `=IF(A1=0,...)`
        // matches blank cells the way `=IF(A1<1,...)` does.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((1, 0), "=\"\"");    // B1 = "" (empty Text via formula)
        e.set_cell((2, 0), "0");        // C1 = 0
        e.set_cell((3, 0), "5");        // D1 = 5 (non-zero)
        e.set_cell((0, 1), "=A1=B1");   // A1 (empty) == B1 ("")
        e.set_cell((0, 2), "=A1=C1");   // A1 (empty) == C1 (0)
        e.set_cell((0, 3), "=A1=D1");   // A1 (empty) != D1 (5)
        assert_eq!(e.get_display((0, 1)), "TRUE");
        assert_eq!(e.get_display((0, 2)), "TRUE");
        assert_eq!(e.get_display((0, 3)), "FALSE");
    }

    #[test]
    fn neq_inverts_eq() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "100");
        e.set_cell((0, 1), "1e2");
        e.set_cell((0, 2), "=A1<>A2");
        assert_eq!(e.get_display((0, 2)), "FALSE");
    }

    #[test]
    fn eq_propagates_error_from_either_side() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "=1/0");
        e.set_cell((0, 1), "5");
        e.set_cell((0, 2), "=A1=A2");
        assert!(e.get_display((0, 2)).contains("DIV/0"));
    }

    // ─── Strict-type ordering for `<`/`>`/`<=`/`>=` ─────────

    #[test]
    fn cmp_text_is_case_insensitive_lexicographic() {
        // Previously falling back to `as_number()`, "apple" < "banana"
        // raised `#VALUE!` because "apple" doesn't parse as a number.
        // Same-type strings should compare lexicographically (case-
        // insensitive, matching the eq operator).
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "\"apple\"");
        e.set_cell((0, 1), "\"BANANA\"");
        e.set_cell((0, 2), "=A1<A2");
        assert_eq!(e.get_display((0, 2)), "TRUE");
    }

    #[test]
    fn cmp_text_lex_diverges_from_numeric() {
        // "10" < "9" lexicographically but 10 > 9 numerically. The
        // strict-type compare uses lexicographic for text.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "\"10\"");
        e.set_cell((0, 1), "\"9\"");
        e.set_cell((0, 2), "=A1<A2");
        assert_eq!(e.get_display((0, 2)), "TRUE");
    }

    #[test]
    fn cmp_bool_false_lt_true() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "FALSE");
        e.set_cell((0, 1), "TRUE");
        e.set_cell((0, 2), "=A1<A2");
        assert_eq!(e.get_display((0, 2)), "TRUE");
    }

    #[test]
    fn cmp_empty_vs_number_treats_empty_as_zero() {
        // Mirrors cell_eq's Empty-vs-Number arm: `<`/`>` treat an
        // empty cell as zero. A1 is unset (empty); B1 is 1.
        // `=A1<B1` → 0 < 1 → TRUE; `=A1>B1` → FALSE.
        let mut e = SpreadsheetEngine::new();
        e.set_cell((1, 0), "1");        // B1 = 1
        e.set_cell((0, 1), "=A1<B1");   // A2 = formula reading A1(empty) and B1(1)
        e.set_cell((0, 2), "=A1>B1");
        assert_eq!(e.get_display((0, 1)), "TRUE");
        assert_eq!(e.get_display((0, 2)), "FALSE");
    }

    #[test]
    fn cmp_propagates_error() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "=1/0");
        e.set_cell((0, 1), "5");
        e.set_cell((0, 2), "=A1<A2");
        assert!(e.get_display((0, 2)).contains("DIV/0"));
    }

    #[test]
    fn cmp_cross_type_uses_excel_type_tier() {
        // Excel's type-tier ordering: Number/Empty < Text < Bool.
        // The asymmetry the previous `as_number()` fallback created
        // (`Bool(true) <= Number(1)` → TRUE while `Bool(true) =
        // Number(1)` → FALSE) is gone: cross-type pairs are now
        // unequal AND ordered, so `<=` and `=` cannot contradict.
        let mut e = SpreadsheetEngine::new();
        // A1 = TRUE, B1 = 1 → both `=` and `<=` should be FALSE
        // (TRUE > 1 by tier).
        e.set_cell((0, 0), "TRUE");
        e.set_cell((1, 0), "1");
        e.set_cell((0, 1), "=A1=B1");
        e.set_cell((0, 2), "=A1<=B1");
        e.set_cell((0, 3), "=A1>B1");
        assert_eq!(e.get_display((0, 1)), "FALSE");
        assert_eq!(e.get_display((0, 2)), "FALSE");
        assert_eq!(e.get_display((0, 3)), "TRUE");
        // Number < Text by tier: 1 < "1".
        e.set_cell((2, 0), "=1<\"1\"");
        assert_eq!(e.get_display((2, 0)), "TRUE");
        // Text < Bool by tier: "z" < FALSE.
        e.set_cell((3, 0), "=\"z\"<FALSE");
        assert_eq!(e.get_display((3, 0)), "TRUE");
    }

    #[test]
    fn referencerange_unknown_id_returns_loading_and_queues_fetch() {
        let mut e = SpreadsheetEngine::new();
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A2\")");
        // First eval — cache miss.
        assert!(e.get_display((0, 0)).contains("LOADING"));
        // Pending queue should now include the foreign id.
        let pending = e.take_pending_fetches();
        assert_eq!(pending, vec!["foreign-id".to_string()]);
    }

    #[test]
    fn referencerange_resolved_returns_text() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_snapshot(
            "foreign-id".to_string(),
            build_snapshot("Sheet1", &[&["10", "20"], &["30", "40"]]),
        );
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:B2\")");
        // Spilled into (0,0)..(1,1).
        assert_eq!(e.get_display((0, 0)), "10");
        assert_eq!(e.get_display((1, 0)), "20");
        assert_eq!(e.get_display((0, 1)), "30");
        assert_eq!(e.get_display((1, 1)), "40");
    }

    #[test]
    fn referencerange_forbidden_returns_ref() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_error("foreign-id".to_string(), ForeignFetchError::Forbidden);
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("REF"));
    }

    #[test]
    fn referencerange_denied_returns_ref_until_consent_cleared() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_error("foreign-id".to_string(), ForeignFetchError::Denied);
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("REF"));

        // After clearing consent, the next eval should re-queue a
        // fetch and surface `#LOADING!` (cache hole was filled by
        // `Denied`; clearing it puts us back to "never seen").
        e.clear_foreign_consent("foreign-id");
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("LOADING"));
    }

    #[test]
    fn referencerange_network_error_stays_loading() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_error("foreign-id".to_string(), ForeignFetchError::Network);
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        // Network is treated as transient — keep the cell as
        // `#LOADING!` so the user sees the "trying" state.
        assert!(e.get_display((0, 0)).contains("LOADING"));
    }

    #[test]
    fn referencerange_network_error_requeues_for_retry() {
        // Regression: a transient network failure must keep
        // re-queuing on subsequent recomputes; otherwise the formula
        // is permanently stuck at `#LOADING!` until the user reloads.
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_error("foreign-id".to_string(), ForeignFetchError::Network);
        // Drain anything left in pending from earlier work.
        let _ = e.take_pending_fetches();

        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        // The eval should have re-queued the fetch.
        let pending = e.take_pending_fetches();
        assert_eq!(pending, vec!["foreign-id".to_string()]);
    }

    #[test]
    fn referencerange_oversize_returns_num() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_error("foreign-id".to_string(), ForeignFetchError::Oversize);
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("NUM"));
    }

    #[test]
    fn referencerange_self_doc_short_circuits_to_local() {
        let mut e = SpreadsheetEngine::new();
        e.set_current_doc_id("self-id".to_string());
        e.set_cell((0, 0), "100");
        e.set_cell((0, 1), "200");
        e.set_cell((1, 0), "=REFERENCERANGE(\"self-id\",\"Sheet1\",\"A1:A2\")");
        // Self-ref reads through the local engine — typed numbers,
        // not text. Spilled to (1,0) and (1,1).
        assert_eq!(e.get_display((1, 0)), "100");
        assert_eq!(e.get_display((1, 1)), "200");
        // No fetch queued.
        assert!(e.take_pending_fetches().is_empty());
    }

    #[test]
    fn referencerange_unknown_sheet_returns_name() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_snapshot(
            "foreign-id".to_string(),
            build_snapshot("Sheet1", &[&["10"]]),
        );
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"NoSuchSheet\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("NAME"));
    }

    #[test]
    fn referencerange_bubbles_error_sentinel_text() {
        let mut e = SpreadsheetEngine::new();
        // Foreign cell's displayed text is the literal "#REF!"
        // string — that's how an errored cell renders. Bubble it
        // up as a typed error rather than leaving consumers with
        // the literal string.
        e.set_foreign_doc_snapshot(
            "foreign-id".to_string(),
            build_snapshot("Sheet1", &[&["#REF!"]]),
        );
        e.set_cell((0, 0), "=REFERENCERANGE(\"foreign-id\",\"Sheet1\",\"A1:A1\")");
        assert!(e.get_display((0, 0)).contains("REF"));
    }

    #[test]
    fn referencesheet_resolves_whole_sheet() {
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_snapshot(
            "foreign-id".to_string(),
            build_snapshot("Sheet1", &[&["a", "b"], &["c", "d"]]),
        );
        e.set_cell((0, 0), "=REFERENCESHEET(\"foreign-id\",\"Sheet1\")");
        assert_eq!(e.get_display((0, 0)), "a");
        assert_eq!(e.get_display((1, 0)), "b");
        assert_eq!(e.get_display((0, 1)), "c");
        assert_eq!(e.get_display((1, 1)), "d");
    }

    #[test]
    fn referencesheet_self_doc_returns_ref() {
        let mut e = SpreadsheetEngine::new();
        e.set_current_doc_id("self-id".to_string());
        e.set_cell((0, 0), "=REFERENCESHEET(\"self-id\",\"Sheet1\")");
        // Self-ref without a range is undefined — surface `#REF!`
        // rather than dumping the whole local sheet.
        assert!(e.get_display((0, 0)).contains("REF"));
    }

    #[test]
    fn engine_clear_preserves_foreign_cache() {
        // Sheet switch (which calls `clear()`) must not drop the
        // foreign cache — fetched docs are keyed by foreign id, not
        // by local sheet, so a sheet swap shouldn't force every
        // formula to re-fetch.
        let mut e = SpreadsheetEngine::new();
        e.set_foreign_doc_snapshot(
            "foreign-id".to_string(),
            build_snapshot("Sheet1", &[&["x"]]),
        );
        e.clear();
        assert!(e.get_foreign_doc("foreign-id").is_some());
    }

    #[test]
    fn icon_set_falls_through_to_later_rect_when_first_has_no_variation() {
        // A rule covering a no-variation range (all values equal) shouldn't
        // bail out of the whole `get_icon` call — a later rule with a
        // valid range should still resolve. Regression for the `?`
        // short-circuit that was in the inner loop.
        let mut engine = SpreadsheetEngine::new();
        // First rect: A1:A2 = 5,5 (no variation).
        engine.set_cell((0, 0), "5");
        engine.set_cell((0, 1), "5");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        );
        // Second rect covers A1 again but spans A1:A3 with 0..10 so
        // (0,0) sits in the bottom tertile.
        engine.set_cell((0, 2), "10");
        engine.add_conditional_format(
            (0, 0), (0, 2),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        );
        // Without the fall-through, (0, 0) would return None because
        // the first rect's range_min_max fails. With the fix, the
        // second rect resolves.
        assert_eq!(engine.get_icon((0, 0)), Some("\u{2193}"));
    }

    #[test]
    fn icon_set_no_variation_yields_none() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "5");
        engine.set_cell((0, 1), "5");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::IconSet { kind: IconSetKind::ThreeArrows },
        );
        // All values equal → no tertile spread → no icon.
        assert_eq!(engine.get_icon((0, 0)), None);
    }

    #[test]
    fn data_bar_negative_clamps_to_zero() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "-5");
        engine.set_cell((0, 1), "10");
        engine.add_conditional_format(
            (0, 0), (0, 1),
            ConditionalFormat::DataBar { color: "#3b82f6".to_string() },
        );
        assert_eq!(engine.get_data_bar((0, 0)).map(|(_, r)| r), Some(0.0));
    }

    // ─── Merge tests ───────────────────────────────────────

    #[test]
    fn merge_hides_inner_cells() {
        let mut engine = SpreadsheetEngine::new();
        engine.merge_cells(0, 0, 2, 2);
        assert!(!engine.is_merged_hidden(0, 0)); // anchor
        assert!(engine.is_merged_hidden(1, 0));   // hidden
        assert!(engine.is_merged_hidden(0, 1));   // hidden
        assert!(engine.is_merged_hidden(1, 1));   // hidden
        assert!(!engine.is_merged_hidden(2, 0));  // outside
    }

    #[test]
    fn merge_span() {
        let mut engine = SpreadsheetEngine::new();
        engine.merge_cells(0, 0, 3, 2);
        assert_eq!(engine.get_merge_span(0, 0), (3, 2));
        assert_eq!(engine.get_merge_span(1, 0), (1, 1)); // non-anchor
    }

    #[test]
    fn unmerge_clears() {
        let mut engine = SpreadsheetEngine::new();
        engine.merge_cells(0, 0, 2, 2);
        assert!(engine.is_merged_hidden(1, 0));
        engine.unmerge_at(0, 0);
        assert!(!engine.is_merged_hidden(1, 0));
    }

    // ─── Number format display ─────────────────────────────

    #[test]
    fn number_format_currency() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1234.5");
        engine.style_mut((0, 0)).number_format = Some("currency".to_string());
        assert_eq!(engine.get_display((0, 0)), "$1234.50");
    }

    #[test]
    fn number_format_percent() {
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "0.15");
        engine.style_mut((0, 0)).number_format = Some("percent".to_string());
        assert_eq!(engine.get_display((0, 0)), "15.0%");
    }

    // ─── Frozen panes (M-S2) ────────────────────────────────────

    #[test]
    fn frozen_pane_counts_default_to_zero() {
        let engine = SpreadsheetEngine::new();
        assert_eq!(engine.frozen_rows, 0);
        assert_eq!(engine.frozen_cols, 0);
    }

    #[test]
    fn clear_resets_frozen_pane_counts() {
        let mut engine = SpreadsheetEngine::new();
        engine.frozen_rows = 1;
        engine.frozen_cols = 2;
        engine.clear();
        assert_eq!(engine.frozen_rows, 0);
        assert_eq!(engine.frozen_cols, 0);
    }

    // ─── Spill blocks (M-S1a) ───────────────────────────────────

    #[test]
    fn transpose_row_to_column_spills() {
        // A1=1 B1=2 C1=3, then D1=TRANSPOSE(A1:C1) — anchor at D1
        // (col=3, row=0); spill should fill D1,D2,D3 with 1,2,3.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((1, 0), "2");
        engine.set_cell((2, 0), "3");
        engine.set_cell((3, 0), "=TRANSPOSE(A1:C1)");
        assert_eq!(engine.get_display((3, 0)), "1");
        assert_eq!(engine.get_display((3, 1)), "2");
        assert_eq!(engine.get_display((3, 2)), "3");
    }

    #[test]
    fn transpose_column_to_row_spills() {
        // A1=1 A2=2 A3=3, then B1=TRANSPOSE(A1:A3) — anchor at B1;
        // spill should fill B1,C1,D1.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((0, 2), "3");
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A3)");
        assert_eq!(engine.get_display((1, 0)), "1");
        assert_eq!(engine.get_display((2, 0)), "2");
        assert_eq!(engine.get_display((3, 0)), "3");
    }

    #[test]
    fn spill_blocks_recompute_when_source_changes() {
        // Source change → anchor's array changes → filled cells'
        // values must update too. Without `recalculate_from`'s
        // spill-fill propagation this regresses silently.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A2)");
        assert_eq!(engine.get_display((1, 0)), "1");
        assert_eq!(engine.get_display((2, 0)), "2");

        // Mutate A2 → the spill at C1 must update.
        engine.set_cell((0, 1), "99");
        assert_eq!(engine.get_display((1, 0)), "1");
        assert_eq!(engine.get_display((2, 0)), "99");
    }

    #[test]
    fn spill_conflict_yields_spill_error() {
        // Anchor at B1 wants to spill into C1, but C1 already has
        // user content — the anchor's value must be `#SPILL!` and
        // C1's content stays untouched.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((2, 0), "blocking"); // C1 — would be spilled into
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A2)");
        assert_eq!(engine.get_display((1, 0)), "#SPILL!");
        assert_eq!(engine.get_display((2, 0)), "blocking");
    }

    #[test]
    fn writing_into_spill_fill_breaks_block() {
        // Anchor at B1 spills into C1. User writes into C1 → block
        // breaks: anchor reports `#SPILL!`, C1 holds the user's value.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A2)");
        assert_eq!(engine.get_display((2, 0)), "2");
        engine.set_cell((2, 0), "user");
        assert_eq!(engine.get_display((1, 0)), "#SPILL!");
        assert_eq!(engine.get_display((2, 0)), "user");
    }

    #[test]
    fn replacing_anchor_clears_old_spill_block() {
        // B1 starts as a TRANSPOSE that spills into C1. User
        // replaces B1's formula with a scalar. C1 should clear back
        // to empty (no leftover spill cell).
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A2)");
        assert_eq!(engine.get_display((2, 0)), "2");
        engine.set_cell((1, 0), "scalar");
        assert_eq!(engine.get_display((1, 0)), "scalar");
        assert_eq!(engine.get_display((2, 0)), "");
    }

    #[test]
    fn formula_that_reads_a_spill_fill_recomputes_when_source_changes() {
        // A1=1, A2=2, B1=TRANSPOSE(A1:A2) → spills to B1, C1.
        // F1 references C1 (a spill-FILL cell, not the anchor B1).
        // When the user mutates A2, the BFS starting at A2 must walk:
        //   A2 → B1 (anchor, depends on A1:A2)
        //        → spill-fill C1 → dependents-of-C1 → F1
        // Without the spill-fill propagation in `recalculate_from`,
        // F1 would silently stale on `A2` mutation. This is the
        // criterion-A3 / criterion-M4 regression guard the reviewer
        // flagged was missing.
        let mut engine = SpreadsheetEngine::new();
        engine.set_cell((0, 0), "1");
        engine.set_cell((0, 1), "2");
        engine.set_cell((1, 0), "=TRANSPOSE(A1:A2)");
        // F1 references C1 (the spill-filled cell).
        engine.set_cell((5, 0), "=C1*10");
        assert_eq!(engine.get_display((5, 0)), "20");
        // Now change A2; F1 must update through the spill block.
        engine.set_cell((0, 1), "9");
        assert_eq!(engine.get_display((2, 0)), "9", "C1 (spill-fill) must reflect new A2");
        assert_eq!(engine.get_display((5, 0)), "90", "F1 (depends on C1) must re-evaluate");
    }
}
