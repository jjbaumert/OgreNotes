# Mermaid state polish — `:::` + off-canvas-notes silent misparses, doc-parity adds

Status: approved 2026-07-11. Tracking issue:
[#47](https://github.com/jjbaumert/OgreNotes/issues/47). Third polish
slice (flowchart #32 → PR #44; sequence #45 → PR #46); method and
contract identical. Gap analysis executed at main `9b862c4` against
https://mermaid.ai/open-source/syntax/stateDiagram.html (20 doc
examples + 17 probes: 15/37 rendered, 22 loud errors; only 8 of 20 doc
examples render today).

## Goal

Eliminate the state renderer's two silent misparses (`:::` transition
targets mislabeling edges; notes drawn outside the viewBox) and close
the cheap doc-parity gaps, bringing state diagrams to the
"never silent, errors name their keyword" bar.

## Scope

### 1. Silent-misparse fixes

**`:::` on a transition target → loud error.** The docs' own
`[*] --> Still:::notMoving` renders an edge labeled `::notMoving`
today: `parse_transition`'s `tail.strip_prefix(':')` label branch eats
the first colon of `:::`. Fix: in `state/parse.rs::parse_transition`,
before the label branch, a tail starting with `:::` errors:
`` `:::` class styling is not supported ``. Source-side `:::` already
errors loudly; styling application stays deferred.

**Notes join the canvas.** Today `state/svg.rs::emit` sizes
`viewBox`/`width`/`height` from `l.size` (nodes/edges/clusters only);
notes are a post-layout overlay, so a left-note lands at negative x
(the docs' own example: rect at x≈-216 in a 113-wide viewBox) and
right-notes clip past the right edge — invisible content while
`render()` succeeds. Fix, all inside `state/svg.rs`:

- Hoist the note-rect geometry (the exact formulas the emission loop
  uses today: box = text_size + (16, 12) padding; x from side + node
  half-width + 12 gap; y centered on the node) into a helper used by
  BOTH the pre-pass and the emission loop, so they cannot drift.
- Pre-pass over `g.notes` computes the union of all note rects with
  the layout extents → overall `(min_x, min_y, max_x, max_y)`
  (layout content contributes `(0, 0, w, h)`).
- `width`/`height`/`viewBox` come from the union; viewBox stays
  `"0 0 w h"`.
- If `min_x < 0` or `min_y < 0`, the entire document body (everything
  after `</defs>`) is wrapped in ONE
  `<g transform="translate(dx,dy)">` with `dx = -min(0, min_x)`,
  `dy = -min(0, min_y)` — no per-element coordinate changes. The
  wrapper is emitted only when a shift is needed (deterministic:
  same input → same output).
- Notes may still overlap neighboring elements (accepted v1 overlay
  behavior, unchanged); they can no longer be off-canvas.

### 2. Cheap adds

- **Bare-id statement**: a statement that is a single valid id
  (`stateId`) declares/ensures that node — the docs' intro example.
  Today it errors `expected a transition …, found ""`.
- **Colon description**: `s2 : This is a state description` sets the
  node's display text, same effect as `state "…" as s2` (used in 4 of
  20 doc blocks; today a confusing transition error). A repeated
  description for the same id appends as an additional line joined
  with `<br/>` — the label emitter already renders multi-line
  displays via `measure::lines`.
- **Trailing `%%` comments**: strip from the first `%%` that is at
  start-of-statement or preceded by whitespace, before statement
  parsing (`Moving --> Still %% comment` is doc-blessed and errors
  today). A transition label containing a literal `%%` loses the
  remainder — matching mermaid's comment handling.
- **Named errors** for `classDef`, `class`, `accTitle`, `accDescr`:
  keyword match in `parse_statement` producing
  `` `classDef` statements are not supported `` (etc.), replacing the
  misleading transition-parser fallthrough messages.
- **Notes on synthetic ids error loudly**: `note right of __start_0`
  currently mints a phantom node (filed on #32). Note targets matching
  the reserved synthetic pattern (ids beginning `__`) error:
  `cannot attach a note to a synthetic state id`. (User-declared ids
  cannot begin with `__`? They CAN today — the id charset allows `_`.
  The check therefore rejects only the reserved prefix `__`, which the
  synthesizer alone should produce; a user who deliberately names a
  state `__x` gets the loud error, which is acceptable and documented
  here.)

### 3. Gallery promotion

`crates/mermaid/examples/state_gallery.rs` (untracked in the
`mermaid-seq-gap` analysis worktree) is committed as a crate example —
same shape as `doc_gallery.rs` / `seq_gallery.rs` — with post-fix
expectation notes: bare-id, colon-description, and comments doc
examples flip to `match`; note examples flip to `match` (on-canvas);
the `:::` doc blocks still error but now name `:::` (or `direction`,
whichever line comes first — notes must state which); composites,
concurrency, `direction`, multi-line notes keep loud-error notes. The
`mermaid-seq-gap` analysis worktree is removed after the copy (its
`seq_gallery.rs` original is already committed on main).

## Out of scope (stays on issue #47's honest-error list)

Composite-as-endpoint transition routing (all three doc composite
examples — the state-side analog of flowchart edge-to-subgraph,
deferred with it); concurrency `--` regions; `direction` statements;
multi-line `note … end note` blocks; classDef/class/`:::` styling
APPLICATION; choice-diamond small-and-empty cosmetics; front-matter
`title:` rendering.

## Error handling / invariants (unchanged contract)

`render()` never panics; exactly one of svg/error; 1-based error
lines; first error wins; unsupported constructs error naming their
keyword; every user string through `escape_xml`; deterministic; caps
unchanged. UTF-8 discipline: new probes (`:::`, `%%`, `__`) are ASCII;
boundary-safe slicing commented per crate convention.

## Testing

- Parser tables: bare id (new node; existing node no-op), colon
  description (set; repeat appends `<br/>`; `state "x" as y` then
  colon description composes), `:::` target error (docs' exact input)
  and source error unchanged, named keyword errors (all four), `%% `
  trailing strip (full-line still works; label keeps text before
  `%%`; `%` alone in a label survives), synthetic-id note rejection
  (`__start_0`), plus `note right of` a normal id still works.
- SVG structural: left-note and right-note rect x/y within
  `0..width` / `0..height`; translate wrapper present exactly when a
  left/top overflow exists and absent otherwise; note text visible
  (x > 0); existing `note_renders` untouched.
- Props soup (`state/props.rs`): add statement variants (bare id,
  colon description, `:::` target, trailing `%%`) and extend the
  noise alphabet with `:` `%`.
- Existing tests are immutable; additions only. (No existing test
  asserts the `:::` label, off-canvas note geometry, or the misleading
  fallthrough messages — verified during the gap analysis.)
- Acceptance sweep: `cargo run -p ogrenotes-mermaid --example
  state_gallery` — every case on its documented side.

## Constraints carried forward

Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
wasm32-clean, deterministic. Branch: worktree
`worktree-mermaid-state-polish` off post-#46 main (`9b862c4`), PR to
main, squash-merge. PR title references "issue #47 highlights" — no
"closes" phrasing (the #32 auto-close lesson).
