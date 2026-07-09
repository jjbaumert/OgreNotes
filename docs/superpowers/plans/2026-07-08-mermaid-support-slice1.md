# Mermaid Support — Slice 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the end-to-end Mermaid pipeline (author → render → export → share) with Pie as the only rendered diagram; all other diagram kinds and parse failures fall back to an error banner + raw source.

**Architecture:** A new standalone workspace crate `crates/mermaid` (`ogrenotes-mermaid`, pure std, `#![forbid(unsafe_code)]`) exposes `render(&str) -> RenderOutput`. A new `NodeType::Mermaid` (leaf atom, source in a `source` attribute) is mirrored across `crates/collab/src/schema.rs` and the frontend schema. The server (`collab::export`) and the client (`frontend` block view + edit modal) both call the same `render()`, so the SVG appears identically in the live view, HTML/PDF export/print, and read-only link-share.

**Tech Stack:** Rust (workspace crate + `collab` native), Leptos/`web_sys` WASM frontend, `yrs` CRDT, Fluent i18n.

## Global Constraints

- Crate `crates/mermaid`: package name `ogrenotes-mermaid`, import path `ogrenotes_mermaid`; `#![forbid(unsafe_code)]`; **no dependencies beyond `std`** (must compile to `wasm32-unknown-unknown` for the frontend).
- `render()` **must never panic** on any input (including empty, huge, non-UTF-8-boundary-ish, adversarial).
- New node type is added to **both** schemas; the collab-side cross-schema tests are the contract — they must stay green.
- Tests are immutable contracts: only add tests; do not weaken existing ones. Adding `Mermaid` to the hardcoded cross-schema lists is the intended update, not a weakening.
- Do **not** `git add -A` / `git add .` in this repo — stage explicit paths only.
- All user-controlled strings interpolated into SVG/HTML must be XML/HTML-escaped; the raw-source error fallback must use `set_text_content` (DOM) / `html_escape` (export), never `set_inner_html` / unescaped interpolation.
- Frontend changes must be verified with an **explicit wasm32 build** (`cd frontend && cargo build --target wasm32-unknown-unknown`), because native `cargo check` skips wasm-only linkage.
- Commit message trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Leaf-atom copy: `Mermaid` is a **leaf** (`leaf: true`) **atom** (`atom: true`), `block: true`, `isolating: true` — mirror `Embed`'s NodeSpec exactly.

---

### Task 1: `crates/mermaid` crate skeleton + `render()` contract + kind detection

Creates the crate, the public types, diagram-kind detection, and the never-panics guarantee. Pie parsing/rendering is Task 2; here every kind except Pie returns a structured error, and Pie returns a "not yet parsed" placeholder error until Task 2 fills it in.

**Files:**
- Create: `crates/mermaid/Cargo.toml`
- Create: `crates/mermaid/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`, add `"crates/mermaid"`)
- Test: inline `#[cfg(test)] mod tests` in `crates/mermaid/src/lib.rs`

**Interfaces:**
- Produces:
  - `pub fn render(source: &str) -> RenderOutput`
  - `pub struct RenderOutput { pub kind: DiagramKind, pub svg: Option<String>, pub error: Option<ParseError> }`
  - `pub enum DiagramKind { Pie, Flowchart, Sequence, State, Class, Er, Unknown }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct ParseError { pub message: String, pub line: Option<usize> }` (derives `Debug, Clone, PartialEq, Eq`)
  - `pub fn detect_kind(source: &str) -> DiagramKind`
  - `impl DiagramKind { pub fn label(self) -> &'static str }`

- [ ] **Step 1: Add the crate to the workspace members list**

In root `Cargo.toml`, add `"crates/mermaid",` to the `members` array (keep `frontend` in `exclude`):

```toml
members = [
    "crates/common",
    "crates/storage",
    "crates/auth",
    "crates/collab",
    "crates/mermaid",
    "crates/search",
    "crates/embeddings",
    "crates/notify",
    "crates/worker",
    "crates/api",
]
```

- [ ] **Step 2: Create `crates/mermaid/Cargo.toml`**

```toml
[package]
name = "ogrenotes-mermaid"
version = "0.1.0"
edition = "2021"
license = "LicenseRef-Proprietary"
publish = false

[lints]
workspace = true

[dependencies]
```

(If `cargo build` reports the `[lints] workspace = true` is undefined for this workspace, drop the `[lints]` block — check whether other crates such as `crates/common/Cargo.toml` use it, and match them.)

- [ ] **Step 3: Write the failing test for kind detection + never-panics + placeholder contract**

Create `crates/mermaid/src/lib.rs` with only the test module referencing not-yet-existing items so it fails to compile:

```rust
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_known_kind() {
        assert_eq!(detect_kind("pie\n\"A\": 1"), DiagramKind::Pie);
        assert_eq!(detect_kind("pie showData"), DiagramKind::Pie);
        assert_eq!(detect_kind("graph TD\nA-->B"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("flowchart LR"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("sequenceDiagram"), DiagramKind::Sequence);
        assert_eq!(detect_kind("stateDiagram-v2"), DiagramKind::State);
        assert_eq!(detect_kind("classDiagram"), DiagramKind::Class);
        assert_eq!(detect_kind("erDiagram"), DiagramKind::Er);
        assert_eq!(detect_kind("nonsense here"), DiagramKind::Unknown);
    }

    #[test]
    fn detection_skips_blank_and_comment_lines() {
        assert_eq!(detect_kind("\n\n  %% a comment\npie\n\"A\": 1"), DiagramKind::Pie);
    }

    #[test]
    fn unsupported_kind_returns_error_with_kind_preserved() {
        let out = render("sequenceDiagram\nAlice->>Bob: hi");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.svg.is_none());
        let err = out.error.expect("unsupported kind must carry an error");
        assert!(err.message.to_lowercase().contains("not yet supported"), "got: {}", err.message);
    }

    #[test]
    fn unknown_kind_returns_error() {
        let out = render("total gibberish");
        assert_eq!(out.kind, DiagramKind::Unknown);
        assert!(out.svg.is_none());
        assert!(out.error.is_some());
    }

    #[test]
    fn render_never_panics_on_adversarial_input() {
        let inputs = [
            "",
            " ",
            "\n\n\n",
            "%%",
            "pie",
            "pie\n:",
            "pie\n\"\":",
            "pie\n\"x\": notanumber",
            &"pie\n".repeat(100_000),
            &"\"a\": 1\n".repeat(100_000),
            "🥧 pie 🥧",
        ];
        for inp in inputs {
            let _ = render(inp); // must return, not panic
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it fails to compile**

Run: `cargo test -p ogrenotes-mermaid`
Expected: FAIL — `cannot find function render`, `cannot find type DiagramKind`, etc.

- [ ] **Step 5: Implement the public types, `detect_kind`, and `render` (Pie path is a placeholder error until Task 2)**

Prepend to `crates/mermaid/src/lib.rs` (above the test module):

```rust
//! Pure-Rust Mermaid → SVG renderer. `render()` never panics; every
//! failure or not-yet-supported diagram kind returns a structured error
//! and no SVG, so callers can fall back to raw source. See
//! docs/superpowers/specs/2026-07-08-mermaid-support-design.md.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramKind {
    Pie,
    Flowchart,
    Sequence,
    State,
    Class,
    Er,
    Unknown,
}

impl DiagramKind {
    /// Human-facing name used in "‹label› not yet supported" errors.
    pub fn label(self) -> &'static str {
        match self {
            DiagramKind::Pie => "pie",
            DiagramKind::Flowchart => "flowchart",
            DiagramKind::Sequence => "sequence",
            DiagramKind::State => "state",
            DiagramKind::Class => "class",
            DiagramKind::Er => "entity-relationship",
            DiagramKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// 1-based source line the error points at, when known.
    pub line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutput {
    pub kind: DiagramKind,
    pub svg: Option<String>,
    pub error: Option<ParseError>,
}

/// First meaningful (non-blank, non-`%%`-comment) line's leading keyword
/// selects the diagram kind.
pub fn detect_kind(source: &str) -> DiagramKind {
    let header = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"));
    let Some(header) = header else {
        return DiagramKind::Unknown;
    };
    let keyword = header.split_whitespace().next().unwrap_or("");
    match keyword {
        "pie" => DiagramKind::Pie,
        "graph" | "flowchart" => DiagramKind::Flowchart,
        "sequenceDiagram" => DiagramKind::Sequence,
        "stateDiagram" | "stateDiagram-v2" => DiagramKind::State,
        "classDiagram" => DiagramKind::Class,
        "erDiagram" => DiagramKind::Er,
        _ => DiagramKind::Unknown,
    }
}

/// Render mermaid `source` to an SVG string. Never panics.
pub fn render(source: &str) -> RenderOutput {
    let kind = detect_kind(source);
    match kind {
        DiagramKind::Pie => {
            // Filled in by Task 2. Until then, a structured error.
            RenderOutput {
                kind,
                svg: None,
                error: Some(ParseError {
                    message: "pie rendering not yet implemented".to_string(),
                    line: None,
                }),
            }
        }
        DiagramKind::Unknown => RenderOutput {
            kind,
            svg: None,
            error: Some(ParseError {
                message: "unrecognized diagram type".to_string(),
                line: None,
            }),
        },
        other => RenderOutput {
            kind,
            svg: None,
            error: Some(ParseError {
                message: format!("{} diagrams are not yet supported", other.label()),
                line: None,
            }),
        },
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p ogrenotes-mermaid`
Expected: PASS (5 tests). Note: `unsupported_kind_returns_error_with_kind_preserved` and the never-panics test pass now; the Pie-specific behavior is exercised in Task 2.

- [ ] **Step 7: Verify the crate compiles to wasm32 (frontend will depend on it)**

Run: `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`
Expected: builds clean. (If the target isn't installed: `rustup target add wasm32-unknown-unknown`.)

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/mermaid/Cargo.toml crates/mermaid/src/lib.rs
git commit -m "feat(mermaid): renderer crate skeleton + kind detection

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Pie parser + SVG rendering

Fills in the Pie branch of `render()`: parse `pie [showData]` / optional `title` / `"label": value` lines and emit a self-contained, theme-aware `<svg>`.

**Files:**
- Create: `crates/mermaid/src/pie.rs`
- Modify: `crates/mermaid/src/lib.rs` (add `mod pie;`, wire the Pie branch, add `escape_xml` if shared)
- Test: inline tests in `crates/mermaid/src/pie.rs`

**Interfaces:**
- Consumes: `ParseError` from Task 1.
- Produces (crate-internal):
  - `pub(crate) struct Pie { pub title: Option<String>, pub show_data: bool, pub slices: Vec<(String, f64)> }`
  - `pub(crate) fn parse(source: &str) -> Result<Pie, ParseError>`
  - `pub(crate) fn render_svg(pie: &Pie) -> String`
  - `pub(crate) fn escape_xml(s: &str) -> String`

- [ ] **Step 1: Write failing tests for the pie parser**

Create `crates/mermaid/src/pie.rs`:

```rust
//! Mermaid `pie` diagram: parser + SVG renderer.

use crate::ParseError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_slices() {
        let p = parse("pie title Pets\n\"Dogs\" : 50\n\"Cats\" : 30").unwrap();
        assert_eq!(p.title.as_deref(), Some("Pets"));
        assert!(!p.show_data);
        assert_eq!(p.slices, vec![("Dogs".to_string(), 50.0), ("Cats".to_string(), 30.0)]);
    }

    #[test]
    fn parses_show_data_flag() {
        let p = parse("pie showData\n\"A\" : 1").unwrap();
        assert!(p.show_data);
    }

    #[test]
    fn parses_bare_unquoted_labels() {
        let p = parse("pie\nDogs : 2\nCats : 3").unwrap();
        assert_eq!(p.slices, vec![("Dogs".to_string(), 2.0), ("Cats".to_string(), 3.0)]);
    }

    #[test]
    fn allows_zero_value() {
        let p = parse("pie\n\"A\" : 0\n\"B\" : 5").unwrap();
        assert_eq!(p.slices[0].1, 0.0);
    }

    #[test]
    fn empty_data_is_error() {
        let err = parse("pie").unwrap_err();
        assert!(err.message.to_lowercase().contains("no data") || err.message.to_lowercase().contains("empty"));
    }

    #[test]
    fn missing_header_is_error() {
        assert!(parse("\"A\" : 1").is_err());
    }

    #[test]
    fn negative_value_is_error() {
        let err = parse("pie\n\"A\" : -3").unwrap_err();
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn non_numeric_value_is_error() {
        assert!(parse("pie\n\"A\" : abc").is_err());
    }

    #[test]
    fn svg_has_header_and_one_path_per_slice() {
        let p = parse("pie\n\"A\" : 1\n\"B\" : 1\n\"C\" : 2").unwrap();
        let svg = render_svg(&p);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert_eq!(svg.matches("<path").count(), 3, "one wedge per slice");
        // theme-aware text
        assert!(svg.contains("currentColor"));
    }

    #[test]
    fn svg_escapes_label_markup() {
        let p = parse("pie\n\"<script>\" : 1").unwrap();
        let svg = render_svg(&p);
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn show_data_appends_values_to_legend() {
        let p = parse("pie showData\n\"A\" : 7").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains('7'));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-mermaid pie`
Expected: FAIL to compile — `parse`, `render_svg`, `Pie` not defined.

- [ ] **Step 3: Implement the pie parser**

Add above the test module in `crates/mermaid/src/pie.rs`:

```rust
pub(crate) struct Pie {
    pub title: Option<String>,
    pub show_data: bool,
    pub slices: Vec<(String, f64)>,
}

/// Parse a `pie` diagram. Line numbers in errors are 1-based.
pub(crate) fn parse(source: &str) -> Result<Pie, ParseError> {
    let mut title = None;
    let mut show_data = false;
    let mut slices = Vec::new();
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            // Header line: `pie` optionally followed by `showData`.
            let mut toks = line.split_whitespace();
            if toks.next() != Some("pie") {
                return Err(ParseError {
                    message: "pie diagram must start with `pie`".to_string(),
                    line: Some(line_no),
                });
            }
            if toks.next() == Some("showData") {
                show_data = true;
            }
            seen_header = true;
            continue;
        }
        // `title <text>` (only before/among data lines).
        if let Some(rest) = line.strip_prefix("title ") {
            title = Some(rest.trim().to_string());
            continue;
        }
        // Data line: `"Label" : value` or `Label : value`.
        let Some((label_part, value_part)) = line.rsplit_once(':') else {
            return Err(ParseError {
                message: format!("expected `label : value`, got {line:?}"),
                line: Some(line_no),
            });
        };
        let label = label_part.trim().trim_matches('"').to_string();
        let value: f64 = value_part.trim().parse().map_err(|_| ParseError {
            message: format!("`{}` is not a number", value_part.trim()),
            line: Some(line_no),
        })?;
        if value < 0.0 || !value.is_finite() {
            return Err(ParseError {
                message: format!("value must be a non-negative number, got {value}"),
                line: Some(line_no),
            });
        }
        slices.push((label, value));
    }

    if !seen_header {
        return Err(ParseError {
            message: "pie diagram must start with `pie`".to_string(),
            line: None,
        });
    }
    if slices.is_empty() {
        return Err(ParseError {
            message: "pie diagram has no data slices".to_string(),
            line: None,
        });
    }
    Ok(Pie { title, show_data, slices })
}

/// XML-escape a user-supplied string before interpolating into SVG.
pub(crate) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
```

- [ ] **Step 4: Implement the SVG renderer**

Append to `crates/mermaid/src/pie.rs` (above the tests):

```rust
const PALETTE: &[&str] = &[
    "#2D5F2D", "#5C3D2E", "#4A90D9", "#D9534F",
    "#F0AD4E", "#5CB85C", "#9B59B6", "#E67E22",
];
const W: f64 = 420.0;
const H: f64 = 300.0;
const CX: f64 = 150.0;
const CY: f64 = 160.0;
const R: f64 = 110.0;

/// Render the pie as a self-contained SVG string. Slice fills come from
/// `PALETTE`; all text uses `currentColor` so it tracks the app theme.
pub(crate) fn render_svg(pie: &Pie) -> String {
    let total: f64 = pie.slices.iter().map(|(_, v)| *v).sum();
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" style="font-family:sans-serif;font-size:12px">"#
    );

    if let Some(title) = &pie.title {
        svg.push_str(&format!(
            r#"<text x="{cx}" y="24" text-anchor="middle" fill="currentColor" style="font-size:15px;font-weight:600">{t}</text>"#,
            cx = CX,
            t = escape_xml(title),
        ));
    }

    // Zero-total: draw an empty outlined circle, still emit one <path> per
    // slice (as zero-length arcs) so callers/tests see slice count.
    let mut angle = -std::f64::consts::FRAC_PI_2; // start at 12 o'clock
    for (i, (label, value)) in pie.slices.iter().enumerate() {
        let frac = if total > 0.0 { value / total } else { 0.0 };
        let sweep = frac * std::f64::consts::TAU;
        let (x0, y0) = (CX + R * angle.cos(), CY + R * angle.sin());
        let end = angle + sweep;
        let (x1, y1) = (CX + R * end.cos(), CY + R * end.sin());
        let large_arc = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let fill = PALETTE[i % PALETTE.len()];
        svg.push_str(&format!(
            r#"<path d="M {CX} {CY} L {x0:.2} {y0:.2} A {R} {R} 0 {large_arc} 1 {x1:.2} {y1:.2} Z" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1"/>"#
        ));
        angle = end;

        // Legend row.
        let ly = 60.0 + i as f64 * 22.0;
        svg.push_str(&format!(
            r#"<rect x="300" y="{ry:.0}" width="14" height="14" fill="{fill}"/>"#,
            ry = ly - 11.0,
        ));
        let legend = if pie.show_data {
            format!("{label} ({})", trim_num(*value))
        } else {
            label.clone()
        };
        svg.push_str(&format!(
            r#"<text x="320" y="{ly:.0}" fill="currentColor">{}</text>"#,
            escape_xml(&legend),
        ));
    }

    svg.push_str("</svg>");
    svg
}

/// Format a value without a trailing `.0` for whole numbers.
fn trim_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}
```

- [ ] **Step 5: Wire the Pie branch in `lib.rs`**

In `crates/mermaid/src/lib.rs`, add `mod pie;` near the top (after the doc comment) and replace the Pie placeholder arm in `render`:

```rust
        DiagramKind::Pie => match pie::parse(source) {
            Ok(p) => RenderOutput { kind, svg: Some(pie::render_svg(&p)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
```

- [ ] **Step 6: Add lib-level tests that Pie now renders and errors flow through**

Add to the `tests` module in `lib.rs`:

```rust
    #[test]
    fn pie_renders_svg_via_public_render() {
        let out = render("pie title T\n\"A\" : 1\n\"B\" : 1");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.error.is_none());
        let svg = out.svg.expect("pie should render");
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn pie_parse_error_flows_through_render() {
        let out = render("pie\n\"A\" : notanumber");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.svg.is_none());
        assert!(out.error.is_some());
    }
```

- [ ] **Step 7: Run all crate tests + wasm build**

Run: `cargo test -p ogrenotes-mermaid`
Expected: PASS (all pie + lib tests).
Run: `cargo build -p ogrenotes-mermaid --target wasm32-unknown-unknown`
Expected: builds clean.

- [ ] **Step 8: Commit**

```bash
git add crates/mermaid/src/lib.rs crates/mermaid/src/pie.rs
git commit -m "feat(mermaid): pie parser + theme-aware SVG renderer

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: `NodeType::Mermaid` in the collab (canonical) schema + cross-schema tests

Adds the variant to `crates/collab/src/schema.rs` and updates the hardcoded cross-schema test lists (the CI contract).

**Files:**
- Modify: `crates/collab/src/schema.rs`

**Interfaces:**
- Produces: `NodeType::Mermaid` (collab), tag string `"mermaid"`, `is_leaf()==true`, `is_block()==true`, in `Doc.valid_children()`.

- [ ] **Step 1: Add the enum variant + `tag_name`/`from_tag`/`is_block`/`is_leaf`/`valid_children` arms, and update every cross-schema test list**

In `crates/collab/src/schema.rs`:

1. Add the variant after `Mention` (near line 73):
```rust
    /// Mermaid diagram block. Leaf atom; source stored in the
    /// `source` attribute. Rendered to SVG by `ogrenotes-mermaid`
    /// on both the client (live view) and server (HTML export).
    /// Mirrors the same-named variant in
    /// `frontend/src/editor/model.rs`.
    Mermaid,
```
2. `tag_name()` (near line 97): add `NodeType::Mermaid => "mermaid",`
3. `from_tag()` (near line 127): add `"mermaid" => Some(NodeType::Mermaid),`
4. `is_block()` `matches!` (near line 157): add `| NodeType::Mermaid`
5. `is_leaf()` `matches!` (near line 172): add `| NodeType::Mermaid`
6. `valid_children()` Doc arm (near line 204): add `NodeType::Mermaid,` to the Doc `&[...]`
7. `valid_children()` leaf `=> &[]` arm (near line 253): add `| NodeType::Mermaid`

Then the tests (same file, `#[cfg(test)] mod tests`):

8. `ALL_NODE_TYPES` const (near line 455): add `NodeType::Mermaid,`
9. `cross_schema_node_type_count` (near line 513): change `24` → `25` (both the number and the message).
10. `cross_schema_tag_names` expected list (near line 573): add `("mermaid", NodeType::Mermaid),`
11. `cross_schema_leaf_nodes` `expected_leaves` (near line 611): add `NodeType::Mermaid,`
12. `cross_schema_valid_children` Doc array (near line 641): add `NodeType::Mermaid,`
13. `node_type_tag_roundtrip` `types` array (near line 355): add `NodeType::Mermaid,`

- [ ] **Step 2: Run the cross-schema tests**

Run: `cargo test -p ogrenotes-collab schema::tests`
Expected: PASS — `cross_schema_node_type_count`, `cross_schema_tag_names`, `cross_schema_leaf_nodes`, `cross_schema_valid_children`, `cross_schema_all_node_tags_roundtrip`, `node_type_tag_roundtrip` all green.

- [ ] **Step 3: Build collab to confirm the exhaustive matches are covered**

Run: `cargo build -p ogrenotes-collab`
Expected: builds (any missing match arm on the exhaustive `NodeType` matches would be a compile error).

- [ ] **Step 4: Commit**

```bash
git add crates/collab/src/schema.rs
git commit -m "feat(collab): add NodeType::Mermaid to canonical schema

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: Mirror `NodeType::Mermaid` into the frontend schema + wire mappings

Adds the variant to `frontend/src/editor/model.rs`, its `NodeSpec` in `frontend/src/editor/schema.rs`, and the yrs-bridge + view tag mappings.

**Files:**
- Modify: `frontend/src/editor/model.rs`
- Modify: `frontend/src/editor/schema.rs`
- Modify: `frontend/src/editor/yrs_bridge.rs`
- Modify: `frontend/src/editor/view.rs`

**Interfaces:**
- Consumes: nothing (parallel to Task 3).
- Produces: frontend `NodeType::Mermaid` with `is_leaf()==true`, `is_atom()==true`, `is_block()==true`, `NodeSpec { leaf:true, atom:true, block:true, isolating:true, valid_children:[], allowed_marks:Some(vec![]) }`, wire tag `"mermaid"`.

- [ ] **Step 1: Add the variant + method arms in `model.rs`**

In `frontend/src/editor/model.rs`:
1. Add the variant after `Mention` (near line 186):
```rust
    /// Mermaid diagram block. Block-level leaf atom; source stored in
    /// the `source` attribute; rendered to SVG by `ogrenotes-mermaid`.
    /// Mirrors the same-named variant in `crates/collab/src/schema.rs`.
    Mermaid,
```
2. `is_leaf()` `matches!` (near line 200): add `| NodeType::Mermaid`
3. `is_atom()` `matches!` (near line 227): add `| NodeType::Mermaid`
4. `is_commentable()` (near line 269, exhaustive): add `NodeType::Mermaid` to the `=> false` group
5. `needs_block_id()` (near line 313, exhaustive): add `NodeType::Mermaid` to the `=> true` group
6. Leave `is_inline`, `is_textblock`, `default_attrs` unchanged (Mermaid is not inline/textblock; source is set at insert time, no default-attrs arm — mirrors Embed).

- [ ] **Step 2: Add the `NodeSpec` + Doc child in `frontend/src/editor/schema.rs`**

1. Add `NodeType::Mermaid,` to the Doc `valid_children` vec (near line 261).
2. Insert the spec (mirror Embed at lines 512-530), near the Embed block:
```rust
    // Mermaid: leaf block atom. Source in the `source` attribute;
    // rendered to SVG by the MermaidView. Mirror of Embed's spec.
    nodes.insert(
        NodeType::Mermaid,
        NodeSpec {
            valid_children: vec![],
            inline_content: false,
            block: true,
            leaf: true,
            code: false,
            atom: true,
            defining: false,
            isolating: true,
            default_attrs: HashMap::new(),
            allowed_marks: Some(vec![]),
        },
    );
```

- [ ] **Step 3: Add the wire-tag mapping in `yrs_bridge.rs` (both directions)**

In `frontend/src/editor/yrs_bridge.rs`:
1. `node_type_to_tag` (around lines 812-821): add `NodeType::Mermaid => "mermaid",`
2. `tag_to_node_type` (around lines 825-852): add `"mermaid" => Some(NodeType::Mermaid),`

- [ ] **Step 4: Add the base tag in `view.rs`**

In `frontend/src/editor/view.rs` `node_type_to_tag` (near line 1505): add `NodeType::Mermaid => "div",` (the `view_for` fallback overrides rendering; the base tag only matters if no block view is registered — a `div` is safe).

- [ ] **Step 5: Build the frontend for wasm32**

Run: `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: builds. Any un-handled exhaustive match arm (`is_commentable`, `needs_block_id`) surfaces here as a compile error.

- [ ] **Step 6: Run frontend editor unit tests (native)**

Run: `cd frontend && cargo test editor::schema editor::model`
Expected: PASS (existing schema/model tests still green with the new variant).

- [ ] **Step 7: Commit**

```bash
git add frontend/src/editor/model.rs frontend/src/editor/schema.rs frontend/src/editor/yrs_bridge.rs frontend/src/editor/view.rs
git commit -m "feat(frontend): mirror NodeType::Mermaid in frontend schema + wire tags

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Server-side `MermaidBlock` validation + registry wiring

Adds `crates/collab/src/blocks/mermaid.rs` implementing `LiveAppBlock`, registers it in `BLOCKS`, and exports `MERMAID_ATTR_NAMES` for the export path. Over-cap source is **hard-rejected** (not clamped), because a silent clamp surfaces as a write-gate canonicalization violation.

**Files:**
- Create: `crates/collab/src/blocks/mermaid.rs`
- Modify: `crates/collab/src/blocks/mod.rs`

**Interfaces:**
- Consumes: `LiveAppBlock`, `BlockValidationError`, `NodeType::Mermaid`.
- Produces:
  - `pub struct MermaidBlock; pub static MERMAID: MermaidBlock;`
  - `pub const MAX_SOURCE_LEN: usize = 20_000;`
  - `pub const MERMAID_ATTR_NAMES: &[&str] = &["source"];`
  - `impl LiveAppBlock for MermaidBlock` with `node_types() -> &[NodeType::Mermaid]`.

- [ ] **Step 1: Write the failing validation tests**

Create `crates/collab/src/blocks/mermaid.rs`:

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mermaid diagram live-app block. `NodeType::Mermaid` is a leaf
//! carrying its diagram source in a `source` attribute. Rendered to
//! SVG by `ogrenotes-mermaid` on the export path; validation here just
//! caps length and preserves `blockId`.

use std::collections::HashMap;

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct MermaidBlock;
pub static MERMAID: MermaidBlock = MermaidBlock;

/// Max diagram source length (chars). Over-cap is hard-rejected so an
/// interactive write is not silently clamped (which the write gate
/// would flag as a canonicalization violation).
pub const MAX_SOURCE_LEN: usize = 20_000;

/// Single source of truth for the attribute names the export path
/// iterates. Mirrors `CALENDAR_ATTR_NAMES` / `CARD_ATTR_NAMES`.
pub const MERMAID_ATTR_NAMES: &[&str] = &["source"];

impl LiveAppBlock for MermaidBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Mermaid]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        if node_type != NodeType::Mermaid {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("node_type"),
                reason: format!("MermaidBlock cannot validate {}", node_type.tag_name()),
            });
        }
        let mut out = HashMap::new();
        let source = attrs.get("source").map(String::as_str).unwrap_or("");
        if source.trim().is_empty() {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: "mermaid source must not be empty".to_string(),
            });
        }
        if source.chars().count() > MAX_SOURCE_LEN {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: format!("source exceeds {MAX_SOURCE_LEN} chars"),
            });
        }
        // Echo unchanged so the write gate sees no canonicalization diff.
        out.insert("source".to_string(), source.to_string());
        // Preserve the CRDT anchor.
        if let Some(bid) = attrs.get("blockId") {
            out.insert("blockId".to_string(), bid.clone());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn valid_source_echoes_unchanged() {
        let out = MERMAID
            .validate_attrs(NodeType::Mermaid, &attrs(&[("source", "pie\n\"A\": 1")]))
            .unwrap();
        assert_eq!(out.get("source").map(String::as_str), Some("pie\n\"A\": 1"));
    }

    #[test]
    fn empty_source_rejected() {
        assert!(MERMAID.validate_attrs(NodeType::Mermaid, &attrs(&[("source", "   ")])).is_err());
    }

    #[test]
    fn oversized_source_rejected() {
        let big = "x".repeat(MAX_SOURCE_LEN + 1);
        assert!(MERMAID.validate_attrs(NodeType::Mermaid, &attrs(&[("source", &big)])).is_err());
    }

    #[test]
    fn preserves_block_id() {
        let out = MERMAID
            .validate_attrs(NodeType::Mermaid, &attrs(&[("source", "pie\n\"A\": 1"), ("blockId", "abc")]))
            .unwrap();
        assert_eq!(out.get("blockId").map(String::as_str), Some("abc"));
    }

    #[test]
    fn wrong_node_type_rejected() {
        assert!(MERMAID.validate_attrs(NodeType::Paragraph, &attrs(&[("source", "x")])).is_err());
    }
}
```

- [ ] **Step 2: Register the block in `mod.rs`**

In `crates/collab/src/blocks/mod.rs`:
1. Add `pub mod mermaid;` next to `pub mod calendar;` / `pub mod kanban;` (near line 23).
2. Add `&mermaid::MERMAID` to the `BLOCKS` array (near line 153):
```rust
pub const BLOCKS: &[&(dyn LiveAppBlock + 'static)] =
    &[&calendar::CALENDAR, &kanban::KANBAN, &mermaid::MERMAID];
```

- [ ] **Step 3: Add a registry test that Mermaid resolves**

Add to the `#[cfg(test)] mod tests` in `crates/collab/src/blocks/mod.rs`:
```rust
    #[test]
    fn block_for_mermaid_resolves() {
        let b = block_for(NodeType::Mermaid).expect("Mermaid has a block");
        assert!(b.node_types().contains(&NodeType::Mermaid));
    }
```

- [ ] **Step 4: Run block tests**

Run: `cargo test -p ogrenotes-collab blocks::`
Expected: PASS, including `block_for_mermaid_resolves`, `node_type_ownership_is_disjoint`, `every_block_owns_at_least_one_node_type`, and the new `mermaid::tests`.

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/blocks/mermaid.rs crates/collab/src/blocks/mod.rs
git commit -m "feat(collab): MermaidBlock validation + registry wiring

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 6: Export — HTML (SVG or error+raw) and Markdown (fenced block)

Wires `collab::export` to render the Mermaid node. HTML inlines the SVG or an error banner + `<pre>` raw source; Markdown emits a fenced ```mermaid block. Adds the `ogrenotes-mermaid` dependency to `collab`.

**Files:**
- Modify: `crates/collab/Cargo.toml`
- Modify: `crates/collab/src/export.rs`
- Test: inline tests in `crates/collab/src/export.rs`

**Interfaces:**
- Consumes: `ogrenotes_mermaid::render`, `NodeType::Mermaid`, `MERMAID_ATTR_NAMES`.

- [ ] **Step 1: Add the dependency**

In `crates/collab/Cargo.toml` `[dependencies]`:
```toml
ogrenotes-mermaid = { path = "../mermaid" }
```
(Use a path dep; the workspace has no central version table entry for it. If the workspace enforces `workspace = true` deps via lints, add `ogrenotes-mermaid = { path = "crates/mermaid" }` to the root `[workspace.dependencies]` and use `{ workspace = true }` here — match the pattern used by `ogrenotes-common` in `crates/collab/Cargo.toml`.)

- [ ] **Step 2: Write failing export tests**

Add to the `#[cfg(test)] mod tests` in `crates/collab/src/export.rs` (mirror how existing export tests build a yrs doc with a node — copy the setup from the nearest existing `to_html` test in this file; the assertions are what matter):

```rust
    #[test]
    fn mermaid_html_inlines_svg_for_valid_pie() {
        // Build a doc with a single Mermaid node whose `source` attr is a
        // valid pie. (Reuse this file's existing doc-building helper/pattern.)
        let html = to_html_of_single_mermaid("pie\n\"A\" : 1\n\"B\" : 1");
        assert!(html.contains("<svg"), "expected inlined SVG, got: {html}");
        assert!(!html.contains("mermaid-error"));
    }

    #[test]
    fn mermaid_html_falls_back_to_raw_source_on_error() {
        let html = to_html_of_single_mermaid("sequenceDiagram\nAlice->>Bob: hi");
        assert!(html.contains("mermaid-error"));
        // raw source preserved and escaped
        assert!(html.contains("Alice-&gt;&gt;Bob") || html.contains("Alice->>Bob"));
        assert!(!html.contains("<svg"));
    }

    #[test]
    fn mermaid_markdown_emits_fenced_block() {
        let md = to_markdown_of_single_mermaid("pie\n\"A\" : 1");
        assert!(md.contains("```mermaid"));
        assert!(md.contains("\"A\" : 1"));
    }
```

Add the two small helpers next to the tests, following the existing doc-construction idiom already used by other `to_html`/`to_markdown` tests in this file (create a `Doc`, get the XML fragment, push a `mermaid` element with a `source` attribute, then call `to_html`/`to_markdown`). If an existing helper already builds a single-node doc, reuse it instead of adding new ones.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p ogrenotes-collab export::tests::mermaid`
Expected: FAIL — either the helper/render isn't wired, or the arms don't exist yet.

- [ ] **Step 4: Add the `node_type_to_html_tag` arm (compile tripwire) + `resolve_html_tag`**

In `crates/collab/src/export.rs`:
- `node_type_to_html_tag` (near line 1051, exhaustive — a missing arm is a compile error once `NodeType::Mermaid` exists): add `NodeType::Mermaid => "div",`
- `resolve_html_tag` needs no special arm (the `_ =>` delegates to `node_type_to_html_tag`).

- [ ] **Step 5: Special-case the Mermaid leaf in `render_node_html`**

In `render_node_html` (near line 739, inside the `if node_type.is_leaf() {` block, alongside the `CalendarEvent`/`Mention` `matches!` special-cases), add before the self-closing default:

```rust
                if matches!(node_type, NodeType::Mermaid) {
                    let source = el.get_attribute(txn, "source").unwrap_or_default();
                    let out_render = ogrenotes_mermaid::render(&source);
                    match out_render.svg {
                        Some(svg) => {
                            // SVG is generated by our own renderer (no
                            // user HTML passes through); the source is
                            // XML-escaped inside the renderer.
                            out.push_str(&format!(
                                "<div class=\"mermaid-block\">{svg}</div>"
                            ));
                        }
                        None => {
                            let msg = out_render
                                .error
                                .map(|e| e.message)
                                .unwrap_or_else(|| "diagram error".to_string());
                            out.push_str(&format!(
                                "<div class=\"mermaid-error\"><p>{}</p><pre>{}</pre></div>",
                                html_escape(&msg),
                                html_escape(&source),
                            ));
                        }
                    }
                    return;
                }
```

- [ ] **Step 6: Add the Markdown arm in `render_node_markdown`**

In `render_node_markdown` (near line 811), add a match arm mirroring the `CodeBlock` arm but reading the `source` attribute:

```rust
                NodeType::Mermaid => {
                    let source = el.get_attribute(txn, "source").unwrap_or_default();
                    out.push_str("```mermaid\n");
                    out.push_str(&source);
                    out.push_str("\n```\n\n");
                }
```

- [ ] **Step 7: Add the `render_html_attrs` arm (optional; keeps `MERMAID_ATTR_NAMES` used)**

`render_node_html` handles Mermaid via the special-case above and returns early, so no attrs arm is strictly required. To avoid an unused-const warning on `MERMAID_ATTR_NAMES`, either reference it in the block module's own tests (already done) or skip adding an export arm. Do **not** add a redundant `render_html_attrs` arm for Mermaid — the special-case owns the whole element.

- [ ] **Step 8: Run tests + build**

Run: `cargo test -p ogrenotes-collab export::tests::mermaid`
Expected: PASS.
Run: `cargo build -p ogrenotes-collab`
Expected: builds (exhaustive `node_type_to_html_tag` arm satisfied).

- [ ] **Step 9: Commit**

```bash
git add crates/collab/Cargo.toml Cargo.lock crates/collab/src/export.rs
git commit -m "feat(collab): render Mermaid to SVG on HTML export, fenced block on markdown

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 7: Frontend block view + insert entry + Fluent strings

Adds `frontend/src/editor/blocks/mermaid.rs` (`MermaidView` + `MermaidInsert`), registers them, and adds the i18n keys. The view renders the SVG (via `set_inner_html`) or an error banner + raw source (via `set_text_content`), and stamps `data-atom-size="1"`, `data-block-id`, `contenteditable="false"`, and a `data-mermaid-action="edit"` hook for Task 8.

**Files:**
- Create: `frontend/src/editor/blocks/mermaid.rs`
- Modify: `frontend/src/editor/blocks/mod.rs`
- Modify: `frontend/locales/en-US/main.ftl`
- Modify: `frontend/Cargo.toml`

**Interfaces:**
- Consumes: `LiveAppBlockView`, `LiveAppBlockInsert`, `Node::element_with_attrs`, `ogrenotes_mermaid::render`.
- Produces: `pub struct MermaidView; pub struct MermaidInsert;`, id `"mermaid"`.

- [ ] **Step 1: Add the dependency to the frontend**

In `frontend/Cargo.toml` `[dependencies]`:
```toml
ogrenotes-mermaid = { path = "../crates/mermaid" }
```

- [ ] **Step 2: Create the block module**

Create `frontend/src/editor/blocks/mermaid.rs`:

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mermaid diagram live-app block (frontend). Renders the SVG produced
//! by `ogrenotes-mermaid`, or an error banner + raw source on failure.
//! Source lives in the `source` attribute; editing goes through the
//! Mermaid modal (see `components::mermaid_modal`).

use std::collections::HashMap;

use web_sys::{Document, Node as DomNode};

use super::super::model::{Fragment, Node, NodeType};
use super::{LiveAppBlockInsert, LiveAppBlockView};

pub struct MermaidView;
pub struct MermaidInsert;

const STARTER_SOURCE: &str = "pie title Pets\n\"Dogs\" : 386\n\"Cats\" : 85\n\"Birds\" : 15";

impl LiveAppBlockView for MermaidView {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Mermaid]
    }

    fn render(
        &self,
        doc: &Document,
        _node_type: NodeType,
        attrs: &HashMap<String, String>,
        _content: &Fragment,
    ) -> Option<DomNode> {
        let source = attrs.get("source").map(String::as_str).unwrap_or("");
        let wrapper = doc.create_element("div").ok()?;
        wrapper.set_attribute("class", "mermaid-block").ok()?;
        wrapper.set_attribute("contenteditable", "false").ok()?;
        // Leaf atom of model size 1 — DOM↔model mapping treats it as opaque.
        wrapper.set_attribute("data-atom-size", "1").ok()?;
        if let Some(bid) = attrs.get("blockId") {
            wrapper.set_attribute("data-block-id", bid).ok()?;
        }
        // Edit hook consumed by the delegated click listener (Task 8).
        wrapper.set_attribute("data-mermaid-action", "edit").ok()?;

        let out = ogrenotes_mermaid::render(source);
        match out.svg {
            Some(svg) => {
                let holder = doc.create_element("div").ok()?;
                holder.set_attribute("class", "mermaid-svg").ok()?;
                // Trusted: SVG is generated by our renderer with the
                // source XML-escaped internally.
                holder.set_inner_html(&svg);
                wrapper.append_child(&holder).ok()?;
            }
            None => {
                let msg = out.error.map(|e| e.message).unwrap_or_else(|| "diagram error".into());
                let banner = doc.create_element("p").ok()?;
                banner.set_attribute("class", "mermaid-error").ok()?;
                banner.set_text_content(Some(&msg));
                let pre = doc.create_element("pre").ok()?;
                // Untrusted source — text content, never inner_html.
                pre.set_text_content(Some(source));
                wrapper.append_child(&banner).ok()?;
                wrapper.append_child(&pre).ok()?;
            }
        }
        Some(wrapper.into())
    }
}

impl LiveAppBlockInsert for MermaidInsert {
    fn id(&self) -> &'static str {
        "mermaid"
    }
    fn label_key(&self) -> &'static str {
        "insert-mermaid-label"
    }
    fn description_key(&self) -> &'static str {
        "insert-mermaid-description"
    }
    fn icon(&self) -> &'static str {
        "\u{1F4CA}" // 📊
    }
    fn build_default_node(&self) -> Node {
        let mut attrs = HashMap::new();
        attrs.insert("source".to_string(), STARTER_SOURCE.to_string());
        Node::element_with_attrs(NodeType::Mermaid, attrs, Fragment::empty())
    }
}
```

(Confirm the exact import path for `Node::element_with_attrs`, `Fragment::empty`, and the `Node`/`Fragment`/`NodeType` re-exports against `frontend/src/editor/blocks/calendar.rs`'s `use` block and match it verbatim.)

- [ ] **Step 3: Register the view + insert**

In `frontend/src/editor/blocks/mod.rs`:
1. `pub mod mermaid;` next to the others (near line 23).
2. Add to `BLOCK_VIEWS` (near line 71): `&mermaid::MermaidView`
3. Add to `BLOCK_INSERTS` (near line 74): `&mermaid::MermaidInsert`
```rust
pub const BLOCK_VIEWS: &[&(dyn LiveAppBlockView + 'static)] =
    &[&calendar::CalendarView, &kanban::KanbanView, &mermaid::MermaidView];
pub const BLOCK_INSERTS: &[&(dyn LiveAppBlockInsert + 'static)] =
    &[&calendar::CalendarInsert, &kanban::KanbanInsert, &mermaid::MermaidInsert];
```

- [ ] **Step 4: Add the Fluent strings**

In `frontend/locales/en-US/main.ftl`, add (match the surrounding `insert-*` key style):
```ftl
insert-mermaid-label = Mermaid diagram
insert-mermaid-description = Insert a diagram rendered from Mermaid text
```

- [ ] **Step 5: Add a native render test (jsdom-free assertion)**

If block views are unit-tested elsewhere with a `web_sys` `Document` behind `wasm_bindgen_test`, add a `wasm_bindgen_test` that builds a `Document`, calls `MermaidView.render(...)` with a valid pie `source`, and asserts the returned element's `outerHTML` contains `<svg` and `data-atom-size="1"`. If the block test harness for calendar/kanban is native-only, mirror whatever assertion style `blocks::calendar` tests use. If neither exists, rely on the wasm32 build (Step 6) + Task 8's flow test and note it in the commit.

- [ ] **Step 6: Build the frontend for wasm32**

Run: `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: builds clean with the new dep + block module.

- [ ] **Step 7: Commit**

```bash
git add frontend/Cargo.toml frontend/Cargo.lock frontend/src/editor/blocks/mermaid.rs frontend/src/editor/blocks/mod.rs frontend/locales/en-US/main.ftl
git commit -m "feat(frontend): Mermaid block view + insert entry

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 8: Edit modal + click-to-edit + save-writes-source wiring

Adds `frontend/src/components/mermaid_modal.rs` (textarea + live preview), a `update_mermaid_source` command, and wires the delegated click listener + modal state + `on_outcome` in `editor_component.rs`, mirroring the Calendar modal wiring.

**Files:**
- Create: `frontend/src/components/mermaid_modal.rs`
- Modify: `frontend/src/components/mod.rs`
- Modify: `frontend/src/editor/commands.rs`
- Modify: `frontend/src/components/editor_component.rs`
- Modify: `frontend/style/main.css` (block + error + modal styling)

**Interfaces:**
- Consumes: `commands` transform pipeline (`Step::SetAttr`, `find_element_by_block_id`), `ogrenotes_mermaid::render`, `a11y::defer_close`.
- Produces:
  - `#[component] pub fn MermaidModal(state: RwSignal<Option<MermaidModalState>>, on_outcome: Callback<MermaidModalOutcome>) -> impl IntoView`
  - `pub struct MermaidModalState { pub block_id: String, pub source: String }`
  - `pub enum MermaidModalOutcome { Save { block_id: String, source: String }, Cancel }`
  - `commands::update_mermaid_source(block_id, source, state, dispatch) -> bool`

- [ ] **Step 1: Add the `update_mermaid_source` command with a failing test**

In `frontend/src/editor/commands.rs`, mirror `update_calendar_attrs` (near line 1315). Add:

```rust
/// Write a Mermaid block's `source` attribute by block id. Emits a
/// single `Step::SetAttr`, preserving the node's identity/position.
pub fn update_mermaid_source(
    block_id: &str,
    source: String,
    state: &EditorState,
    dispatch: impl FnOnce(Transaction),
) -> bool {
    let Some((offset, _node)) = find_element_by_block_id(&state.doc, block_id) else {
        return false;
    };
    let mut txn = state.transaction();
    match txn.step(Step::SetAttr {
        pos: offset,
        attr: "source".to_string(),
        value: source,
    }) {
        Ok(next) => txn = next,
        Err(_) => return true,
    }
    dispatch(txn);
    true
}
```

(Match the exact types of `EditorState`, `Transaction`, `Step::SetAttr` fields, and `find_element_by_block_id`'s signature against the `update_calendar_attrs` definition — copy them verbatim. The `SetAttr` field names/types must match; if `attr`/`value` differ, use the observed names.)

Add a test mirroring the existing `update_calendar_attrs` test (build a doc with a Mermaid node carrying a `blockId`, call `update_mermaid_source`, assert the dispatched transaction sets `source`):

```rust
    #[test]
    fn update_mermaid_source_sets_attr() {
        // Build an EditorState with one Mermaid node with blockId "m1".
        // (Reuse the doc-building helper the calendar command tests use.)
        let state = state_with_mermaid("m1", "pie\n\"A\": 1");
        let mut captured = None;
        let ok = update_mermaid_source("m1", "pie\n\"B\": 2".into(), &state, |t| captured = Some(t));
        assert!(ok);
        let txn = captured.expect("dispatched");
        assert!(txn_sets_source(&txn, "pie\n\"B\": 2"));
    }
```

(Use the same helper/assertion utilities the calendar-command tests already use; if they inspect `txn.steps` directly, mirror that.)

Run: `cd frontend && cargo test editor::commands::` → expect FAIL, then implement, then PASS.

- [ ] **Step 2: Create the modal component**

Create `frontend/src/components/mermaid_modal.rs`, mirroring `calendar_modal.rs`'s component shape (`state: RwSignal<Option<...>>`, `on_outcome: Callback<...>`, close via `a11y::defer_close` to avoid the modal-close panic):

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mermaid edit modal: a source textarea with a live SVG preview.

use leptos::prelude::*;

use crate::a11y;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MermaidModalState {
    pub block_id: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub enum MermaidModalOutcome {
    Save { block_id: String, source: String },
    Cancel,
}

#[component]
pub fn MermaidModal(
    #[prop(into)] state: RwSignal<Option<MermaidModalState>>,
    on_outcome: Callback<MermaidModalOutcome>,
) -> impl IntoView {
    // Working copy of the source, seeded when the modal opens.
    let draft = RwSignal::new(String::new());
    Effect::new(move |_| {
        if let Some(s) = state.get() {
            draft.set(s.source);
        }
    });

    // Live preview: render on each keystroke.
    let preview = move || {
        let src = draft.get();
        let out = ogrenotes_mermaid::render(&src);
        match out.svg {
            Some(svg) => view! { <div class="mermaid-svg" inner_html=svg></div> }.into_any(),
            None => {
                let msg = out.error.map(|e| e.message).unwrap_or_default();
                view! { <p class="mermaid-error">{msg}</p> }.into_any()
            }
        }
    };

    let save = {
        let state = state;
        move |_| {
            if let Some(s) = state.get() {
                let source = draft.get();
                let cb = on_outcome;
                let block_id = s.block_id.clone();
                a11y::defer_close(move || {
                    state.set(None);
                    cb.run(MermaidModalOutcome::Save { block_id, source });
                });
            }
        }
    };
    let cancel = move |_| {
        let cb = on_outcome;
        a11y::defer_close(move || {
            state.set(None);
            cb.run(MermaidModalOutcome::Cancel);
        });
    };

    view! {
        <Show when=move || state.get().is_some() fallback=|| ()>
            <div class="modal-overlay mermaid-modal">
                <div class="modal-body">
                    <textarea
                        class="mermaid-source"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                    ></textarea>
                    <div class="mermaid-preview">{preview}</div>
                    <div class="modal-actions">
                        <button on:click=cancel>"Cancel"</button>
                        <button class="primary" on:click=save.clone()>"Save"</button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
```

(Match `a11y::defer_close`'s exact signature and `Callback::run` vs `.call` against `calendar_modal.rs`; adjust `event_target_value`/imports to the versions that file uses. This is the reference to mirror, not invent.)

- [ ] **Step 3: Export the module**

In `frontend/src/components/mod.rs`, add `pub mod mermaid_modal;` next to `pub mod calendar_modal;`.

- [ ] **Step 4: Wire modal state + delegated click + on_outcome in `editor_component.rs`**

Mirror the calendar wiring exactly:
1. State signal (near line 1691):
```rust
let mermaid_modal_state: RwSignal<Option<crate::components::mermaid_modal::MermaidModalState>> = RwSignal::new(None);
```
2. Render the modal in the view tree (near line 2830, beside `<CalendarModal .../>`):
```rust
<crate::components::mermaid_modal::MermaidModal
    state=mermaid_modal_state
    on_outcome=on_mermaid_outcome
/>
```
3. In the delegated click listener (the `Effect` around lines 1751-1794 that reads `data-calendar-action`), add a branch that detects `target.closest("[data-mermaid-action]")`, reads the block id from the `.mermaid-block[data-block-id]` ancestor and the current `source` attribute (from the model via `view`/state, or an attribute stamped on the wrapper), builds a `MermaidModalState`, `stop_propagation()` + `prevent_default()`, and `mermaid_modal_state.set(Some(state))`. Follow `calendar_click_outcome` (lines 1194-1259) as the template for reading the id and building state.
4. Build `on_mermaid_outcome` (mirror `on_modal_outcome` at lines 2740-2791): on `Save { block_id, source }`, borrow `view`, build the `dispatch_fn` via `apply_and_notify`, and call `commands::update_mermaid_source(&block_id, source, state, dispatch)`. On `Cancel`, do nothing.

Because the exact borrow/closure shapes here are intricate, copy the calendar `on_modal_outcome` closure and the click-listener `Effect` verbatim, then rename Calendar→Mermaid and swap the command call. Do not hand-roll new borrow patterns.

- [ ] **Step 5: Add styles**

In `frontend/style/main.css`, add minimal rules (match existing block styling conventions):
```css
.mermaid-block { display: block; margin: 0.5rem 0; cursor: pointer; }
.mermaid-svg svg { max-width: 100%; height: auto; }
.mermaid-error { color: var(--danger, #d9534f); }
.mermaid-error pre, .mermaid-modal .mermaid-source { white-space: pre-wrap; font-family: monospace; }
.mermaid-modal .modal-body { display: grid; gap: 0.75rem; min-width: 640px; }
.mermaid-modal .mermaid-source { min-height: 200px; width: 100%; }
```

- [ ] **Step 6: Build wasm32 + run frontend tests**

Run: `cd frontend && cargo build --target wasm32-unknown-unknown`
Expected: builds clean.
Run: `cd frontend && cargo test editor::commands:: editor::blocks::`
Expected: PASS (including `update_mermaid_source_sets_attr`).

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/mermaid_modal.rs frontend/src/components/mod.rs frontend/src/editor/commands.rs frontend/src/components/editor_component.rs frontend/style/main.css
git commit -m "feat(frontend): Mermaid edit modal + click-to-edit + save wiring

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Final verification (after Task 8)

- [ ] `cargo test -p ogrenotes-mermaid` — crate green.
- [ ] `cargo test -p ogrenotes-collab` — schema, blocks, export green.
- [ ] `cargo build` (workspace) — native build clean.
- [ ] `cd frontend && cargo build --target wasm32-unknown-unknown` — wasm build clean.
- [ ] `cd frontend && cargo test` — frontend native tests green.
- [ ] Manual smoke (via `/run` or local stack): insert a Mermaid block → renders starter pie; edit → live preview updates; save → block re-renders; export the doc to HTML → contains `<svg`; enter invalid source → error banner + raw source shown, nothing lost.

## Notes for the implementer

- The four extraction findings this plan is built on: (1) `render_node_html` has no per-type match — leaf-with-body types use `matches!` special-cases with an early `return` (Task 6 Step 5); (2) both schemas + 6 hardcoded cross-schema test lists must be updated (Task 3); (3) the write gate is registry-driven — adding to `BLOCKS` is enough, but `validate_attrs` must **echo** `source` and hard-reject over-cap (Task 5); (4) atom-delete needs only `is_atom()==true` + `atom:true` + `data-atom-size` — no keymap/handler edits (Tasks 4, 7).
- If any "near line N" anchor has drifted, search for the adjacent `Embed`/`Calendar`/`CodeBlock` token named in the step — the pattern, not the line number, is authoritative.
