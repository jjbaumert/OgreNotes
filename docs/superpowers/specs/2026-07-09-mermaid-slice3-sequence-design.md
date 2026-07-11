# Mermaid slice 3 — sequence diagrams

Status: approved 2026-07-09. Parent design:
`docs/superpowers/specs/2026-07-08-mermaid-support-design.md` (slice table).
Predecessors: slice 1 (pipeline + pie), slice 2 (flowcharts + layered layout
engine — NOT used here; sequence layout is bespoke and simpler).

## Goal

`ogrenotes_mermaid::render()` renders `sequenceDiagram` sources to SVG.
Everything rides the existing pipeline — no schema, frontend, or export
changes: the moment the `Sequence` arm returns SVG instead of "not yet
supported", every surface (live view, modal preview, HTML/PDF export,
read-only link-share) picks it up.

## Scope

- **Participants**: `participant A`, `participant A as Alice Smith`,
  `actor B` (stick-figure rendering), aliases; implicit creation on first
  message reference (display = id). Column order = order of first
  appearance.
- **Messages**: all six arrow forms — `->` (solid, no head), `-->` (dotted,
  no head), `->>` (solid, arrowhead), `-->>` (dotted, arrowhead), `-x` /
  `--x` (cross head), `-)` / `--)` (async open head); message text after
  `:`; self-messages (loop-back shape); `+`/`-` activation shorthand
  suffixes on the target/source.
- **Activations**: `activate A` / `deactivate A` statements and the
  shorthand; nested activations stack with a small x-offset per depth.
  Deactivating a non-active lifeline is a per-line error (mermaid.js
  errors too).
- **Notes**: `Note left of A: text`, `Note right of A: text`,
  `Note over A: text`, `Note over A,B: text` (spanning).
- **Fragments**: `loop label`, `alt label` / `else label`, `opt label`,
  `par label` / `and label`, `critical label`, `break label`, closed by
  `end`; nested to `MAX_FRAGMENT_DEPTH`. `else` outside `alt`, `and`
  outside `par`, `end` with no open fragment, and EOF with unclosed
  fragments (error at the opening line) are per-line errors.
- **`autonumber`**: prefixes messages with 1., 2., … from that point on.
- **Comments** `%%`, blank lines, and the `sequenceDiagram` header line.

`<br/>`-multiline display names, message text, and note text are all IN
scope (the shared `measure` module already handles line splitting).

Explicitly out of scope (clear per-line parse error, never silent partial
render): `box`/`end` participant grouping, `create`/`destroy`, `rect`
background highlighting, `links`/`link`/`properties`, `par over`; any
other unknown statement errors with its keyword named.

## Architecture — `crates/mermaid/src/sequence/`

Mirrors `flowchart/`'s structure. Shares `crate::escape_xml` and the text
measurement module — **`measure` is promoted from `flowchart::measure` to a
crate-level `crate::measure`** (flowchart re-exports or updates its `use`
paths; no behavior change; all existing tests stay green unchanged).

### `sequence/parse.rs`

`parse(source) -> Result<SeqDiagram, ParseError>` (1-based error lines,
first error wins, never panics — UTF-8 slice discipline as audited in
slice 2):

```rust
pub(crate) struct Participant { pub id: String, pub display: String, pub is_actor: bool }
pub(crate) enum LineStyle { Solid, Dotted }
pub(crate) enum Head { None, Arrow, Cross, Async }
pub(crate) enum Event {
    Message { from: usize, to: usize, line: LineStyle, head: Head,
              text: String, activate_target: bool, deactivate_source: bool },
    Note { placement: NotePlacement, text: String },       // LeftOf(p) | RightOf(p) | Over(a, Option<b>)
    FragmentOpen { kind: FragmentKind, label: String },    // Loop|Alt|Opt|Par|Critical|Break
    FragmentDivider { label: String },                     // else / and
    FragmentClose,
    Activate { p: usize },
    Deactivate { p: usize },
    Autonumber,
}
pub(crate) struct SeqDiagram { pub participants: Vec<Participant>, pub events: Vec<Event> }
```

### `sequence/layout.rs`

Pure, unit-testable, deterministic, never panics. Two passes:

1. **Columns**: initial x by first-appearance order; minimum gap between
   adjacent lifelines; each adjacent pair widened to fit the widest
   message/note label that spans it (notes `over A,B` span multiple
   columns; `left of`/`right of` extend the canvas margins as needed).
   Participant box width from `measure::text_size(display)`.
2. **Rows**: walk events top-down; each message/note advances y by a
   content-derived row height (self-messages and multi-line texts take
   extra); fragment opens reserve a frame-top strip (label tab) and pushes
   the nesting stack; closes pop and record the frame rect (frames pad
   horizontally by depth so nested frames inset visibly); activation
   spans recorded as (participant, depth, y-start..y-end); autonumber is
   a counter, not geometry.

Output `SeqLayout { columns: Vec<f64>, rows: …, messages: …, notes: …,
activations: …, frames: …, size }` — plain data for svg.rs.

**Caps** (server-side rendering of untrusted content; bounded by
construction): `MAX_PARTICIPANTS = 50`, `MAX_EVENTS = 1000`,
`MAX_FRAGMENT_DEPTH = 16` — over-cap → "diagram too large" ParseError.
`render()`'s existing `MAX_SOURCE_LEN` gate applies before any parsing.

### `sequence/svg.rs`

Document order: defs (existing `mmd-arrow` marker + new `mmd-cross`,
`mmd-async` markers) → fragment frames (outermost first) → lifelines
(dashed vertical lines) → activation rects → messages (+ autonumber
prefixes) → notes → participant boxes top AND bottom (mermaid default),
stick figure for `actor`. Theme: `currentColor` text/strokes,
`var(--mermaid-node-fill, #ececff)` participant boxes,
`var(--mermaid-note-fill, #fff5ad)` notes,
`var(--mermaid-cluster-fill, #7773)` frame label tabs. Every user string
(display names, message text, note text, fragment labels) through
`crate::escape_xml`; ids never emitted.

### Integration

`lib.rs`: `DiagramKind::Sequence` arm → `sequence::render_sequence(source)`.
Sequence/State/Class/Er note in the "not yet supported" arm shrinks to
State/Class/Er.

## Error handling

Same contract: never panics; exactly one of svg/error; line numbers on
parse and structural errors; raw source never lost (callers' fallback).

## Testing

- **Parser table tests**: every arrow form; aliases; actor; implicit
  participants; `+`/`-` shorthand; every fragment kind + nesting +
  unbalanced/misplaced errors (else outside alt, and outside par, end
  unmatched, EOF-unclosed at opening line); note placements incl.
  spanning; autonumber; out-of-scope statement errors with keyword named;
  UTF-8 adversarial inputs (multi-byte whitespace, emoji in labels).
- **Layout unit tests**: column widening by message width; adjacent-pair
  independence; activation stacking depths; frame containment (every
  frame strictly contains its rows; nested frames strictly inside
  parents); self-message extra height; monotone row y's; determinism.
- **E2E structural SVG tests** (~8 canonical diagrams via `render()`):
  arrows/markers present per kind, note fill, frame label tabs, actor
  stick figure, autonumber prefixes, escaping (`<script>` in message
  text), top+bottom participant boxes.
- **Property test** (dev-dep proptest, existing pattern): arbitrary
  event sequences (valid participant indices, balanced-or-not fragments
  pre-filtered to valid) through layout → no panic, monotone rows, frame
  containment; plus `render()` XOR invariant on arbitrary source lines.
- **Adversarial additions** to the lib never-panics list (fragment bombs,
  activation churn, 50-participant fan).

## Constraints carried forward

- Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
  wasm32-clean, `license.workspace = true`; deterministic; never panics.
- Tests immutable: slice 1/2 assertions untouched; `measure` promotion
  must not alter any existing test.
- Branch: new worktree off post-merge main (after PR #17 lands).
