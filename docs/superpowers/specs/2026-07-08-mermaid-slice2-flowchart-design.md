# Mermaid slice 2 — flowchart + layered layout engine

Status: approved 2026-07-08 (Approach A). Parent design:
`docs/superpowers/specs/2026-07-08-mermaid-support-design.md` (slice table).

## Goal

`ogrenotes_mermaid::render()` renders `graph` / `flowchart` sources to SVG,
with layout quality comparable to mermaid.js for the diagram sizes that occur
in documents. Everything rides slice 1's pipeline unchanged — no schema,
frontend, or export edits: the moment the `Flowchart` arm returns SVG instead
of "not yet supported", every surface (live view, modal preview, HTML/PDF
export, read-only link-share) picks it up.

## Scope

Full flowchart language coverage:
- Directions: `TD`/`TB`, `BT`, `LR`, `RL`.
- Node shapes: rectangle `[..]`, rounded `(..)`, stadium `([..])`, circle
  `((..))`, double circle `(((..)))`, diamond `{..}`, hexagon `{{..}}`,
  parallelogram `[/../]`, reverse parallelogram `[\..\]`, trapezoid `[/..\]`,
  reverse trapezoid `[\../]`, cylinder `[(..)]`, asymmetric flag `>..]`.
- Edges: arrow `-->`, open `---`, dotted `-.->`, thick `==>`; labels via
  `|text|` and `-- text -->` forms; chains (`A --> B --> C`); multiple edges
  per statement (`A & B --> C`).
- Subgraphs: `subgraph id [title]` … `end`, nested, with edges crossing
  cluster boundaries.
- Styling: `classDef`, `class`, `:::` inline class references.
- `%%` comments.

Explicitly out of scope (clear per-line parse error, never silent partial
render): `click`, `linkStyle`, `style`, `accTitle`/`accDescr`, markdown in
labels, `direction` statements inside subgraphs (v1 subgraphs inherit the
graph direction), auto text wrapping (explicit `<br/>` only).

## Architecture — two independent modules

### `crates/mermaid/src/layout/` — mermaid-agnostic layered engine

The reusable asset (slice 4's State/Class/ER consume it untouched).

**Input** (crate-internal API):
```rust
pub(crate) struct LayoutInput {
    pub nodes: Vec<LNode>,        // id = index; (width, height); Option<cluster id>
    pub edges: Vec<LEdge>,        // from, to, Option<(label_w, label_h)>
    pub clusters: Vec<LCluster>,  // Option<parent>, (title_w, title_h)
    pub direction: Direction,     // TB | BT | LR | RL
}
```
**Output**: `Layout { node_centers: Vec<(f64,f64)>, edge_paths: Vec<EdgePath>,
cluster_rects: Vec<Rect>, size: (f64,f64) }` where `EdgePath` carries a
polyline, a label anchor, and the true (pre-reversal) direction for arrowheads.

**Invariants**: never panics; no NaN/infinite coordinates; deterministic
(stable sorts, index tiebreaks, no randomness); nodes never overlap; a node's
rank strictly increases along every non-reversed edge.

**Stages** (one file each; all operate in TB space — LR/RL/BT are coordinate
transforms applied last):
1. `acyclic.rs` — DFS back-edge detection; back-edges reversed and flagged.
2. `rank.rs` — longest-path ranking, then pull-up tightening.
3. `order.rs` — dummy-node insertion for multi-rank edges (edge labels become
   sized dummy nodes to reserve space); barycenter down/up sweeps with a
   crossing counter; stop on no-improvement or a fixed iteration cap.
4. `position.rs` — median/priority x-assignment, a few bounded sweeps.
   Deliberately not literal Brandes-Köpf; replaceable behind the stage API.
5. `route.rs` — polylines through dummy positions, corners rounded with
   quadratic béziers; label placed at its reserved slot.

**Clusters — recursive collapse-expand**: each cluster laid out as its own
subgraph, collapsed to a super-node in the parent, expanded in place after;
cross-boundary edges re-route from the cluster border to the true inner node
post-expansion. Accepted v1 trade-off: cross-boundary paths can be less
polished than mermaid.js on dense diagrams; upgradeable inside the module
without API change.

**Safety caps** (untrusted input renders server-side on export): `MAX_NODES =
400`, `MAX_EDGES = 1000`, bounded sweep counts. Over-cap returns a
"diagram too large" `ParseError` — never unbounded CPU.

### `crates/mermaid/src/flowchart/` — parser, measurement, SVG

- `parse.rs` — statement-oriented parser producing
  `FlowGraph { nodes, edges, subgraphs, class_defs, direction }`. Errors are
  `ParseError { message, line }` (1-based), first error wins. Unknown
  statement kinds error per the out-of-scope list above.
- `measure.rs` — char-class width table (narrow / normal / wide / CJK ×2)
  × font-size constant, generous padding; `<br/>` splits lines. Returns the
  (w, h) each node/label contributes to layout.
- `svg.rs` + `shapes.rs` — shape library keyed by shape kind (center + size +
  label in, path/group out); shared `<defs>` arrowhead markers (one per edge
  kind); cluster boxes with title strips behind content; `classDef` styles as
  inline `style` attributes (CSP-allowed, same as slice 1's pie); theme text
  via `currentColor`; every user string through the existing `escape_xml`.

### Integration

`lib.rs`'s `DiagramKind::Flowchart` arm: `flowchart::parse` → `measure` →
`layout::run` → `flowchart::svg::emit`. All failures flow into slice 1's
error + raw-source fallback on every surface. `MAX_SOURCE_LEN` (shared
constant) already caps input size ahead of parsing.

## Error handling

Same contract as slice 1: `render()` never panics; exactly one of
`svg`/`error` is set; parse and capacity errors carry a line number when
known; raw source is never lost.

## Testing

- **Layout unit tests** per stage on abstract graphs: acyclic output has no
  back-edges; rank monotonicity; sweeps never increase crossings; no node
  overlaps; no NaN; determinism (same input twice → identical output);
  cluster containment (members inside their rect, rects nested per tree).
- **Parser table tests**: every shape bracket, every edge variant, both label
  syntaxes, chains, `&` fan-out, nested subgraphs, classDef/class/`:::`,
  comment/blank handling, each out-of-scope statement errors with its line.
- **End-to-end structural tests**: ~10 canonical diagrams asserting SVG
  structure (element counts, marker presence, cluster rect count, label
  text present, viewBox sanity) — not float-exact goldens.
- **Adversarial**: never-panics + svg-XOR-error over hostile inputs (deep
  nesting, cycles, self-loops, duplicate ids, huge labels, over-cap graphs).
- **Property tests** (`proptest` as **dev-dependency only** — runtime stays
  zero-dep; dev-deps don't affect the wasm bundle): arbitrary digraphs with
  random sizes/clusters through the full layout pipeline hold the invariants.

## Constraints carried forward

- Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
  wasm32-clean; `license.workspace = true`.
- The CI WASM bundle-size gate polices added code weight.
- Branch: new worktree off post-merge main (after PR #4 lands).
