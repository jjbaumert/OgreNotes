# Mermaid diagram support — design

Status: approved 2026-07-08. Slice 1 is the first implementation spec;
slices 2–4 get their own specs when reached.

## Goal

Let users author and render Mermaid diagrams inside OgreNotes documents.
Diagrams must render identically in three surfaces: the live editor view,
exported/printed output (HTML/PDF), and read-only link-share views.

## Key decisions

### Pure-Rust SVG renderer, not mermaid.js

Rendering is a pure-Rust source-string → SVG-string transform, mirroring the
existing `frontend/src/components/spreadsheet_chart.rs` pattern (charts are
already rendered as Rust-built SVG, no JS chart library). A plain `<svg>`
string renders in all three surfaces with **zero JavaScript** — which is the
whole reason it satisfies export/PDF/read-only for free, and why a JS library
(CSP-hostile, WASM-bundle bloat) was rejected.

### One shared renderer crate

The frontend depends on **no** workspace crate today: the schema is duplicated
between `frontend/src/editor/schema.rs` and `crates/collab/src/schema.rs`
precisely because `collab` pulls in `tokio`/`yrs`, which do not compile to
wasm32. The renderer must run on **both** sides — client (WASM, live view) and
server (`collab::export`, native) — so it lives in a new standalone workspace
crate `crates/mermaid` (`ogrenotes-mermaid`) with **no WASM-hostile deps**
(std only). Both sides call the same `render()`:

- Frontend (WASM): `editor::blocks::mermaid` renders the in-doc preview.
- Server (native): `collab::export::render_node_html` inlines the SVG.

The Cargo package is `ogrenotes-mermaid`; its Rust import path is
`ogrenotes_mermaid`, used verbatim on both sides.

This is strictly better than duplicating the renderer per-side the way the
schema is duplicated: one renderer, no drift.

**Rejected alternatives:** (a) duplicate the renderer per side — too much code
to keep in sync; (b) render client-side and persist the SVG into the CRDT so
export re-emits it — bloats the document, goes stale, and makes export /
read-only depend on some client having rendered first.

### Block model: dedicated Mermaid block, attribute-stored source

A new `NodeType::Mermaid`, a **block-level leaf atom** like `Embed`, storing
the diagram source in a `source` attribute. It uses the editor's existing
custom-block mechanism `LiveAppBlockView` (`frontend/src/editor/blocks/mod.rs`):
a registered block owns its DOM subtree and the editor does not recurse into
it. Kanban / Calendar / Embed all use this — they render a display projection
and edit via a modal, never inline contenteditable. The Mermaid block follows
Embed exactly: the block renders the SVG preview; editing the source happens in
a modal (code pane + live preview). (An earlier idea to store the source as
inline-editable `CodeBlock`-style text was dropped: it would require a
node-view hybrid the editor does not have.)

### Diagram families (whole feature, across all slices)

Flowchart, Sequence, Pie, and State/Class/ER. Each family is its own
parser + layout. Reimplementing all mermaid diagram types is out of scope;
these four families are the target.

### Error UX

`render()` never panics. On a parse error or an unsupported/not-yet-implemented
diagram kind it returns a structured error and no SVG; every caller then shows
a parse-error banner plus the raw source in a `<pre>` block. Nothing the user
typed is ever lost.

## Decomposition into slices

Each slice is an independent spec → plan → ship cycle.

| Slice | Scope | Rationale |
|---|---|---|
| **1** | Renderer crate skeleton + `render()` contract + full block pipeline (NodeType in both schemas, insert entry, view render, edit modal, export HTML, error+raw fallback) **+ Pie** | Pie is trivial, so it proves the end-to-end pipeline — the pipeline is the risk, not the pie |
| **2** | **Flowchart** + shared layered-graph (dagre-style) layout engine | The big one; the layout engine is the reusable asset |
| **3** | **Sequence** diagrams (bespoke lifeline layout) | Independent of the graph engine |
| **4** | **State / Class / ER** | Reuse slice 2's layout engine; mostly parsing + node-box variants |

---

## Slice 1 — spec

**Goal:** the entire pipeline working end-to-end with Pie as the only real
diagram.

### A. `crates/mermaid` crate (new workspace member)

Pure Rust, `#![forbid(unsafe_code)]`, no deps beyond `std`.

```rust
pub fn render(source: &str) -> RenderOutput;   // never panics

pub struct RenderOutput {
    pub kind: DiagramKind,
    pub svg: Option<String>,      // Some on success
    pub error: Option<ParseError> // Some on failure / unsupported
}
pub enum DiagramKind { Pie, Flowchart, Sequence, State, Class, Er, Unknown }
pub struct ParseError { pub message: String, pub line: Option<usize> }
```

- **Detection:** the first non-blank, non-`%%`-comment line's leading keyword
  maps to a `DiagramKind`. Slice 1 implements **Pie** only; every other *known*
  kind returns `error` = "‹kind› not yet supported" with `svg: None`. `Unknown`
  returns a parse error.
- **Pie parser:** `pie [showData]`, optional `title …`, then `"Label" : value`
  lines (labels may be quoted or bare; values are numbers). Empty data or a
  missing `pie` header is an error.
- **Pie SVG:** self-contained `<svg>` with slices, a legend, and optional raw
  values when `showData` is set. Slice fills use a ported 8-hue palette; text
  uses `currentColor` so it tracks the light/dark theme.

### B. Block model (both schemas, mirrored)

- New `NodeType::Mermaid`: block-level **leaf** atom (mirrors `Embed`). Source
  in a `source` attribute. Added to `Doc.valid_children` in both
  `frontend/src/editor/schema.rs` and `crates/collab/src/schema.rs`; the
  cross-schema CI test's expected node set is updated.
- `is_leaf()` includes `Mermaid`.
- Server validation: new `crates/collab/src/blocks/mermaid.rs` implementing
  `LiveAppBlock` — caps `source` length and rejects empty source.

### C. Frontend

- `frontend/src/editor/blocks/mermaid.rs`:
  - `MermaidView: LiveAppBlockView` — calls `ogrenotes_mermaid::render`; shows
    the SVG on success, or an error banner + `<pre>` raw source on failure.
    The wrapper carries `data-atom-size` and the block registers backspace &
    forward-delete atom handlers (per the known Embed atom-delete gotcha).
  - `MermaidInsert: LiveAppBlockInsert` — `build_default_node` seeds a starter
    pie diagram; registered in `BLOCK_VIEWS` / `BLOCK_INSERTS` so the block
    appears in the slash menu, block-menu, and command palette at once.
- `frontend/src/components/mermaid_modal.rs`: a code textarea + **live preview**
  (debounced `render()` on each edit), mirroring `calendar_modal` /
  `kanban_card_modal`. Save writes the `source` attribute via the transform
  pipeline.
- Fluent keys `insert-mermaid-label` / `insert-mermaid-description` added to
  `frontend/locales/en-US/main.ftl`.
- `frontend/Cargo.toml`: `ogrenotes-mermaid = { path = "../crates/mermaid" }`
  (compiles to wasm32 — pure std).

### D. Export / read-only (server)

- `collab::export::render_node_html`: on `Mermaid`, read `source`, call
  `ogrenotes_mermaid::render`, inline the SVG — or an error banner + `<pre>` raw source
  on failure. This one code path covers HTML / PDF / print **and** read-only
  link-share.
- `collab::export::render_node_markdown`: emit a ` ```mermaid ` fenced block
  carrying the source (round-trips).
- `crates/collab/Cargo.toml` gains the `ogrenotes-mermaid` dependency.

### E. Data flow

Insert → `MermaidInsert::build_default_node` (starter source in `source`) →
transform places the node → `MermaidView::render` shows the SVG preview →
Edit affordance opens `mermaid_modal` (textarea + live preview via `render`) →
save writes the `source` attr via the transform pipeline → CRDT syncs →
export / read-only re-render through the same `render()`.

### F. Error handling

`render()` never panics. Unsupported kinds and parse failures both return a
structured error; both callers (view + export) render the error banner and the
raw source. Nothing is lost.

### G. Testing

- **Crate:** pie parser cases (title, `showData`, quoted + bare labels, zero,
  empty); kind detection per keyword; a **never-panics** test over adversarial
  and oversized input; SVG output contains the expected `<svg …>` header and
  the right slice count.
- **Schema:** cross-schema CI test updated; `LiveAppBlock` source-cap
  validation test (too-long and empty rejected).
- **Export:** `to_html` with a Mermaid node → output contains `<svg`; the
  unsupported/error case → output contains the raw source.
- **Frontend:** an **explicit wasm32 build** with the new dependency (a native
  `cargo check` skips wasm linkage and would not catch a wasm-incompatible
  dep); a `MermaidView` render assertion; atom backspace / forward-delete
  regression tests.

### Deferred / decided

- Markdown **export** emits ` ```mermaid ` for round-trip fidelity. Markdown
  **import** mapping ` ```mermaid ` fenced blocks → a `Mermaid` node is
  **deferred** past slice 1 to keep scope tight (authoring is via the dedicated
  block, not fenced blocks).
- Starter template is a small pie diagram.
