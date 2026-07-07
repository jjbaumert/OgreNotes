# Pivot Tables — Design

## Context

Phase 3's M-S2 row "Pivot tables" shipped as v1: a `=PIVOT(range,
row, val, agg)` formula function plus a context-menu "Insert
Pivot..." wizard that prompts for column indices and writes the
formula. Both are recorded as v1, with
the drag-drop configurator deferred to a follow-up.

This document is that follow-up. It replaces the v1 surface (no
backward-compatibility shim — the system has no live users) with
a typed PivotTable object + drag-drop editor sidebar that mirrors
Excel and Google Sheets parity.

The decision to drop v1 outright was confirmed by the user. Three
priorities drove the architecture:

1. **Great UX** — direct manipulation of a typed pivot object,
   drag-drop fields between Rows / Cols / Values / Filters zones.
2. **Easy XLSX import/export** — the model maps ~1:1 to OOXML's
   `PivotCache` + `PivotTable` XML parts.
3. **Useful for agentic harnesses** — a typed schema is the
   intent. An LLM tool-use call like `pivot.add_row_group(2)` or
   `pivot.set_summarize_fn(0, "AVERAGE")` operates on named fields
   instead of string-edits to formula contents.

## Goals

- One model, one entry point: drag-drop sidebar editor.
- PivotTable is a typed object anchored to a cell; the rendered
  output spills into the grid via the existing dynamic-array
  spill mechanism (M-S1a).
- Multi-row, multi-col, multi-value, multi-filter — full Sheets
  API parity for the in-scope subset (see Out of scope below).
- Excel parity for field discovery (checkbox + search),
  date / numeric grouping, layout style (Compact / Outline /
  Tabular), and grand-total state (None / Rows / Cols / Both).
- Persistence rides the existing sheet-level attribute pattern
  (last-writer-wins on the pivot blob, same as conditional
  formats); fine-grained conflict-free merging within a pivot
  is a v2.x consideration.
- XLSX round-trip with no information loss for the in-scope
  fields.

## Non-goals (v2.x and beyond)

- Calculated fields (custom formulas as PivotValue): listed in
  Sheets API but rare; Sheets users hand-roll with a helper col
  on the source.
- Pivot charts (rendering pivot output as a chart): orthogonal
  feature; can layer on once pivots ship.
- Slicer / timeline UI (Excel-style external visual filter
  controls).
- "Show Values As" derived display modes (% of grand total, % of
  row, running total): Sheets has them; defer to v2.1.
- Custom subtotals per row group (sum + average + count for the
  same value column).
- Joining multiple source ranges as one pivot input.
- Refresh-on-schedule (we recompute synchronously on every
  source-range change anyway).

## Architecture overview

```
┌──────────────────────────────────────────────────────────┐
│  Spreadsheet doc tree (YJS / Node)                       │
│   sheet[0]                                               │
│     cells: { (col,row) → CellNode }                      │
│     pivots: [ PivotTable {anchor, source, rows, ...} ]   │
│   sheet[1] ...                                           │
└──────────────┬───────────────────────────────────────────┘
               │ persisted via ATTR_PIVOT_* (round-trips
               │ through CRDT and YJS doc storage)
               ▼
┌──────────────────────────────────────────────────────────┐
│  SpreadsheetEngine (sync, frontend)                       │
│   pivots: HashMap<(sheet, anchor), PivotTable>            │
│   pivot_outputs: HashMap<(sheet, anchor), Vec<Vec<…>>>    │
│   recompute_pivots() — called on source-range edits,      │
│                        on pivot config edits,             │
│                        and on cross-doc snapshot updates  │
└──────────────┬───────────────────────────────────────────┘
               │
               │ output spills via existing dynamic-array
               │ spill machinery (M-S1a)
               ▼
┌──────────────────────────────────────────────────────────┐
│  View layer (Leptos)                                      │
│   • Anchor cell renders top-left of spill                 │
│   • Surrounding spill cells render virtual values         │
│   • Click in spill → select anchor → open editor sidebar  │
│   • Drag-drop sidebar mutates PivotTable in place;        │
│     engine recomputes; spill repaints reactively.         │
└───────────────────────────────────────────────────────────┘
```

## Engine layer

### Module location

New file: `frontend/src/spreadsheet/pivot.rs`. The existing
`fn_pivot` in `functions.rs` (line ~1625) is removed along with
its dispatch arm and unit tests. The wizard in
`spreadsheet_view/context_menu.rs` is removed. v1 callers vanish.

### Types

```rust
pub struct PivotTable {
    pub anchor: (usize, usize),          // (col, row) in the sheet
    pub source: SourceRange,              // address of input range
    pub rows: Vec<PivotGroup>,            // nesting order = vec order
    pub cols: Vec<PivotGroup>,
    pub values: Vec<PivotValue>,
    pub filters: Vec<PivotFilterSpec>,
    pub value_layout: ValueLayout,        // Horizontal | Vertical
    pub layout_style: LayoutStyle,        // Compact | Outline | Tabular
    pub grand_totals: GrandTotals,        // None | Rows | Cols | Both
    pub subtotals_position: SubtotalsPos, // Above | Below
}

pub enum SourceRange {
    Local { sheet: usize, range: RangeRef },
    Foreign { doc_id: String, sheet_name: String, range: RangeRef },
    // Foreign reuses the cross-doc references plumbing — pivot
    // over a REFERENCERANGE is a natural extension.
}

pub struct PivotGroup {
    pub source_col: usize,                // 0-based offset into source
    pub sort_order: SortOrder,            // Asc | Desc | None
    pub show_totals: bool,
    pub label: Option<String>,            // overrides source header
    pub sort_by_value: Option<usize>,     // index into PivotTable.values
    pub kind: PivotGroupKind,             // Direct | Date(_) | NumericBin{_}
}

pub enum PivotGroupKind {
    Direct,                               // exact value (default)
    Date(DateGranularity),                // bucket dates
    NumericBin { width: f64, start: Option<f64> },
}

pub enum DateGranularity { Year, Quarter, Month, Day, Hour }

pub struct PivotValue {
    pub source_col: usize,
    pub summarize_fn: SummarizeFn,
    pub display_name: Option<String>,
}

pub enum SummarizeFn {
    Sum, Count, CountA, Average,
    Min, Max, Median, Product,
    StdDev, StdDevP, Var, VarP,
}

pub enum PivotFilterCondition {
    ValueIn(Vec<String>),                 // discrete value picker
    Number(NumberFilter),                 // >, <, =, between
    Text(TextFilter),                     // contains, equals, starts
    Empty, NotEmpty,
}

pub struct PivotFilterSpec {
    pub source_col: usize,
    pub condition: PivotFilterCondition,
    pub visible_values: Option<HashSet<String>>,  // explicit allow-list
}

pub enum ValueLayout { Horizontal, Vertical }
pub enum LayoutStyle { Compact, Outline, Tabular }
pub enum GrandTotals { None, Rows, Cols, Both }
pub enum SubtotalsPos { Above, Below }
pub enum SortOrder { Asc, Desc, None }
```

### Evaluator

`pivot.rs::eval_pivot(pt: &PivotTable, src: &Vec<Vec<CellValue>>)
-> Vec<Vec<CellValue>>` returns the rendered 2D grid:

1. Filter source rows by `filters`.
2. Derive group keys per row (consults `PivotGroup.kind`):
   - `Direct` — exact value (default).
   - `Date(g)` — parse value as serial number or ISO date, map
     to `g`'s bucket label (e.g. `Date(Month)` → `"2026-05"`,
     `Date(Quarter)` → `"2026-Q2"`).
   - `NumericBin { width, start }` — bucket numeric values into
     `[start + n·width, start + (n+1)·width)` ranges; emit
     bucket label like `"10–20"`.
3. Group by row keys (compound key = tuple of row group keys);
   group by col keys.
4. For each (row_key, col_key, value_idx) tuple, fold matching
   source rows with `summarize_fn`.
5. Emit output, shape governed by `layout_style`:
   - **Compact** — all row-group keys stacked into a single
     leftmost column with indentation. Subtotal rows for each
     parent. (Excel default.)
   - **Outline** — each row group in its own column;
     subtotals appear on the parent-row line of the next group
     down.
   - **Tabular** — each row group in its own column;
     subtotals appear as separate dedicated rows.
   For all three: header row(s) carry col group keys × value
   labels (interleaving governed by `value_layout` —
   Horizontal stacks values across, Vertical stacks them down
   inside each col group).
6. Per-group subtotal rows appear above or below their group
   per `subtotals_position`. Grand totals appear only when
   `grand_totals` requests them: `Rows` adds a bottom grand-
   total row; `Cols` adds a rightmost grand-total column;
   `Both` adds both; `None` omits both.
7. Apply per-group `sort_order` (and `sort_by_value` when set).

**Auto-detection**: when a column is dragged into Rows or Cols
and no explicit `kind` is set, the editor inspects the source
column's prevailing type and seeds:
- date column → `Date(Month)` (Excel's behavior).
- numeric column → `Direct` (no auto-bin; user opts in).
- text column → `Direct`.

Non-numeric values are skipped for SUM / AVG / MIN / MAX (matches
Excel range-aggregation; same rule v1 had). COUNT counts numeric
cells only; COUNTA counts non-empty.

### Engine integration

`SpreadsheetEngine` gains:

- `pivots: HashMap<(usize, (usize, usize)), PivotTable>` — keyed
  by (sheet_idx, anchor).
- `pivot_outputs: HashMap<(usize, (usize, usize)), Vec<Vec<CellValue>>>`
  — cached output, invalidated on edit.
- `pub fn recompute_pivot(&mut self, sheet, anchor)` — re-evals
  one pivot.
- `pub fn recompute_pivots_for_source(&mut self, sheet, range)` —
  invalidates every pivot whose source overlaps `range`.

Hook points:
- `set_cell((col, row), …)` already triggers dependency recompute;
  extend to call `recompute_pivots_for_source` when the cell is
  inside any pivot's source range.
- `set_foreign_doc_snapshot(id, snap)` — invalidate pivots whose
  source is `Foreign { doc_id: id, … }`.

Spill rendering reuses the existing dynamic-array spill code
(M-S1a). The pivot output is treated as a spilled array anchored
at `pivot.anchor`. The grid renderer already knows how to draw
spilled cells; the only new bit is: clicking a spilled cell that
belongs to a pivot opens the editor sidebar (rather than only
selecting the anchor).

## View layer

### Files touched

- `frontend/src/spreadsheet/pivot.rs` (new) — types, evaluator,
  CRDT (de)serialization helpers.
- `frontend/src/spreadsheet/eval.rs` — engine fields + recompute
  hooks.
- `frontend/src/components/spreadsheet_view.rs` — sidebar wiring,
  click-into-spill detection.
- `frontend/src/components/spreadsheet_view/pivot_editor.rs`
  (new) — the drag-drop sidebar component.
- `frontend/src/components/spreadsheet_view/persistence.rs` —
  `ATTR_PIVOT_*` round-trip.
- `frontend/src/components/spreadsheet_view/context_menu.rs` —
  remove "Insert Pivot..." wizard; add "Insert Pivot Table"
  action that places an anchor and opens the editor.
- `frontend/src/spreadsheet/functions.rs` — remove `"PIVOT" =>
  fn_pivot(...)` dispatch arm; remove `fn_pivot` body and tests.

### The drag-drop sidebar

Layout (Excel-shaped: field list on top, four zones below):

```
┌─ Pivot table editor ─────────────┐
│ Source: A1:E100        [edit]    │
│                                  │
│ Layout: Compact ▼  Totals: Both ▼│
│                                  │
│ ┌─ Fields ─────────────────────┐ │
│ │ 🔍 Search...                 │ │
│ │ ☐ T  Region                  │ │
│ │ ☐ T  Product                 │ │
│ │ ☐ #  Revenue                 │ │
│ │ ☐ 📅 OrderDate               │ │
│ │ ☐ T  Quarter                 │ │
│ └──────────────────────────────┘ │
│                                  │
│ Suggestions:                     │
│   Sum of Revenue by Region [+]   │
│   Count of Orders by Product [+] │
│                                  │
│ Rows                             │
│ ┌─────────────────────────────┐  │
│ │ ⋮⋮ Region         ↑Asc ☑️ ✕│  │
│ │ ⋮⋮ OrderDate ▶ Month ↓ ☑️ ✕│  │
│ └─────────────────────────────┘  │
│                                  │
│ Columns                          │
│ ┌─────────────────────────────┐  │
│ │ (drop fields here)          │  │
│ └─────────────────────────────┘  │
│                                  │
│ Values                           │
│ ┌─────────────────────────────┐  │
│ │ ⋮⋮ Revenue     SUM ▼    ✕  │  │
│ └─────────────────────────────┘  │
│                                  │
│ Filters                          │
│ ┌─────────────────────────────┐  │
│ │ ⋮⋮ Quarter     Q4 only  ✕  │  │
│ └─────────────────────────────┘  │
└──────────────────────────────────┘
```

State:

- `pivot_editor_open: RwSignal<Option<(usize, (usize, usize))>>` —
  Some when sidebar is showing for a specific (sheet, anchor).
- `field_search: RwSignal<String>` — case-insensitive substring
  filter on the field list.
- The sidebar reads/writes `engine.pivots` directly through the
  existing engine-mutex pattern; on each mutation it bumps
  `grid_version` to repaint.

Field list (top half):

- Lists every column header from the source range with a type
  icon (`T` text, `#` number, `📅` date) and a checkbox.
- Search input narrows by case-insensitive substring on the
  column name.
- Checkbox click default-routes by detected type:
  - text → Rows
  - date → Cols (with auto-`Date(Month)` kind)
  - number → Values (with auto-`SummarizeFn::Sum`)
  Sheets doesn't auto-route on click; we follow Excel because
  checkbox-first discovery is the reason we added the field
  list.
- Drag from the field list to any zone bypasses default-routing
  and places the field where dropped.
- Unchecking a field removes it from whatever zone(s) hold it.

Top-of-sidebar dropdowns:

- **Layout** (Compact / Outline / Tabular) mutates
  `PivotTable.layout_style`.
- **Totals** (None / Rows / Cols / Both) mutates
  `PivotTable.grand_totals`.
- **Subtotals position** lives in a small popover off the gear
  icon next to the Totals dropdown — toggles between Above and
  Below, defaulting to Below (Excel default).

Drag-drop primitives:

- HTML5 native drag-drop API (no library): `draggable=true` on
  field-list rows and zone entries; `dragover`/`drop` on zones.
- Drag from field list to zone adds the field.
- Drag from zone to zone moves the field.
- Drag within a zone reorders (changes the YArray order; nests
  in Rows / Cols).
- Drag back to the field list (or off-zone) removes.
- Clicking the agg dropdown on a Values entry opens a small
  popover with the SummarizeFn list.
- Per-row chip shows a `▶ <param>` indicator when its `kind`
  is `Date(_)` or `NumericBin { .. }`; clicking it opens a
  popover to edit the granularity (Year / Quarter / Month /
  Day / Hour) or bin width.
- Filter chip ("Q4 only") opens a popover that builds a
  `PivotFilterSpec`.

### Suggestions panel

Top of the sidebar lists 3–5 auto-suggested pivots based on the
source schema:

- For each text column, suggest "<numeric col agg> by <text col>".
- For two text columns + one numeric: suggest a 2-D pivot.
- For a date column: suggest grouping by year / month / day.

Click a suggestion → it instantiates the PivotTable. Cheap UX
win, no LLM in the loop.

### Insert flow

1. User selects a range with header row.
2. Right-click → "Insert Pivot Table" (or toolbar button).
3. Dialog asks for destination cell (default = below source).
4. New empty `PivotTable` created at destination; source range
   copied; sidebar opens with the suggestion panel pre-populated.
5. User drags fields → engine recomputes → spill renders.

### Click-into-spill

When a cell within a pivot's spill area is clicked:
- Selection lands on the anchor cell.
- A subtle "Editing pivot at A1" ribbon appears above the grid.
- Sidebar opens automatically if not already open.

## Persistence

### Doc-tree representation

The pivot list is stored as a serialized JSON blob in a
sheet-level attribute, matching the existing pattern used for
conditional formats and named ranges (`persistence.rs::serialize_*`
/ `deserialize_*`). The CRDT round-trip happens at the
attribute-map level — `yrs` merges concurrent attribute writes
last-writer-wins on the whole pivot vector. Fine-grained
conflict-free merging on individual pivot fields would require a
nested YArray/YMap representation; the trade-off mirrors what
conditional formats already accept (a sheet-wide CF rule set is
also a single JSON blob).

Schema lives in the sheet `<table>` element's attribute map under
`ATTR_PIVOTS = "pivots"`:

```json
[{
  "anchor": [c, r],
  "source": {"kind": "local",   "range": "A1:E100"}
          | {"kind": "foreign", "doc": "<id>", "sheet": "<name>", "range": "A1:E100"},
  "rows":    [<group>...],
  "cols":    [<group>...],
  "values":  [<value>...],
  "filters": [<filter>...],
  "valueLayout": "horizontal" | "vertical",
  "layout":      "compact" | "outline" | "tabular",
  "totals":      "none" | "rows" | "cols" | "both",
  "subtotals":   "above" | "below"
}]
```

`<group>` carries `col`, `sort`, `showTotals`, optional `label`,
optional `sortByValue`, and a `kind` of `"direct"`,
`{"date":"month"|...}`, or `{"numericBin":{"width":...,"start":...?}}`.
`<value>` carries `col`, `fn`, optional `name`. `<filter>`
carries `col` and a `cond` payload (one of `valueIn`/`numGt`/
`numLt`/`numEq`/`numBetween`/`textContains`/`textEquals`/
`textStartsWith`/`empty`/`notEmpty`).

Bad payloads silently drop on load (the malformed-load test
asserts this) — preferring partial loss over refusing to open
the sheet.

### Attribute namespace

Constants in `persistence.rs`:

```
ATTR_PIVOT_ANCHOR
ATTR_PIVOT_SOURCE
ATTR_PIVOT_VALUE_LAYOUT
ATTR_PIVOT_LAYOUT_STYLE        // "compact" | "outline" | "tabular"
ATTR_PIVOT_GRAND_TOTALS        // "none" | "rows" | "cols" | "both"
ATTR_PIVOT_SUBTOTALS_POSITION  // "above" | "below"
ATTR_PIVOT_ROWS
ATTR_PIVOT_COLS
ATTR_PIVOT_VALUES
ATTR_PIVOT_FILTERS
ATTR_PG_SOURCE_COL             // PivotGroup
ATTR_PG_SORT_ORDER
ATTR_PG_SHOW_TOTALS
ATTR_PG_LABEL
ATTR_PG_SORT_BY_VALUE
ATTR_PG_KIND                   // "direct" | "date" | "numeric_bin"
ATTR_PG_KIND_PARAM             // granularity name ("month") or
                               // numeric bin "width:start" (start
                               // omitted if None)
ATTR_PV_SOURCE_COL             // PivotValue
ATTR_PV_SUMMARIZE_FN
ATTR_PV_DISPLAY_NAME
ATTR_PF_SOURCE_COL             // PivotFilter
ATTR_PF_CONDITION
ATTR_PF_VISIBLE_VALUES
```

## XLSX import/export

Two layers, in tension:

**v2.0 (shipping now): pivot output cells survive the round-trip.**
`crates/collab/src/export.rs` reads cell text from the YJS doc;
the persistence layer (`spreadsheet_view/persistence.rs`) now
writes the rendered display text of every spill cell with an
`ATTR_SPILLED = "spilled"` marker. XLSX export naturally captures
the visible output. On OgreNotes-side reload, the loader skips
marked cells; the engine's `set_pivots` reinstalls them through
the spill block from the typed `ATTR_PIVOTS` config.

This means:
- OgreNotes → XLSX: pivot output cells appear as plain values
  in Excel. The user sees the pivoted data; clicking a cell in
  Excel shows the value, not "this is part of a pivot".
- XLSX (Excel-authored pivot) → OgreNotes: the pivot is read as
  plain cells. Native pivots in the XLSX file are not parsed
  into `PivotTable` configs; they survive only as data.
- OgreNotes → OgreNotes: full round-trip (typed `PivotTable`
  via `ATTR_PIVOTS`) — unaffected.

**v2.1 (deferred): native bidirectional Excel pivots.** Requires
hand-rolled OOXML emission via `quick-xml` + `zip` for these
parts, which `rust_xlsxwriter` doesn't support natively:

| Our type           | OOXML part                                   |
|--------------------|----------------------------------------------|
| PivotTable         | `xl/pivotTables/pivotTableN.xml`             |
| Source cache       | `xl/pivotCache/pivotCacheDefinitionN.xml` + `pivotCacheRecordsN.xml` |
| PivotGroup         | `<rowFields>` / `<colFields>` + `<pivotFields>` flags |
| PivotValue         | `<dataFields>` with `subtotal` attribute     |
| PivotFilterSpec    | `<filters>` / `<filter>` and `<colItems>`    |
| `layout_style`     | `<pivotTableDefinition compact="…" outline="…" />` |
| `grand_totals`     | `<pivotTableDefinition rowGrandTotals="…" colGrandTotals="…" />` |
| `subtotals_position` | `<pivotField subtotalTop="…" />`           |
| `PivotGroupKind::Date(g)` | `<pivotField numFmtId="…">` with `<fieldGroup>` + `<rangePr>` (groupBy=days/months/quarters/years) + `<groupItems>` for bucket labels |
| `PivotGroupKind::NumericBin` | `<pivotField>` with `<fieldGroup>` + numeric `<rangePr>` (autoStart/autoEnd/groupInterval) + `<groupItems>` |

**Why v2.1, not v2.0**: the existing XLSX layer is data-only —
`rust_xlsxwriter` doesn't expose PivotCache/PivotTable APIs and
`calamine` skips those parts on read. Hand-rolling those parts
through `quick-xml` + the `zip` crate is multi-week work and
duplicates serialization logic the OgreNotes engine already has.
The v2.0 spill-cell path delivers the user-visible "pivot data
makes it to Excel" outcome with three lines of persistence
changes.

For v2 import: parse the relevant XML parts; emit the typed
PivotTable; attach to the anchor cell by the OOXML `location`
ref.

For v2 export: walk every PivotTable in the doc; write a
PivotCache (capturing the source data at export time) + a
PivotTable XML referencing it.

CSV is unaffected — pivots are not representable in CSV; the
CSV exporter writes the spilled output (current behavior for
spilled arrays).

## Verification

### Unit tests (`frontend/src/spreadsheet/pivot.rs::tests`)

- `pivot_single_row_field_sum` — replicates the v1 SUM test.
- `pivot_two_row_fields_nested_grouping` — Region > Product.
- `pivot_one_row_one_col_two_values` — ensures interleaved
  header.
- `pivot_value_layout_horizontal_vs_vertical` — both layouts on
  the same input.
- `pivot_filter_value_in_excludes_rows` — `ValueIn(["Q1","Q2"])`.
- `pivot_filter_number_gt` — `Number(>5)`.
- `pivot_show_totals_emits_subtotal_rows` — rows with subtotal.
- `pivot_grand_total_row` — bottom-of-output grand total.
- `pivot_sort_by_value_descending` — sort row groups by a
  values[] cell.
- `pivot_summarize_fn_all_variants` — at least one row per
  SummarizeFn variant.
- `pivot_recompute_on_source_edit` — write a source cell, verify
  cached output invalidates.
- `pivot_foreign_source_uses_cross_doc_snapshot` — Source =
  Foreign, sets snapshot, verify output renders.
- `pivot_date_grouping_by_month_buckets_correctly` — source has
  daily dates spanning 6 months; `kind = Date(Month)` yields 6
  row buckets with correct sums.
- `pivot_date_grouping_by_quarter` — same input grouped by
  quarter yields 2 buckets.
- `pivot_numeric_bin_grouping_by_width` — bins integers
  0..100 by `NumericBin { width: 10, start: Some(0) }`; assert
  10 buckets with the right labels.
- `pivot_layout_style_compact_vs_tabular` — same input, two
  styles; assert different output column counts and subtotal
  placement.
- `pivot_grand_totals_rows_only` — `grand_totals = Rows`;
  assert grand-total row present, no grand-total column.
- `pivot_subtotals_above_vs_below` — same input, both
  positions; assert subtotal row index differs.
- `pivot_field_list_routes_by_type` — sidebar test: checkbox
  click on a text col adds it to `rows`; on a number adds it to
  `values` with `SummarizeFn::Sum`; on a date adds it to `cols`
  with `kind = Date(Month)`.

### Integration tests (Rust, in `crates/collab/`)

- `xlsx_import_pivot_table_round_trips` — load a fixture XLSX
  with a known pivot, verify our schema matches; export and
  re-import, verify identity.
- `xlsx_pivot_with_filter_round_trips` — adds a `<filters>` clause.

### Doctor scenario (Playwright)

`scripts/frontend-doctor/scenarios/pivot-editor.js`:

1. Login + create a doc; insert a spreadsheet; type some sample
   data with header row.
2. Right-click in source → "Insert Pivot Table".
3. Confirm sidebar opens.
4. Drag a column-header chip into Rows zone; assert spill
   appears with one column of group labels.
5. Drag a numeric column into Values zone; assert a SUM column
   appears.
6. Click the SUM dropdown → AVERAGE; assert spill repaints.
7. Drag Rows entry up/down (reorder); assert spill nesting
   re-orders.
8. Edit a source cell; assert spill recomputes.
9. Delete the pivot; assert spill clears.

### Manual smoke

- Open a doc with a v2-authored pivot, save, reload — pivot
  re-renders identically.
- Two browsers editing the same pivot's row groups simultaneously
  — final state has both edits (CRDT merge).
- XLSX export → open in Excel — pivot is a real Excel pivot
  table, refreshable.
- XLSX import (Excel-authored pivot) → pivot renders in OgreNotes
  with same values.

## Phased rollout (multi-week)

1. **Engine model + evaluator** (~4 days): types (incl. four
   new enums for layout / totals / subtotals / group kinds),
   `eval_pivot` with grouping-by-date / numeric-bin and three
   layout-style branches, the unit tests above. No UI; tests
   drive the API.
2. **Persistence** (~2 days): `ATTR_PIVOT_*` constants
   (including layout / totals / subtotals / kind / kind-param),
   YJS round-trip, conflict-merge tests for concurrent zone
   edits.
3. **Drag-drop sidebar UI** (~1.5 weeks): `pivot_editor.rs`
   component, HTML5 drag-drop primitives, field list with
   search + checkbox auto-route, agg / filter / kind popovers,
   layout + totals dropdowns, suggestion panel, click-into-
   spill detection.
4. **XLSX import/export** (~1 week): translate to/from
   `xl/pivotTables/...` and `xl/pivotCache/...` XML parts
   (including `<fieldGroup>` / `<rangePr>` for date / numeric
   grouping); round-trip tests with fixture files.
5. **Doctor scenario + design close** (~2 days): `pivot-editor`
   Playwright scenario; tick the Pivot row in the Phase 3
   carry-forwards as ✅ shipped.

**Total**: ~3.5 weeks.

## Critical files

**Frontend Rust:**
- `frontend/src/spreadsheet/pivot.rs` (new) — types, evaluator,
  CRDT (de)serializers.
- `frontend/src/spreadsheet/eval.rs` — engine fields,
  `recompute_pivots_for_source`, hook into `set_cell`.
- `frontend/src/spreadsheet/functions.rs` — remove `fn_pivot`,
  remove `"PIVOT"` dispatch arm, remove v1 tests.
- `frontend/src/components/spreadsheet_view/pivot_editor.rs`
  (new) — drag-drop sidebar.
- `frontend/src/components/spreadsheet_view.rs` — sidebar slot,
  click-into-spill, suggestion plumbing.
- `frontend/src/components/spreadsheet_view/persistence.rs` —
  ATTR_PIVOT_* round-trip.
- `frontend/src/components/spreadsheet_view/context_menu.rs` —
  remove the wizard, add "Insert Pivot Table" entry.

**Backend Rust:**
- `crates/collab/src/export.rs` — XLSX pivot translation
  (new section gated on `xlsx` feature).

**Tests:**
- `frontend/src/spreadsheet/pivot.rs::tests` (unit).
- `crates/collab/tests/test_xlsx_pivot_round_trip.rs` (new).
- `scripts/frontend-doctor/doctor.js` + new
  `scenarios/pivot-editor.js`.

**Design docs:**
- `design/pivot-tables.md` (this file).

## Open questions for impl phase

- **Subtotal placement**: above or below each row group?
  Excel defaults below; Sheets gives a toggle. v2.0 ships below
  with no toggle; toggle is a small follow-up.
- **Multiple values + cols layout**: when both `cols` and
  `values` have entries, which interleaves? Excel: values
  interleave inside col groups (each col group repeats every
  value). Sheets: same. We follow that; `value_layout` only
  controls Horizontal vs Vertical for the values themselves.
- **Empty pivot rendering**: when no fields are in any zone,
  what does the spill show? v2.0: empty (no spill). The anchor
  cell shows nothing; the grid shows a hint chip "Add fields to
  build pivot" only when sidebar is open.
- **Foreign source pivots and consent**: `SourceRange::Foreign`
  rides on the cross-doc references plumbing — same per-session
  consent prompt applies. No new consent surface.
- **LLM tool surface**: separate v2.x deliverable — a small
  module exposing typed `pivot.add_row_group(col)`,
  `pivot.set_summarize_fn(idx, fn)`, etc. as a JSON-RPC-ish
  surface. Out of scope for this design but the typed schema
  here is the foundation.
