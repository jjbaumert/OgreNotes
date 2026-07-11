# Mermaid sequence polish — semicolon silent-misparse fix + doc-parity adds

Status: approved 2026-07-11. Tracking issue:
[#45](https://github.com/jjbaumert/OgreNotes/issues/45). Sibling of the
flowchart polish (#32 → PR #44); method and contract identical. Gap
analysis executed 2026-07-11 against
https://mermaid.ai/open-source/syntax/sequenceDiagram.html (63 cases:
31 rendered, 32 loud errors, 1 new silent divergence).

## Goal

Eliminate the sequence parser's one silent misparse (semicolon
statement separation) and close the cheap doc-parity gaps, bringing
sequence diagrams to the same "never silent, errors name their
keyword" bar the flowchart parser reached in PR #44.

## Scope

### 1. Semicolon statement separator (silent-misparse fix)

Mermaid treats `;` as a line terminator everywhere — message text,
note text, aliases (its docs mandate `#59;` for a literal semicolon).
Our parser splits on newlines only, so `A->>B: hi; B-->>A: yo`
renders ONE message labeled `hi; B-->>A: yo`.

Fix: a `split_statements` helper in `sequence/parse.rs`; every source
line is split on `;` into statements EXCEPT a `;` that closes an
entity-code token — `#` followed by one or more ASCII alphanumerics
then `;` (`#59;`, `#9829;`, `#infin;`). Each fragment parses as a full
statement carrying the line's 1-based number. Consequences:

- `A->>B: hi; B-->>A: yo` → two messages (matches mermaid).
- `A->>B: x#59;y` → one message, literal `#59;` text (entity codes
  stay literal — the accepted divergence, unchanged).
- Header trailing `;` (`sequenceDiagram;`) keeps working (empty
  fragments skipped); existing header tests unaffected.
- The flowchart's `split_statements` is quote-aware and stays where it
  is; the sequence one is entity-aware. They are different grammars —
  no shared helper is forced in this slice.

### 2. Cheap adds

- **Spaced activation shorthand**: `Alice ->>+ John: text` — the
  target-participant scan gains a `trim_start()` after stripping the
  `+`/`-` marker. The docs' Background-Highlighting example uses this
  spelling.
- **`option` divider inside `critical`**: parsed like the existing
  `else` (alt) / `and` (par) dividers, valid only in a `critical`
  block; `option` outside `critical` errors loudly with the same
  context-check pattern `else`/`and` use today.
- **Bidirectional arrows `<<->>` (solid) and `<<-->>` (dotted)**:
  supported — one message with a filled-arrow head at BOTH ends,
  emitted as `marker-start` + `marker-end` (`mmd-arrow` is already
  `orient="auto-start-reverse"`). Activation shorthand composes as for
  other arrows.
- **`Note over` with 3+ participants**: all comma-listed participants
  are interned (creating lifelines as needed) and the note spans
  min..max of their x positions. Closes the #32-filed silent drop of
  the third participant.
- **`<br/>` in participant display names**: `draw_box` and
  `draw_actor` in `sequence/svg.rs` render display text as per-line
  `<tspan>`s via `measure::lines` (measurement is already
  multi-line; only rendering lags). Closes the other #32-filed item.
- **Keyword-naming errors** for remaining unsupported arrow families,
  replacing the generic `unsupported statement` message:
  - half-arrows (tokens containing `-|`, `|-`, `--/`, `--\`, `/-`,
    `\-` in arrow position) → error naming "half arrows";
  - central connections (`()` adjacent to an arrow token or
    participant, e.g. `A->>()B`, `A()->>B`) → error naming
    "central connections".
  Detection is best-effort pattern recognition in the error path only
  — it must never affect the parse of supported spellings.

### 3. Gallery promotion

`crates/mermaid/examples/seq_gallery.rs` (currently untracked in the
analysis worktree `mermaid-seq-gap`) is committed as a crate example —
same shape as the flowchart `doc_gallery.rs` — with expectation notes
updated to post-fix behavior: the semicolon case renders two messages;
spaced shorthand, `option`, bidirectional, and `Note over A,B,C` move
to `match`; half-arrows and central connections keep `error` notes now
naming their construct. The `mermaid-seq-gap` analysis worktree is
removed after the file is copied.

## Out of scope (stays on issue #45's honest-error list)

`@{ type: … }` participant stereotypes; `create`/`destroy`; `box`
grouping (incl. colors/transparent); `rect` background highlighting;
`autonumber <start> <increment>` / `autonumber off` (deliberate);
actor `link`/`links` menus; entity-code decoding; mermaid's boxed
autonumber rendering (ours stays an inline label prefix); the 4px
self-message reservation nit (#32).

## Error handling / invariants (unchanged contract)

`render()` never panics; exactly one of svg/error; 1-based error lines
(fragments report their line's number); first error wins; unsupported
constructs error naming their keyword; every user string through
`escape_xml`; deterministic; caps unchanged (participants cap, message
cap, `MAX_SOURCE_LEN` gate before front-matter stripping in `render()`
— untouched). UTF-8 discipline: `;`, `#`, and entity-code characters
checked are ASCII; splits at `char_indices` offsets of ASCII chars are
boundary-safe (commented per crate convention).

## Testing

- Parser tables: semicolon split matrix (mid-message split, entity
  guard `#59;`/`#9829;`/`#infin;`, `;` in note text, trailing `;`,
  `;;`, entity-like non-entities `#5 9;` / `#;`), spaced `+`/`-`
  (activate and deactivate), `option` in/outside `critical`,
  bidirectional solid+dotted (both heads set), `Note over A,B,C`
  interning + span, keyword-naming error assertions for half-arrows
  and central connections.
- SVG structural: bidirectional message has `marker-start` AND
  `marker-end`; 3-participant note x-span covers the outermost
  lifelines; multi-line participant display renders one `<tspan>` per
  line in both box and actor forms.
- Regression: the exact gap-analysis input `A->>B: hi; B-->>A: yo`
  asserts TWO messages with the right labels.
- Props soup (`sequence/props.rs`): add statement variants with `;`
  separators, `#59;` text, and bidirectional arrows; extend the noise
  alphabet with `;` and `#`.
- Existing tests are immutable; additions only. (No existing test
  asserts the one-message semicolon parse or single-line participant
  rendering — verified during the gap analysis.)
- Acceptance sweep: `cargo run -p ogrenotes-mermaid --example
  seq_gallery` — every case on its documented side.

## Constraints carried forward

Crate stays `#![forbid(unsafe_code)]`, zero runtime dependencies,
wasm32-clean, deterministic. Branch: worktree
`worktree-mermaid-seq-polish` off post-#44 main (`1bb6257`), PR to
main, squash-merge. PR title must NOT contain "closes #45" phrasing
(the #32 auto-close lesson) — reference the issue as "issue #45
highlights" instead.
