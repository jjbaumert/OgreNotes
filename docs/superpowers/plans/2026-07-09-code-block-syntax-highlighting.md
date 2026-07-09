# Code Block Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Syntax-highlight code blocks in the editor and in HTML export, with a language-selector chip, driven by a new shared pure-Rust tokenizer crate.

**Architecture:** A new dep-free workspace crate `crates/highlight` (`ogrenotes-highlight`) tokenizes source for 20 languages via one data-driven generic lexer plus two bespoke lexers (HTML, YAML). The editor's `render_node` CodeBlock arm emits transparent `<span class="tok-*">` chunks inside the existing contenteditable `<pre><code>` (zero position-mapping changes — the DOM walkers already treat bare `span` as a transparent mark wrapper). A native `<select>` chip overlay sets the existing `language` attr via a new `Step::SetAttr` command. Server HTML export emits inline-styled spans from the same crate.

**Tech Stack:** Rust (edition 2021 for the crate, matching `crates/mermaid`), Leptos 0.7 CSR frontend, yrs on the export side, proptest (dev-only, native-only) for the partition invariant.

**Spec:** `docs/superpowers/specs/2026-07-09-code-block-syntax-highlighting-design.md`

## Global Constraints

- **Partition invariant (hard):** for every language and every input, `concat(tokens[].text) == source` byte-for-byte and `highlight` never panics. Violation drifts every caret position after the first divergence.
- **WASM bundle gate:** gzipped `_bg.wasm` must stay under `WASM_GZ_LIMIT = 1700000` bytes (`.github/workflows/bundle-size.yml:70`). The crate must stay dependency-free (proptest is dev-dep only, and native-only — see frontend/Cargo.toml:150 for why proptest must never reach a wasm target).
- **Perf guard:** blocks over `MAX_HIGHLIGHT_CHARS = 50_000` chars render plain.
- **Token spans must stay transparent to the position walkers:** never set `data-atom-size`, `data-sentinel`, or a leaf tag on them; tag is always `span`.
- **Existing tests are immutable** except the one deliberate behavior-change update in Task 8 (`html_code_block_with_language`, export.rs:1518), which is explicitly licensed by the approved spec (highlighted exports change the emitted HTML shape).
- **Don't edit `design/`** — the `CodeBlockLowlight` drift is already recorded in the spec.
- Frontend is outside the workspace: frontend builds/tests run from `frontend/`.
- Don't use `git add -A` / `git add .` — stage files by name.
- Token colors have one source of truth: `ogrenotes_highlight::color_for`. The CSS variables in `frontend/style/main.css` mirror it and carry a comment pointing back at the crate.

---

### Task 1: Crate scaffold — token types, `Language`, palette

**Files:**
- Create: `crates/highlight/Cargo.toml`
- Create: `crates/highlight/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — `members` list and `[workspace.dependencies]`)

**Interfaces:**
- Produces (later tasks rely on these exact signatures):
  - `pub enum TokenKind { Keyword, Type, String, Comment, Number, Function, Meta, Plain }`
  - `impl TokenKind { pub fn css_class(self) -> Option<&'static str> }`
  - `pub struct Token<'a> { pub text: &'a str, pub kind: TokenKind }`
  - `pub enum Language` (20 variants) with `pub const ALL: [Language; 20]`, `pub fn from_tag(tag: &str) -> Option<Language>`, `pub fn tag(self) -> &'static str`, `pub fn label(self) -> &'static str`
  - `pub fn color_for(kind: TokenKind, dark: bool) -> Option<&'static str>`
  - `pub const MAX_HIGHLIGHT_CHARS: usize = 50_000;`
  - `pub fn highlight(source: &str, lang: Language) -> Vec<Token<'_>>` (stub in this task; real in Task 2)

- [ ] **Step 1: Register the crate in the workspace**

In root `Cargo.toml`, add `"crates/highlight",` to `members` after `"crates/mermaid",`, and under `# Workspace crates` in `[workspace.dependencies]` add:

```toml
ogrenotes-highlight = { path = "crates/highlight" }
```

- [ ] **Step 2: Create `crates/highlight/Cargo.toml`** (mirrors `crates/mermaid/Cargo.toml`)

```toml
[package]
name = "ogrenotes-highlight"
version = "0.1.0"
edition = "2021"
license.workspace = true
publish = false

[dependencies]

[dev-dependencies]
proptest = { workspace = true }
```

- [ ] **Step 3: Write failing tests** — create `crates/highlight/src/lib.rs` with module docs, the types below, and tests. Write the tests FIRST inside the same file, run to see them fail to compile, then fill in the impl.

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pure-Rust syntax tokenizer shared by the WASM frontend (editor
//! rendering) and the server (HTML export) — same shared-crate shape
//! as `ogrenotes-mermaid`. std-only, no dependencies, wasm-clean.
//!
//! Hard invariant: `highlight` is a pure partition of its input —
//! the concatenation of every token's `text` equals `source`
//! byte-for-byte, and it never panics. The editor's caret mapping
//! depends on this (see `frontend/src/editor/view.rs` DOM walkers).

mod lexer;
mod langs;
mod bespoke;

/// Blocks longer than this render unhighlighted (linear lexing is
/// cheap, but the editor re-renders per keystroke — don't gamble on
/// pathological blocks).
pub const MAX_HIGHLIGHT_CHARS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,
    String,
    Comment,
    Number,
    Function,
    Meta,
    Plain,
}

impl TokenKind {
    /// CSS class the editor stamps on the token's `<span>`.
    /// `None` (Plain) renders as a bare text node.
    pub fn css_class(self) -> Option<&'static str> {
        match self {
            TokenKind::Keyword => Some("tok-keyword"),
            TokenKind::Type => Some("tok-type"),
            TokenKind::String => Some("tok-string"),
            TokenKind::Comment => Some("tok-comment"),
            TokenKind::Number => Some("tok-number"),
            TokenKind::Function => Some("tok-function"),
            TokenKind::Meta => Some("tok-meta"),
            TokenKind::Plain => None,
        }
    }
}

/// Single source of truth for token colors. The CSS custom
/// properties in `frontend/style/main.css` mirror these values;
/// HTML export inlines them directly.
pub fn color_for(kind: TokenKind, dark: bool) -> Option<&'static str> {
    Some(match (kind, dark) {
        (TokenKind::Keyword, false) => "#cf222e",
        (TokenKind::Keyword, true) => "#ff7b72",
        (TokenKind::Type, false) => "#953800",
        (TokenKind::Type, true) => "#ffa657",
        (TokenKind::String, false) => "#0a3069",
        (TokenKind::String, true) => "#a5d6ff",
        (TokenKind::Comment, false) => "#6e7781",
        (TokenKind::Comment, true) => "#8b949e",
        (TokenKind::Number, false) => "#0550ae",
        (TokenKind::Number, true) => "#79c0ff",
        (TokenKind::Function, false) => "#8250df",
        (TokenKind::Function, true) => "#d2a8ff",
        (TokenKind::Meta, false) => "#116329",
        (TokenKind::Meta, true) => "#7ee787",
        (TokenKind::Plain, _) => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Json,
    Toml,
    Yaml,
    Bash,
    Sql,
    Html,
    Css,
    Java,
    Kotlin,
    CSharp,
    C,
    Cpp,
    Go,
    Dockerfile,
    Hcl,
    Protobuf,
}

impl Language {
    pub const ALL: [Language; 20] = [
        Language::Rust,
        Language::JavaScript,
        Language::TypeScript,
        Language::Python,
        Language::Json,
        Language::Toml,
        Language::Yaml,
        Language::Bash,
        Language::Sql,
        Language::Html,
        Language::Css,
        Language::Java,
        Language::Kotlin,
        Language::CSharp,
        Language::C,
        Language::Cpp,
        Language::Go,
        Language::Dockerfile,
        Language::Hcl,
        Language::Protobuf,
    ];

    /// Resolve a code block's `language` attribute (as written by the
    /// UI, markdown import, or pasted `language-*` classes) to a
    /// supported language. Case-insensitive, alias-tolerant. `None`
    /// means render plain.
    pub fn from_tag(tag: &str) -> Option<Language> {
        Some(match tag.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "javascript" | "js" | "jsx" | "mjs" | "node" => Language::JavaScript,
            "typescript" | "ts" | "tsx" => Language::TypeScript,
            "python" | "py" | "python3" => Language::Python,
            "json" | "jsonc" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "bash" | "sh" | "shell" | "zsh" | "console" => Language::Bash,
            "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" => Language::Sql,
            "html" | "htm" | "xml" | "svg" => Language::Html,
            "css" | "scss" | "less" => Language::Css,
            "java" => Language::Java,
            "kotlin" | "kt" | "kts" => Language::Kotlin,
            "csharp" | "cs" | "c#" => Language::CSharp,
            "c" | "h" => Language::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "go" | "golang" => Language::Go,
            "dockerfile" | "docker" | "containerfile" => Language::Dockerfile,
            "hcl" | "terraform" | "tf" | "tfvars" => Language::Hcl,
            "protobuf" | "proto" => Language::Protobuf,
            _ => return None,
        })
    }

    /// Canonical tag written to the `language` attr by the selector.
    pub fn tag(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Json => "json",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Bash => "bash",
            Language::Sql => "sql",
            Language::Html => "html",
            Language::Css => "css",
            Language::Java => "java",
            Language::Kotlin => "kotlin",
            Language::CSharp => "csharp",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Go => "go",
            Language::Dockerfile => "dockerfile",
            Language::Hcl => "hcl",
            Language::Protobuf => "protobuf",
        }
    }

    /// Human label for the selector chip.
    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Python => "Python",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Bash => "Bash",
            Language::Sql => "SQL",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Java => "Java",
            Language::Kotlin => "Kotlin",
            Language::CSharp => "C#",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Go => "Go",
            Language::Dockerfile => "Dockerfile",
            Language::Hcl => "HCL",
            Language::Protobuf => "Protobuf",
        }
    }
}

/// Tokenize `source`. Total function: never panics; the concatenation
/// of all returned token texts equals `source` exactly.
pub fn highlight(source: &str, lang: Language) -> Vec<Token<'_>> {
    match lang {
        Language::Html => bespoke::html(source),
        Language::Yaml => bespoke::yaml(source),
        other => lexer::tokenize(source, &langs::spec_for(other)),
    }
}
```

For this task only, make `lexer.rs`, `langs.rs`, and `bespoke.rs` compile-only stubs (replaced in Tasks 2–4):

```rust
// src/lexer.rs
use crate::{Token, TokenKind};
pub(crate) struct LexerSpec;
pub(crate) fn tokenize<'a>(src: &'a str, _spec: &LexerSpec) -> Vec<Token<'a>> {
    if src.is_empty() { Vec::new() } else { vec![Token { text: src, kind: TokenKind::Plain }] }
}
```

```rust
// src/langs.rs
use crate::lexer::LexerSpec;
use crate::Language;
pub(crate) fn spec_for(_lang: Language) -> LexerSpec { LexerSpec }
```

```rust
// src/bespoke.rs
use crate::{Token, TokenKind};
pub(crate) fn html(src: &str) -> Vec<Token<'_>> {
    if src.is_empty() { Vec::new() } else { vec![Token { text: src, kind: TokenKind::Plain }] }
}
pub(crate) fn yaml(src: &str) -> Vec<Token<'_>> { html(src) }
```

Tests at the bottom of `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tag_resolves_canonical_and_aliases() {
        assert_eq!(Language::from_tag("rust"), Some(Language::Rust));
        assert_eq!(Language::from_tag("rs"), Some(Language::Rust));
        assert_eq!(Language::from_tag("TypeScript"), Some(Language::TypeScript));
        assert_eq!(Language::from_tag("  yml "), Some(Language::Yaml));
        assert_eq!(Language::from_tag("c++"), Some(Language::Cpp));
        assert_eq!(Language::from_tag("c#"), Some(Language::CSharp));
        assert_eq!(Language::from_tag("tf"), Some(Language::Hcl));
    }

    #[test]
    fn from_tag_rejects_unknown() {
        assert_eq!(Language::from_tag(""), None);
        assert_eq!(Language::from_tag("mermaid"), None, "mermaid fences stay plain");
        assert_eq!(Language::from_tag("brainfuck"), None);
    }

    #[test]
    fn every_language_round_trips_its_canonical_tag() {
        for lang in Language::ALL {
            assert_eq!(Language::from_tag(lang.tag()), Some(lang), "tag {}", lang.tag());
        }
    }

    #[test]
    fn plain_has_no_class_and_no_color_others_have_both() {
        for kind in [TokenKind::Keyword, TokenKind::Type, TokenKind::String,
                     TokenKind::Comment, TokenKind::Number, TokenKind::Function,
                     TokenKind::Meta] {
            assert!(kind.css_class().is_some());
            assert!(color_for(kind, false).is_some());
            assert!(color_for(kind, true).is_some());
        }
        assert_eq!(TokenKind::Plain.css_class(), None);
        assert_eq!(color_for(TokenKind::Plain, false), None);
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ogrenotes-highlight`
Expected: PASS (4 tests). Also run `cargo check` at the root — workspace must still build.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/highlight
git commit -m "feat(highlight): scaffold ogrenotes-highlight crate — token types, Language registry, palette"
```

---

### Task 2: Generic lexer engine + Rust spec + partition property test

**Files:**
- Rewrite: `crates/highlight/src/lexer.rs` (replace Task 1 stub)
- Rewrite: `crates/highlight/src/langs.rs` (replace stub; Rust only for now — other languages return a keyword-less spec so they still tokenize strings/comments generically without misfiring)
- Create: `crates/highlight/tests/partition.rs`

**Interfaces:**
- Consumes: `Token`, `TokenKind`, `Language` from Task 1.
- Produces: `pub(crate) struct LexerSpec { … }` with `pub(crate) const DEFAULT`, `pub(crate) fn tokenize<'a>(src: &'a str, spec: &LexerSpec) -> Vec<Token<'a>>`, `pub(crate) fn spec_for(lang: Language) -> LexerSpec`. Tasks 3–4 fill in more specs; the engine itself is FROZEN after this task.

- [ ] **Step 1: Write the failing engine tests** — in `lexer.rs`'s test module (write tests first, stub `tokenize` still in place from Task 1 so they fail):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Language, TokenKind, highlight};

    fn kinds_of(src: &str, lang: Language) -> Vec<(String, TokenKind)> {
        highlight(src, lang)
            .into_iter()
            .map(|t| (t.text.to_string(), t.kind))
            .collect()
    }

    fn assert_partition(src: &str, lang: Language) {
        let joined: String = highlight(src, lang).iter().map(|t| t.text).collect();
        assert_eq!(joined, src, "partition violated for {lang:?}");
    }

    #[test]
    fn rust_keywords_types_and_functions() {
        let toks = kinds_of("fn main() { let x: u32 = 5; }", Language::Rust);
        assert!(toks.contains(&("fn".into(), TokenKind::Keyword)));
        assert!(toks.contains(&("let".into(), TokenKind::Keyword)));
        assert!(toks.contains(&("u32".into(), TokenKind::Type)));
        assert!(toks.contains(&("main".into(), TokenKind::Function)));
        assert!(toks.contains(&("5".into(), TokenKind::Number)));
    }

    #[test]
    fn rust_line_and_block_comments() {
        let toks = kinds_of("x // hi\n/* multi\nline */ y", Language::Rust);
        assert!(toks.contains(&("// hi".into(), TokenKind::Comment)));
        assert!(toks.contains(&("/* multi\nline */".into(), TokenKind::Comment)));
        assert_partition("x // hi\n/* multi\nline */ y", Language::Rust);
    }

    #[test]
    fn rust_strings_with_escapes_and_raw() {
        // Outer literal must be r##…## — the sample code itself
        // contains a r#"…"# raw string.
        let src = r##"let s = "a\"b"; let r = r#"raw"#;"##;
        let toks = kinds_of(src, Language::Rust);
        assert!(toks.contains(&(r#""a\"b""#.into(), TokenKind::String)));
        assert!(toks.iter().any(|(t, k)| t.starts_with("r#\"") && *k == TokenKind::String));
    }

    #[test]
    fn rust_attr_is_meta_and_lifetimes_stay_plain() {
        let toks = kinds_of("#[derive(Debug)]\nfn f<'a>(x: &'a str) {}", Language::Rust);
        assert!(toks.contains(&("#[derive(Debug)]".into(), TokenKind::Meta)));
        // `'` is NOT a Rust string delimiter here — lifetimes must not
        // open a string and swallow the rest of the signature.
        assert!(toks.contains(&("str".into(), TokenKind::Type)));
    }

    #[test]
    fn unterminated_string_and_comment_reach_eof_without_panic() {
        assert_partition("\"never closed", Language::Rust);
        assert_partition("/* never closed", Language::Rust);
        assert_partition("r#\"never closed", Language::Rust);
    }

    #[test]
    fn single_line_string_stops_at_newline() {
        // The newline is NOT part of the string token.
        let toks = kinds_of("\"open\nnext", Language::Rust);
        assert_eq!(toks[0], ("\"open".into(), TokenKind::String));
        assert_partition("\"open\nnext", Language::Rust);
    }

    #[test]
    fn number_dot_only_consumed_before_digit() {
        // `1..10` must not swallow both dots (range syntax).
        let toks = kinds_of("1..10", Language::Rust);
        assert_eq!(toks[0], ("1".into(), TokenKind::Number));
        assert!(toks.contains(&("10".into(), TokenKind::Number)));
        // but a real float works
        let toks = kinds_of("1.5", Language::Rust);
        assert_eq!(toks[0], ("1.5".into(), TokenKind::Number));
    }

    #[test]
    fn multibyte_input_is_sliced_on_char_boundaries() {
        assert_partition("let s = \"héllo → 世界\"; // ünïcode 🎉", Language::Rust);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ogrenotes-highlight`
Expected: FAIL (stub returns one Plain token).

- [ ] **Step 3: Implement the engine** — replace `lexer.rs` body:

```rust
use crate::{Token, TokenKind};

/// Declarative description of a language for the generic lexer.
/// Most supported languages are "C-ish comments + quoted strings +
/// keyword table" and fit this; HTML and YAML get bespoke lexers.
pub(crate) struct LexerSpec {
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    /// String delimiters. A string runs to the matching delimiter,
    /// honoring `\` escapes, and stops (unterminated) at a newline
    /// unless the delimiter is in `multiline_delims`.
    pub string_delims: &'static [char],
    pub multiline_delims: &'static [char],
    /// Python/TOML `"""…"""` / `'''…'''`.
    pub triple_quoted: bool,
    /// Rust `r"…"` / `r#"…"#` / `r##"…"##`.
    pub rust_raw_strings: bool,
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    /// Identifiers starting with an uppercase letter render as Type.
    pub caps_types: bool,
    /// An identifier immediately followed by `(` renders as Function.
    pub fn_calls: bool,
    /// Keywords match case-insensitively (SQL).
    pub ci_keywords: bool,
    /// `#…` to end of line is Meta (Rust attributes, C preprocessor).
    /// Mutually exclusive with `#` in `line_comments`.
    pub hash_meta: bool,
    /// `@identifier` is Meta (decorators, annotations, CSS at-rules).
    pub at_meta: bool,
    /// `$identifier` and `${…}` are Meta (bash, HCL interpolation).
    pub dollar_meta: bool,
}

impl LexerSpec {
    pub(crate) const DEFAULT: LexerSpec = LexerSpec {
        line_comments: &[],
        block_comment: None,
        string_delims: &[],
        multiline_delims: &[],
        triple_quoted: false,
        rust_raw_strings: false,
        keywords: &[],
        types: &[],
        caps_types: false,
        fn_calls: false,
        ci_keywords: false,
        hash_meta: false,
        at_meta: false,
        dollar_meta: false,
    };
}

/// Tokenize `src` against `spec`.
///
/// Partition-by-construction: the cursor `i` only ever advances, every
/// emitted token is the contiguous slice `&src[start..end]` between
/// cursor positions, and pending plain text is flushed before every
/// non-plain token — so the concatenation of all tokens is exactly
/// `src`. All indices come from `char_indices`/`find` on `src`, so
/// slicing stays on char boundaries.
pub(crate) fn tokenize<'a>(src: &'a str, spec: &LexerSpec) -> Vec<Token<'a>> {
    let mut out: Vec<Token<'a>> = Vec::new();
    let mut plain_start: Option<usize> = None;
    let mut i = 0;

    while i < src.len() {
        let rest = &src[i..];
        // `rest` is non-empty (i < len) and starts on a char boundary.
        let c = rest.chars().next().unwrap();

        // Block comment
        if let Some((open, close)) = spec.block_comment {
            if rest.starts_with(open) {
                flush_plain(&mut out, src, &mut plain_start, i);
                let end = rest[open.len()..]
                    .find(close)
                    .map(|p| i + open.len() + p + close.len())
                    .unwrap_or(src.len());
                out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
                i = end;
                continue;
            }
        }

        // `#…` meta (checked before line comments; specs never set both)
        if spec.hash_meta && c == '#' {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = rest.find('\n').map(|p| i + p).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Meta });
            i = end;
            continue;
        }

        // Line comment
        if spec.line_comments.iter().any(|lc| rest.starts_with(lc)) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = rest.find('\n').map(|p| i + p).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            i = end;
            continue;
        }

        // Rust raw string
        if spec.rust_raw_strings && c == 'r' {
            if let Some(len) = rust_raw_string_len(rest) {
                flush_plain(&mut out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..i + len], kind: TokenKind::String });
                i += len;
                continue;
            }
        }

        // Triple-quoted string
        if spec.triple_quoted && (rest.starts_with("\"\"\"") || rest.starts_with("'''")) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let delim = &rest[..3];
            let end = rest[3..]
                .find(delim)
                .map(|p| i + 3 + p + 3)
                .unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::String });
            i = end;
            continue;
        }

        // String
        if spec.string_delims.contains(&c) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let multiline = spec.multiline_delims.contains(&c);
            let end = string_end(src, i, c, multiline);
            out.push(Token { text: &src[i..end], kind: TokenKind::String });
            i = end;
            continue;
        }

        // @meta / $meta
        if (spec.at_meta && c == '@') || (spec.dollar_meta && c == '$') {
            if let Some(end) = meta_end(src, i, c) {
                flush_plain(&mut out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..end], kind: TokenKind::Meta });
                i = end;
                continue;
            }
            // bare @/$ falls through to plain
        }

        // Number
        if c.is_ascii_digit() {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = number_end(src, i);
            out.push(Token { text: &src[i..end], kind: TokenKind::Number });
            i = end;
            continue;
        }

        // Identifier / keyword / type / function
        if c.is_alphabetic() || c == '_' {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = ident_end(src, i);
            let word = &src[i..end];
            let kind = classify_word(word, spec, src[end..].starts_with('('));
            out.push(Token { text: word, kind });
            i = end;
            continue;
        }

        // Anything else accumulates as plain text.
        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += c.len_utf8();
    }

    flush_plain(&mut out, src, &mut plain_start, src.len());
    out
}

fn flush_plain<'a>(
    out: &mut Vec<Token<'a>>,
    src: &'a str,
    start: &mut Option<usize>,
    end: usize,
) {
    if let Some(s) = start.take() {
        if s < end {
            out.push(Token { text: &src[s..end], kind: TokenKind::Plain });
        }
    }
}

/// End of a quoted string starting at `start` (which holds `delim`).
/// Honors `\` escapes. A non-multiline string left open at a newline
/// ends *before* the newline (the newline stays plain).
fn string_end(src: &str, start: usize, delim: char, multiline: bool) -> usize {
    let body = start + delim.len_utf8();
    let mut escaped = false;
    for (off, ch) in src[body..].char_indices() {
        let pos = body + off;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '\n' && !multiline {
            return pos;
        } else if ch == delim {
            return pos + ch.len_utf8();
        }
    }
    src.len()
}

/// Length of a Rust raw string (`r"…"`, `r#"…"#`, …) at the start of
/// `rest`, or None if `rest` isn't one. Unterminated → rest of input.
fn rust_raw_string_len(rest: &str) -> Option<usize> {
    let after_r = &rest[1..];
    let hashes = after_r.len() - after_r.trim_start_matches('#').len();
    let quote_at = 1 + hashes;
    if !rest[quote_at..].starts_with('"') {
        return None;
    }
    let closer: String = format!("\"{}", "#".repeat(hashes));
    let body = quote_at + 1;
    Some(
        rest[body..]
            .find(&closer)
            .map(|p| body + p + closer.len())
            .unwrap_or(rest.len()),
    )
}

/// End of `@ident`, `$ident`, or `${…}` starting at `start`; None if
/// the sigil isn't followed by an identifier char or `{`.
fn meta_end(src: &str, start: usize, sigil: char) -> Option<usize> {
    let body = start + sigil.len_utf8();
    let next = src[body..].chars().next()?;
    if next == '{' && sigil == '$' {
        // ${…} to the closing brace (or end of line, whichever first)
        for (off, ch) in src[body..].char_indices() {
            match ch {
                '}' => return Some(body + off + 1),
                '\n' => return Some(body + off),
                _ => {}
            }
        }
        return Some(src.len());
    }
    if next.is_alphabetic() || next == '_' {
        return Some(ident_end(src, body));
    }
    None
}

fn number_end(src: &str, start: usize) -> usize {
    let mut end = start;
    let mut chars = src[start..].char_indices().peekable();
    while let Some((off, ch)) = chars.next() {
        let pos = start + off;
        if ch.is_ascii_alphanumeric() || ch == '_' {
            end = pos + ch.len_utf8();
        } else if ch == '.' {
            // consume the dot only when a digit follows (1.5 yes, 1..10 no)
            match chars.peek() {
                Some((_, d)) if d.is_ascii_digit() => end = pos + 1,
                _ => break,
            }
        } else {
            break;
        }
    }
    end
}

fn ident_end(src: &str, start: usize) -> usize {
    src[start..]
        .char_indices()
        .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '_')
        .map(|(off, _)| start + off)
        .unwrap_or(src.len())
}

fn classify_word(word: &str, spec: &LexerSpec, next_is_paren: bool) -> TokenKind {
    let is_kw = if spec.ci_keywords {
        spec.keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
    } else {
        spec.keywords.contains(&word)
    };
    if is_kw {
        return TokenKind::Keyword;
    }
    if spec.types.contains(&word) {
        return TokenKind::Type;
    }
    if spec.caps_types && word.chars().next().is_some_and(|c| c.is_uppercase()) {
        return TokenKind::Type;
    }
    if spec.fn_calls && next_is_paren {
        return TokenKind::Function;
    }
    TokenKind::Plain
}
```

- [ ] **Step 4: Add the Rust spec** — replace `langs.rs`:

```rust
use crate::lexer::LexerSpec;
use crate::Language;

/// Spec table for every generically-lexed language. HTML and YAML are
/// bespoke (see `bespoke.rs`) and never reach this function via
/// `highlight`, but return a safe default if they do.
pub(crate) fn spec_for(lang: Language) -> LexerSpec {
    match lang {
        Language::Rust => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            // NOTE: `'` is deliberately NOT a delimiter — lifetimes.
            string_delims: &['"'],
            rust_raw_strings: true,
            hash_meta: true,
            fn_calls: true,
            caps_types: true,
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate",
                "dyn", "else", "enum", "extern", "false", "fn", "for", "if",
                "impl", "in", "let", "loop", "match", "mod", "move", "mut",
                "pub", "ref", "return", "self", "static", "struct", "super",
                "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            types: &[
                "bool", "char", "str", "String", "i8", "i16", "i32", "i64",
                "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
                "f32", "f64", "Vec", "Option", "Result", "Box", "Rc", "Arc",
                "HashMap", "HashSet", "Self",
            ],
            ..LexerSpec::DEFAULT
        },
        // Tasks 3–4 fill these in; until then every other language
        // tokenizes with no rules (single Plain run) — still a valid
        // partition.
        _ => LexerSpec::DEFAULT,
    }
}
```

- [ ] **Step 5: Write the partition property test** — create `crates/highlight/tests/partition.rs`:

```rust
//! The one invariant everything depends on: `highlight` is a pure
//! partition of its input and never panics, for every language and
//! any input. If this fails, editor caret positions drift — do not
//! weaken it; fix the lexer.

use ogrenotes_highlight::{highlight, Language};
use proptest::prelude::*;

fn assert_partition(src: &str) {
    for lang in Language::ALL {
        let joined: String = highlight(src, lang).iter().map(|t| t.text).collect();
        assert_eq!(joined, src, "partition violated for {lang:?} on {src:?}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_unicode_partitions(src in "\\PC*") {
        assert_partition(&src);
    }

    #[test]
    fn code_shaped_input_partitions(
        src in r#"[ -~\n\t"'`#@$/*\\{}\[\]<>]{0,300}"#
    ) {
        assert_partition(&src);
    }
}

#[test]
fn tricky_fixed_cases_partition() {
    for src in [
        "",
        "\n\n\n",
        "\"",
        "'",
        "r#\"",
        "\\",
        "\"esc\\\"",
        "/*/",
        "// eof no newline",
        "#",
        "@",
        "$",
        "${unclosed",
        "\"\"\"",
        "1.",
        "1..2",
        "0xFFcafe_u32",
        "héllo wörld 世界 🎉",
        "a\u{0}b",       // NUL byte
        "\u{2028}line sep",
    ] {
        assert_partition(src);
    }
}
```

- [ ] **Step 6: Run all crate tests**

Run: `cargo test -p ogrenotes-highlight`
Expected: PASS — engine unit tests, partition property tests (512 cases × 20 languages each), fixed cases.

- [ ] **Step 7: Commit**

```bash
git add crates/highlight
git commit -m "feat(highlight): generic lexer engine, Rust spec, partition property test"
```

---

### Task 3: Generic specs for the remaining 14 generically-lexed languages

**Files:**
- Modify: `crates/highlight/src/langs.rs` (replace the `_ => LexerSpec::DEFAULT` arm; keep it ONLY for `Html | Yaml`)

**Interfaces:**
- Consumes: `LexerSpec`/`DEFAULT` exactly as frozen in Task 2. Do NOT modify `lexer.rs` in this task — if a language seems to need an engine change, stop and surface it instead.

- [ ] **Step 1: Write failing per-language smoke tests** — append to the `langs.rs` test module (create it):

```rust
#[cfg(test)]
mod tests {
    use crate::{highlight, Language, TokenKind};

    fn has(src: &str, lang: Language, text: &str, kind: TokenKind) -> bool {
        highlight(src, lang).iter().any(|t| t.text == text && t.kind == kind)
    }

    #[test]
    fn javascript_and_typescript() {
        assert!(has("const x = `a ${b}\nc`;", Language::JavaScript, "const", TokenKind::Keyword));
        assert!(has("const x = `a\nb`;", Language::JavaScript, "`a\nb`", TokenKind::String));
        assert!(has("interface Foo {}", Language::TypeScript, "interface", TokenKind::Keyword));
        assert!(has("let n: number = 1;", Language::TypeScript, "number", TokenKind::Type));
    }

    #[test]
    fn python() {
        assert!(has("def f():\n  # hi\n  return None", Language::Python, "def", TokenKind::Keyword));
        assert!(has("def f():\n  # hi\n  return None", Language::Python, "# hi", TokenKind::Comment));
        assert!(has("s = '''multi\nline'''", Language::Python, "'''multi\nline'''", TokenKind::String));
        assert!(has("@cached\ndef g(): pass", Language::Python, "@cached", TokenKind::Meta));
    }

    #[test]
    fn json_toml_bash() {
        assert!(has("{\"a\": true}", Language::Json, "true", TokenKind::Keyword));
        assert!(has("{\"a\": 1}", Language::Json, "\"a\"", TokenKind::String));
        assert!(has("# t\nkey = \"v\"", Language::Toml, "# t", TokenKind::Comment));
        assert!(has("if [ -z \"$HOME\" ]; then\nfi", Language::Bash, "if", TokenKind::Keyword));
        assert!(has("echo $HOME ${PATH}", Language::Bash, "$HOME", TokenKind::Meta));
        assert!(has("echo ${PATH}", Language::Bash, "${PATH}", TokenKind::Meta));
    }

    #[test]
    fn sql_case_insensitive_keywords() {
        assert!(has("select * from t;", Language::Sql, "select", TokenKind::Keyword));
        assert!(has("SELECT * FROM t;", Language::Sql, "SELECT", TokenKind::Keyword));
        assert!(has("-- note\nSELECT 1", Language::Sql, "-- note", TokenKind::Comment));
        assert!(has("WHERE name = 'bob'", Language::Sql, "'bob'", TokenKind::String));
    }

    #[test]
    fn jvm_and_dotnet() {
        assert!(has("public class Foo {}", Language::Java, "class", TokenKind::Keyword));
        assert!(has("@Override\nvoid f() {}", Language::Java, "@Override", TokenKind::Meta));
        assert!(has("val x: Int = 1", Language::Kotlin, "val", TokenKind::Keyword));
        assert!(has("using System;", Language::CSharp, "using", TokenKind::Keyword));
    }

    #[test]
    fn c_cpp_go() {
        assert!(has("#include <stdio.h>\nint main() {}", Language::C, "#include <stdio.h>", TokenKind::Meta));
        assert!(has("template<typename T> T f();", Language::Cpp, "template", TokenKind::Keyword));
        assert!(has("func main() { s := `raw\nstring` }", Language::Go, "func", TokenKind::Keyword));
        assert!(has("s := `raw\nstring`", Language::Go, "`raw\nstring`", TokenKind::String));
    }

    #[test]
    fn css_dockerfile_hcl_protobuf() {
        assert!(has("@media screen { color: red; }", Language::Css, "@media", TokenKind::Meta));
        assert!(has("a { color: red; }", Language::Css, "color", TokenKind::Keyword));
        assert!(has("FROM rust:1.79 AS builder", Language::Dockerfile, "FROM", TokenKind::Keyword));
        assert!(has("# comment\nRUN make", Language::Dockerfile, "# comment", TokenKind::Comment));
        assert!(has("resource \"aws_s3_bucket\" \"b\" {}", Language::Hcl, "resource", TokenKind::Keyword));
        // Interpolation sits INSIDE a quoted string, and strings win:
        // the whole quoted region is one String token. (`dollar_meta`
        // still colors interpolation in heredocs/unquoted contexts.)
        assert!(has("bucket = \"${var.name}\"", Language::Hcl, "\"${var.name}\"", TokenKind::String));
        assert!(has("message Foo { int32 id = 1; }", Language::Protobuf, "message", TokenKind::Keyword));
    }
}
```

- [ ] **Step 2: Run to verify failures**

Run: `cargo test -p ogrenotes-highlight langs`
Expected: FAIL for every language except the already-specced Rust.

- [ ] **Step 3: Fill in the spec table.** Replace `langs.rs`'s `_ => LexerSpec::DEFAULT` arm with the fourteen specs below plus a final `Language::Html | Language::Yaml => LexerSpec::DEFAULT` arm (unreachable via `highlight`, kept for totality):

```rust
        Language::JavaScript => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            fn_calls: true,
            keywords: &[
                "async", "await", "break", "case", "catch", "class", "const",
                "continue", "debugger", "default", "delete", "do", "else",
                "export", "extends", "false", "finally", "for", "function",
                "if", "import", "in", "instanceof", "let", "new", "null",
                "of", "return", "static", "super", "switch", "this", "throw",
                "true", "try", "typeof", "undefined", "var", "void", "while",
                "with", "yield",
            ],
            types: &[
                "Array", "Boolean", "Date", "Error", "JSON", "Map", "Math",
                "Number", "Object", "Promise", "Proxy", "Reflect", "RegExp",
                "Set", "String", "Symbol", "WeakMap", "WeakSet", "console",
                "document", "window",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::TypeScript => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            fn_calls: true,
            at_meta: true, // decorators
            keywords: &[
                // JS set
                "async", "await", "break", "case", "catch", "class", "const",
                "continue", "debugger", "default", "delete", "do", "else",
                "export", "extends", "false", "finally", "for", "function",
                "if", "import", "in", "instanceof", "let", "new", "null",
                "of", "return", "static", "super", "switch", "this", "throw",
                "true", "try", "typeof", "undefined", "var", "void", "while",
                "with", "yield",
                // TS additions
                "abstract", "any", "as", "declare", "enum", "implements",
                "interface", "is", "keyof", "module", "namespace", "never",
                "private", "protected", "public", "readonly", "satisfies",
                "type", "unknown",
            ],
            types: &[
                "Array", "Boolean", "Date", "Error", "JSON", "Map", "Math",
                "Number", "Object", "Promise", "Record", "RegExp", "Set",
                "String", "Symbol", "Partial", "Required", "Readonly", "Pick",
                "Omit", "console", "document", "window",
                "boolean", "number", "object", "string", "symbol", "bigint",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Python => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            triple_quoted: true,
            fn_calls: true,
            at_meta: true, // decorators
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "case",
                "class", "continue", "def", "del", "elif", "else", "except",
                "False", "finally", "for", "from", "global", "if", "import",
                "in", "is", "lambda", "match", "None", "nonlocal", "not",
                "or", "pass", "raise", "return", "True", "try", "while",
                "with", "yield",
            ],
            types: &[
                "bool", "bytes", "dict", "float", "frozenset", "int", "list",
                "object", "set", "str", "tuple", "type", "self", "Exception",
                "ValueError", "TypeError", "KeyError", "RuntimeError",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Json => LexerSpec {
            // JSONC tolerance: comments are illegal in strict JSON but
            // common in config files; harmless to color them.
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &["true", "false", "null"],
            ..LexerSpec::DEFAULT
        },
        Language::Toml => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            triple_quoted: true,
            keywords: &["true", "false"],
            ..LexerSpec::DEFAULT
        },
        Language::Bash => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            dollar_meta: true,
            keywords: &[
                "if", "then", "elif", "else", "fi", "for", "while", "until",
                "do", "done", "case", "esac", "in", "function", "select",
                "return", "break", "continue", "local", "export", "readonly",
                "declare", "set", "unset", "shift", "exit", "trap", "source",
                "alias", "eval", "exec",
            ],
            types: &["echo", "printf", "read", "cd", "pwd", "test"],
            ..LexerSpec::DEFAULT
        },
        Language::Sql => LexerSpec {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['\''],
            ci_keywords: true,
            fn_calls: true,
            keywords: &[
                "select", "from", "where", "insert", "into", "values",
                "update", "set", "delete", "create", "table", "index",
                "view", "drop", "alter", "add", "primary", "key", "foreign",
                "references", "join", "inner", "left", "right", "full",
                "outer", "on", "as", "and", "or", "not", "null", "is", "in",
                "exists", "between", "like", "limit", "offset", "order",
                "by", "group", "having", "distinct", "union", "all", "case",
                "when", "then", "else", "end", "begin", "commit", "rollback",
                "transaction", "constraint", "unique", "default", "check",
                "if", "returning", "with",
            ],
            types: &[
                "int", "integer", "bigint", "smallint", "decimal", "numeric",
                "float", "real", "double", "varchar", "char", "text",
                "boolean", "date", "time", "timestamp", "timestamptz",
                "blob", "serial", "uuid", "json", "jsonb", "bytea",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Css => LexerSpec {
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            at_meta: true, // @media, @import, @keyframes …
            keywords: &[
                "color", "background", "background-color", "margin",
                "margin-top", "margin-right", "margin-bottom", "margin-left",
                "padding", "padding-top", "padding-right", "padding-bottom",
                "padding-left", "display", "position", "top", "right",
                "bottom", "left", "width", "height", "max-width",
                "max-height", "min-width", "min-height", "border",
                "border-radius", "font", "font-size", "font-family",
                "font-weight", "font-style", "line-height", "text-align",
                "text-decoration", "flex", "flex-direction", "align-items",
                "justify-content", "gap", "grid", "grid-template-columns",
                "grid-template-rows", "overflow", "overflow-x", "overflow-y",
                "opacity", "z-index", "cursor", "transition", "transform",
                "animation", "box-shadow", "outline", "content", "float",
                "clear", "visibility", "white-space", "vertical-align",
                "letter-spacing", "word-break", "box-sizing", "resize",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Java => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            caps_types: true,
            fn_calls: true,
            at_meta: true, // annotations
            keywords: &[
                "abstract", "assert", "break", "case", "catch", "class",
                "const", "continue", "default", "do", "else", "enum",
                "extends", "false", "final", "finally", "for", "goto", "if",
                "implements", "import", "instanceof", "interface", "native",
                "new", "null", "package", "permits", "private", "protected",
                "public", "record", "return", "sealed", "static", "strictfp",
                "super", "switch", "synchronized", "this", "throw", "throws",
                "transient", "true", "try", "var", "void", "volatile",
                "while", "yield",
            ],
            types: &[
                "boolean", "byte", "char", "double", "float", "int", "long",
                "short",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Kotlin => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            multiline_delims: &[], // ("""…""" handled by triple_quoted)
            triple_quoted: true,
            caps_types: true,
            fn_calls: true,
            at_meta: true,
            dollar_meta: true, // string templates outside quotes are rare, harmless
            keywords: &[
                "abstract", "actual", "annotation", "as", "break", "by",
                "catch", "class", "companion", "const", "constructor",
                "continue", "crossinline", "data", "do", "else", "enum",
                "expect", "external", "false", "final", "finally", "for",
                "fun", "get", "if", "import", "in", "infix", "init",
                "inline", "inner", "interface", "internal", "is", "lateinit",
                "noinline", "null", "object", "open", "operator", "out",
                "override", "package", "private", "protected", "public",
                "reified", "return", "sealed", "set", "super", "suspend",
                "tailrec", "this", "throw", "true", "try", "typealias",
                "val", "var", "vararg", "when", "where", "while",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::CSharp => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            caps_types: true,
            fn_calls: true,
            keywords: &[
                "abstract", "as", "async", "await", "base", "bool", "break",
                "byte", "case", "catch", "char", "checked", "class", "const",
                "continue", "decimal", "default", "delegate", "do", "double",
                "else", "enum", "event", "explicit", "extern", "false",
                "finally", "fixed", "float", "for", "foreach", "goto", "if",
                "implicit", "in", "int", "interface", "internal", "is",
                "lock", "long", "namespace", "new", "null", "object",
                "operator", "out", "override", "params", "private",
                "protected", "public", "readonly", "record", "ref", "return",
                "sbyte", "sealed", "short", "sizeof", "stackalloc", "static",
                "string", "struct", "switch", "this", "throw", "true", "try",
                "typeof", "uint", "ulong", "unchecked", "unsafe", "ushort",
                "using", "var", "virtual", "void", "volatile", "when",
                "where", "while", "yield",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::C => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            hash_meta: true, // #include, #define, …
            fn_calls: true,
            keywords: &[
                "auto", "break", "case", "char", "const", "continue",
                "default", "do", "double", "else", "enum", "extern", "float",
                "for", "goto", "if", "inline", "int", "long", "register",
                "restrict", "return", "short", "signed", "sizeof", "static",
                "struct", "switch", "typedef", "union", "unsigned", "void",
                "volatile", "while",
            ],
            types: &[
                "bool", "size_t", "ssize_t", "int8_t", "int16_t", "int32_t",
                "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
                "FILE", "NULL", "true", "false",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Cpp => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            hash_meta: true,
            fn_calls: true,
            keywords: &[
                // C subset
                "auto", "break", "case", "char", "const", "continue",
                "default", "do", "double", "else", "enum", "extern", "float",
                "for", "goto", "if", "inline", "int", "long", "register",
                "return", "short", "signed", "sizeof", "static", "struct",
                "switch", "typedef", "union", "unsigned", "void", "volatile",
                "while",
                // C++ additions
                "alignas", "alignof", "bool", "catch", "class", "concept",
                "constexpr", "consteval", "constinit", "const_cast",
                "decltype", "delete", "dynamic_cast", "explicit", "export",
                "false", "final", "friend", "mutable", "namespace", "new",
                "noexcept", "nullptr", "operator", "override", "private",
                "protected", "public", "reinterpret_cast", "requires",
                "static_assert", "static_cast", "template", "this",
                "thread_local", "throw", "true", "try", "typeid", "typename",
                "using", "virtual",
            ],
            types: &[
                "std", "string", "vector", "map", "set", "unordered_map",
                "unique_ptr", "shared_ptr", "size_t", "int32_t", "uint32_t",
                "int64_t", "uint64_t",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Go => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'], // raw strings
            fn_calls: true,
            keywords: &[
                "break", "case", "chan", "const", "continue", "default",
                "defer", "else", "fallthrough", "for", "func", "go", "goto",
                "if", "import", "interface", "map", "package", "range",
                "return", "select", "struct", "switch", "type", "var",
                "true", "false", "nil", "iota",
            ],
            types: &[
                "bool", "byte", "complex64", "complex128", "error",
                "float32", "float64", "int", "int8", "int16", "int32",
                "int64", "rune", "string", "uint", "uint8", "uint16",
                "uint32", "uint64", "uintptr", "any",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Dockerfile => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            dollar_meta: true,
            // Instructions are conventionally uppercase; include both
            // cases since the parser is position-blind.
            keywords: &[
                "FROM", "RUN", "CMD", "LABEL", "EXPOSE", "ENV", "ADD",
                "COPY", "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG",
                "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL", "AS",
                "from", "run", "cmd", "label", "expose", "env", "add",
                "copy", "entrypoint", "volume", "user", "workdir", "arg",
                "as",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::Hcl => LexerSpec {
            line_comments: &["#", "//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            dollar_meta: true,
            fn_calls: true,
            keywords: &[
                "resource", "variable", "output", "module", "provider",
                "data", "locals", "terraform", "for_each", "count",
                "depends_on", "if", "for", "in", "true", "false", "null",
            ],
            types: &[
                "string", "number", "bool", "list", "map", "set", "object",
                "tuple",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Protobuf => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &[
                "syntax", "package", "import", "option", "message", "enum",
                "service", "rpc", "returns", "oneof", "map", "repeated",
                "optional", "required", "reserved", "extend", "extensions",
                "to", "weak", "public", "true", "false",
            ],
            types: &[
                "double", "float", "int32", "int64", "uint32", "uint64",
                "sint32", "sint64", "fixed32", "fixed64", "sfixed32",
                "sfixed64", "bool", "string", "bytes",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Html | Language::Yaml => LexerSpec::DEFAULT,
```

- [ ] **Step 4: Run all crate tests (including the partition suite)**

Run: `cargo test -p ogrenotes-highlight`
Expected: PASS. The partition suite now exercises real specs for 18 of 20 languages.

- [ ] **Step 5: Commit**

```bash
git add crates/highlight/src/langs.rs
git commit -m "feat(highlight): lexer specs for the 14 remaining generic languages"
```

---

### Task 4: Bespoke lexers — HTML and YAML

**Files:**
- Rewrite: `crates/highlight/src/bespoke.rs` (replace Task 1 stub)

**Interfaces:**
- Consumes: `Token`, `TokenKind` from Task 1.
- Produces: `pub(crate) fn html(src: &str) -> Vec<Token<'_>>` and `pub(crate) fn yaml(src: &str) -> Vec<Token<'_>>` — same partition/totality contract as the generic engine.

- [ ] **Step 1: Write failing tests** — test module in `bespoke.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::{highlight, Language, TokenKind};

    fn has(src: &str, lang: Language, text: &str, kind: TokenKind) -> bool {
        highlight(src, lang).iter().any(|t| t.text == text && t.kind == kind)
    }

    fn assert_partition(src: &str, lang: Language) {
        let joined: String = highlight(src, lang).iter().map(|t| t.text).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn html_tags_attrs_strings_comments() {
        let src = "<!-- c --><div class=\"box\" id='x'>text</div>";
        assert!(has(src, Language::Html, "<!-- c -->", TokenKind::Comment));
        assert!(has(src, Language::Html, "div", TokenKind::Keyword));
        assert!(has(src, Language::Html, "class", TokenKind::Type));
        assert!(has(src, Language::Html, "\"box\"", TokenKind::String));
        assert!(has(src, Language::Html, "'x'", TokenKind::String));
        assert_partition(src, Language::Html);
    }

    #[test]
    fn html_text_between_tags_stays_plain_and_unterminated_is_safe() {
        assert!(has("<p>hello</p>", Language::Html, "hello", TokenKind::Plain));
        assert_partition("<div class=\"open", Language::Html);
        assert_partition("<!-- never closed", Language::Html);
        assert_partition("< 5 and > 3", Language::Html); // bare angle brackets
    }

    #[test]
    fn yaml_keys_comments_values() {
        let src = "# top\nname: ogre\ncount: 3\nflag: true\nitems:\n  - \"quoted\"\n";
        assert!(has(src, Language::Yaml, "# top", TokenKind::Comment));
        assert!(has(src, Language::Yaml, "name", TokenKind::Keyword));
        assert!(has(src, Language::Yaml, "count", TokenKind::Keyword));
        assert!(has(src, Language::Yaml, "3", TokenKind::Number));
        assert!(has(src, Language::Yaml, "true", TokenKind::Type));
        assert!(has(src, Language::Yaml, "\"quoted\"", TokenKind::String));
        assert_partition(src, Language::Yaml);
    }

    #[test]
    fn yaml_document_marker_and_no_key_lines() {
        let src = "---\n- plain item\nurl: http://x/y:z\n";
        assert!(has(src, Language::Yaml, "---", TokenKind::Meta));
        assert_partition(src, Language::Yaml);
    }
}
```

- [ ] **Step 2: Run to verify failures**

Run: `cargo test -p ogrenotes-highlight bespoke`
Expected: FAIL (stub emits one Plain token).

- [ ] **Step 3: Implement both lexers** — replace `bespoke.rs`:

```rust
//! Hand-written lexers for languages the generic engine can't model:
//! HTML (tag structure) and YAML (line-oriented keys). Same contract
//! as the generic engine: pure partition, never panics.

use crate::{Token, TokenKind};

/// HTML/XML. State machine: outside tags everything is Plain except
/// `<!-- … -->` comments; inside `<…>` the tag name is Keyword,
/// attribute names Type, quoted values String.
pub(crate) fn html(src: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut plain_start: Option<usize> = None;
    let mut i = 0;

    while i < src.len() {
        let rest = &src[i..];

        if rest.starts_with("<!--") {
            flush(&mut out, src, &mut plain_start, i);
            let end = rest[4..].find("-->").map(|p| i + 4 + p + 3).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            i = end;
            continue;
        }

        // A tag opens only when `<` is followed by a letter, `/`, `!`, or `?`.
        if rest.starts_with('<')
            && rest[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?')
        {
            flush(&mut out, src, &mut plain_start, i);
            i = lex_tag(src, i, &mut out);
            continue;
        }

        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += rest.chars().next().unwrap().len_utf8();
    }
    flush(&mut out, src, &mut plain_start, src.len());
    out
}

/// Lex one `<…>` region starting at `start` (which holds `<`).
/// Returns the index just past the closing `>` (or EOF). Pushes:
/// punctuation as Plain, tag name as Keyword, attr names as Type,
/// quoted values as String.
fn lex_tag<'a>(src: &'a str, start: usize, out: &mut Vec<Token<'a>>) -> usize {
    let mut i = start;
    let mut seen_name = false;
    let mut plain_start: Option<usize> = None;

    while i < src.len() {
        let c = src[i..].chars().next().unwrap();
        match c {
            '>' => {
                flush(out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..i + 1], kind: TokenKind::Plain });
                return i + 1;
            }
            '"' | '\'' => {
                flush(out, src, &mut plain_start, i);
                let end = quoted_end(src, i, c);
                out.push(Token { text: &src[i..end], kind: TokenKind::String });
                i = end;
            }
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' => {
                flush(out, src, &mut plain_start, i);
                let end = src[i..]
                    .char_indices()
                    .find(|(_, ch)| !ch.is_ascii_alphanumeric() && !"-_:.".contains(*ch))
                    .map(|(off, _)| i + off)
                    .unwrap_or(src.len());
                let kind = if seen_name { TokenKind::Type } else { TokenKind::Keyword };
                seen_name = true;
                out.push(Token { text: &src[i..end], kind });
                i = end;
            }
            c => {
                if plain_start.is_none() {
                    plain_start = Some(i);
                }
                i += c.len_utf8();
            }
        }
    }
    flush(out, src, &mut plain_start, src.len());
    src.len()
}

/// YAML, line-oriented: full-line/trailing `#` comments, `key:` as
/// Keyword, quoted scalars as String, bare numbers as Number,
/// true/false/null as Type, `---`/`...` document markers as Meta.
pub(crate) fn yaml(src: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut line_start = 0;

    while line_start <= src.len() {
        let line_end = src[line_start..]
            .find('\n')
            .map(|p| line_start + p)
            .unwrap_or(src.len());
        lex_yaml_line(src, line_start, line_end, &mut out);
        if line_end == src.len() {
            break;
        }
        // The newline itself is a Plain token.
        out.push(Token { text: &src[line_end..line_end + 1], kind: TokenKind::Plain });
        line_start = line_end + 1;
    }
    out
}

fn lex_yaml_line<'a>(src: &'a str, start: usize, end: usize, out: &mut Vec<Token<'a>>) {
    let line = &src[start..end];
    if line.is_empty() {
        return;
    }
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();

    if trimmed == "---" || trimmed == "..." {
        if indent_len > 0 {
            out.push(Token { text: &src[start..start + indent_len], kind: TokenKind::Plain });
        }
        out.push(Token { text: &src[start + indent_len..end], kind: TokenKind::Meta });
        return;
    }

    // Walk the line: indent / `- ` prefix as plain, then optional
    // `key:` then scalars. Trailing comments start at an unquoted `#`.
    let mut i = start;
    let mut plain_start: Option<usize> = None;
    let mut key_possible = true;

    while i < end {
        let c = src[i..].chars().next().unwrap();

        if c == '#' {
            flush(out, src, &mut plain_start, i);
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            return;
        }
        if c == '"' || c == '\'' {
            flush(out, src, &mut plain_start, i);
            let str_end = quoted_end(src, i, c).min(end);
            out.push(Token { text: &src[i..str_end], kind: TokenKind::String });
            i = str_end;
            key_possible = false;
            continue;
        }
        if c.is_ascii_digit() {
            flush(out, src, &mut plain_start, i);
            let num_end = src[i..end]
                .char_indices()
                .find(|(_, ch)| !ch.is_ascii_alphanumeric() && *ch != '.' && *ch != '_')
                .map(|(off, _)| i + off)
                .unwrap_or(end);
            out.push(Token { text: &src[i..num_end], kind: TokenKind::Number });
            i = num_end;
            key_possible = false;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            flush(out, src, &mut plain_start, i);
            let word_end = src[i..end]
                .char_indices()
                .find(|(_, ch)| !ch.is_alphanumeric() && !"-_".contains(*ch))
                .map(|(off, _)| i + off)
                .unwrap_or(end);
            let word = &src[i..word_end];
            // `key:` (colon followed by space/EOL) → Keyword
            let colon_next = src[word_end..end].starts_with(':')
                && src[word_end + 1..end]
                    .chars()
                    .next()
                    .map_or(true, |n| n == ' ' || n == '\t');
            let kind = if key_possible && colon_next {
                TokenKind::Keyword
            } else if matches!(word, "true" | "false" | "null" | "yes" | "no") {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            out.push(Token { text: word, kind });
            i = word_end;
            if kind == TokenKind::Keyword {
                key_possible = false;
            }
            continue;
        }

        // `- ` sequence dashes, colons, whitespace, everything else: plain
        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += c.len_utf8();
    }
    flush(out, src, &mut plain_start, end);
}

/// Shared: end of a `"…"`/`'…'` region with backslash escapes,
/// clamped to EOF. (No newline clamp — HTML attr values may wrap.)
fn quoted_end(src: &str, start: usize, delim: char) -> usize {
    let body = start + delim.len_utf8();
    let mut escaped = false;
    for (off, ch) in src[body..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == delim {
            return body + off + ch.len_utf8();
        }
    }
    src.len()
}

fn flush<'a>(out: &mut Vec<Token<'a>>, src: &'a str, start: &mut Option<usize>, end: usize) {
    if let Some(s) = start.take() {
        if s < end {
            out.push(Token { text: &src[s..end], kind: TokenKind::Plain });
        }
    }
}
```

Note: `lex_yaml_line`'s number/word scans clamp at `end` (the line boundary), and `quoted_end` inside YAML is `.min(end)`-clamped, so a quote left open on one line can't swallow the next line's newline token — that would double-emit the newline and break the partition. The partition property test in `tests/partition.rs` covers YAML and HTML automatically (it iterates `Language::ALL`).

One subtlety: `key_possible` starts true even after the `- ` list prefix (the prefix runs through the plain accumulator without touching `key_possible`), so `- name: x` colors `name` — intended.

- [ ] **Step 4: Run all crate tests**

Run: `cargo test -p ogrenotes-highlight`
Expected: PASS, including the partition property suite over all 20 languages.

- [ ] **Step 5: Commit**

```bash
git add crates/highlight/src/bespoke.rs
git commit -m "feat(highlight): bespoke HTML and YAML lexers"
```

---

### Task 5: Editor rendering — token spans in the live view + CSS

**Files:**
- Modify: `frontend/Cargo.toml` (add dependency)
- Modify: `frontend/src/editor/view.rs` (`NodeType::CodeBlock` arm at ~1194–1210; new helper + tests)
- Modify: `frontend/style/main.css` (token colors)

**Interfaces:**
- Consumes: `ogrenotes_highlight::{highlight, Language, TokenKind, MAX_HIGHLIGHT_CHARS}` (Task 1/2 signatures).
- Produces: DOM shape `<pre><code class="language-{lang}">…<span class="tok-*">…</span>…</code></pre>`; a pure helper `fn code_block_plain_text(content: &Fragment) -> Option<String>` used by tests and the render arm.

**Position-mapping safety (why no walker changes):** `is_mark_tag` (view.rs:1818) already treats bare `span` (no `data-atom-size`) as a transparent wrapper; both walkers (`find_in_element` view.rs:1587, `dom_to_model_walk` view.rs:1686) recurse into transparent wrappers and sum text-node lengths. Token spans must therefore carry ONLY a `class` attribute. Never add `data-atom-size`, `data-sentinel`, or `contenteditable` to them.

- [ ] **Step 1: Add the dependency** — in `frontend/Cargo.toml` `[dependencies]`, directly under the `ogrenotes-mermaid` entry:

```toml
# Pure-Rust syntax tokenizer, zero deps, wasm-clean. Shared with the
# backend (crates/highlight) — server HTML export uses the same
# tokenizer + palette, so editor and export always agree.
ogrenotes-highlight = { path = "../crates/highlight" }
```

- [ ] **Step 2: Write the failing native tests** — in view.rs's existing test module (near `position_sizes_match_code_marks_in_list`, view.rs:2099):

```rust
    #[test]
    fn code_block_plain_text_concatenates_bare_runs() {
        let content = Fragment::from(vec![
            Node::text("fn main() {\n"),
            Node::text("    println!(\"hi\");\n}"),
        ]);
        assert_eq!(
            code_block_plain_text(&content).as_deref(),
            Some("fn main() {\n    println!(\"hi\");\n}")
        );
    }

    #[test]
    fn code_block_plain_text_bails_on_marks_or_elements() {
        // Marked text (schema forbids it, but render defensively) →
        // fall back to the un-highlighted path rather than dropping marks.
        let marked = Fragment::from(vec![Node::text_with_marks(
            "x",
            vec![Mark::new(MarkType::Bold)],
        )]);
        assert_eq!(code_block_plain_text(&marked), None);

        let with_element = Fragment::from(vec![
            Node::text("a"),
            Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![])),
        ]);
        assert_eq!(code_block_plain_text(&with_element), None);
    }

    #[test]
    fn code_block_plain_text_empty_is_none() {
        assert_eq!(code_block_plain_text(&Fragment::from(vec![])), None);
    }

    #[test]
    fn position_sizes_match_code_block_with_language() {
        // Token spans are render-only: the MODEL is unchanged, so the
        // walker-size mirror must agree exactly as it does today.
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "rust".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("fn main() {}\nlet x = 1;")]),
            )]),
        );
        assert_sizes_match(&doc);
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `cd frontend && cargo test code_block_plain_text`
Expected: FAIL — `code_block_plain_text` not defined.

- [ ] **Step 4: Implement.** Add the helper near `render_node` (below the `// ─── DOM Rendering ───` banner, view.rs:1151):

```rust
/// The code block's content as one plain string — `None` when the
/// content is anything but bare (unmarked) text runs, or empty.
/// Highlighting only applies to the plain-text case; anything else
/// falls back to `render_children` so nothing is ever dropped.
fn code_block_plain_text(content: &Fragment) -> Option<String> {
    let mut text = String::new();
    for child in &content.children {
        match child {
            Node::Text { text: t, marks } if marks.is_empty() => text.push_str(t),
            _ => return None,
        }
    }
    if text.is_empty() { None } else { Some(text) }
}
```

Replace the `NodeType::CodeBlock` arm (view.rs:1194–1210) with:

```rust
                NodeType::CodeBlock => {
                    let pre = doc.create_element("pre").ok()?;
                    if let Some(bid) = attrs.get("blockId") {
                        pre.set_attribute("data-block-id", bid).ok()?;
                    }
                    apply_block_align(&pre, attrs);
                    let code = doc.create_element("code").ok()?;
                    if let Some(lang) = attrs.get("language") {
                        if !lang.is_empty() {
                            code.set_attribute("class", &format!("language-{lang}"))
                                .ok()?;
                        }
                    }

                    // Syntax highlighting: render-only token spans.
                    // Bare `span`s with just a class are transparent
                    // to the DOM↔model walkers (is_mark_tag), so the
                    // model and caret mapping are untouched. The
                    // tokenizer is a pure partition of the text, so
                    // the summed text-node lengths stay identical to
                    // the plain path.
                    let lang = attrs
                        .get("language")
                        .and_then(|l| ogrenotes_highlight::Language::from_tag(l));
                    let plain = code_block_plain_text(content);
                    match (lang, plain) {
                        (Some(lang), Some(text))
                            if char_len(&text)
                                <= ogrenotes_highlight::MAX_HIGHLIGHT_CHARS =>
                        {
                            for token in ogrenotes_highlight::highlight(&text, lang) {
                                match token.kind.css_class() {
                                    None => {
                                        code.append_child(
                                            &doc.create_text_node(token.text),
                                        )
                                        .ok()?;
                                    }
                                    Some(cls) => {
                                        let span = doc.create_element("span").ok()?;
                                        span.set_attribute("class", cls).ok()?;
                                        span.set_text_content(Some(token.text));
                                        code.append_child(&span).ok()?;
                                    }
                                }
                            }
                        }
                        _ => render_children(doc, &code, content),
                    }
                    pre.append_child(&code).ok()?;
                    return Some(pre.into());
                }
```

`char_len` is the model's char-count helper already used throughout view.rs — confirm its import at the top of the file (it's referenced by the walkers) and reuse it as-is.

- [ ] **Step 5: Add token CSS** — in `frontend/style/main.css`. Find the editor's code-block styles (search `pre` / `code` rules near the editor section) and append after them:

```css
/* ── Code block syntax highlighting ─────────────────────────────
   Token colors mirror crates/highlight/src/lib.rs::color_for —
   that crate is the single source of truth (HTML export inlines
   the same values). Update both together. */
:root {
  --tok-keyword: #cf222e;
  --tok-type: #953800;
  --tok-string: #0a3069;
  --tok-comment: #6e7781;
  --tok-number: #0550ae;
  --tok-function: #8250df;
  --tok-meta: #116329;
}
[data-theme="dark"] {
  --tok-keyword: #ff7b72;
  --tok-type: #ffa657;
  --tok-string: #a5d6ff;
  --tok-comment: #8b949e;
  --tok-number: #79c0ff;
  --tok-function: #d2a8ff;
  --tok-meta: #7ee787;
}
.tok-keyword { color: var(--tok-keyword); }
.tok-type { color: var(--tok-type); }
.tok-string { color: var(--tok-string); }
.tok-comment { color: var(--tok-comment); font-style: italic; }
.tok-number { color: var(--tok-number); }
.tok-function { color: var(--tok-function); }
.tok-meta { color: var(--tok-meta); }
```

Before writing, grep main.css for how the existing dark theme overrides variables (`grep -n 'data-theme="dark"' frontend/style/main.css | head -3`) and match that exact selector form (e.g. `:root[data-theme="dark"]` vs `[data-theme="dark"]`).

- [ ] **Step 6: Run native tests and the wasm check**

Run: `cd frontend && cargo test && cargo check --target wasm32-unknown-unknown`
Expected: PASS / clean check. (Native `cargo check` alone is NOT sufficient for frontend changes.)

- [ ] **Step 7: Commit**

```bash
git add frontend/Cargo.toml frontend/Cargo.lock frontend/src/editor/view.rs frontend/style/main.css
git commit -m "feat(editor): render syntax-highlight token spans in code blocks"
```

---

### Task 6: `set_code_block_language` command + language getter

**Files:**
- Modify: `frontend/src/editor/commands.rs` (new commands next to `is_in_code_block`, commands.rs:433; tests in the existing test module)

**Interfaces:**
- Consumes: `resolve_parent_type` (commands.rs:54), `Step::SetAttr` (as used by `update_mermaid_source`, commands.rs:1413).
- Produces (Task 7 relies on these):
  - `pub fn set_code_block_language(lang: &str, state: &EditorState, dispatch: Option<&dyn Fn(Transaction)>) -> bool`
  - `pub fn code_block_language(state: &EditorState) -> Option<String>` — `None` = cursor not in a code block; `Some("")` = in a code block with no language.

- [ ] **Step 1: Write the failing tests** — in commands.rs's test module, following the existing `simple_doc()`/`EditorState::create_default` pattern:

```rust
    fn code_block_doc(language: Option<&str>) -> Node {
        let mut attrs = std::collections::HashMap::new();
        if let Some(lang) = language {
            attrs.insert("language".to_string(), lang.to_string());
        }
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("fn main() {}")]),
            )]),
        )
    }

    #[test]
    fn code_block_language_reads_attr() {
        let state = EditorState::create_default(code_block_doc(Some("rust")));
        // cursor into the code block's text (pos 1 = before 'f')
        let state = EditorState { selection: Selection::cursor(1), ..state };
        assert_eq!(code_block_language(&state).as_deref(), Some("rust"));
    }

    #[test]
    fn code_block_language_empty_when_unset_none_when_outside() {
        let state = EditorState::create_default(code_block_doc(None));
        let inside = EditorState { selection: Selection::cursor(1), ..state };
        assert_eq!(code_block_language(&inside).as_deref(), Some(""));

        let para = EditorState::create_default(simple_doc());
        let outside = EditorState { selection: Selection::cursor(1), ..para };
        assert_eq!(code_block_language(&outside), None);
    }

    #[test]
    fn set_code_block_language_dispatches_set_attr() {
        let state = EditorState::create_default(code_block_doc(None));
        let state = EditorState { selection: Selection::cursor(1), ..state };
        let dispatched = RefCell::new(Vec::new());
        let dispatch = |txn: Transaction| dispatched.borrow_mut().push(txn);
        assert!(set_code_block_language("python", &state, Some(&dispatch)));
        assert_eq!(dispatched.borrow().len(), 1);
        // Move the transaction out — don't assume Transaction: Clone.
        let txn = dispatched.borrow_mut().remove(0);
        let new_state = state.apply(txn);
        let updated = EditorState { selection: Selection::cursor(1), ..new_state };
        assert_eq!(code_block_language(&updated).as_deref(), Some("python"));
    }

    #[test]
    fn set_code_block_language_refuses_outside_code_block() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState { selection: Selection::cursor(1), ..state };
        let dispatched = RefCell::new(Vec::new());
        let dispatch = |txn: Transaction| dispatched.borrow_mut().push(txn);
        assert!(!set_code_block_language("python", &state, Some(&dispatch)));
        assert!(dispatched.borrow().is_empty());
    }
```

Adapt to the test module's actual imports (it already has `RefCell`, `EditorState`, `Selection`, `Fragment` — check the module head and mirror whichever dispatch-capture idiom nearby command tests use; if an existing test captures dispatched transactions differently, follow that idiom instead of `RefCell<Vec<_>>`, but keep the assertions).

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && cargo test code_block_language`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement** — directly below `is_in_code_block` (commands.rs:436):

```rust
/// Language attribute of the code block containing the cursor.
/// `None` = not in a code block; `Some("")` = code block, no language.
pub fn code_block_language(state: &EditorState) -> Option<String> {
    let (nt, _) = resolve_parent_type(state)?;
    if nt != NodeType::CodeBlock {
        return None;
    }
    let rp = resolve(&state.doc, state.selection.from())?;
    Some(
        rp.node_at(rp.depth, &state.doc)
            .attrs()
            .get("language")
            .cloned()
            .unwrap_or_default(),
    )
}

/// Set (or clear, with `""`) the `language` attribute of the code
/// block containing the cursor. Same targeted `Step::SetAttr` shape
/// as `update_mermaid_source` — node identity and content untouched.
/// Returns `false` when the cursor is not in a code block.
pub fn set_code_block_language(
    lang: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((NodeType::CodeBlock, abs_pos)) = resolve_parent_type(state) else {
        return false;
    };
    if let Some(dispatch) = dispatch {
        if let Ok(txn) = state.transaction().step(Step::SetAttr {
            pos: abs_pos,
            attr: "language".to_string(),
            value: lang.to_string(),
        }) {
            dispatch(txn);
        }
    }
    true
}
```

Note: `heading_level` (commands.rs:414) is the precedent for the attr-read pattern in `code_block_language` — keep the same `resolve`/`node_at` idiom. If `Step::SetAttr`'s field names differ from `update_mermaid_source`'s usage, copy that call site exactly.

- [ ] **Step 4: Run the tests**

Run: `cd frontend && cargo test code_block_language`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/editor/commands.rs
git commit -m "feat(editor): set_code_block_language / code_block_language commands"
```

---

### Task 7: Language-selector chip overlay

**Files:**
- Create: `frontend/src/components/code_lang_chip.rs`
- Modify: `frontend/src/components/mod.rs` (register the module — match how `mermaid_modal` is registered)
- Modify: `frontend/src/components/editor_component.rs` (signal + state-change hook + mount)
- Modify: `frontend/style/main.css` (chip styles)

**Interfaces:**
- Consumes: `commands::{is_in_code_block, code_block_language, set_code_block_language}` (Task 6), `ogrenotes_highlight::Language::{ALL, tag, label, from_tag}` (Task 1).
- Produces: `pub struct CodeLangChipState { pub top: f64, pub right: f64, pub current: String }`, `#[component] pub fn CodeLangChip(state: RwSignal<Option<CodeLangChipState>>, on_select: Callback<String>) -> impl IntoView`.

**Design constraint:** the chip lives OUTSIDE the contenteditable container (a positioned sibling overlay), so it can never disturb the DOM↔model walkers and never gets destroyed by `render()`'s `set_inner_html("")`. Visibility is caret-based (`is_in_code_block`); the command targets the caret's block. (Spec mentioned hover-visibility as well — deliberately trimmed: a hover-shown chip would need block-targeted dispatch for a block the caret isn't in; noted as a follow-up, not silently dropped.)

- [ ] **Step 1: Read the wiring precedents first** (do not skip):
  - `frontend/src/components/editor_component.rs:1700-1730` — where `mermaid_modal_state` and sibling signals are declared.
  - `editor_component.rs:2140-2200` — `on_mermaid_outcome`: how a component callback borrows `view_ref`, builds a dispatch closure, and routes `on_change`/`on_state_change`/`on_mapping`.
  - `editor_component.rs:2940-2960` — where `<MermaidModal …/>` is mounted in the view tree.
  - Find where the component reacts to editor state changes (the code path that fires `props.on_state_change` / recomputes toolbar state after a dispatch) — the chip-position update hooks in there.

- [ ] **Step 2: Write the chip component** — `frontend/src/components/code_lang_chip.rs`:

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Language-selector chip for code blocks: a small native `<select>`
//! pinned to the top-right of the code block that contains the
//! caret. Rendered OUTSIDE the contenteditable tree (positioned
//! sibling overlay), so the editor's DOM↔model position walkers
//! never see it and `render()`'s full rebuild never destroys it.
//!
//! A native `<select>` keeps this keyboard-accessible for free and
//! avoids bespoke popover/focus code.

use leptos::prelude::*;

use ogrenotes_highlight::Language;

/// `Some` → visible at (top, right) within the editor overlay;
/// `None` → hidden. `current` is the block's raw `language` attr
/// ("" = plain text; may be an unsupported tag like "mermaid").
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLangChipState {
    pub top: f64,
    pub right: f64,
    pub current: String,
}

#[component]
pub fn CodeLangChip(
    #[prop(into)] state: RwSignal<Option<CodeLangChipState>>,
    /// Fires with the canonical tag of the chosen language
    /// ("" for Plain text).
    on_select: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || state.get().is_some()>
            {move || state.get().map(|s| {
                let current = s.current.clone();
                let known = Language::from_tag(&current).is_some();
                view! {
                    <div
                        class="code-lang-chip"
                        style=format!("top:{}px;right:{}px;", s.top, s.right)
                    >
                        <select
                            aria-label="Code block language"
                            on:change=move |ev| {
                                on_select.run(event_target_value(&ev));
                            }
                        >
                            <option value="" selected=current.is_empty()>
                                "Plain text"
                            </option>
                            // Unsupported tag (e.g. markdown-imported
                            // "mermaid"): show it, unhighlighted, so the
                            // user sees what's set rather than a lie.
                            <Show when={
                                let current = current.clone();
                                move || !current.is_empty() && !known
                            }>
                                <option value=current.clone() selected=true>
                                    {current.clone()}
                                </option>
                            </Show>
                            {Language::ALL
                                .into_iter()
                                .map(|lang| {
                                    let tag = lang.tag();
                                    view! {
                                        <option
                                            value=tag
                                            selected={
                                                Language::from_tag(&s.current)
                                                    == Some(lang)
                                            }
                                        >
                                            {lang.label()}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </div>
                }
            })}
        </Show>
    }
}
```

Register in `frontend/src/components/mod.rs` exactly the way `mermaid_modal` is (same `pub mod` / re-export style).

- [ ] **Step 3: Wire into `editor_component.rs`.** Three additions, mirroring the mermaid-modal plumbing you read in Step 1 (adapt captured variable names to that code — the shapes below are the contract, the local names come from the file):

(a) Signal, next to `mermaid_modal_state` (~1717):

```rust
    let code_lang_chip_state: RwSignal<Option<CodeLangChipState>> = RwSignal::new(None);
```

(b) Position updater — a helper function in the same file plus a call to it wherever the component reacts to a completed dispatch/state change (the same place toolbar state is refreshed). It reads the live DOM selection and finds the enclosing `<pre>`:

```rust
/// Recompute the language-chip overlay state from the current editor
/// state + live DOM selection. Chip shows iff the caret is inside a
/// code block. Coordinates are relative to the editor wrapper (the
/// chip's offset parent), which must be position:relative.
fn refresh_code_lang_chip(
    state: &crate::editor::state::EditorState,
    wrapper: &web_sys::Element,
    chip: RwSignal<Option<CodeLangChipState>>,
) {
    use crate::editor::commands;

    let Some(current) = commands::code_block_language(state) else {
        chip.set(None);
        return;
    };
    // Find the code block's <pre> from the DOM selection anchor.
    let pre = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_selection().ok().flatten())
        .and_then(|s| s.anchor_node())
        .and_then(|n| match n.dyn_ref::<web_sys::Element>() {
            Some(el) => Some(el.clone()),
            None => n.parent_element(),
        })
        .and_then(|el| el.closest("pre").ok().flatten());
    let Some(pre) = pre else {
        chip.set(None);
        return;
    };
    let pre_rect = pre.get_bounding_client_rect();
    let wrap_rect = wrapper.get_bounding_client_rect();
    chip.set(Some(CodeLangChipState {
        top: pre_rect.top() - wrap_rect.top() + 4.0,
        right: wrap_rect.right() - pre_rect.right() + 4.0,
        current,
    }));
}
```

`web_sys` feature flags: `Selection` and `DomRect` may need adding to the `web-sys` features list in `frontend/Cargo.toml` (`[dependencies.web-sys]` — check whether `Selection` is already listed; `get_selection`, `closest`, `get_bounding_client_rect` require `Selection`, `Element`, `DomRect`).

Call `refresh_code_lang_chip(...)` from the state-change path identified in Step 1, and also clear the chip (`chip.set(None)`) on editor blur if the file has a blur handler that hides other floating UI (match what the at-menu does on blur).

(c) Selection callback + mount, next to the MermaidModal mount (~2949). The dispatch plumbing copies `on_mermaid_outcome` (2161–2190) — borrow `view_ref`, apply through the history ref, fire the change callbacks:

```rust
    let on_code_lang_select = Callback::new(move |tag: String| {
        // identical borrow/dispatch scaffolding to on_mermaid_outcome —
        // state comes from the view, the transaction goes through the
        // same history + on_change/on_state_change/on_mapping routing.
        // Inside that scaffolding the actual command call is:
        //   commands::set_code_block_language(&tag, &state, Some(&dispatch));
        // After dispatch, refresh_code_lang_chip(...) so the chip's
        // `current` reflects the new value immediately.
    });
```

```rust
            <CodeLangChip
                state=code_lang_chip_state
                on_select=on_code_lang_select
            />
```

The mount must be inside the positioned editor wrapper element (the same container whose rect `refresh_code_lang_chip` subtracts) — NOT inside the contenteditable div.

- [ ] **Step 4: Chip CSS** — append to the token-color block in `main.css`:

```css
.code-lang-chip {
  position: absolute;
  z-index: 20;
}
.code-lang-chip select {
  font-size: 12px;
  padding: 1px 4px;
  border-radius: 4px;
  border: 1px solid var(--border-color, #d0d7de);
  background: var(--bg-elev-1, #fff);
  color: inherit;
  opacity: 0.55;
}
.code-lang-chip select:hover,
.code-lang-chip select:focus {
  opacity: 1;
}
```

(Check `main.css` for the real border/background variable names near the `.mermaid-block` styles at main.css:5869 and use those.)

- [ ] **Step 5: Build both targets**

Run: `cd frontend && cargo test && cargo check --target wasm32-unknown-unknown`
Expected: PASS / clean. (The chip has no native-testable pure logic beyond what Task 6 covered; its correctness is exercised in Task 9's runtime verification.)

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/code_lang_chip.rs frontend/src/components/mod.rs frontend/src/components/editor_component.rs frontend/style/main.css frontend/Cargo.toml frontend/Cargo.lock
git commit -m "feat(editor): language-selector chip for code blocks"
```

---

### Task 8: Highlighted HTML export

**Files:**
- Modify: `crates/collab/Cargo.toml` (add dependency)
- Modify: `crates/collab/src/export.rs` (`render_node_html` at 723; new helper; tests)
- Modify (deliberate behavior change): the existing test `html_code_block_with_language` (export.rs:1518)

**Interfaces:**
- Consumes: `ogrenotes_highlight::{highlight, color_for, Language, TokenKind, MAX_HIGHLIGHT_CHARS}`.
- Produces: HTML export shape for a code block with a *supported* language becomes `<pre><code class="language-{lang}">…<span style="color:#…">…</span>…</code></pre>`. Unknown/empty language output is byte-identical to today (`<pre>…</pre>` via the generic path).

**Wire-shape note (deliberate, spec-approved):** today's export emits `<pre class="language-x">text</pre>` — the class sits on `<pre>` and there is no `<code>` wrapper. The new shape matches the live editor DOM and highlight.js conventions; the frontend paste path already reads `language-*` off either element (`code_language_from_class`, clipboard.rs:432 scans class tokens) and flattens spans via `text_content()` (clipboard.rs:291), so round-trip is safe. This is exactly the change the existing test update below surfaces.

- [ ] **Step 1: Add the dependency** — in `crates/collab/Cargo.toml` `[dependencies]`, next to `ogrenotes-mermaid`:

```toml
ogrenotes-highlight = { workspace = true }
```

- [ ] **Step 2: Write the failing tests** — in export.rs's test module, next to `html_code_block_no_language` (export.rs:2073):

```rust
    #[test]
    fn html_code_block_highlights_supported_language() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, "fn main() {}");
        });
        let html = to_html(&doc);
        assert!(html.contains("<pre><code class=\"language-rust\">"), "got: {html}");
        // `fn` is a keyword token with the light-palette keyword color.
        assert!(
            html.contains("<span style=\"color:#cf222e\">fn</span>"),
            "got: {html}"
        );
        assert!(html.contains("</code></pre>"), "got: {html}");
        // Reassembling the visible text must reproduce the source.
        assert!(html.contains("main"), "got: {html}");
    }

    #[test]
    fn html_code_block_escapes_hostile_content_per_token() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, "let x = \"</code><script>alert(1)</script>\";");
        });
        let html = to_html(&doc);
        assert!(!html.contains("<script>"), "raw script must never appear: {html}");
        assert!(html.contains("&lt;script&gt;"), "got: {html}");
    }

    #[test]
    fn html_code_block_unknown_language_keeps_legacy_shape() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "mermaid");
            insert_text(txn, &cb, "pie title x");
        });
        let html = to_html(&doc);
        // Exactly today's output: class on <pre>, no <code>, no spans.
        assert!(html.contains("<pre class=\"language-mermaid\">"), "got: {html}");
        assert!(!html.contains("<code"), "got: {html}");
        assert!(!html.contains("tok-"), "got: {html}");
    }

    #[test]
    fn html_code_block_oversized_renders_unhighlighted() {
        let big = "x".repeat(ogrenotes_highlight::MAX_HIGHLIGHT_CHARS + 1);
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, &big);
        });
        let html = to_html(&doc);
        assert!(html.contains("<pre><code class=\"language-rust\">"), "got: {html}");
        assert!(!html.contains("<span"), "no spans over the size cap: {html}");
    }
```

Match the surrounding tests' exact `doc_with`/`insert_text` helper usage (they're defined in this test module — copy the idiom from `html_code_block_no_language`).

- [ ] **Step 3: Update the one existing test (deliberate behavior change).** `html_code_block_with_language` (export.rs:1518) currently asserts `html.contains("fn main() {}")` — token spans now split that text. Change ONLY the two assertions to:

```rust
        assert!(html.contains("class=\"language-rust\""), "got: {html}");
        assert!(html.contains("main"), "got: {html}");
```

Add a comment above the test: `// Updated for syntax-highlighted export (2026-07-09 spec): token spans split the literal text; class moved to the <code> wrapper.`

- [ ] **Step 4: Run to verify failures**

Run: `cargo test -p ogrenotes-collab html_code_block`
Expected: the four new tests FAIL; the updated test FAILS (still old output).

- [ ] **Step 5: Implement.** In `render_node_html` (export.rs:723), after `node_type` is resolved and BEFORE the `is_leaf()` branch, add:

```rust
            // Syntax-highlighted code blocks take a dedicated path so
            // the export matches the live editor DOM
            // (<pre><code class="language-x">…tok spans…</code></pre>).
            // Unknown/empty languages and non-text content fall through
            // to the generic path — byte-identical to the pre-highlight
            // output.
            if node_type == NodeType::CodeBlock {
                if render_code_block_highlighted(txn, el, out) {
                    return;
                }
            }
```

And add the helper next to `render_html_attrs`:

```rust
/// Emit a highlighted code block. Returns false (emitting nothing)
/// when the language is unsupported/empty, the content isn't plain
/// text runs, or the text exceeds the highlight size cap — the
/// caller then falls through to the legacy generic path.
fn render_code_block_highlighted<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
) -> bool {
    let lang_attr = match el.get_attribute(txn, "language") {
        Some(l) if !l.is_empty() => l,
        _ => return false,
    };
    let Some(lang) = ogrenotes_highlight::Language::from_tag(&lang_attr) else {
        return false;
    };

    // Collect the block's text; bail on any non-text child.
    let mut text = String::new();
    let len = el.len(txn);
    for i in 0..len {
        match el.get(txn, i) {
            Some(XmlOut::Text(t)) => text.push_str(&t.get_string(txn)),
            Some(_) => return false,
            None => {}
        }
    }

    out.push_str(&format!(
        "<pre><code class=\"language-{}\">",
        html_escape_attr(&lang_attr)
    ));
    if text.chars().count() > ogrenotes_highlight::MAX_HIGHLIGHT_CHARS {
        out.push_str(&html_escape(&text));
    } else {
        for token in ogrenotes_highlight::highlight(&text, lang) {
            match ogrenotes_highlight::color_for(token.kind, false) {
                None => out.push_str(&html_escape(token.text)),
                Some(color) => out.push_str(&format!(
                    "<span style=\"color:{}\">{}</span>",
                    color,
                    html_escape(token.text)
                )),
            }
        }
    }
    out.push_str("</code></pre>");
    true
}
```

Check how `render_text_html` (export.rs:809 area) actually reads a text node's string (`t.get_string(txn)` or similar) and use the same call. Note `XmlOut::Text(t)` variants appear at export.rs:205/809 — copy the working accessor.

- [ ] **Step 6: Run the export tests**

Run: `cargo test -p ogrenotes-collab export`
Expected: PASS, including all pre-existing export tests (`html_code_block_no_language` proves the legacy path is untouched) and `to_plain_text`/search-related tests (text extraction reads the model, not the HTML — unaffected).

- [ ] **Step 7: Commit**

```bash
git add crates/collab/Cargo.toml Cargo.lock crates/collab/src/export.rs
git commit -m "feat(export): inline-styled syntax highlighting in HTML export

Deliberate wire-shape change for supported languages:
<pre class=\"language-x\"> becomes <pre><code class=\"language-x\">
with per-token inline-styled spans, matching the live editor DOM.
Unknown/empty languages keep the exact legacy output. Updates
html_code_block_with_language accordingly (behavior change, per
the approved 2026-07-09 spec)."
```

---

### Task 9: Full verification + runtime check

**Files:** none (verification only)

- [ ] **Step 1: Full workspace test suite**

Run: `cargo test --workspace`
Expected: PASS. Pay attention to `ogrenotes-collab` (export + schema-duality CI test) and `ogrenotes-highlight`.

- [ ] **Step 2: Frontend native tests + wasm build**

Run: `cd frontend && cargo test && cargo check --target wasm32-unknown-unknown`
Expected: PASS / clean.

- [ ] **Step 3: Bundle size sanity**

Run: `cd frontend && trunk build --release && sh -c 'gzip -c dist/*_bg.wasm | wc -c'`
Expected: a number comfortably under `1700000` (current baseline ≈ 1.36 MiB ≈ 1,426,000; the highlight crate should add well under 100 KB). If it's over ~1,500,000, stop and investigate before proceeding.

- [ ] **Step 4: Runtime verification (verify skill / run the app).** Use the project's run/verify flow to drive the real editor:
  1. Create a code block via the toolbar `</>` button, type `fn main() { println!("hi"); }`.
  2. The chip appears top-right showing "Plain text"; select "Rust" → keywords/strings colorize immediately.
  3. Type inside the highlighted block — caret must land exactly where typed, including after newlines and inside strings; Backspace/Delete/Enter behave normally (this is the position-mapping acceptance check).
  4. Select-all-copy the block, paste into a fresh paragraph area → pastes as a code block with language preserved, no `tok-` markup in the model.
  5. Markdown-import a ` ```python ` fence → renders highlighted without touching the chip.
  6. Toggle dark theme → token colors switch.
  7. Export the doc as HTML → code block is colored in the exported file; a ` ```mermaid ` code block (not the Mermaid atom) exports exactly as before.

- [ ] **Step 5: Wrap up the branch.** All commits are already on the worktree branch. Verify `git log --oneline main..HEAD` shows the 8 task commits + the spec/plan docs, then use superpowers:finishing-a-development-branch (typically: push and open a PR referencing the spec; do NOT merge without review).

---

## Notes for the reviewer / executor

- **Engine freeze:** `lexer.rs` is frozen after Task 2. Language-quality gaps found later are spec-table tweaks (Task 3 file), never engine rewrites mid-plan.
- **Known fidelity limits (accepted by design):** no nested-grammar handling (JS inside HTML `<script>` stays plain-ish), no string-interpolation highlighting inside quoted strings (JS `${}` / Kotlin `$x` render as part of the string), Rust char literals uncolored (lifetime safety), SQL identifiers-in-quotes treated as strings. These are GitHub-comment-level trade-offs, not bugs.
- **Deviations from spec (surfaced):** (1) chip visibility is caret-based only; hover-based visibility was trimmed (needs block-targeted dispatch — a clean follow-up once code blocks carry blockIds at creation). (2) The spec said "searchable list"; the plan uses a native `<select>` (20 options, browser type-ahead) — free accessibility, no bespoke popover code. Swap for a filtered popover later if the language list grows.
- **Design-doc drift:** `design/rich-text-editor.md:666` (`CodeBlockLowlight`) — already recorded in the spec; do not edit `design/`.
