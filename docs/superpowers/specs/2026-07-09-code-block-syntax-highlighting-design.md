# Code Block Syntax Highlighting — Design

**Date:** 2026-07-09
**Status:** Approved (design); spec pending user review
**Scope slice:** render-time highlighting + language selector + highlighted HTML export

## Summary

Add syntax highlighting to code blocks. A new shared pure-Rust crate
(`crates/highlight`) tokenizes code for ~19 curated languages; the
editor renders token `<span>`s inside the existing contenteditable
`<pre><code>`; a floating language-selector chip sets the existing
`language` attribute; server-side HTML export emits inline-styled
highlighted code from the same crate.

No schema change: `code_block` already carries a free-form `language`
attribute threaded through markdown import (`frontend/src/editor/markdown.rs:219`),
markdown/HTML export (`crates/collab/src/export.rs:874,1141`), and the
live view (`frontend/src/editor/view.rs:1201`, currently only a
`class="language-{lang}"` hook).

## Why this approach

Three engine options were considered:

1. **Shared pure-Rust crate with hand-written lexers (chosen).**
   Mermaid-precedent (`crates/mermaid`): std-only, wasm-clean, usable
   by both frontend and server. Tiny bundle cost (tens of KB) against
   the hard 1.7 MB gzip WASM gate (`.github/workflows/bundle-size.yml`,
   currently ~1.36 MB). Fidelity is GitHub-comment-level, not
   full-grammar — acceptable for a notes tool.
2. **syntect (trimmed)** — full Sublime-grammar fidelity, but
   300 KB–1 MB+ gzipped; real risk of blowing or crowding the bundle
   gate. Rejected.
3. **highlight.js via JS interop** — zero WASM cost, but breaks the
   all-Rust story, has no server-export parity, and adds a JS asset
   surface. Rejected.

Two rendering options were considered:

1. **Token spans inside the editable `<code>` (chosen).** Evidence
   from the position-mapping architecture (see Invariants): the
   DOM↔model walkers (`find_in_element` view.rs:1587,
   `dom_to_model_walk` view.rs:1686) sum text-node lengths and treat
   `span`/`code` without `data-atom-size` as transparent wrappers
   (`is_mark_tag` view.rs:1818); all edits are `beforeinput` +
   `prevent_default` reconciled by a full re-render from the model
   (view.rs:127–161), so spans are regenerated per keystroke and never
   desync; paste flattens code-block DOM via `text_content()`
   (`frontend/src/editor/clipboard.rs:291`). Zero mapping changes
   required.
2. **Overlay** (transparent editable text over a positioned colored
   copy) — strictly more complex, fights the full-re-render model.
   Rejected.

A dedicated highlighted-block atom (Mermaid-style leaf) was rejected
because code blocks must stay inline-editable.

## Component 1: `crates/highlight` (`ogrenotes-highlight`)

Pure Rust, std-only, no deps, wasm-clean. Added to the workspace like
`crates/mermaid`.

### API

```rust
pub enum TokenKind { Keyword, Type, String, Comment, Number, Function, Meta, Plain }

pub struct Token<'a> { pub text: &'a str, pub kind: TokenKind }

pub enum Language { Rust, JavaScript, TypeScript, Python, Json, Toml,
    Yaml, Bash, Sql, Html, Css, Java, Kotlin, CSharp, C, Cpp, Go,
    Dockerfile, Hcl, Protobuf }

impl Language {
    /// Case-insensitive; handles aliases: js/javascript, ts, py,
    /// c++/cpp, cs/csharp, sh/shell/bash, yml, tf/hcl, proto, docker.
    pub fn from_tag(tag: &str) -> Option<Language>;
}

/// Total function: never panics; worst case one Plain token.
pub fn highlight(source: &str, lang: Language) -> Vec<Token<'_>>;

pub enum Theme { Light, Dark }

/// Single source of truth for token colors, shared by client CSS
/// and export inline styles.
pub fn color_for(kind: TokenKind, theme: Theme) -> Option<&'static str>; // None for Plain — no sentinel value
```

### Lexers

- One **data-driven generic lexer** parameterized by
  `LexerSpec { line_comment, block_comments, string_delims,
  raw_string, keywords, types, … }` covers the C-family majority
  (Rust, JS, TS, Java, Kotlin, C#, C, C++, Go, SQL, Protobuf, HCL,
  Bash, Python, TOML, JSON — with per-language spec entries).
- Small bespoke lexers where the generic shape doesn't fit:
  HTML (tags/attrs), CSS (selectors/properties), YAML (keys/scalars),
  Dockerfile (instructions).
- Lexers are total: no panics, no unwraps on input-derived values;
  unrecognized text falls through as `Plain`.
- `Function` tokens use one uniform heuristic: an identifier
  immediately followed by `(` (whitespace-tolerant), enabled per
  `LexerSpec`; languages where that misfires (YAML, JSON, TOML,
  Dockerfile) disable it. `Type` tokens come from a per-language
  builtin-type list plus, where the flag is set, capitalized
  identifiers (Rust, Java, Kotlin, C#, Go).

### Invariants (hard requirements)

1. **Partition invariant:** `concat(tokens[].text) == source`, byte
   for byte, for every language and every input — including `\n`,
   `\t`, and all whitespace. Violation drifts every caret position
   after the first divergence (the mention-chip bug class; see
   `assert_sizes_match` harness, view.rs:1934–1973). Enforced by a
   property test run against every `Language` with arbitrary inputs.
2. **Totality:** `highlight` never panics on any input.
3. Tokens carry no markup — plain text slices only.

## Component 2: Editor rendering

`frontend/src/editor/view.rs`, `NodeType::CodeBlock` arm
(~view.rs:1194–1210):

- If `attrs["language"]` resolves via `Language::from_tag`, tokenize
  the block's concatenated text and append one
  `<span class="tok-{kind}">` element per non-Plain token (Plain
  tokens append as bare text nodes) inside the `<code>` element,
  instead of `render_children`.
- Spans carry **no** `data-atom-size`, no `data-sentinel`, and use the
  `span` tag — this keeps them on the transparent path of both
  position walkers with zero walker changes.
- Empty or unknown language → existing plain `render_children` path,
  unchanged.
- **Perf guard:** blocks whose text exceeds 50,000 chars render plain
  (lexing is linear, but re-render runs per keystroke; don't gamble on
  pathological blocks).
- Token colors via CSS custom properties (`--tok-keyword`, …) defined
  for light and dark themes from the crate's `color_for` table, so
  themes stay consistent. Class names in DOM; CSS variables in the
  stylesheet.

The model is untouched: code-block content remains plain text
children. Search indexing (`to_plain_text` → `extract_text`,
export.rs:82,183) is therefore unaffected.

## Component 3: Language selector

- A small chip/dropdown shown at the top-right of a code block when
  the caret is inside it or the pointer hovers it.
- Rendered as a Leptos overlay component **outside** the
  contenteditable DOM, absolutely positioned over the block (so it can
  never disturb position mapping or get swallowed by
  `set_inner_html("")` re-renders).
- Displays the current language (or "Plain text"); opens a searchable
  list of the 19 supported languages + "Plain text".
- Dispatches new `commands::set_code_block_language(lang: &str)` which
  updates the block's `language` attr via a normal transaction (same
  shape as `commands::update_mermaid_source`,
  frontend/src/editor/commands.rs:1398). Setting "Plain text" clears
  the attr.
- `commands::set_code_block` continues to create blocks with empty
  attrs. No auto-detection (YAGNI).
- The attr stays free-form (markdown import may write any string);
  unknown values render plain and the selector shows the raw tag.

## Component 4: Server HTML export

`crates/collab/src/export.rs` HTML code-block arm (~1140–1143):

- When the language resolves, emit
  `<pre><code class="language-{lang}">` containing per-token
  `<span style="color:#…">` inline styles from `color_for(kind,
  /*dark=*/false)` — inline styles so exported HTML is standalone/
  email-safe with no stylesheet dependency.
- Every token's text is HTML-escaped individually (hostile content
  such as `</code><script>` must escape exactly as today).
- Unknown/empty language: current output, unchanged.
- Markdown export unchanged (already round-trips the language tag).

## Error handling

- `highlight` is total; there is no error path to surface. Any lexer
  shortfall degrades to `Plain` tokens (uncolored text), never to a
  broken document or panic.
- Unknown language tags degrade to today's rendering everywhere.

## Testing

- **`crates/highlight`:** per-language lexer unit tests (keywords,
  strings incl. escapes, line + block comments, numbers); the
  partition property test (arbitrary inputs × every language, assert
  `concat == source` and no panic); alias-resolution tests for
  `from_tag`.
- **Editor (view.rs):** `position_sizes_match_highlighted_code_block`
  using the existing `assert_sizes_match` harness — a code block with
  a resolving language whose DOM contains token spans must produce
  identical position sizes to the model; caret round-trip through
  `find_dom_position`/`dom_position_to_model` inside token spans.
- **Export (crates/collab):** HTML export emits styled spans for a
  known language; escaping test with hostile code content; unknown
  language emits today's exact output (regression).
- **Builds:** explicit wasm32 frontend build (native `cargo check`
  is insufficient for the frontend); bundle-size CI gate verifies the
  (expected tens-of-KB) delta.
- Existing tests are immutable; all of the above are additions.

## Out of scope (this slice)

- Triple-backtick input rule and `Ctrl+Alt+C` shortcut (design/
  rich-text-editor.md:420 — separate slice).
- Language auto-detection.
- Highlighting in other surfaces (chat messages, spreadsheet cells).
- Line numbers, line highlighting, copy-button chrome.
- Mermaid markdown-import mapping (` ```mermaid ` fences still import
  as CodeBlock with language="mermaid"; it will render plain since
  `from_tag("mermaid")` is None — acceptable, pre-existing behavior).

## Design-doc drift note (report, don't edit)

`design/rich-text-editor.md:666` names a `CodeBlockLowlight` node that
"extends CodeBlock". This implementation keeps a single `code_block`
node and adds render-time highlighting — functionally equivalent, no
new node type. Per repo policy, `design/` is not edited as a side
effect; this divergence is recorded here as a finding with proposed
text: *"CodeBlockLowlight: implemented as render-time highlighting on
the existing CodeBlock node (language attr), not a separate node
type."*
