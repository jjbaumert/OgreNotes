# Mermaid slice 4 — state, class, and ER diagrams

Status: approved 2026-07-10 (Approach A). Parent design:
`docs/superpowers/specs/2026-07-08-mermaid-support-design.md` (slice table).
Predecessors: slice 1 (pipeline + pie), slice 2 (flowcharts + layered
layout engine), slice 3 (sequence diagrams). This is the final slice of
the approved scope.

## Goal

`ogrenotes_mermaid::render()` renders `stateDiagram-v2`/`stateDiagram`,
`classDiagram`, and `erDiagram` sources to SVG. All three are
box-and-arrow graphs laid out by the EXISTING slice-2 layered engine
(`crate::layout`) — no new layout algorithms. Everything rides the
existing pipeline: no schema, frontend, or export changes.

## Scope

### State diagrams (`stateDiagram-v2`, plain `stateDiagram` accepted)
- Simple states (`s1`, `state "Long name" as s1`), display labels.
- Labeled transitions `s1 --> s2: event`.
- `[*]` start/end pseudo-states (synthetic nodes: filled circle for
  start, ring for end; a fresh synthetic node per `[*]` occurrence in
  the role it appears — start when source, end when target).
- Composite states `state X { … }`, nested — mapped onto the layout
  engine's CLUSTER machinery (collapse-expand, proven at depth).
- `<<choice>>` (diamond) and `<<fork>>`/`<<join>>` (bars) stereotypes
  on `state` declarations.
- `note left of X: text` / `note right of X: text` (rendered as note
  boxes adjacent to the state's laid-out position; single-line
  placement rule: a synthetic note node connected by an invisible edge
  is NOT used — notes are positioned post-layout beside their state).
- Out of scope (per-line error): concurrency regions `--`, history
  states (`[H]`, `[H*]`), `direction` statements, transitions with
  guards/actions beyond the plain `: label` text.

### Class diagrams (`classDiagram`)
- Class declarations (`class Name`, inline member blocks
  `class Name { +field type; +method(arg) ret }` and dotted forms
  `Name : +member`).
- Three-compartment box rendering: name (+ optional `<<annotation>>`
  line), attributes, methods — split by mermaid's rule (entries with
  `(` are methods).
- Visibility markers `+ - # ~` rendered verbatim; generics `~T~`
  rendered literally as typed (no font styling in v1).
- All six relationship kinds with distinct markers/line styles:
  - `<|--` inheritance (hollow triangle, solid), `<|..` realization
    (hollow triangle, dashed)
  - `*--` composition (filled diamond), `o--` aggregation (hollow
    diamond)
  - `-->` / `--` association (open arrow / plain, solid), `..>`
    dependency (open arrow, dashed)
- Multiplicities (`"1" -- "many"`) and relationship labels (`: label`).
- Out of scope (per-line error): namespaces, `<<static>>`/abstract
  member markers (the annotation form on the CLASS is in scope; member
  modifiers `$`/`*` render verbatim rather than styled), two-way link
  syntax, `style`/`cssClass`/`callback`/`click`/`note for`.

### ER diagrams (`erDiagram`)
- Entities with attribute blocks:
  `CUSTOMER { string name PK "comment-out-of-scope" }` — rows are
  `type name [PK|FK]` (a trailing quoted comment is a per-line error,
  matching the "never silent" principle).
- Entity boxes rendered as a title bar + attribute grid (columns:
  type, name, key markers).
- Relationships with full crow's-foot cardinalities on BOTH ends:
  `||` exactly-one, `|o` zero-or-one, `}|` one-or-more, `}o`
  zero-or-more — e.g. `CUSTOMER ||--o{ ORDER : places`.
- Identifying (`--`, solid) vs non-identifying (`..`, dashed) lines;
  relationship labels required by mermaid's grammar (`: label`).
- Out of scope (per-line error): attribute comments (above), UNIQUE
  key marker, multi-key combinations beyond PK/FK, aliased entities.

## Architecture — Approach A

Three sibling modules mirroring the established pattern, plus one thin
shared adapter:

```
crates/mermaid/src/
  boxgraph.rs        shared adapter: measured nodes + typed edges +
                     optional clusters + direction → crate::layout::run
                     → Layout (error → ParseError{line:None}); ~40 lines;
                     exactly what flowchart/mod.rs does inline today
                     (flowchart is NOT refactored onto it in this slice —
                     no churn in shipped code; a follow-up may unify)
  state/mod.rs       model + render_state entry
  state/parse.rs     grammar → StateGraph
  state/svg.rs       state shapes (rounded boxes, start/end circles,
                     choice diamond, fork/join bars), cluster boxes for
                     composites, note boxes, transition edges
  class/mod.rs       model + render_class entry
  class/parse.rs     grammar → ClassGraph
  class/svg.rs       compartment boxes, six relationship markers,
                     multiplicity + label text
  er/mod.rs          model + render_er entry
  er/parse.rs        grammar → ErGraph
  er/svg.rs          entity grids, crow's-foot marker pairs
                     (marker-start + marker-end per cardinality symbol)
```

- Node sizes from `crate::measure` over compartment/attribute lines
  (max line width per compartment; box = padded max across
  compartments; grid rows uniform height `LINE_H + padding`).
- Edge labels/multiplicities ride the layout engine's existing
  edge-label reservation (label size on `LEdge`); multiplicity end
  labels are placed post-layout near the edge endpoints (offset along
  the final segment), not via dummy nodes.
- `lib.rs`: three arms flip to `state::render_state` /
  `class::render_class` / `er::render_er`. `detect_kind` already
  recognizes all three keywords (and strips a trailing `;`).
- Caps: inherited from the layout engine (MAX_NODES 400, MAX_EDGES
  1000, MAX_DUMMY_SLOTS 20k) + `MAX_SOURCE_LEN` in `render()`; parsers
  additionally bail early past `crate::layout::MAX_EDGES` on relationship
  fan-out, matching the flowchart precedent.

## Error handling

Unchanged contract: `render()` never panics; exactly one of svg/error;
1-based error lines; first error wins; unknown/out-of-scope statements
error naming their keyword; raw source never lost. UTF-8 slice
discipline per the slice-2/3 conventions (ASCII-predicate counting or
boundary-safe splits only).

## Consequences for existing tests (planned, sanctioned)

Flipping the last three kinds empties the "not yet supported" arm to
`Unknown` only:
- `lib.rs::each_unsupported_kind_error_names_its_label` is RETIRED
  (deleted with a rationale comment referencing this spec) — its
  target arm has no variants left, exactly as slice 3's review
  predicted.
- `crates/collab/src/export.rs::mermaid_html_falls_back_to_raw_source_on_error`
  switches its fixture to a permanently-invalid source (unknown-kind
  gibberish), ending the per-slice churn — its purpose is "any error
  falls back", which survives all future kind additions.
- `lib.rs::unsupported_kind_returns_error_with_kind_preserved` is
  RETIRED with the same rationale (same emptied arm).

## Testing

- Per-family parser table tests (every statement form, every
  relationship operator/cardinality, stereotypes, nested composites,
  out-of-scope errors with line numbers, UTF-8 adversarial inputs).
- Layout-mapping tests per family (composite → cluster indices;
  synthetic [*] nodes; edge kind → marker/line-style mapping tables).
- E2E structural SVG tests (~6 per family via `render()`): boxes/
  compartments/grids present, correct markers per relationship kind,
  crow's-foot pairs on both ends, escaping (`<script>` in labels),
  viewBox sanity.
- Adversarial never-panic additions per family; one statement-soup
  property per family (256 cases) asserting the XOR invariant.
- The two fixture retirements + one new permanently-invalid-fixture
  test as above.

## Constraints carried forward

- Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
  wasm32-clean; deterministic; never panics; tests immutable except
  the three sanctioned retirements/switches listed above.
- Branch: worktree `worktree-mermaid-state-class-er` off post-slice-3
  main (created).
