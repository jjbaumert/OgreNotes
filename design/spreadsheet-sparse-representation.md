# Spreadsheet Sparse Representation (DRAFT — issue #122)

**Status:** DRAFT for human review. Not yet approved; do not implement against this.
Companion to #121 (per-commit persist costs) and #122 (this design investigation).

## Problem

The spreadsheet's persisted yrs document — and every pass that touches it — is a
dense `rows × cols` Node tree regardless of how many cells hold data. A sheet
with 20 used cells can carry a 300×27 (8,100-cell) materialized tree, because the
persisted extent only ever grows (keyboard navigation and wheel-scroll extend
`grid_rows` by +20; the next commit persists that extent forever).

Measured evidence (Firefox profiles on ogrenotes.example.com, edit workload):

- **Before #121:** edit-path script time was a 73–84% call chain; the persist
  subtree alone was ~35–44% of it. The hot leaf split into two ~39% chains — the
  two per-commit O(doc) yrs passes (`doc_to_ydoc_bytes` wasted REST encode and the
  full-doc `sync_model_to_ydoc` diff).
- **After #121** (lazy REST encode + diff-aware `sync_model_to_ydoc_diffed` with a
  positional `last_synced` cache in `CollabClient`): both yrs passes are gone.
  What remains (commit `87ddbed` baseline) is one ~77% chain whose hot half
  bottoms out in a single self-heavy, allocator/copy-shaped leaf at ~37%. Its
  feeders are the dense-Node *lifecycle* this design covers — per commit:

| Per-commit pass | Anchor | Cost |
|---|---|---|
| Full-tree rebuild | `build_doc_with_sheets` → `build_table_from_engine` (`frontend/src/components/spreadsheet_view/persistence.rs`) | O(R×C); two `format!` blockId strings + 2–3 `HashMap`s per cell |
| Existing-doc clone | `persist` closure, `spreadsheet_view.rs:1616` (`s.doc.clone()`) | O(doc) deep clone |
| Normalize clone | `normalize_doc` inside `sync_model_to_ydoc_diffed` (`yrs_bridge.rs:215`), per send flush; plus `state.doc.clone()` in the debounce Effect (`pages/document.rs:776`) | O(doc) |
| Structural hash | `Node::structural_hash` in the send Effect (`pages/document.rs:766`), per keystroke-commit | O(doc) walk |
| Engine resync | `sync_engine_from_doc_sheet` via the editor_state Effect (`spreadsheet_view.rs:2263`) — re-runs on the state change `persist()` itself just produced | O(R×C) parse + formula recompute |

For the 300×27/20-used reference sheet, one cell commit walks ~8,100 cells five
times (~40k node visits) and performs ~50k short-lived allocations to change one
string.

## Current state

**Already sparse:** `SpreadsheetEngine` (`frontend/src/spreadsheet/eval.rs`)
stores raw values, formulas, computed values, and styles as
`HashMap<CellAddr, …>`. The grid render reads the engine, not the doc.

**Already incremental:** `sync_model_to_ydoc_diffed` (`yrs_bridge.rs:210`) skips
subtrees equal to the cached `last_synced` doc; a one-cell commit touches one
paragraph's yrs subtree. The skip is safe under stale caches (equality with
`last_synced` means no local change to contribute).

**Dense:** the persisted doc shape (`Table` → `TableRow` per row → `TableCell` +
`Paragraph` per cell, every cell of the extent) and every walker of it:
`build_table_from_engine`, `sync_engine_from_doc_sheet`, `snapshot_foreign_doc`,
`normalize_doc`, `structural_hash`, `read_doc_from_ydoc`, backend
`crates/collab/src/export.rs` (`to_csv`/`to_xlsx`/`to_docx` iterate the yrs XML
tree positionally) and `import_spreadsheet.rs` (writes the same tree).

**Anchoring facts (verified):**

- Cell comments use a *synthetic* deterministic id `cell-s{sheet}r{row}c{col}`
  (`spreadsheet_view/cell_comment.rs::cell_block_id`) — a pure function of
  coordinates, never resolved against the Node tree.
- Remote-cursor awareness uses the synthetic `ss:{sheet}:c:{r}:{c}` shape, parsed
  by `parse_ss_block_id` (`spreadsheet_view.rs:1007`) without consulting the doc.
- The position-derived blockIds *inside* the doc (`ss:{sheet}:c:{r}:{c}`,
  `ss:{sheet}:p:{r}:{c}`, `ss:{sheet}:r:{r}`, written in
  `build_table_from_engine`) exist for one consumer: `find_match` in
  `yrs_bridge.rs`, which gives yrs sync stable structural identity across saves.

So **neither comments nor cursors require empty cells to be materialized**. The
addressing-stability constraint reduces to: materialized cells must keep the
`ss:` blockId scheme so `find_match` stays incremental, and coordinate-derived
anchors must remain valid for unmaterialized cells (they already are).

## Constraints (from #122 / standing policy)

1. yrs collaborative editing must not be impaired (correctness over speed).
2. Persisted-doc / wire-shape changes are breaking by default; existing
   DynamoDB/S3 snapshots and live peers need a migration or dual-read story.
3. Schema duality: `crates/collab/src/schema.rs` (canonical; `cross_schema_*`
   CI tests in its `tests` module) ↔ `frontend/src/editor/schema.rs`.
4. Callers to audit: backend `export.rs` / `import_spreadsheet.rs`,
   markdown-table paste, pivot spill (`ATTR_SPILLED`), the `868032e` truncation
   guard, `snapshot_foreign_doc` cross-doc references.

## Candidates

### (a) Sparse child list with explicit row/col attrs

**Shape.** `Table` keeps only non-empty rows (`row="r"` attr); each row keeps
only used cells (`col="c"` attr). "Used" = raw value, style, spilled marker, or
merged-region membership. BlockIds unchanged (`ss:{sheet}:c:{r}:{c}`). A version
attr on the table (e.g. `ssv="2"`) marks the sparse shape.

**CRDT merge.** Same XML-element-per-cell structure, so `find_match` blockId
matching still drives the diff. Concurrent edits to *different* cells merge
cleanly (distinct blockIds). Two peers concurrently materializing the *same*
empty cell insert duplicate siblings with the same blockId; the next local sync
collapses them (`block_id_map` keeps one, `remove_unmatched` deletes the other)
— same-cell conflict degrades to LWW, which is today's semantics too. Concurrent
materialization at different positions can make reused indices non-monotonic and
trip `apply_actions`' slow path occasionally (bounded, correct).

**Anchoring.** Unchanged — comments and awareness are coordinate-synthetic.
`sync_engine_from_doc_sheet`, `snapshot_foreign_doc`, backend `to_csv`/`to_xlsx`
must read `row`/`col` attrs instead of positional enumeration.

**Schema duality.** No new NodeType or tag; new well-known attrs on
`table_row`/`table_cell`. Both schema files document the attrs; the
`cross_schema_*` tests change little (they assert tags/marks, not attrs) — a new
duality test asserting the attr names should be added.

**Migration.** Dual-read plus lazy upgrade on first write (the #92
blockId-healing precedent): readers accept both shapes (no `row`/`col` attrs ⇒
positional v1); the first persist from an upgraded client writes sparse. Old
clients would mis-place sparse cells, so write-enable only after a release in
which all readers understand `ssv="2"`. Mixed-version live collab during that
window is the risk to test. No offline S3/DDB rewrite needed.

**Perf (300×27, 20 used).** Per commit: rebuild/clone/hash/resync walk ~20 cells
(~100 node visits) instead of ~8,100 — per-commit work drops ~99%. Per load:
O(used). Mid-sheet holes handled (unlike (b)).

**Caveats.** `normalize_doc` removes empty `Table`s — an all-empty sparse sheet
would vanish; needs an exception (keep tables bearing `sheetName`/`ssv`), a
behavior change with its own tests. Row/col insert-delete still rewrites
shifted blockIds (same as today). **Risk: medium** — wire-shape change with
dual-read; ~6 reader/writer sites; two schema files + CI test; collab e2e
required.

### (b) Trim-only (no schema change)

**Shape.** Keep the dense logical model, but persist only the used bounding box:
stop passing the view-scroll `grid_rows`/`grid_cols` into
`build_table_from_engine` and clamp to a `last_used_cell` extended to cover
styles, merges, spills, and validations (today `last_used_cell`
(`spreadsheet_view.rs:474`) only inspects raw values — using it as-is would drop
formatting-only cells). The view extent is kept via small `gridRows`/`gridCols`
attrs on the `Table` node (O(1), backward-compatible — no empty-cell
materialization); the client seeds its view extent from them on load, falling
back to `max(doc_extent, 10)` when absent, exactly as now.

**CRDT merge / anchoring / schema.** Unchanged — same dense shape, smaller.
Trailing-row removal is an ordinary diff `sync_model_to_ydoc_diffed` already
emits minimally. No migration: old docs shrink on first commit from an upgraded
client; old clients read trimmed docs fine.

**Perf.** Per commit and per load: O(bounding box of used cells). For the
reference sheet (20 cells in, say, a 10×5 box): ~50 cells per pass vs 8,100 —
most of the real-world win, because the observed bloat is trailing scroll
extent. No help for genuine mid-sheet holes (a used cell at Z300 keeps the box
large).

**Risk: low.** One writer function + a sharper "used" predicate + regression
tests (must not regress the `868032e` data-loss guard or merged/styled-cell
round-trips).

### (c) yrs::Map keyed by cell address

**Shape.** Replace the XML table with a dedicated yrs `Map` root (e.g.
`ss:{sheet}`), key `"r:c"`, value a serialized `CellData` (text, formula, style,
spilled). Sheet-level attrs (pivots, merges, frozen panes…) in a sibling map.

**CRDT merge.** Native per-key LWW map semantics — the theoretically right
structure for sparse cells; concurrent distinct-cell edits never interact, no
duplicate-sibling transient. Loses yrs *text* merging within a cell (cell value
becomes an atomic register) — acceptable for cells, but a real semantics change.

**Anchoring.** Synthetic anchors unaffected, but the doc model no longer
contains the sheet at all: `read_doc_from_ydoc`, `EditorState`, comments-pane
block lookup, search indexing, backend `export.rs`/`import_spreadsheet.rs`,
`snapshot_foreign_doc`, REST content endpoints — every consumer of "the document
is one XML fragment named `content`" needs a second code path.

**Schema duality.** New persisted root + value encoding: both schema files, new
CI tests, a documented wire format. Largest possible surface.

**Migration.** True format break: dual-read plus a v1-table → v2-map converter
on first edit, with a long tail of v1-only readers (server-side export must
understand both forever, or snapshots get rewritten offline).

**Perf.** Per commit O(1); per load O(used). Best asymptotics; also the largest
WS-payload and snapshot-size reduction.

**Risk: high.** Breaks doc/editor uniformity; touches L2–L5; weeks of work.

### (d) Rc / copy-on-write structural sharing in the Node model (orthogonal)

**Shape.** `Fragment.children: Vec<Node>` → shared children (e.g.
`Rc<[Node]>` or `Node::Element{ content: Rc<Fragment> }`) with
`Rc::make_mut`-style CoW on edit. Pure in-memory change: persisted bytes, wire
shape, schemas, and CRDT semantics are untouched.

**Effect.** `existing.clone`, `normalize_doc`'s rebuild of unchanged subtrees,
and the debounce `doc.clone()` become O(changed-path) pointer bumps — the ~37%
allocator/copy-shaped leaf is mostly these clones — and `structural_hash` can
memoize per shared subtree. Does *not* fix `build_doc_with_sheets` (constructs
fresh nodes from the engine each commit; would need to reuse the previous
table's unchanged rows to benefit) or `sync_engine_from_doc_sheet`.

**Risk: medium-high for its payoff.** `Node` is the editor's central type; wide
mechanical refactor (`Rc` is `!Send` — fine in WASM; no backend duality impact
since `crates/collab/src/schema.rs` has no Node type). "Tests are immutable"
applies — any test edit flags a behavior change.

### (e) Skip engine resync for engine-originated changes (orthogonal, small)

The Effect at `spreadsheet_view.rs:2263` re-parses the whole doc back into the
engine on every `editor_state` change — including the one `persist()` itself
just emitted, where the doc was built *from* the engine and the resync is
redundant by construction. Fix: a generation flag set by `persist()` and
consumed by the Effect (the `remote_update_flag` pattern in
`pages/document.rs`); remote updates and sheet switches still resync. Removes
one full O(R×C) parse + recompute per commit. No shape, schema, or CRDT impact.
**Risk: low** — the subtlety is not skipping legitimate remote echoes (two-peer
e2e check).

## Interaction analysis

- **(b), (d), (e) are mutually independent and combine with anything.** (b)+(e)
  alone remove the extent multiplier and one of the five passes with zero wire
  impact.
- **(b) then (a):** (a) strictly subsumes (b)'s win and adds mid-sheet-hole
  handling; (b) first still pays off because it ships in days and shrinks the
  docs (a) will later migrate.
- **(d) after (b):** once passes are O(bounding box), the clone share of the
  profile may drop below the threshold where a Node-wide refactor is justified —
  measure before committing.
- **(c) is the end-state shape** if spreadsheets outgrow the document-uniform
  model (virtualized 10k-row sheets); it supersedes (a). (a) first is not
  wasted: its reader/writer audit is the same audit (c) needs.

## Recommendation

**Stage 1 (now, no wire change): (b) + (e).** Trim-only persist with a
style/merge/spill-aware used-extent predicate, plus the engine-resync skip.
Re-profile on ogrenotes.example.com against the #122 baseline.

**Stage 2 (next, deliberate wire change): (a)** sparse child list with
`ssv="2"`, behind a **capability bar**: the server advertises a minimum client
capability for `ssv="2"` docs (or the WS handshake rejects old clients on such
docs), so stale clients fail loudly instead of silently mis-reading sparse docs
and persisting data loss. Dual-read still ships one release ahead, but the bar
is the safety backstop, not dual-read alone. Plus lazy upgrade on first edit
and reader updates in `sync_engine_from_doc_sheet`, `snapshot_foreign_doc`,
backend `export.rs`, and XLSX import (the `normalize_doc` exception lands
earlier, in ticket ①). Two-peer collab e2e (including a mixed dense/sparse
session) before merge.

**Hold (d)** pending the Stage 1 re-profile; pursue only if clone costs still
dominate. **Decline (c) for now** — its asymptotic edge over (a) (O(1) vs
O(used) per commit) does not justify breaking the single-XML-fragment document
model that export, import, search, comments-pane lookup, and the REST content
path all assume; revisit if a virtualized large-sheet milestone lands.

Proposed tickets: ① (b) trim-only persist + used-extent predicate +
`gridRows`/`gridCols` view-extent attrs + the `normalize_doc`
empty-spreadsheet exception with targeted tests for both modes; ② (e) resync
skip; ③ re-profile gate; ④ (a) implementation (dual-read release, then
write-enable release, plus the backend capability check for `ssv="2"` docs —
**cross-crate**: touches `crates/` as well as the frontend); ⑤ optional (d)
spike, gated on ③.

## Decisions (project owner, 2026-06)

1. **Mixed-version collab window for (a): capability bar, not dual-read
   alone.** The server advertises a minimum client capability for `ssv="2"`
   docs (or the WS handshake rejects old clients on such docs), so stale
   clients fail loudly instead of silently mis-reading sparse docs and
   persisting data loss. Dual-read still ships one release ahead, but the bar
   is the safety backstop. This adds a backend capability check to ticket ④ —
   cross-crate (touches `crates/`).
2. **Trim-only (b) keeps the view extent** via a small `gridRows`/`gridCols`
   attr on the `Table` node — backward-compatible, no empty-cell
   materialization. Folded into ticket ①.
3. **normalize_doc exception is bundled into ticket ①**, not a separate
   ticket: empty spreadsheet `Table`s bearing `sheetName` survive;
   document-mode empty tables keep being removed. Targeted tests for both
   modes ship with ①.

Still open: backend export under (a) — learn row/col attrs (dual-read
forever) vs materialize density on read — and whether Stage 1 closes #121's
residual chain (the ticket ③ re-profile gate decides whether Stage 2 stays
"next" or becomes "when sheets grow").
