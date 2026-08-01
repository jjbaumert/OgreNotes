# Presentations P1 — Deck Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `DocType::Presentation` end-to-end: a slide-deck document kind with a canvas editor (positioned frames of existing blocks), layout presets, built-in themes, frame-anchored comments, and degraded HTML/Markdown export.

**Architecture:** Per `design/presentations.md` (read it first). The persisted/synced form is a normal yrs document — `Doc → Slide → Frame → existing blocks` — using two new `NodeType` variants mirrored across `crates/collab/src/schema.rs` and `frontend/src/editor/model.rs`. The client keeps a dedicated working model (`frontend/src/presentation/`) and a `DeckView` component, following the `SpreadsheetView` precedent exactly: build model from `editor_state` in an Effect, write back via `on_state_change` with a `persist_origin` guard.

**Tech Stack:** Rust workspace (axum, yrs, DynamoDB) + Leptos 0.7 WASM frontend (`frontend/` is OUTSIDE the workspace — always `cd frontend/` to build/test it).

## Global Constraints

- Existing tests are behavioral contracts. The ONLY existing tests this plan may modify are the schema-duality maintenance tests (`cross_schema_*`, `node_type_tag_roundtrip`, `as_str_agrees_with_serde`, `lowercase_enums_round_trip`), which are explicitly designed to be updated when a variant is added — each task names exactly which ones.
- Never `git add -A` or `git add .` — stage named files only (untracked `verification/` must stay untracked).
- Do not edit `design/`, `framework/`, or `runbook/` files.
- Identifiers are raw `String` (project-wide `identifier_strategy = "string-grandfathered"`); do not introduce ID newtypes.
- Backend tests: `cargo test -p <crate>` from the repo root. Frontend tests: `cd frontend && cargo test` (native). Frontend compile check for wasm-gated code: `cd frontend && cargo check --target wasm32-unknown-unknown`.
- New tag strings are snake_case: `"slide"`, `"frame"`, serde token `"presentation"`.
- Server-side attr validation must echo accepted attrs **byte-identically** (`surface_canonicalization_diff` in `crates/collab/src/blocks/validate_writes.rs:517` flags any rewrite as a violation). Validators reject bad values; *readers* clamp. Never clamp in `validate_attrs`.
- Presence (`frame` awareness field) and present mode are **out of scope** — they land in P2 with the other awareness-protocol work. The `/present` route is P2 (note: the real doc route is `d/:id`, not `/doc/:id`).

---

### Task 1: Backend `DocType::Presentation`

**Files:**
- Modify: `crates/storage/src/models/mod.rs` (enum at :46, `as_str` at :52, tests at :244 and :270)
- Modify: `crates/api/src/routes/metrics.rs:85` (`validate_page` allowlist)
- Modify: `crates/api/src/routes/ask.rs` (`"enum"` lists at :525, :574, :595; doc comment at :87-90)
- Test: `crates/api/tests/test_documents_presentation.rs` (create)

**Interfaces:**
- Consumes: existing `CreateDocumentRequest { doc_type: Option<DocType> }` (`documents.rs:133-147`, JSON field `docType`) — serde carries the new variant with no route change.
- Produces: `DocType::Presentation` with wire token `"presentation"` and `as_str() == "presentation"`. All later tasks rely on this exact string.

- [ ] **Step 1: Write the failing unit-test updates**

In `crates/storage/src/models/mod.rs`, extend the two variant-enumerating tests (these are the sanctioned duality-maintenance updates):

```rust
    // in as_str_agrees_with_serde (:244):
    assert_eq!(DocType::Presentation.as_str(), token(&DocType::Presentation));

    // in lowercase_enums_round_trip (:270), extend the array:
    for v in [
        DocType::Document,
        DocType::Spreadsheet,
        DocType::Chat,
        DocType::Presentation,
    ] {
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ogrenotes-storage doc_type 2>&1 | tail -20` (adjust the package name to what `crates/storage/Cargo.toml` declares — check `name =` first).
Expected: compile error `no variant named Presentation`.

- [ ] **Step 3: Add the variant**

```rust
pub enum DocType {
    Document,
    Spreadsheet,
    Chat,
    Presentation,
}

impl DocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocType::Document => "document",
            DocType::Spreadsheet => "spreadsheet",
            DocType::Chat => "chat",
            DocType::Presentation => "presentation",
        }
    }
}
```

- [ ] **Step 4: Run storage tests**

Run: `cargo test -p <storage-package> 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Widen the string-literal doc-type lists (the compiler will NOT find these)**

- `crates/api/src/routes/metrics.rs:85` — add a `"presentation" => Ok("presentation"),` arm next to the `"spreadsheet"` arm in `validate_page` (otherwise deck RUM beacons are rejected).
- `crates/api/src/routes/ask.rs` — add `"presentation"` to each of the three JSON `"enum": ["document", "spreadsheet", "chat"]` lists (:525, :574, :595) and update the `SsePayload::Source` doc comment (:87-90).

- [ ] **Step 6: Write the failing route integration test**

Create `crates/api/tests/test_documents_presentation.rs`. Copy the setup harness from the top of an existing route test (open `crates/api/tests/test_quip_start.rs` or the closest documents test and mirror its app/state bootstrap exactly — do not invent a harness):

```rust
// Body of the test once the harness gives you an authed client:
// POST /documents with {"title": "Deck", "docType": "presentation"}
// Assert: 201, response JSON docType == "presentation".
// Then GET the document meta (same route the frontend uses) and
// assert docType == "presentation" round-trips through DynamoDB.
```

- [ ] **Step 7: Run it**

Run: `cargo test -p <api-package> --test test_documents_presentation 2>&1 | tail -10`
Expected: PASS with no production change — `documents.rs:243` (`req.doc_type.unwrap_or(DocType::Document)`) already forwards the variant. If it fails, diagnose before proceeding; do not patch the test.

- [ ] **Step 8: Commit**

```bash
git add crates/storage/src/models/mod.rs crates/api/src/routes/metrics.rs crates/api/src/routes/ask.rs crates/api/tests/test_documents_presentation.rs
git commit -m "feat(presentations): add DocType::Presentation end-to-end on the backend"
```

---

### Task 2: Backend `Slide` / `Frame` schema variants

**Files:**
- Modify: `crates/collab/src/schema.rs` (enum :4-86, `tag_name` :90, `from_tag` :122, `is_block` :155, `valid_children` :210, `ALL_NODE_TYPES` :479, tests :351, :534, :572, :636, :656, :667)

**Interfaces:**
- Produces: `NodeType::Slide` (tag `"slide"`, container, child of `Doc`, children `[Frame]`) and `NodeType::Frame` (tag `"frame"`, container, children = the block set). Every later backend task matches on these variants.

- [ ] **Step 1: Update the duality tests first (they are the spec)**

All in `crates/collab/src/schema.rs` tests — the comment block at :469 says these must move with the enum:
- `cross_schema_node_type_count` (:534): `26` → `28`.
- `ALL_NODE_TYPES` (:479): append `NodeType::Slide, NodeType::Frame`.
- `cross_schema_tag_names` (:572): append `("slide", NodeType::Slide), ("frame", NodeType::Frame)`.
- `node_type_tag_roundtrip` (:351): append the two variants to its own (duplicate) array.
- `cross_schema_valid_children` (:667): the hardcoded `Doc.valid_children()` expectation gains `NodeType::Slide` at the end; add assertions `Slide.valid_children() == [Frame]` and that `Frame.valid_children()` equals the block list from Step 3.
- `cross_schema_leaf_nodes` (:636) / `cross_schema_inline_nodes` (:656): no edit needed — Slide/Frame are non-leaf, non-inline blocks and the sweeps pass automatically; confirm by reading them.

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p <collab-package> cross_schema 2>&1 | tail -5`
Expected: compile error (unknown variants).

- [ ] **Step 3: Add the variants and arms**

Enum (after `Mermaid`, with doc comments in the house style):

```rust
    /// design/presentations.md — one slide in a presentation deck.
    /// Container of `Frame` children. Attrs: `layout` (preset id),
    /// `background` (optional theme-relative override).
    Slide,
    /// design/presentations.md — a positioned frame on a slide.
    /// Container of ordinary blocks. Attrs: `x`,`y`,`w`,`h`
    /// (normalized 0..1), `z` (stacking), `role`
    /// (`content` | `notes`).
    Frame,
```

Arms (match existing style exactly):

```rust
// tag_name:
            NodeType::Slide => "slide",
            NodeType::Frame => "frame",
// from_tag:
            "slide" => Some(NodeType::Slide),
            "frame" => Some(NodeType::Frame),
// is_block: add `| NodeType::Slide | NodeType::Frame` to the alternation.
// valid_children — Doc arm gains NodeType::Slide (last entry); then:
            NodeType::Slide => &[NodeType::Frame],
            NodeType::Frame => &[
                NodeType::Paragraph,
                NodeType::Heading,
                NodeType::BulletList,
                NodeType::OrderedList,
                NodeType::TaskList,
                NodeType::Blockquote,
                NodeType::CodeBlock,
                NodeType::HorizontalRule,
                NodeType::Image,
                NodeType::Table,
                NodeType::Embed,
                NodeType::Calendar,
                NodeType::Kanban,
                NodeType::Mermaid,
            ],
```

Design note honored: `Doc.valid_children` is shared across doc types (the signature has no doc-type parameter), so "in a presentation, Doc contains only Slides" is enforced **by construction** — only `DeckView` creates slides, and nothing in the flow editor inserts them. Do not add a doc-type parameter to `valid_children`.

- [ ] **Step 4: Run collab tests**

Run: `cargo test -p <collab-package> 2>&1 | tail -5`
Expected: PASS (if `export.rs` fails to compile on its exhaustive `node_type_to_html_tag` match, add a temporary arm `NodeType::Slide | NodeType::Frame => "div",` — Task 4 replaces it).

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/schema.rs crates/collab/src/export.rs
git commit -m "feat(presentations): add Slide and Frame node types to the collab schema"
```

---

### Task 3: Backend presentation live-app block (attr validation)

**Files:**
- Create: `crates/collab/src/blocks/presentation.rs`
- Modify: `crates/collab/src/blocks/mod.rs` (module decl :25-28, `BLOCKS` :151-155, ownership tests :167-213)

**Interfaces:**
- Consumes: `LiveAppBlock` trait (`blocks/mod.rs:133-149`), `BlockValidationError { node_type, field, reason }`.
- Produces: `pub static PRESENTATION: PresentationBlock`, `pub const SLIDE_ATTR_NAMES: &[&str] = &["layout", "background"]`, `pub const FRAME_ATTR_NAMES: &[&str] = &["x", "y", "w", "h", "z", "role"]`, and export helpers `html_tag`, `html_attrs`, `markdown_placeholder` (same shapes as `blocks/calendar.rs:267,278,335`). Task 4 calls the helpers.

- [ ] **Step 1: Write failing validator tests** (in `presentation.rs` `#[cfg(test)]`, mirroring `calendar.rs` test style)

```rust
    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn frame_accepts_valid_geometry_verbatim() {
        let a = attrs(&[("x", "0.25"), ("y", "0"), ("w", "0.5"), ("h", "0.333"),
                        ("z", "2"), ("role", "content")]);
        let out = PRESENTATION.validate_attrs(NodeType::Frame, &a).unwrap();
        assert_eq!(out, a); // byte-identical echo — never canonicalize
    }

    #[test]
    fn frame_rejects_out_of_range_geometry() {
        for bad in [("x", "1.5"), ("x", "-0.1"), ("w", "0"), ("h", "nan"), ("x", "abc")] {
            let a = attrs(&[bad]);
            assert!(PRESENTATION.validate_attrs(NodeType::Frame, &a).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn frame_rejects_bad_role_and_z() {
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[("role", "banner")])).is_err());
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[("z", "2.5")])).is_err());
    }

    #[test]
    fn frame_accepts_absent_attrs() {
        // Absent attrs are fine — readers apply defaults (x=0,y=0,w=1,h=1,z=0,role=content).
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[])).is_ok());
    }

    #[test]
    fn slide_validates_layout_and_background() {
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("layout", "two-column")])).is_ok());
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("layout", "pyramid")])).is_err());
        let long = "x".repeat(300);
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("background", &long)])).is_err()); // cap 200 chars
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p <collab-package> blocks::presentation 2>&1 | tail -5`
Expected: compile error (module missing).

- [ ] **Step 3: Implement the module**

```rust
//! Presentation deck block — Slide/Frame attr validation and export
//! helpers. See design/presentations.md and blocks/calendar.rs (the
//! reference implementation this mirrors).

use std::collections::HashMap;
use crate::schema::NodeType;
use super::{BlockValidationError, LiveAppBlock};

pub struct PresentationBlock;
pub static PRESENTATION: PresentationBlock = PresentationBlock;

pub const LAYOUTS: &[&str] = &["title", "title-content", "two-column", "blank"];
pub const ROLES: &[&str] = &["content", "notes"];
pub const SLIDE_ATTR_NAMES: &[&str] = &["layout", "background"];
pub const FRAME_ATTR_NAMES: &[&str] = &["x", "y", "w", "h", "z", "role"];
const BACKGROUND_MAX_LEN: usize = 200;

impl LiveAppBlock for PresentationBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Slide, NodeType::Frame]
    }
    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        match node_type {
            NodeType::Slide => validate_slide_attrs(attrs),
            NodeType::Frame => validate_frame_attrs(attrs),
            other => Err(err(other, "node_type", "not a presentation node")),
        }
    }
}

fn err(node_type: NodeType, field: &'static str, reason: &str) -> BlockValidationError {
    BlockValidationError { node_type, field: field.into(), reason: reason.into() }
}

/// Accept-verbatim-or-reject. NEVER rewrite a value: the write gate's
/// canonicalization diff (validate_writes.rs) treats any change as a
/// violation, and legitimate frame drags would trip it. Readers clamp.
fn validate_frame_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    for key in ["x", "y", "w", "h"] {
        if let Some(v) = attrs.get(key) {
            let f: f64 = v.parse().map_err(|_| err(NodeType::Frame, "geometry",
                &format!("{key} is not a number: {v}")))?;
            let ok = f.is_finite() && (0.0..=1.0).contains(&f)
                && !((key == "w" || key == "h") && f == 0.0);
            if !ok {
                return Err(err(NodeType::Frame, "geometry",
                    &format!("{key} out of range 0..=1: {v}")));
            }
        }
    }
    if let Some(v) = attrs.get("z") {
        v.parse::<i64>().map_err(|_| err(NodeType::Frame, "z",
            &format!("z is not an integer: {v}")))?;
    }
    if let Some(v) = attrs.get("role") {
        if !ROLES.contains(&v.as_str()) {
            return Err(err(NodeType::Frame, "role", &format!("unknown role: {v}")));
        }
    }
    Ok(attrs.clone())
}

fn validate_slide_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    if let Some(v) = attrs.get("layout") {
        if !LAYOUTS.contains(&v.as_str()) {
            return Err(err(NodeType::Slide, "layout", &format!("unknown layout: {v}")));
        }
    }
    if let Some(v) = attrs.get("background") {
        if v.len() > BACKGROUND_MAX_LEN {
            return Err(err(NodeType::Slide, "background", "background too long"));
        }
    }
    Ok(attrs.clone())
}

// ── export helpers (called from export.rs match arms, Task 4) ──

pub fn html_tag(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Slide => "section",
        _ => "div", // Frame
    }
}

/// Pre-escaped attr string with a LEADING space (calendar.rs contract).
pub fn html_attrs(node_type: NodeType, attrs: &HashMap<String, String>) -> String {
    let names = match node_type {
        NodeType::Slide => SLIDE_ATTR_NAMES,
        _ => FRAME_ATTR_NAMES,
    };
    let mut out = String::new();
    for name in names {
        if let Some(v) = attrs.get(*name) {
            out.push_str(&format!(" data-{}=\"{}\"", name, crate::export::html_escape(v)));
        }
    }
    match node_type {
        NodeType::Slide => out.push_str(" class=\"deck-slide\""),
        _ => out.push_str(" class=\"deck-frame\""),
    }
    out
}

pub fn markdown_placeholder(node_type: NodeType, _attrs: &HashMap<String, String>) -> String {
    match node_type {
        NodeType::Slide => String::new(), // heading emitted by the export arm (needs the slide number)
        _ => String::new(),
    }
}
```

(If `html_escape` is private to `export.rs`, make it `pub(crate)` — check how `calendar.rs:278` escapes and copy that mechanism instead if it differs.)

Register in `blocks/mod.rs`:

```rust
pub mod presentation;
// ...
pub const BLOCKS: &[&(dyn LiveAppBlock + 'static)] =
    &[&calendar::CALENDAR, &kanban::KANBAN, &mermaid::MERMAID, &presentation::PRESENTATION];
```

- [ ] **Step 4: Run tests (validators + ownership invariants)**

Run: `cargo test -p <collab-package> blocks 2>&1 | tail -5`
Expected: PASS, including the pre-existing `node_type_ownership_is_disjoint` sweep now covering Slide/Frame.

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/blocks/presentation.rs crates/collab/src/blocks/mod.rs
git commit -m "feat(presentations): register Slide/Frame live-app validation block"
```

---

### Task 4: Backend HTML/Markdown export for decks

**Files:**
- Modify: `crates/collab/src/export.rs` (`node_type_to_html_tag` :1320, `render_html_attrs` :1421, `render_node_markdown` :1043, plus the two entry fns `to_html`/`to_markdown` for the slide counter)
- Test: same file, `#[cfg(test)]` block (find the existing export tests and add alongside)

**Interfaces:**
- Consumes: `blocks::presentation::{html_tag, html_attrs, SLIDE_ATTR_NAMES, FRAME_ATTR_NAMES}` (Task 3); `collect_named_attrs` helper (`export.rs:1256`, the Kanban-era shape).
- Produces: decks export as `<section class="deck-slide">…` / `## Slide N` with frames in z-then-position order. This is the degraded export the design promises; PDF is P2.

- [ ] **Step 1: Write failing golden tests**

Build a fixture deck with the same doc-construction helpers the existing export tests use (read two existing tests first and copy their yrs-doc setup). Deck: 2 slides; slide 1 has two frames with `z=1,y=0.1` and `z=0,y=0.5` each containing one paragraph.

```rust
    #[test]
    fn deck_html_export_slides_as_sections_frames_z_ordered() {
        let doc = fixture_deck(); // built with the file's existing helpers
        let html = to_html(&doc);
        assert!(html.contains(r#"<section data-layout="blank" class="deck-slide">"#));
        // z=0 frame renders before z=1 frame despite tree order:
        let low = html.find("low-z-text").unwrap();
        let high = html.find("high-z-text").unwrap();
        assert!(low < high);
    }

    #[test]
    fn deck_markdown_export_numbers_slides() {
        let md = to_markdown(&fixture_deck());
        assert!(md.contains("## Slide 1"));
        assert!(md.contains("## Slide 2"));
        let s1 = md.find("## Slide 1").unwrap();
        let s2 = md.find("## Slide 2").unwrap();
        assert!(s1 < s2);
    }

    #[test]
    fn deck_export_ignores_malformed_geometry() {
        // Frame with x="garbage" must still render (sorted as 0.0), not panic.
        let md = to_markdown(&fixture_deck_with_bad_geometry());
        assert!(md.contains("bad-geometry-text"));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p <collab-package> deck_ 2>&1 | tail -10`
Expected: FAIL (temporary `"div"` tag arm from Task 2, no ordering, no headings).

- [ ] **Step 3: Implement**

Slide numbering: `render_node_markdown`/`render_node_html` have no sibling-index parameter, so use a thread-local counter reset by the entry points:

```rust
thread_local! {
    static SLIDE_NO: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
// At the top of to_html, to_html_with_comments, to_markdown,
// to_markdown_with_comments:  SLIDE_NO.with(|c| c.set(0));
```

`node_type_to_html_tag` — replace the temporary arm:

```rust
        NodeType::Slide | NodeType::Frame => {
            crate::blocks::presentation::html_tag(nt)
        }
```

`render_html_attrs` — new arm before the `_ => {}` (:1563), Kanban-style:

```rust
        NodeType::Slide | NodeType::Frame => {
            let names: &[&str] = if node_type == NodeType::Slide {
                crate::blocks::presentation::SLIDE_ATTR_NAMES
            } else {
                crate::blocks::presentation::FRAME_ATTR_NAMES
            };
            let collected = collect_named_attrs(txn, el, names);
            attrs.push_str(&crate::blocks::presentation::html_attrs(node_type, &collected));
        }
```

Z-then-position child ordering — a shared helper both renderers call for `Slide` elements:

```rust
/// Indices of a Slide's element children sorted by (z, y, x); text
/// children (invalid inside Slide) keep tree order at the end.
/// Malformed numbers sort as 0.0 — export must never fail on bad attrs.
fn slide_child_order<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> Vec<u32> {
    let mut keyed: Vec<(i64, f64, f64, u32)> = Vec::new();
    for i in 0..el.len(txn) {
        let (z, y, x) = match el.get(txn, i) {
            Some(yrs::XmlOut::Element(child)) => {
                let num = |k: &str| child.get_attribute(txn, k)
                    .and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
                let z = child.get_attribute(txn, "z")
                    .and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                (z, num("y"), num("x"))
            }
            _ => (i64::MAX, 0.0, 0.0),
        };
        keyed.push((z, y, x, i));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0)
        .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal)));
    keyed.into_iter().map(|(_, _, _, i)| i).collect()
}
```

In `render_node_html`'s non-leaf path, special-case Slide: instead of the default child walk, iterate `slide_child_order` and recurse per child. In `render_node_markdown`, add a `NodeType::Slide` arm before the `_` fall-through (:1245):

```rust
                NodeType::Slide => {
                    let n = SLIDE_NO.with(|c| { c.set(c.get() + 1); c.get() });
                    out.push_str(&format!("## Slide {n}\n\n"));
                    for i in slide_child_order(txn, el) {
                        if let Some(child) = el.get(txn, i) {
                            render_node_markdown(txn, &child, out, depth);
                        }
                    }
                    out.push('\n');
                }
```

`Frame` needs no markdown arm (the `_` fall-through renders its children, which is correct).

- [ ] **Step 4: Run all collab tests**

Run: `cargo test -p <collab-package> 2>&1 | tail -5`
Expected: PASS (existing export goldens untouched).

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/export.rs
git commit -m "feat(presentations): degraded HTML/Markdown export for decks"
```

---

### Task 5: Frontend `Slide` / `Frame` node types + bridge

**Files:**
- Modify: `frontend/src/editor/model.rs` (enum :139; exhaustive matches: `is_commentable` :262, `needs_block_id` :311, plus `is_leaf` :205 / `is_inline` :220 / `is_block` :225 / `is_atom` :230 / `is_code` :235 / `is_textblock` :345 / `default_attrs` :353; `normalize_doc` carve-out near :1025)
- Modify: `frontend/src/editor/schema.rs` (`default_schema()` :270, `Doc` children :280-294, `is_isolating` :68)
- Modify: `frontend/src/editor/yrs_bridge.rs` (`node_type_to_tag` :857, `tag_to_node_type` :894)
- Modify: `frontend/src/editor/view.rs` (`node_type_to_tag` :1764)
- Test: `frontend/src/editor/yrs_bridge.rs` + `model.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: backend tags `"slide"` / `"frame"` (Task 2) — the four string tables must agree or the cross-schema contract breaks.
- Produces: frontend `NodeType::Slide` / `NodeType::Frame` with `needs_block_id() == true` both, `is_commentable()`: Frame `true`, Slide `false`. Task 6's deck model builds on these.

- [ ] **Step 1: Write failing round-trip test** (in `yrs_bridge.rs` tests, mirroring an existing round-trip test there)

```rust
    #[test]
    fn deck_doc_roundtrips_through_ydoc() {
        let frame = Node::element_with_attrs(
            NodeType::Frame,
            [("x", "0.1"), ("y", "0.2"), ("w", "0.5"), ("h", "0.3"), ("z", "1"), ("role", "content")]
                .iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph, Fragment::from(vec![Node::text("hello deck")]),
            )]),
        );
        let slide = Node::element_with_attrs(
            NodeType::Slide,
            [("layout", "blank")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            Fragment::from(vec![frame]),
        );
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![slide]));
        let bytes = doc_to_ydoc_bytes(&doc);
        let back = ydoc_bytes_to_doc(&bytes).unwrap();
        // normalize strips nothing from a deck (empty-slide carve-out):
        assert_eq!(back.attrs(), doc.attrs());
        let slide_back = match &back { Node::Element { content, .. } => &content.children[0], _ => panic!() };
        assert_eq!(slide_back.node_type(), Some(NodeType::Slide));
        assert!(slide_back.block_id().is_some(), "slides must carry blockIds");
    }

    #[test]
    fn empty_slide_survives_normalize() {
        let slide = Node::element(NodeType::Slide);
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![slide]));
        let n = crate::editor::model::normalize_doc(&doc);
        let count = match &n { Node::Element { content, .. } => content.children.len(), _ => 0 };
        assert_eq!(count, 1, "normalize_doc must not strip empty slides");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && cargo test deck_doc 2>&1 | tail -5`
Expected: compile error (unknown variants).

- [ ] **Step 3: Implement**

- `model.rs` enum: add `Slide, Frame` (with short doc comments referencing design/presentations.md).
- `is_block`: both true. `is_leaf` / `is_inline` / `is_atom` / `is_code` / `is_textblock`: both false. `default_attrs`: none.
- `is_commentable` (exhaustive): `Slide => false`, `Frame => true` (frame-anchored comments).
- `needs_block_id` (exhaustive): both `=> true` — mandatory, or yrs sync degrades to full rewrites (see the pathology note at `model.rs:311-330`).
- `normalize_doc`: next to the `is_spreadsheet_table` carve-out (:1025), keep empty `Slide`/`Frame` nodes (match on `node_type` — no attr sniffing needed since the type is explicit).
- `schema.rs` `default_schema()`: `NodeSpec`s for both; `Doc` children list gains `Slide`; `Slide` children `[Frame]`; `Frame` children = same block list as the backend (Task 2 Step 3 — copy it exactly); `is_isolating`: both `true` (cross-frame joins must not happen; per commit `2180db3`, cross joins key on the isolating flag).
- `yrs_bridge.rs`: `node_type_to_tag` gains `NodeType::Slide => "slide", NodeType::Frame => "frame"`; `tag_to_node_type` the inverse.
- `view.rs:1764` `node_type_to_tag`: `NodeType::Slide => "section", NodeType::Frame => "div"` (placeholder — decks never render through the flow editor, but the match is total).

- [ ] **Step 4: Run frontend tests + wasm check**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`
Expected: PASS / clean check.

- [ ] **Step 5: Add the yrs-level concurrent-merge test** (design/presentations.md "Testing": concurrent move+resize must converge)

In `yrs_bridge.rs` tests: build one deck ydoc, clone its state into two `yrs::Doc`s; on doc A change the frame's `x`/`y` attrs (simulating a move), on doc B change `w`/`h` (a resize); exchange updates both ways (`encode_state_as_update` / `apply_update`, same helpers the existing bridge tests use — copy their exchange plumbing); assert both docs converge to the identical tree carrying A's position AND B's size. Add a second case where both sides move the same frame and assert the docs converge to the same (either) value.

Run: `cd frontend && cargo test concurrent 2>&1 | tail -5` — expected PASS.

- [ ] **Step 6: Run backend duality tests again** (they pin the frontend contract)

Run: `cargo test -p <collab-package> cross_schema 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/editor/model.rs frontend/src/editor/schema.rs frontend/src/editor/yrs_bridge.rs frontend/src/editor/view.rs
git commit -m "feat(presentations): mirror Slide/Frame node types in the frontend schema"
```

---

### Task 6: Frontend deck working model

**Files:**
- Create: `frontend/src/presentation/mod.rs`, `frontend/src/presentation/model.rs`
- Modify: `frontend/src/main.rs:21` (add `mod presentation;` next to `mod spreadsheet;`)

**Interfaces:**
- Consumes: `Node` / `Fragment` (`editor/model.rs:437`), `generate_block_id()` (`model.rs:6`).
- Produces (Task 9-12 depend on these exact signatures):

```rust
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }   // always clamped
pub struct DeckFrame { pub block_id: String, pub rect: Rect, pub z: i64,
                       pub role: FrameRole, pub content: Fragment }
pub enum FrameRole { Content, Notes }
pub struct DeckSlide { pub block_id: String, pub layout: String,
                       pub background: Option<String>, pub frames: Vec<DeckFrame> }
pub struct Deck { pub theme: String, pub slide_size: String,  // "16:9" default; persisted as Doc attr slideSize
                  pub slides: Vec<DeckSlide> }
pub fn deck_from_doc(doc: &Node) -> Deck
pub fn deck_to_doc(deck: &Deck) -> Node
impl Rect { pub fn clamped(x: f64, y: f64, w: f64, h: f64) -> Rect }
```

- [ ] **Step 1: Write failing model tests** (`presentation/model.rs` `#[cfg(test)]`)

```rust
    #[test]
    fn rect_clamps_to_unit_square() {
        let r = Rect::clamped(-0.5, 1.5, 2.0, 0.0);
        assert_eq!((r.x, r.y), (0.0, 1.0));
        assert!(r.w <= 1.0 && r.w >= MIN_FRAME_DIM);
        assert!(r.h >= MIN_FRAME_DIM); // zero/negative sizes clamp to a minimum, never 0
    }

    #[test]
    fn deck_roundtrips_doc() {
        let deck = fixture_deck(); // 2 slides, 3 frames, one role=notes
        let doc = deck_to_doc(&deck);
        assert_eq!(deck_from_doc(&doc), deck);
    }

    #[test]
    fn deck_from_doc_defaults_missing_attrs() {
        // A Frame with no geometry attrs reads as x=0,y=0,w=1,h=1,z=0,role=content;
        // a Doc with no theme reads as DEFAULT_THEME; garbage numbers clamp.
        let doc = fixture_doc_with_missing_and_garbage_attrs();
        let deck = deck_from_doc(&doc);
        assert_eq!(deck.theme, DEFAULT_THEME);
        assert_eq!(deck.slides[0].frames[0].rect, Rect::clamped(0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn deck_to_doc_preserves_block_ids() {
        // Round-trip must keep every blockId byte-identical, or yrs sync
        // rewrites the world on every persist (find_match aligns on blockId).
        let deck = fixture_deck();
        let doc = deck_to_doc(&deck);
        let doc2 = deck_to_doc(&deck_from_doc(&doc));
        assert_eq!(doc, doc2);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && cargo test presentation:: 2>&1 | tail -5`
Expected: compile error.

- [ ] **Step 3: Implement**

`presentation/model.rs` (~150 lines). Constants: `pub const DEFAULT_THEME: &str = "slate";`, `pub const MIN_FRAME_DIM: f64 = 0.02;`. `deck_from_doc` walks `Doc` children, skipping non-`Slide` nodes defensively; frames read attrs with `attrs.get("x").and_then(|v| v.parse().ok()).unwrap_or(0.0)` then `Rect::clamped`. `deck_to_doc` builds `Node::element_with_attrs` trees, writing geometry as short strings (`format!("{:.4}", x)` — 4 decimals is sub-pixel at 4K and keeps attr churn small; write `role` only when `Notes`, `z` only when non-zero, to keep attr diffs minimal — but ALWAYS write back a frame's existing attrs unchanged when the value is unchanged, by reading from the same formatted representation). Preserve `block_id` from the model into `attrs["blockId"]`; generate via `generate_block_id()` only for newly created slides/frames.

`presentation/mod.rs`: `pub mod model;` re-exports.

- [ ] **Step 4: Run tests**

Run: `cd frontend && cargo test presentation:: 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/presentation frontend/src/main.rs
git commit -m "feat(presentations): deck working model with doc round-trip"
```

---

### Task 7: Layout presets, themes, and deck CSS

**Files:**
- Create: `frontend/src/presentation/presets.rs`, `frontend/src/presentation/themes.rs`, `frontend/style/presentation.css`
- Modify: `frontend/src/presentation/mod.rs` (module decls), `frontend/index.html` (copy-file link, next to `spreadsheet.css` :44-58)

**Interfaces:**
- Consumes: `DeckSlide`, `DeckFrame`, `Rect` (Task 6).
- Produces:

```rust
// presets.rs
pub struct LayoutPreset { pub id: &'static str, pub label_key: &'static str,
                          pub frames: &'static [PresetFrame] }
pub struct PresetFrame { pub rect: (f64, f64, f64, f64), pub placeholder_key: &'static str,
                         pub heading: bool }
pub const LAYOUT_PRESETS: &[LayoutPreset]  // ids: title, title-content, two-column, blank
pub fn instantiate(preset: &LayoutPreset) -> DeckSlide
// themes.rs
pub struct DeckTheme { pub id: &'static str, pub label_key: &'static str }
pub const DECK_THEMES: &[DeckTheme]  // six ids: slate, paper, midnight, ember, forest, ocean
pub fn theme_class(theme_id: &str) -> String  // "deck-theme-slate" (unknown id -> default)
```

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn presets_instantiate_with_ids_and_clamped_rects() {
        for p in LAYOUT_PRESETS {
            let s = instantiate(p);
            assert_eq!(s.layout, p.id);
            assert!(!s.block_id.is_empty());
            for f in &s.frames {
                assert!(!f.block_id.is_empty());
                assert!(f.rect.x + f.rect.w <= 1.0 + 1e-9, "{} overflows", p.id);
            }
        }
        assert_eq!(LAYOUT_PRESETS.iter().filter(|p| p.id == "blank").count(), 1);
        assert!(instantiate(&LAYOUT_PRESETS.iter().find(|p| p.id == "blank").unwrap())
            .frames.is_empty());
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        assert_eq!(theme_class("nope"), theme_class(DEFAULT_THEME));
        assert_eq!(DECK_THEMES.len(), 6);
    }
```

- [ ] **Step 2: Run to verify failure**, then implement.

Preset geometries: `title` = one heading frame (0.1, 0.35, 0.8, 0.2) + subtitle (0.1, 0.58, 0.8, 0.1); `title-content` = heading (0.06, 0.06, 0.88, 0.12) + body (0.06, 0.22, 0.88, 0.7); `two-column` = heading (0.06, 0.06, 0.88, 0.12) + left (0.06, 0.22, 0.42, 0.7) + right (0.52, 0.22, 0.42, 0.7); `blank` = no frames. `instantiate` builds each frame's `content` as one `Heading` (when `heading`) or `Paragraph` with an i18n placeholder — add the `label_key`/`placeholder_key` strings to the i18n `.ftl` files (grep an existing `calendar` key to find them; add `deck-*` keys to every locale file, English text elsewhere until translated, matching how other new keys were introduced — check `git log -p` on a `.ftl` file if unsure).

`presentation.css` (~150 lines): `.deck-view` grid (strip / canvas / pane), `.deck-canvas` (aspect-ratio 16/9, `position: relative`, `container-type: size`), `.deck-frame` (`position: absolute`, geometry via inline `style:left/top/width/height` percentages set in Rust), `.deck-frame--selected` (handle outlines), `.deck-slide-thumb` (CSS `transform: scale()` live thumbnails), and six `.deck-theme-<id>` classes each defining `--deck-bg`, `--deck-heading-color`, `--deck-text-color`, `--deck-accent` with light/dark variants via the existing `:root[data-theme="dark"]` convention (tokens-dark.css pattern).

`index.html`: `<link data-trunk rel="copy-file" href="style/presentation.css" />`.

- [ ] **Step 3: Run tests + wasm check**

Run: `cd frontend && cargo test presentation:: 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/presentation frontend/style/presentation.css frontend/index.html frontend/i18n
git commit -m "feat(presentations): layout presets, six built-in themes, deck stylesheet"
```

(Adjust the i18n path to wherever the `.ftl` files actually live — find them with `ls frontend/**/*.ftl` before staging.)

---

### Task 8: Deck creation entry points

**Files:**
- Modify: every caller of `create_document_with_type` (find with `grep -rn "create_document_with_type" frontend/src` — the "New spreadsheet" entries; add a sibling "New presentation" item to each menu)
- Modify: `frontend/src/components/document_details.rs:63` (doc-type label), icon maps at `frontend/src/components/ask_dialog.rs:38`, `frontend/src/components/search_dialog.rs:48`, `frontend/src/components/at_menu.rs:784`

**Interfaces:**
- Consumes: `create_document_with_type` (`frontend/src/api/documents.rs:278` area) — already takes the type as a `String`; pass `"presentation"`.
- Produces: user-visible "New presentation" creation affordances; deck icon/label consistency across dialogs.

- [ ] **Step 1: Enumerate the call sites**

Run: `grep -rn "create_document_with_type\|\"spreadsheet\"" frontend/src --include=*.rs | grep -v spreadsheet_view | grep -v editor/`
For each hit that is a creation menu or a doc-type icon/label match, mirror the spreadsheet entry with a `"presentation"` one (i18n keys `deck-new`, `deck-doc-type-label`; pick an icon consistent with the existing icon set — read how the spreadsheet icon is defined and choose the presentation-screen glyph from the same set).

- [ ] **Step 2: Verify by compile + grep**

Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`
Then: `grep -rn "presentation" frontend/src/components --include=*.rs | wc -l` — expect hits in every file listed above.

- [ ] **Step 3: Commit**

```bash
git add -u frontend/src frontend/i18n
git commit -m "feat(presentations): New presentation creation entries and doc-type icons"
```

(`git add -u` stages only tracked modified files — allowed; it is `-A`/`.` that is banned.)

---

### Task 9: `DeckView` skeleton + page dispatch

**Files:**
- Create: `frontend/src/components/deck_view.rs`
- Modify: `frontend/src/components/mod.rs` (module decl), `frontend/src/pages/document.rs` (dispatch :3133-3172 and the doc-type gates at :1212, :1235, :1591, :2498, :2815, :3427, :3491)

**Interfaces:**
- Consumes: `Deck`/`deck_from_doc`/`deck_to_doc` (Task 6), presets/themes (Task 7), `EditorState`, `sync` plumbing signals from `document.rs`.
- Produces the component signature Tasks 10-12 extend:

```rust
#[component]
pub fn DeckView(
    editor_state: ReadSignal<Option<EditorState>>,
    on_state_change: Callback<EditorState>,
    on_change: Callback<()>,          // REST-fallback ping, same as spreadsheet
    doc_id: String,
    readonly: bool,
    on_request_frame_comment: Callback<String>,   // frame block_id (Task 12 wires the popup)
    frame_threads: ReadSignal<Vec<String>>,       // block_ids that have open threads
) -> impl IntoView
```

- [ ] **Step 1: Write the pure-logic tests first** (deck mutations, in `deck_view.rs` `#[cfg(test)]` — UI is exercised by doctor scenarios in Task 13, but every mutation is a testable pure function)

```rust
    #[test]
    fn slide_ops_add_duplicate_delete_reorder() {
        let mut deck = fixture_deck_two_slides();
        add_slide(&mut deck, 1, &LAYOUT_PRESETS[3]);        // insert after index 1
        assert_eq!(deck.slides.len(), 3);
        let dup = duplicate_slide(&mut deck, 0);
        assert_ne!(deck.slides[1].block_id, deck.slides[0].block_id, "dup gets fresh ids");
        assert_eq!(deck.slides[1].frames.len(), deck.slides[0].frames.len());
        assert!(deck.slides[1].frames.iter().zip(&deck.slides[0].frames)
            .all(|(a, b)| a.block_id != b.block_id));
        let _ = dup;
        move_slide(&mut deck, 3, 0);
        delete_slide(&mut deck, 0);
        assert_eq!(deck.slides.len(), 3);
    }

    #[test]
    fn delete_last_slide_leaves_one_blank() {
        let mut deck = fixture_deck_one_slide();
        delete_slide(&mut deck, 0);
        assert_eq!(deck.slides.len(), 1, "a deck always has >= 1 slide");
        assert!(deck.slides[0].frames.is_empty());
    }
```

`add_slide(deck: &mut Deck, after: usize, preset: &LayoutPreset)`, `duplicate_slide(&mut Deck, idx) -> usize`, `move_slide(&mut Deck, from, to)`, `delete_slide(&mut Deck, idx)` are free functions in `deck_view.rs` (or `presentation/model.rs` if you prefer; keep them out of the component closure so they stay testable).

- [ ] **Step 2: Run to verify failure, implement the functions, run to green.**

Run: `cd frontend && cargo test deck_view 2>&1 | tail -5`

- [ ] **Step 3: Build the component skeleton**

Copy the SpreadsheetView architecture wholesale (all anchors verified):

- `ensure_presentation_css()` mirroring `spreadsheet_view.rs:1205-1221` (`id="presentation-css"`, `href="/presentation.css"`).
- Local state: `RwSignal<Deck>`, `(active_slide, set_active_slide)`, `(selected_frame, set_selected_frame): signal::<Option<String>>` (frame block_id), `(persist_origin, set_persist_origin) = signal(false)` — the feedback-loop guard from `spreadsheet_view.rs:1357`.
- **Doc → model Effect** (the `spreadsheet_view.rs:2349` pattern — an `Effect::new`, never inside the render closure; see the mutex-re-entrancy comment at :2340): on `editor_state` change, if `persist_origin` is set, clear it and skip; else `deck_from_doc(&state.doc)` into the signal. If the doc has zero slides (fresh presentation), immediately create one `blank` slide and persist.
- **persist()** closure (the `spreadsheet_view.rs:1686-1719` pattern):

```rust
    let persist = move || {
        let doc = deck.with_untracked(|d| deck_to_doc(d));
        set_persist_origin.set(true);
        on_state_change.run(EditorState::create_default(doc));
        on_change.run(());
    };
```

- Render: slide strip (`<For each=slides>` of scaled thumbnails + add/duplicate/delete buttons + drag reorder calling `move_slide` + `persist()`), canvas rendering the active slide's frames sorted by z as absolutely-positioned `<div class="deck-frame">` with `style:left=format!("{}%", r.x * 100.0)` etc., frame content rendered read-only in this task (reuse the editor's DOM rendering by calling the same node-render path the thumbnails use — if no such standalone helper exists, render paragraphs/headings as plain elements for now; Task 11 brings real editing), theme class from `theme_class(&deck.theme)` on the canvas root, and a theme-picker dropdown writing `deck.theme` + `persist()`.

- [ ] **Step 4: Wire the page dispatch** (`pages/document.rs`)

- In the mount closure (:3133): before the spreadsheet branch, add `if doc_type.get() == "presentation" { return view! { <DeckView editor_state=editor_state on_state_change=on_state_change.clone() on_change=on_change_ss.clone() doc_id=doc_id() readonly=!can_edit.get() on_request_frame_comment=... frame_threads=... /> }.into_any(); }` (reuse `on_change_ss` — the REST-fallback debounce at :1516 is doc-type-agnostic; pass placeholder callbacks for the Task-12 props until then).
- Widen the spreadsheet-only gates to include presentations (each is a one-line `== "spreadsheet"` → `matches!(doc_type.get().as_str(), "spreadsheet" | "presentation")` change): remote_state bridge Effect :1212, EditorState init Effect :1235, title-derivation skip :1591, rename-via-prompt :2498, header title `<input>` :2815, duplicate-dialog branch :3427. `DocumentDetailsDialog` :3491 already receives `doc_type` — extend its label match (Task 8 did `document_details.rs`).

- [ ] **Step 5: Verify**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`
Expected: PASS / clean.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/deck_view.rs frontend/src/components/mod.rs frontend/src/pages/document.rs
git commit -m "feat(presentations): DeckView skeleton with slide strip, themes, and page dispatch"
```

---

### Task 10: Canvas interactions — select, drag, resize, keymap

**Files:**
- Modify: `frontend/src/components/deck_view.rs`
- Create: `frontend/src/presentation/geometry.rs` (pure interaction math)

**Interfaces:**
- Consumes: `Rect`, `DeckFrame`, `selected_frame` signal (Task 9).
- Produces (pure, unit-tested):

```rust
pub enum DragKind { Move, Resize(Corner) }        // Corner: Nw, Ne, Sw, Se
pub fn apply_drag(rect: Rect, kind: DragKind, dx: f64, dy: f64) -> Rect  // clamped
pub fn snap(rect: Rect, others: &[Rect], threshold: f64) -> (Rect, Vec<Guide>)
pub struct Guide { pub axis: Axis, pub at: f64 }  // rendered as snap lines
pub fn next_frame_id(frames: &[DeckFrame], current: Option<&str>) -> Option<String> // z-then-position cycle
pub fn nudge(rect: Rect, dx: f64, dy: f64) -> Rect
```

- [ ] **Step 1: Write failing geometry tests**

```rust
    #[test]
    fn drag_move_clamps_inside_slide() {
        let r = Rect::clamped(0.8, 0.8, 0.3, 0.3); // w gets clamped to fit? No: w stays, x clamps
        let out = apply_drag(r, DragKind::Move, 0.5, 0.5);
        assert!(out.x + out.w <= 1.0 + 1e-9 && out.y + out.h <= 1.0 + 1e-9);
    }

    #[test]
    fn resize_never_collapses_below_min() {
        let r = Rect::clamped(0.1, 0.1, 0.3, 0.3);
        let out = apply_drag(r, DragKind::Resize(Corner::Se), -0.5, -0.5);
        assert!(out.w >= MIN_FRAME_DIM && out.h >= MIN_FRAME_DIM);
    }

    #[test]
    fn snap_attracts_to_slide_center_and_edges() {
        let r = Rect::clamped(0.496, 0.3, 0.2, 0.2); // center-x ≈ 0.596 → no; left ≈ 0.5-ish
        let (snapped, guides) = snap(Rect::clamped(0.492, 0.3, 0.2, 0.2), &[], 0.01);
        // frame center 0.592; left edge 0.492 ≈ nothing; center-snap: place a frame whose
        // center is within threshold of 0.5 and assert it lands exactly centered:
        let (c, g) = snap(Rect::clamped(0.395, 0.3, 0.2, 0.2), &[], 0.01);
        assert!((c.x + c.w / 2.0 - 0.5).abs() < 1e-9);
        assert!(!g.is_empty());
        let _ = (r, snapped, guides);
    }

    #[test]
    fn tab_cycles_frames_in_z_then_position_order() {
        let frames = fixture_frames(); // shuffled z values
        let first = next_frame_id(&frames, None).unwrap();
        let mut seen = vec![first.clone()];
        let mut cur = Some(first);
        for _ in 1..frames.len() {
            cur = next_frame_id(&frames, cur.as_deref());
            seen.push(cur.clone().unwrap());
        }
        assert_eq!(next_frame_id(&frames, cur.as_deref()), Some(seen[0].clone()), "wraps");
        let mut sorted = seen.clone(); sorted.dedup();
        assert_eq!(sorted.len(), frames.len(), "visits every frame once");
    }
```

- [ ] **Step 2: Run to verify failure, implement `geometry.rs`, run to green.**

Run: `cd frontend && cargo test geometry 2>&1 | tail -5`

- [ ] **Step 3: Wire pointer + key events in `DeckView`**

- Pointer: `on:pointerdown` on frame → select + `set_pointer_capture`; `on:pointermove` converts pixel deltas to normalized units (divide by the canvas `getBoundingClientRect()` size), applies `apply_drag` + `snap` into a **transient drag signal** (render from it for 60fps feedback); `on:pointerup` commits the final rect into the deck + `persist()` — one yrs write per gesture, not per mousemove. Resize handles are four corner divs with their own pointerdown.
- Render snap `Guide`s as absolutely-positioned 1px lines while dragging.
- Keymap (a `on:keydown` handler on the focused canvas, implementing the design's matrix exactly — design/presentations.md "Canvas keymap matrix"): Enter = enter frame edit (Task 11 makes this real; stub selects), Escape = deselect, Delete/Backspace = delete selected frame + `persist()`, Tab/Shift-Tab = `next_frame_id` cycle, arrows = `nudge` by 0.01 (Shift: 0.05) + persist on keyup (coalesce repeats), Cmd/Ctrl-D = duplicate frame (fresh blockIds) + persist, prevent default browser bookmark.
- "Add text frame" toolbar button: creates a `content` frame (0.3, 0.3, 0.4, 0.2) with an empty paragraph; "Add notes frame" only via slide menu (renders in a collapsed drawer below the canvas, not on it — `role=notes` frames are never positioned on the canvas per the design).

- [ ] **Step 4: Verify**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/presentation/geometry.rs frontend/src/presentation/mod.rs frontend/src/components/deck_view.rs
git commit -m "feat(presentations): canvas drag/resize/snap and the frame keymap"
```

---

### Task 11: Frame text editing (embedded editor)

**Files:**
- Modify: `frontend/src/components/deck_view.rs`, `frontend/src/presentation/model.rs` (splice helper)

**Interfaces:**
- Consumes: `EditorComponent` + `EditorProps` (`frontend/src/components/editor_component.rs:32-60`), `doc_to_ydoc_bytes` (`yrs_bridge.rs:20`).
- Produces: double-click-to-edit frames; `pub fn replace_frame_content(deck: &mut Deck, frame_id: &str, content: Fragment) -> bool`.

**Approach (from the design + SpreadsheetView's cell-editor precedent):** an editing frame mounts a scoped `EditorComponent` whose document is a synthetic `Doc` containing just that frame's children. On every inner `on_state_change`, splice the inner doc's children back into the frame and `persist()`. While a frame is being edited, the doc→model resync Effect must not clobber that frame (same reason as `persist_origin`): skip updating the *edited frame's* content from remote, but still apply remote changes to everything else; last write wins on the edited frame, which matches spreadsheet cell-editing behavior.

- [ ] **Step 1: Write failing splice tests**

```rust
    #[test]
    fn replace_frame_content_swaps_only_that_frame() {
        let mut deck = fixture_deck_two_slides();
        let target = deck.slides[1].frames[0].block_id.clone();
        let new_content = Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph, Fragment::from(vec![Node::text("edited")]))]);
        assert!(replace_frame_content(&mut deck, &target, new_content.clone()));
        assert_eq!(deck.slides[1].frames[0].content, new_content);
        assert_ne!(deck.slides[0].frames[0].content, new_content);
        assert!(!replace_frame_content(&mut deck, "missing-id", Fragment::empty()));
    }

    #[test]
    fn resync_from_doc_preserves_edited_frame() {
        // merge_remote_deck(local, remote, editing: Some(frame_id)) keeps the
        // local content of the edited frame but adopts remote everything-else.
        let local = fixture_deck_two_slides();
        let mut remote = local.clone();
        remote.slides[0].frames[0].rect = Rect::clamped(0.5, 0.5, 0.4, 0.3);
        let editing = local.slides[1].frames[0].block_id.clone();
        let merged = merge_remote_deck(&local, remote.clone(), Some(&editing));
        assert_eq!(merged.slides[0].frames[0].rect, remote.slides[0].frames[0].rect);
        assert_eq!(merged.slides[1].frames[0].content, local.slides[1].frames[0].content);
    }
```

- [ ] **Step 2: Run to verify failure, implement `replace_frame_content` + `merge_remote_deck` in `presentation/model.rs`, run to green.**

- [ ] **Step 3: Mount the inner editor**

In `DeckView`: `(editing_frame, set_editing_frame): signal::<Option<String>>`. On double-click (or Enter on a selected frame): build `let inner_doc = Node::element_with_content(NodeType::Doc, frame.content.clone());` and mount

```rust
<EditorComponent props=EditorProps {
    initial_content: Some(doc_to_ydoc_bytes(&inner_doc)),
    on_change: Callback::new(|_| {}),        // outer persist handles transport
    on_state_change: Callback::new(move |st: EditorState| {
        let content = match &st.doc { Node::Element { content, .. } => content.clone(), _ => return };
        deck.update(|d| { replace_frame_content(d, &frame_id, content); });
        persist();
    }),
    command_signal: inner_toolbar_cmd,       // fresh signal; deck toolbar feeds it
    remote_state: inner_remote,              // always None — remote merge is handled at deck level
    doc_id: doc_id.clone(),
    on_scroll: None, on_mapping: None,
    on_request_comment: None,                // frame comments come from the frame chrome (Task 12)
    readonly: false,
} />
```

Escape or clicking outside → `set_editing_frame.set(None)` (the resync Effect resumes full-merge for that frame). Pass `editing_frame` into the doc→model Effect and use `merge_remote_deck` there.

- [ ] **Step 4: Verify**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/deck_view.rs frontend/src/presentation/model.rs
git commit -m "feat(presentations): in-frame text editing via embedded editor"
```

---

### Task 12: Frame comments

**Files:**
- Modify: `frontend/src/components/deck_view.rs`, `frontend/src/pages/document.rs`

**Interfaces:**
- Consumes: `CommentPopup` (mounted once at page level, `document.rs:3528-3558` — props `block_id`, `anchor_start`, `anchor_end`, `is_new`, ...), `ConversationPane` `on_threads_loaded: Callback<Vec<InlineThreadInfo>>` (`conversation_pane.rs:19-51`), `create_thread(doc_id, message, block_id, None, None)` semantics (`api/comments.rs:67` — blockId present ⇒ inline thread).
- Produces: comment affordance on the selected frame; thread badges on frames; pane filtered to the current slide.

- [ ] **Step 1: Write the failing filter test**

```rust
    #[test]
    fn threads_filter_to_slide_frames() {
        let deck = fixture_deck_two_slides();
        let slide0_ids: Vec<String> =
            deck.slides[0].frames.iter().map(|f| f.block_id.clone()).collect();
        let threads = vec![
            (slide0_ids[0].clone(), "t1".to_string()),
            (deck.slides[1].frames[0].block_id.clone(), "t2".to_string()),
            ("orphan".to_string(), "t3".to_string()),
        ];
        let visible = threads_for_slide(&deck, 0, &threads);
        assert_eq!(visible, vec!["t1".to_string()]);
    }
```

`pub fn threads_for_slide(deck: &Deck, slide: usize, threads: &[(String, String)]) -> Vec<String>` — (block_id, thread_id) pairs in, thread_ids whose block_id belongs to a frame of that slide out.

- [ ] **Step 2: Run to verify failure, implement, run to green.**

- [ ] **Step 3: Wire the UI**

- Selected-frame chrome gains a comment button → `on_request_frame_comment.run(frame_block_id)`.
- In `document.rs`, the DeckView's `on_request_frame_comment` callback sets the existing popup signals exactly as `request_comment` does at :2090-2115, minus the selection-offset logic: `set_popup_block_id.set(Some(bid)); set_popup_anchor_start.set(None); set_popup_anchor_end.set(None); set_popup_is_new.set(true); set_popup_thread_id.set(None);` plus whatever popup-position signals that closure sets (copy them verbatim from the surrounding code).
- `frame_threads` prop: the page already receives inline threads via `ConversationPane`'s `on_threads_loaded` (`document.rs:3224-3252` mount); map them to block_ids and pass the signal into DeckView. Frames whose block_id has a thread render a small badge; clicking it opens the popup on that thread (`set_popup_thread_id.set(Some(tid)); set_popup_is_new.set(false);`).
- Deck comments pane: keep mounting `ConversationPane` as-is (it is doc-shaped, works unchanged); the slide-filter uses `threads_for_slide` client-side to decide badge visibility only. Full pane filtering can ride `filter_thread_id` later — do not extend `ConversationPane`'s props in P1.

- [ ] **Step 4: Verify**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/deck_view.rs frontend/src/pages/document.rs
git commit -m "feat(presentations): frame-anchored comments in DeckView"
```

---

### Task 13: Full verification + doctor scenario

**Files:**
- Create: a `deck-basics` scenario in `scripts/frontend-doctor/doctor.js` (the doctor only runs scenarios that exist in this file — write it now even though it runs post-deploy)

**Interfaces:**
- Consumes: everything above.
- Produces: green workspace + frontend + wasm; a committed doctor scenario for the eventual deployed check.

- [ ] **Step 1: Backend full sweep**

Run: `cargo test --workspace 2>&1 | tail -10`
Expected: PASS. Investigate any failure — do not weaken an existing test.

- [ ] **Step 2: Frontend native + wasm**

Run: `cd frontend && cargo test 2>&1 | tail -5 && cargo check --target wasm32-unknown-unknown 2>&1 | tail -3`
Expected: PASS / clean. (WASM check is mandatory: native `cargo check` silently skips `cfg(target_arch = "wasm32")` code.)

- [ ] **Step 3: Write the doctor scenario**

Read two existing scenarios in `scripts/frontend-doctor/doctor.js` (e.g. the block-links and doc-mentions ones) and mirror their structure exactly for `deck-basics`: log in → create a presentation via the New menu → assert DeckView canvas renders → add a slide from the `title-content` preset → double-click the body frame, type text, Escape → reload → assert the text persisted. Keep assertions on visible DOM (`.deck-slide-thumb` count, frame text), not internals.

- [ ] **Step 4: Trunk build smoke**

Run: `cd frontend && trunk build 2>&1 | tail -5`
Expected: clean bundle (presentation.css copied).

- [ ] **Step 5: Commit**

```bash
git add scripts/frontend-doctor/doctor.js
git commit -m "test(presentations): deck-basics frontend-doctor scenario"
```

---

## Post-plan notes for the reviewer

- **Deferred to P2 (per design phasing):** present mode + `d/:id/present` route (declare before `d/:id/:slug` — ordering matters), presenter view, live-follow awareness field (touches the golden protocol fixtures on both sides), PDF slide renderer (`to_pdf` today is text-flow only — a real layout renderer is new work), mobile view.
- **Deferred to P3:** `FeedbackPrompt`/`FeedbackResponse` block.
- **Known accepted risks:** `render_html_attrs`'s `_` arm and markdown's `_` fall-through are silent for future variants (pre-existing pattern); `Doc.valid_children` now permits `Slide` in any doc — enforced by construction, documented in Task 2.
- The design doc's `/doc/{id}/present` path is wrong (`d/:id` is the real route) — report as a drift finding when P2 starts; do not edit `design/` from this plan.
