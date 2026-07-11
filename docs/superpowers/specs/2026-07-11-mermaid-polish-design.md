# Mermaid polish — silent-misparse fixes + flowchart doc-parity adds

Status: approved 2026-07-11. Tracking issue:
[#32](https://github.com/jjbaumert/OgreNotes/issues/32) (fix-first +
cheap-adds sections; the issue's deferred list stays deferred). Parent
design: `docs/superpowers/specs/2026-07-08-mermaid-support-design.md`.
Predecessors: slices 1–4 (PRs #4, #17, #26, #31 — all merged).

## Goal

Eliminate the two known **silent misparses** in the flowchart parser
(the defect class our "never silent" contract forbids) and close the
cheap doc-parity gaps identified by the flowchart doc-example matrix,
without touching the deferred/cosmetic items in issue #32.

## Scope

### 1. Unified edge-operator scanner (core change)

Replace the two fixed operator tables in
`crates/mermaid/src/flowchart/parse.rs::parse_edge_op` with one scanner:

```
edge-op := [reverse-terminator] body [terminator]
reverse-terminator := '<' | 'o' | 'x'     (bidirectional / reverse head)
body := run of '-' (open) | '-.-'-style dotted run | '=' (thick)
        | '~' (invisible)
terminator := '>' | 'o' | 'x' | none      (arrow / circle / cross / open)
```

Delivered by that one mechanism:

- **Circle/cross edge ends — SUPPORTED** (fixes silent misparse #1).
  `A--oB`, `A--xB`, `A-->B`, `A---xB`, dotted/thick variants: the
  terminator character is consumed by the operator, so phantom nodes
  `oB`/`xB` become impossible. New `Head::Circle` / `Head::Cross` on
  flowchart edges; `mmd-cross` marker exists (sequence work), circle
  marker is new. Disambiguation matches mermaid: after an operator
  body, a `o`/`x` immediately followed by more id characters is STILL
  the terminator (mermaid draws circle-to-B for `A---oB`); nodes whose
  ids merely start with `o`/`x` remain reachable via every operator
  with an explicit `>` terminator or whitespace (`A--> oB`), same as
  mermaid.
- **Multi-length arrows**: body runs longer than the minimum
  (`---->`, `====>`, `-..->`, `A --text---- E`) collapse to the base
  kind. Mermaid uses extra length as a rank-span hint; we treat all
  lengths equally in v1 — a quiet cosmetic divergence, same accepted
  category as our other layout differences (documented here, not an
  error).
- **Bidirectional `A<-->B`** (also `x--x`, `o--o`, mixed): one edge
  carrying a head at each end, rendered via `marker-start` +
  `marker-end` — the mechanism the ER renderer already uses.
- **Invisible links `A~~~B`**: parsed as a real edge that participates
  in layout but emits no path element (and no markers).
- **No-space label spellings**: `A-.text.-B`, `A==text==>B`,
  `A--text-->B`. The inline-label branch drops its
  must-have-a-space-after-opener requirement and scans to the closing
  operator run instead. Spaced forms keep working unchanged.

### 2. Edge-to-subgraph ids — LOUD ERROR (fixes silent misparse #2)

When an edge endpoint names a known subgraph id (`sgID-->C` after
`subgraph sgID[…] … end`), the statement errors:
`edges to/from subgraph ids are not yet supported (subgraph "sgID")`,
with the statement's 1-based line. Real cluster-boundary edge routing
is layout work and stays deferred (issue #32). Ordering note: subgraph
ids may be referenced by edges written ABOVE the subgraph block in real
mermaid; the check therefore runs as a post-parse validation pass over
all edges against the final subgraph-id set, not inline during
statement parsing.

### 3. Small independents

- **YAML front matter**: `render()` in `lib.rs` strips one leading
  front-matter block (first line exactly `---`, up to the next line
  exactly `---`) before `detect_kind`. Contents are ignored in v1
  (mermaid puts config/theme there, which we don't consume). Line
  numbers in subsequent errors still refer to the ORIGINAL source
  (offset preserved). Unterminated front matter is a loud error.
  Applies to every diagram kind, since it precedes kind detection.
- **`classDef default`**: after parsing, the `default` class's style
  (if defined) applies to every node with no explicit class
  assignment, matching mermaid. Explicitly-classed nodes are
  unaffected. Same style allowlist as every classDef.
- **Subroutine shape `A[[text]]`**: 14th flowchart shape — rectangle
  plus two inner vertical lines inset from the left/right edges.
  Bracket matcher tries `[[` before `[` (the existing longest-first
  pattern).
- **Doc gallery promoted**: `crates/mermaid/examples/doc_gallery.rs`
  (currently untracked working tool) is committed as a crate example,
  with expectation notes updated to the new behavior — the
  silent-divergence cases become rendered-correct or errored-loud.
  `MERMAID-DOC-PARITY.md` and `RESTART-mermaid.md` at old worktree
  roots stay untracked (superseded by issue #32 + this spec).

## Out of scope (stays on issue #32's deferred list)

`@{ shape: … }` v11.3+ syntax; markdown strings; `click`/`linkStyle`/
`style` statements; `direction` inside subgraphs; HTML entity codes
(`#35;`); edge ids (`e1@-->`) and edge animations; ACTUAL
edge-to-subgraph routing; FontAwesome icons; rank-span effect of
multi-length arrows; the slice-2/3/4 deferred cosmetics.

## Error handling / invariants (unchanged contract)

`render()` never panics; exactly one of svg/error; 1-based error lines
(against original source, including under front-matter stripping);
first error wins; unsupported constructs error naming their keyword;
every user string through `escape_xml`; deterministic output; caps
unchanged (MAX_NODES 400, MAX_EDGES 1000, MAX_DUMMY_SLOTS 20k,
MAX_SOURCE_LEN 20k). UTF-8 discipline: the operator scanner consumes
only ASCII characters (`-.=~<>ox`), so char-count == byte-len
reasoning applies at each slice site (commented per convention).

## Testing

- Parser table tests for every new operator spelling: circle/cross on
  every body kind, both ends, multi-length runs, `~~~`, `<-->`,
  no-space label forms — including the three former phantom-node
  sources (`A-->oB`, `A-->xB`, `A---xB`) as regression tests asserting
  the RIGHT graph (edge A→B with the right head, no third node).
- Edge-to-subgraph loud error, including the edge-above-subgraph
  ordering case.
- Front matter: skipped for each kind, error-line offset preserved,
  unterminated block errors, `---` mid-document NOT treated as front
  matter.
- `classDef default` applied to unclassed nodes only.
- Subroutine shape: SVG structure (rect + two inner lines), bracket
  precedence vs `[` and `[(`.
- SVG structural: `marker-start` present for reversed/bidirectional
  heads; NO path emitted for invisible edges; circle marker defined
  once in defs.
- Statement-soup property test extended with the new operator
  characters (`o x < ~`), 256 cases, XOR invariant.
- Existing tests are immutable; expected additions only. (If an
  existing test literally asserts the old phantom-node parse, that is
  a sanctioned behavior-change edit — none is known to exist; the
  plan must verify and list any such test explicitly.)

## Constraints carried forward

Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
wasm32-clean, deterministic. Branch: worktree
`worktree-mermaid-polish` off post-slice-4 main (`97ff178`), PR to
main, squash-merge.
