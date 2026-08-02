# Presentations P2 — Presenting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the presenting half of presentations — full-screen present mode, presenter view with speaker notes, live follow-the-presenter over awareness, PDF slide export, and mobile view/present.

**Architecture:** A new sidebar-free route (`/d/:id/present`) renders the existing `render_deck_canvas` at viewport size; `?presenter=1` adds next-slide preview + notes + timer. Live follow rides the existing WS awareness channel via one new optional `presenting` field (the relay is an opaque pass-through, so only the two endpoint structs and the golden fixtures change). PDF slide export is a new one-page-per-slide renderer in `crates/collab/src/export.rs`, fed by a new backend theme-color table mirrored from the frontend CSS.

**Tech Stack:** Rust workspace (axum, yrs, printpdf 0.9) + Leptos 0.7 WASM frontend (`frontend/` is OUTSIDE the workspace — always `cd frontend/` for its cargo commands).

## Global Constraints

- Existing tests are behavioral contracts. The only files where existing tests may be *extended* (never weakened) are the ones each task names explicitly.
- Never `git add -A` or `git add .` — stage named files only.
- Do not edit `design/`, `framework/`, or `runbook/`. Drift is reported, not applied.
- Identifiers are raw `String` (`identifier_strategy = "string-grandfathered"`); no ID newtypes.
- Backend tests: `cargo test -p ogrenotes-collab` / `-p ogrenotes-api` from the repo root. Frontend: `cd frontend && cargo test`, plus the mandatory `cargo check --target wasm32-unknown-unknown` (a native check silently skips `cfg(target_arch = "wasm32")` code).
- Server-side attr validators are **accept-verbatim-or-reject; never rewrite a value** (`crates/collab/src/blocks/presentation.rs:38-40`). Readers clamp.
- New i18n keys go in `frontend/locales/en-US/main.ftl` **only** — the other five locales (`ar de es fr it`) are deliberately frozen and fall back to en-US.
- Awareness wire changes must follow `tests/fixtures/protocol/awareness/README.md`'s contract: a field added to one side without a fixture fails the sibling side's test. That is the point; satisfy it, don't route around it.
- The deck canvas markup is shared by the editor, the slide-strip thumbnails, and (now) present mode — changes to `render_deck_canvas` affect all three.

---

## File Structure

**Backend**
- `crates/collab/src/themes.rs` *(new)* — deck theme id + four hex colors; the source of truth the PDF renderer needs (CSS is unavailable server-side).
- `crates/collab/src/export.rs` *(modify)* — deck detection + `to_pdf_slides`; existing flow-text path untouched for non-decks.
- `crates/collab/src/awareness.rs` *(modify)* — `presenting` field + fixture test.
- `crates/collab/src/lib.rs` *(modify)* — `pub mod themes;`.

**Frontend**
- `frontend/src/lib.rs`, `frontend/src/main.rs` *(modify)* — promote `presentation` into the lib crate so CI's `cargo test --lib` gates its tests.
- `frontend/src/presentation/nav.rs` *(new)* — pure slide-navigation helpers (index/blockId mapping, next/prev).
- `frontend/src/pages/present.rs` *(new)* — the present-mode page (both plain and `?presenter=1` layouts).
- `frontend/src/components/deck_view.rs` *(modify)* — `pub(crate)` on the shared renderers; a "Present" entry point.
- `frontend/src/collab/ws_client.rs` *(modify)* — `presenting` on payload/cursor/send.
- `frontend/src/app.rs`, `frontend/src/pages/mod.rs` *(modify)* — route + module.
- `frontend/src/pages/document.rs` *(modify)* — PDF export menu entry.
- `frontend/style/presentation.css` *(modify)* — present/presenter/mobile styles.
- `tests/fixtures/protocol/awareness/presenting.json` *(new)* + README row.
- `scripts/frontend-doctor/doctor.js`, `.github/workflows/playwright.yml` *(modify)* — `deck-present` scenario.

---

### Task 1: Promote `presentation` into the lib crate

CI runs `cargo test --lib` for the frontend (`.github/workflows/ci.yml:73`), but `presentation` is declared only in `frontend/src/main.rs:20` — so every P1/frame-blocks test in `presentation/` (geometry, model, presets, themes) has **never** run in CI. `frontend/src/lib.rs:17-19` documents this exact fix for `menu_nav`.

**Files:**
- Modify: `frontend/src/lib.rs` (module list, after `pub mod observability;`)
- Modify: `frontend/src/main.rs:20`

**Interfaces:**
- Produces: `ogrenotes_frontend::presentation::*` — every later frontend task's imports resolve identically from both targets (`crate::presentation::…` still works inside the binary via the re-export).

- [ ] **Step 1: Confirm the tests are currently invisible to CI's command**

Run: `cd frontend && cargo test --lib 2>&1 | grep -cE "presentation::(model|geometry|presets|themes)"`
Expected: `0` (they don't run). Also run `cargo test --lib 2>&1 | grep -E "^test result"` and note the count — Step 4 must show it grow.

- [ ] **Step 2: Add the module to the lib crate**

In `frontend/src/lib.rs`, after `pub mod observability;`:

```rust
// Deck model, layout presets, themes, and canvas geometry for
// presentations (components/deck_view.rs in the binary consumes
// them). Lib-visible so `cargo test --lib` — the CI tier-1 command —
// runs their unit tests; before this they compiled only into the
// binary target and never gated CI.
pub mod presentation;
```

- [ ] **Step 3: Re-export instead of re-declaring in the binary**

In `frontend/src/main.rs`, replace `mod presentation;` with (mirroring the `i18n` treatment two lines above it):

```rust
pub use ogrenotes_frontend::presentation;
```

- [ ] **Step 4: Verify the tests now run under the CI command**

Run: `cd frontend && cargo test --lib 2>&1 | grep -cE "presentation::(model|geometry|presets|themes)"`
Expected: a non-zero count (≈40).
Run: `cd frontend && cargo test 2>&1 | grep -E "test result: ok\. [0-9]{3,}"` — both totals still pass, 0 failed.
Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -1` — clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib.rs frontend/src/main.rs
git commit -m "test(presentations): run presentation module tests under cargo test --lib"
```

---

### Task 2: Backend deck theme table + duality tests

The PDF slide renderer needs theme colors server-side, where CSS doesn't exist. `crates/collab` has no themes module today (verified: zero `DECK_THEMES`/`deck-theme` hits under `crates/`).

**Files:**
- Create: `crates/collab/src/themes.rs`
- Modify: `crates/collab/src/lib.rs` (add `pub mod themes;` next to the other `pub mod` lines)

**Interfaces:**
- Produces (Task 3 consumes):

```rust
pub struct DeckTheme { pub id: &'static str, pub bg: &'static str, pub heading: &'static str, pub text: &'static str, pub accent: &'static str }
pub const DECK_THEMES: &[DeckTheme];              // 6 entries
pub fn theme_by_id(id: &str) -> &'static DeckTheme; // unknown -> slate
pub fn hex_to_rgb(hex: &str) -> Option<(f32, f32, f32)>; // 0.0..=1.0 components
```

- [ ] **Step 1: Write the failing tests**

Create `crates/collab/src/themes.rs` with this test module (implementation comes in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_theme_falls_back_to_slate() {
        assert_eq!(theme_by_id("nope").id, "slate");
        assert_eq!(theme_by_id("ocean").id, "ocean");
        assert_eq!(DECK_THEMES.len(), 6);
    }

    #[test]
    fn hex_parses_to_unit_components() {
        let (r, g, b) = hex_to_rgb("#ffffff").unwrap();
        assert!((r - 1.0).abs() < 1e-6 && (g - 1.0).abs() < 1e-6 && (b - 1.0).abs() < 1e-6);
        let (r, _, _) = hex_to_rgb("#000000").unwrap();
        assert!(r.abs() < 1e-6);
        assert!(hex_to_rgb("ffffff").is_none(), "must require the leading #");
        assert!(hex_to_rgb("#fff").is_none(), "short form unsupported");
        assert!(hex_to_rgb("#gggggg").is_none());
        for t in DECK_THEMES {
            for hex in [t.bg, t.heading, t.text, t.accent] {
                assert!(hex_to_rgb(hex).is_some(), "{} has unparseable {hex}", t.id);
            }
        }
    }

    /// Duality: the frontend owns these colors in CSS custom properties
    /// and the ids in `presentation/themes.rs`. A drift on either side
    /// silently changes what a deck looks like in the app vs its PDF.
    /// Mirrors the source-text assertion precedent at
    /// `crates/api/src/routes/ws.rs:1358` (subprotocol constants).
    #[test]
    fn theme_table_matches_the_frontend() {
        let css = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/style/presentation.css"
        ))
        .expect("frontend presentation.css must be readable from the collab crate tests");
        let ids_rs = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/src/presentation/themes.rs"
        ))
        .expect("frontend themes.rs must be readable");
        for t in DECK_THEMES {
            assert!(
                ids_rs.contains(&format!("id: \"{}\"", t.id)),
                "frontend themes.rs is missing theme id {}",
                t.id
            );
            let selector = format!(".deck-theme-{} {{", t.id);
            let start = css
                .find(&selector)
                .unwrap_or_else(|| panic!("presentation.css has no {selector}"));
            let block = &css[start..start + css[start..].find('}').expect("unterminated block")];
            for (var, want) in [
                ("--deck-bg", t.bg),
                ("--deck-heading-color", t.heading),
                ("--deck-text-color", t.text),
                ("--deck-accent", t.accent),
            ] {
                assert!(
                    block.contains(&format!("{var}: {want}")),
                    "theme {} : CSS {var} does not match the backend table value {want}",
                    t.id
                );
            }
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-collab themes 2>&1 | tail -5`
Expected: compile error (module not declared / items missing).

- [ ] **Step 3: Implement**

Prepend to `crates/collab/src/themes.rs`:

```rust
//! Deck theme colors — the server-side mirror of the frontend's
//! `.deck-theme-<id>` CSS custom properties.
//!
//! The frontend renders decks with CSS; the PDF exporter has no CSS,
//! so the four palette colors live here as hex strings and a unit test
//! asserts they still match `frontend/style/presentation.css`. Ids and
//! their order mirror `frontend/src/presentation/themes.rs`.

/// One deck theme: the id persisted in the `Doc`'s `theme` attribute,
/// plus the four colors the canvas (and the PDF renderer) paint with.
pub struct DeckTheme {
    pub id: &'static str,
    pub bg: &'static str,
    pub heading: &'static str,
    pub text: &'static str,
    pub accent: &'static str,
}

/// Light-mode palette values, verbatim from `presentation.css`. PDF is
/// a light-only medium, so the `:root[data-theme="dark"]` variants are
/// deliberately not mirrored.
pub const DECK_THEMES: &[DeckTheme] = &[
    DeckTheme { id: "slate",    bg: "#2a3440", heading: "#f5f7fa", text: "#c9d1d9", accent: "#5b9bd5" },
    DeckTheme { id: "paper",    bg: "#f5f0e8", heading: "#1a1a1a", text: "#3a3a3a", accent: "#2d5f2d" },
    DeckTheme { id: "midnight", bg: "#12121f", heading: "#ffffff", text: "#b8b8d0", accent: "#7c6ff0" },
    DeckTheme { id: "ember",    bg: "#2b1810", heading: "#ffe8d6", text: "#e0c4a8", accent: "#e8743b" },
    DeckTheme { id: "forest",   bg: "#14231a", heading: "#e8f5e9", text: "#b8d4bc", accent: "#4a9960" },
    DeckTheme { id: "ocean",    bg: "#0b2027", heading: "#e0f7fa", text: "#a8d8de", accent: "#1ab0c4" },
];

/// Unknown / absent ids fall back to the first theme, matching
/// `theme_class`'s behavior on the frontend.
pub fn theme_by_id(id: &str) -> &'static DeckTheme {
    DECK_THEMES.iter().find(|t| t.id == id).unwrap_or(&DECK_THEMES[0])
}

/// `#rrggbb` -> unit-interval RGB components for printpdf. Strict:
/// requires the `#`, exactly six hex digits.
pub fn hex_to_rgb(hex: &str) -> Option<(f32, f32, f32)> {
    let body = hex.strip_prefix('#')?;
    if body.len() != 6 || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&body[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    Some((v(0)?, v(2)?, v(4)?))
}
```

In `crates/collab/src/lib.rs`, add `pub mod themes;` alongside the other module declarations (keep alphabetical order with its neighbors).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p ogrenotes-collab themes 2>&1 | grep -E "test result"`
Expected: PASS (3 tests). If `theme_table_matches_the_frontend` fails, the CSS is the source of truth — copy the CSS value into the table, do not edit the CSS.

- [ ] **Step 5: Commit**

```bash
git add crates/collab/src/themes.rs crates/collab/src/lib.rs
git commit -m "feat(presentations): backend deck theme color table with frontend duality test"
```

---

### Task 3: PDF slide renderer — one page per slide

Today `to_pdf` flows plain text onto A4 portrait pages (`crates/collab/src/export.rs:601`), so a deck exports as one run-on paragraph per slide. Decks are self-describing — their top-level children are `Slide` elements — so no `doc_type` plumbing is needed.

**Files:**
- Modify: `crates/collab/src/export.rs` (`to_pdf_with_comments` ~:601; helpers `collect_block_text` :705, `wrap_text` :724, `extract_text` :314)

**Interfaces:**
- Consumes: `crate::themes::{theme_by_id, hex_to_rgb}` (Task 2); `NodeType::{Slide, Frame}`; `blocks::presentation::FRAME_ATTR_NAMES`.
- Produces: `to_pdf`/`to_pdf_with_comments` render decks as landscape one-page-per-slide; non-deck documents keep byte-identical behavior.

- [ ] **Step 1: Discover the printpdf 0.9 drawing ops (do not guess)**

The existing code only emits text ops. Find the real variant names for filled rectangles:

```bash
find ~/.cargo/registry/src -maxdepth 3 -type d -name 'printpdf-0.9*'
grep -rn "^    [A-Z][A-Za-z]*" "$(find ~/.cargo/registry/src -maxdepth 3 -type d -name 'printpdf-0.9*' | head -1)/src/ops.rs" | head -60
```

Record in your report the exact variants for: setting a fill color, drawing/filling a polygon or rectangle, and the color type constructor. Everything below uses the placeholder names `SetFillColor`/`DrawPolygon`/`Color::Rgb(Rgb::new(r,g,b,None))` — **substitute the real ones**. If 0.9 offers no usable fill primitive, fall back to: no background fill, text only, at slide geometry — and say so in the report.

- [ ] **Step 2: Write the failing tests**

Add to `export.rs`'s existing `#[cfg(test)] mod tests` (find `pdf_export_with_comments_is_valid_and_nonempty` around :3216 and add beside it). Build the deck fixture with the same yrs-doc helpers the neighboring tests use — read two of them first:

```rust
    #[cfg(feature = "pdf")]
    #[test]
    fn deck_pdf_has_one_landscape_page_per_slide() {
        // 3 slides, each with a heading frame and a body frame.
        let doc = fixture_deck_three_slides();
        let bytes = to_pdf(&doc);
        assert!(bytes.starts_with(b"%PDF-"));
        // printpdf writes one `/Type /Page` object per page.
        let text = String::from_utf8_lossy(&bytes);
        let pages = text.matches("/Type /Page\n").count() + text.matches("/Type/Page\n").count();
        assert_eq!(pages, 3, "one page per slide");
        // Landscape 16:9 — MediaBox width must exceed height.
        assert!(
            text.contains("/MediaBox"),
            "page boxes must be emitted so the aspect can be asserted"
        );
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn deck_pdf_contains_frame_text_in_slide_order() {
        let doc = fixture_deck_ordered_text(); // slide1 "ALPHA", slide2 "BETA"
        let bytes = to_pdf(&doc);
        let text = String::from_utf8_lossy(&bytes);
        let a = text.find("ALPHA").expect("slide 1 text present");
        let b = text.find("BETA").expect("slide 2 text present");
        assert!(a < b, "slides must render in document order");
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn non_deck_pdf_is_unchanged_by_the_slide_path() {
        // A plain document must still take the A4 flow-text path.
        let doc = fixture_plain_paragraphs(); // reuse an existing fixture helper
        let bytes = to_pdf(&doc);
        assert!(bytes.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&bytes);
        let pages = text.matches("/Type /Page\n").count() + text.matches("/Type/Page\n").count();
        assert_eq!(pages, 1, "short plain doc stays a single A4 page");
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn deck_pdf_tolerates_malformed_geometry() {
        let doc = fixture_deck_bad_geometry(); // x="garbage", w="99", h=""
        let bytes = to_pdf(&doc); // must not panic
        assert!(bytes.starts_with(b"%PDF-"));
    }
```

If the `/Type /Page` spelling assertion proves brittle against printpdf's actual output, replace it with `pdf_extract`-based page counting (the crate is already a `pdf` feature dep) — but keep the assertion *behavioral* (page count, order), never "it returned bytes".

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p ogrenotes-collab --features pdf deck_pdf 2>&1 | tail -15`
Expected: FAIL — a deck currently renders one A4 portrait page of run-on text.

- [ ] **Step 4: Implement**

In `export.rs`, add the deck branch at the top of `to_pdf_with_comments` and a new renderer beside it:

```rust
/// True when this document is a slide deck: its top-level children are
/// `Slide` elements. Decks are self-describing, so the exporter needs
/// no `doc_type` plumbing from the API layer.
#[cfg(feature = "pdf")]
fn is_deck(doc: &Doc) -> bool {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else { return false };
    (0..fragment.len(&txn)).any(|i| {
        matches!(fragment.get(&txn, i), Some(XmlOut::Element(el))
            if NodeType::from_tag(el.tag().as_ref()) == Some(NodeType::Slide))
    })
}
```

At the top of `to_pdf_with_comments`, before the existing flow-text body:

```rust
    if is_deck(doc) {
        // Decks render as slides (landscape, one page each); comments
        // are omitted — a slide deck's PDF is the deck, and the flow
        // path's trailing comment section has no place on a slide.
        return to_pdf_slides(doc);
    }
```

Then the renderer. Geometry: slide `Doc` attr `theme`; frame attrs `x`/`y`/`w`/`h` normalized 0..1 (clamp on read — malformed values must never panic), `z` for paint order, `role="notes"` frames excluded (they are speaker notes, not slide content). Page: 297mm × 167mm (A4 landscape width at 16:9). PDF's origin is bottom-left, the model's `y` is top-down — so `pdf_y = page_h - (y + h) * page_h`.

```rust
#[cfg(feature = "pdf")]
fn to_pdf_slides(doc: &Doc) -> Vec<u8> {
    use printpdf::{
        BuiltinFont, Mm, Op, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, PdfWarnMsg,
        Point, Pt, TextItem,
    };
    const PAGE_W_MM: f32 = 297.0;
    const PAGE_H_MM: f32 = 167.0; // 16:9 at A4-landscape width
    const HEADING_PT: f32 = 28.0;
    const BODY_PT: f32 = 14.0;
    const LINE_RATIO: f32 = 1.25; // leading as a multiple of font size
    // Chars-per-line estimate for Helvetica at BODY_PT across a full
    // page width; scaled per frame by its fractional width. Same
    // proportional-font caveat as the flow path's WRAP_CHARS.
    const FULL_WIDTH_CHARS: f32 = 95.0;

    let theme_id = doc_attr(doc, "theme").unwrap_or_default();
    let theme = crate::themes::theme_by_id(&theme_id);
    let (bg_r, bg_g, bg_b) = crate::themes::hex_to_rgb(theme.bg).unwrap_or((1.0, 1.0, 1.0));
    let (h_r, h_g, h_b) = crate::themes::hex_to_rgb(theme.heading).unwrap_or((0.0, 0.0, 0.0));
    let (t_r, t_g, t_b) = crate::themes::hex_to_rgb(theme.text).unwrap_or((0.0, 0.0, 0.0));

    let font = PdfFontHandle::Builtin(BuiltinFont::Helvetica);
    let mut pages: Vec<PdfPage> = Vec::new();

    for slide in collect_slides(doc) {
        let mut ops: Vec<Op> = Vec::new();
        // Background fill — SUBSTITUTE the real ops found in Step 1.
        ops.push(Op::SetFillColor { col: printpdf::Color::Rgb(printpdf::Rgb::new(bg_r, bg_g, bg_b, None)) });
        ops.push(Op::DrawPolygon { /* full-page rect from (0,0) to (W,H) */ });

        for frame in slide.content_frames_sorted_by_z() {
            let is_heading = frame.first_child_is_heading;
            let size = if is_heading { HEADING_PT } else { BODY_PT };
            let (fr, fg, fb) = if is_heading { (h_r, h_g, h_b) } else { (t_r, t_g, t_b) };
            let max_chars = ((FULL_WIDTH_CHARS * frame.w as f32) as usize).max(8);
            let lines = wrap_text(&frame.text, max_chars);

            let x_pt = Pt::from(Mm(PAGE_W_MM * frame.x as f32)).0;
            // Model y is top-down; PDF y is bottom-up.
            let top_pt = Pt::from(Mm(PAGE_H_MM * (1.0 - frame.y as f32))).0;

            ops.push(Op::SetFillColor { col: printpdf::Color::Rgb(printpdf::Rgb::new(fr, fg, fb, None)) });
            ops.push(Op::StartTextSection);
            ops.push(Op::SetFont { font: font.clone(), size: Pt(size) });
            ops.push(Op::SetLineHeight { lh: Pt(size * LINE_RATIO) });
            ops.push(Op::SetTextCursor { pos: Point { x: Pt(x_pt), y: Pt(top_pt - size) } });
            for line in lines {
                if !line.is_empty() {
                    ops.push(Op::ShowText { items: vec![TextItem::Text(line)] });
                }
                ops.push(Op::AddLineBreak);
            }
            ops.push(Op::EndTextSection);
        }
        pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), ops));
    }

    if pages.is_empty() {
        pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), Vec::new()));
    }
    let mut pdf = PdfDocument::new("OgreNotes deck export");
    pdf.with_pages(pages);
    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    pdf.save(&PdfSaveOptions::default(), &mut warnings)
}
```

Write the two readers this needs, with these exact types (the renderer above consumes them):

```rust
/// One frame flattened for PDF: geometry already clamped, text already
/// extracted. `is_heading` drives font size and color choice.
#[cfg(feature = "pdf")]
struct PdfFrame {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    z: i64,
    is_heading: bool,
    text: String,
}

/// Content frames of one slide, already filtered (`role != "notes"`)
/// and sorted by `z` so later frames paint on top.
#[cfg(feature = "pdf")]
fn collect_slides(doc: &Doc) -> Vec<Vec<PdfFrame>>;

/// Deck-level `Doc` attribute (`theme`, `slideSize`). These live in the
/// root-level `docAttrs` yrs Map the frontend bridge writes — grep
/// `docAttrs` in `frontend/src/editor/yrs_bridge.rs` for the exact key
/// names and mirror the read side here.
#[cfg(feature = "pdf")]
fn doc_attr(doc: &Doc, name: &str) -> Option<String>;
```

Inside `collect_slides`: parse each frame's `x`/`y`/`w`/`h` with `attr.and_then(|v| v.parse::<f64>().ok()).filter(|f| f.is_finite())`, defaulting `x=0.0, y=0.0, w=1.0, h=1.0`, then `.clamp(0.0, 1.0)`; `z` via `parse::<i64>().unwrap_or(0)`; `is_heading` = the frame's first element child is a `Heading`; `text` = the existing `extract_text` over the frame element. In the renderer, replace `slide.content_frames_sorted_by_z()` with iterating the `Vec<PdfFrame>` directly and `frame.first_child_is_heading` with `frame.is_heading`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p ogrenotes-collab --features pdf 2>&1 | grep -E "test result|FAILED"`
Expected: PASS, including the pre-existing `pdf_export_with_comments_is_valid_and_nonempty` (the non-deck path must be untouched).
Run: `cargo test -p ogrenotes-collab 2>&1 | grep -E "test result: ok"` — the default-feature build still passes.

- [ ] **Step 6: Verify the API path end-to-end**

Run: `cargo test -p ogrenotes-api test_export_pdf 2>&1 | grep -E "test result"`
Expected: PASS unchanged (`crates/api/src/routes/documents.rs:2085` needs no edit — it calls `to_pdf_with_comments`, which now branches internally).

- [ ] **Step 7: Commit**

```bash
git add crates/collab/src/export.rs
git commit -m "feat(presentations): PDF slide renderer — one landscape page per slide"
```

---

### Task 4: Frontend PDF export entry

`ExportFormat` (`frontend/src/pages/document.rs:46-73`) has no `Pdf` variant, so the backend's PDF export has never been reachable from the UI.

**Files:**
- Modify: `frontend/src/pages/document.rs:46-73` (enum + `wire()` + `ext()`), and the export-menu construction that lists the variants (grep `ExportFormat::Xlsx` for every site)
- Modify: `frontend/locales/en-US/main.ftl`

**Interfaces:**
- Consumes: `GET /documents/:id/export/pdf` (already live).
- Produces: a "PDF" item in the export menu for every doc type.

- [ ] **Step 1: Add the variant**

```rust
enum ExportFormat { Html, Markdown, Csv, Xlsx, Pdf }
```
and in the two matches: `Self::Pdf => "pdf"` (wire) and `Self::Pdf => "pdf"` (ext).

- [ ] **Step 2: Add the menu item**

Run `grep -n "ExportFormat::Xlsx" frontend/src/pages/document.rs` and mirror each site for `Pdf`, using a new i18n key. Add to `frontend/locales/en-US/main.ftl` beside the other export keys (find them with `grep -n "menu-export" frontend/locales/en-US/main.ftl`):

```
menu-export-pdf = PDF
```

- [ ] **Step 3: Verify**

Run: `cd frontend && cargo test 2>&1 | grep -E "test result: ok\. [0-9]{3,}|FAILED"` — pass.
Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -1` — clean.
Run: `grep -c '"pdf"' frontend/src/pages/document.rs` — at least 1.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/pages/document.rs frontend/locales/en-US/main.ftl
git commit -m "feat(presentations): expose PDF in the export menu"
```

---

### Task 5: Awareness `presenting` field, end to end

The WS relay (`crates/api/src/routes/ws.rs:1045`) decodes → overwrites `user_id` → re-encodes → broadcasts with no per-field logic, so a new optional field rides through untouched. Only the two endpoint structs, the golden fixtures, and their tests change.

**Files:**
- Modify: `crates/collab/src/awareness.rs` (struct :50-96, the `MAX_AWARENESS_FIELD_BYTES` doc list :23-33, per-field length validation in `decode_awareness`, `empty_state`, fixture consts :329-338, tests :380-404)
- Modify: `frontend/src/collab/ws_client.rs` (`AwarenessPayload` :115-148, `send_awareness` :1051, `RemoteCursor` :170-184, `handle_awareness` :458, fixture consts :1618-1627, struct-literal tests :1510/:1537/:1562)
- Modify: `frontend/src/pages/document.rs` (the three `send_awareness` call sites: :1397, :1406, :1520)
- Create: `tests/fixtures/protocol/awareness/presenting.json`
- Modify: `tests/fixtures/protocol/awareness/README.md` (the "Current fixtures" table, :39-47)

**Interfaces:**
- Produces: `AwarenessState.presenting: Option<String>` / `AwarenessPayload.presenting: Option<String>` / `RemoteCursor.presenting: Option<String>` — a slide `block_id`. `send_awareness` gains a trailing `presenting: Option<&str>` parameter. Task 8 consumes all three.

- [ ] **Step 1: Write the fixture and the failing tests**

Create `tests/fixtures/protocol/awareness/presenting.json`:

```json
{
    "user_id": "usr_9tKp2wQ4mNvXbZr7sLa1e",
    "name": "Presenter",
    "color": 3,
    "presenting": "slide-abc123"
}
```

Add a row to the README's "Current fixtures" table describing it: `presenting.json` — a presenter broadcasting the slide they are on (live follow-the-presenter, P2).

Backend, in `crates/collab/src/awareness.rs` beside the other fixture tests:

```rust
    const FIXTURE_PRESENTING: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/presenting.json");

    #[test]
    fn fixture_presenting_preserved() {
        assert_fixture_round_trips(FIXTURE_PRESENTING, "presenting.json");
    }

    #[test]
    fn presenting_field_is_length_capped() {
        let long = "x".repeat(MAX_AWARENESS_FIELD_BYTES + 1);
        let raw = serde_json::json!({
            "user_id": "u", "name": "n", "color": 0, "presenting": long
        })
        .to_string();
        assert!(
            decode_awareness(raw.as_bytes()).is_none(),
            "an over-long presenting id must be rejected like every other string field"
        );
    }
```

Frontend, in `frontend/src/collab/ws_client.rs` beside its fixture tests:

```rust
    const FIXTURE_PRESENTING: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/presenting.json");

    #[test]
    fn fixture_presenting_round_trips() {
        assert_awareness_fixture_round_trips(FIXTURE_PRESENTING, "presenting.json");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p ogrenotes-collab awareness 2>&1 | tail -8`
Expected: FAIL — the round-trip drops the unknown `presenting` key.

- [ ] **Step 3: Implement the backend side**

In `crates/collab/src/awareness.rs`: add to `AwarenessState`, after `typing_thread_id`:

```rust
    /// P2 live follow-the-presenter: the slide `block_id` this user is
    /// currently presenting, when they are in present mode. Absent for
    /// every ordinary editing session. Ephemeral by construction — the
    /// state vanishes with the connection, which is what ends a
    /// presenting session (design/presentations.md, "Live follow").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presenting: Option<String>,
```

Add `presenting` to the string-field list in the `MAX_AWARENESS_FIELD_BYTES` doc comment (:23-33), to the per-field length validation inside `decode_awareness` (mirror the `typing_thread_id` check exactly), and to `empty_state` (`presenting: None`).

- [ ] **Step 4: Implement the frontend side**

In `frontend/src/collab/ws_client.rs`:
- `AwarenessPayload`: add `#[serde(skip_serializing_if = "Option::is_none")] presenting: Option<String>,`.
- `send_awareness`: add a trailing parameter `presenting: Option<&str>` and set `presenting: presenting.map(|s| s.to_string())` in the payload literal.
- `RemoteCursor`: add `pub presenting: Option<String>,`.
- `handle_awareness`: add `presenting: state.presenting.clone(),` to the `RemoteCursor` literal.
- The three exhaustive struct-literal tests (:1510, :1537, :1562) each gain `presenting: None` (in `awareness_payload_optional_fields_omitted`, also assert the key is absent from the serialized JSON, matching how that test treats the other optional fields).

In `frontend/src/pages/document.rs`, the three existing `send_awareness` call sites (:1397, :1406, :1520) each gain a trailing `None` — an editing session is not presenting.

- [ ] **Step 5: Run both sides**

Run: `cargo test -p ogrenotes-collab awareness 2>&1 | grep -E "test result"` — PASS.
Run: `cd frontend && cargo test awareness 2>&1 | grep -E "test result"` — PASS.
Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -1` — clean.

- [ ] **Step 6: Commit**

```bash
git add crates/collab/src/awareness.rs frontend/src/collab/ws_client.rs frontend/src/pages/document.rs tests/fixtures/protocol/awareness/presenting.json tests/fixtures/protocol/awareness/README.md
git commit -m "feat(presentations): add the presenting field to the awareness protocol"
```

---

### Task 6: Slide navigation helpers + present route + page

**Files:**
- Create: `frontend/src/presentation/nav.rs`
- Modify: `frontend/src/presentation/mod.rs` (declare + re-export)
- Create: `frontend/src/pages/present.rs`
- Modify: `frontend/src/pages/mod.rs`, `frontend/src/app.rs`
- Modify: `frontend/src/components/deck_view.rs` (`pub(crate)` on shared renderers + a Present button)
- Modify: `frontend/style/presentation.css`, `frontend/locales/en-US/main.ftl`

**Interfaces:**
- Consumes: `Deck`, `DeckSlide`, `deck_from_doc` (`presentation::model`); `ydoc_bytes_to_doc` (`frontend/src/editor/yrs_bridge.rs:120`); `crate::api::documents::get_content(id) -> Result<Vec<u8>, ApiClientError>` (`frontend/src/api/documents.rs:630`).
- Produces:

```rust
// presentation/nav.rs
pub fn next_index(current: usize, len: usize) -> usize;   // clamps at len-1; 0 when empty
pub fn prev_index(current: usize) -> usize;               // saturating
pub fn index_of_slide(deck: &Deck, block_id: &str) -> Option<usize>;
pub fn slide_block_id(deck: &Deck, index: usize) -> Option<String>;
// components/deck_view.rs
pub(crate) fn render_deck_canvas(slide: &DeckSlide, theme: &str) -> AnyView;
pub(crate) fn render_frame_content(content: &Fragment) -> AnyView;
pub(crate) fn ensure_presentation_css();
```

Task 7 and Task 8 build on all of these.

- [ ] **Step 1: Write the failing nav tests**

Create `frontend/src/presentation/nav.rs` with:

```rust
//! Pure slide-navigation helpers shared by the deck editor and present
//! mode. `active_slide` is a positional index in the UI, but live
//! follow (P2) broadcasts a slide `block_id` — these functions are the
//! only place that mapping lives.

use super::model::Deck;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::model::{Deck, DeckSlide, DEFAULT_THEME};

    fn deck_with(ids: &[&str]) -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: ids
                .iter()
                .map(|id| DeckSlide {
                    block_id: (*id).to_string(),
                    layout: "blank".to_string(),
                    background: None,
                    frames: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn next_clamps_at_the_last_slide() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 2, "no wrap past the end");
        assert_eq!(next_index(0, 0), 0, "empty deck is inert");
        assert_eq!(next_index(9, 3), 2, "out-of-range clamps into the deck");
    }

    #[test]
    fn prev_clamps_at_the_first_slide() {
        assert_eq!(prev_index(2), 1);
        assert_eq!(prev_index(0), 0, "no wrap before the start");
    }

    #[test]
    fn block_id_and_index_round_trip() {
        let d = deck_with(&["s1", "s2", "s3"]);
        assert_eq!(index_of_slide(&d, "s2"), Some(1));
        assert_eq!(index_of_slide(&d, "missing"), None);
        assert_eq!(slide_block_id(&d, 2).as_deref(), Some("s3"));
        assert_eq!(slide_block_id(&d, 9), None);
        // The mapping must survive a reorder — this is exactly why live
        // follow broadcasts ids, not indices.
        let mut reordered = d.clone();
        reordered.slides.swap(0, 2);
        assert_eq!(index_of_slide(&reordered, "s3"), Some(0));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && cargo test presentation::nav 2>&1 | tail -5`
Expected: compile error (module not declared, functions missing).

- [ ] **Step 3: Implement the helpers**

Prepend to `nav.rs`:

```rust
pub fn next_index(current: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (current + 1).min(len - 1)
}

pub fn prev_index(current: usize) -> usize {
    current.saturating_sub(1)
}

pub fn index_of_slide(deck: &Deck, block_id: &str) -> Option<usize> {
    deck.slides.iter().position(|s| s.block_id == block_id)
}

pub fn slide_block_id(deck: &Deck, index: usize) -> Option<String> {
    deck.slides.get(index).map(|s| s.block_id.clone())
}
```

`next_index(9, 3)` returns 2 because `(9+1).min(2)` = 2. In `frontend/src/presentation/mod.rs` add `pub mod nav;` and `pub use nav::{index_of_slide, next_index, prev_index, slide_block_id};`. If `Deck` doesn't already derive `Clone`, add it (the reorder test needs it).

Run: `cd frontend && cargo test presentation::nav 2>&1 | grep -E "test result"` — PASS.

- [ ] **Step 4: Make the shared renderers reachable**

In `frontend/src/components/deck_view.rs`, change `fn render_deck_canvas`, `fn render_frame_content`, and `fn ensure_presentation_css` to `pub(crate) fn` (leave `render_node_readonly` private — it is reached through `render_frame_content`).

- [ ] **Step 5: Write the present page**

Create `frontend/src/pages/present.rs`:

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Full-screen present mode for slide decks (P2).
//!
//! A sidebar-free route (`/d/:id/present`) that renders the *same*
//! `render_deck_canvas` the editor and thumbnails use — the canvas is
//! already a fixed-aspect, container-queried surface, so presenting is
//! a layout change, not a second renderer. Read-only by construction:
//! the deck is fetched once and never written back.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use wasm_bindgen::JsCast;

use crate::components::deck_view::{ensure_presentation_css, render_deck_canvas};
use crate::editor::yrs_bridge::ydoc_bytes_to_doc;
use crate::presentation::model::{deck_from_doc, Deck, DEFAULT_THEME};
use crate::presentation::nav::{next_index, prev_index};
use crate::presentation::themes::theme_class;

#[component]
pub fn PresentPage() -> impl IntoView {
    ensure_presentation_css();
    let params = use_params_map();
    let doc_id = move || params.read().get("id").unwrap_or_default();
    let query = use_query_map();
    let is_presenter_view = move || query.read().get("presenter").is_some();

    let deck = RwSignal::new(Deck {
        theme: DEFAULT_THEME.to_string(),
        slide_size: "16:9".to_string(),
        slides: Vec::new(),
    });
    let (idx, set_idx) = signal(0usize);
    let (loaded, set_loaded) = signal(false);

    // Fetch once. Present mode is a read-only view of the deck as it
    // stands when the presenter opens it; live content sync arrives
    // with follow-the-presenter (next task).
    {
        let id = doc_id();
        leptos::task::spawn_local(async move {
            if let Ok(bytes) = crate::api::documents::get_content(&id).await {
                if let Ok(node) = ydoc_bytes_to_doc(&bytes) {
                    deck.set(deck_from_doc(&node));
                }
            }
            set_loaded.set(true);
        });
    }

    let go_next = move || set_idx.set(next_index(idx.get_untracked(), deck.with_untracked(|d| d.slides.len())));
    let go_prev = move || set_idx.set(prev_index(idx.get_untracked()));

    // Window-level keydown: the overlay owns the whole page, and a
    // container-scoped handler would need focus management the browser
    // fullscreen transition can steal. Same listener style as
    // deck_view.rs's outside-click handler.
    {
        let navigate = use_navigate();
        let id = doc_id();
        let handle = leptos::ev::window_event_listener_untyped("keydown", move |ev: web_sys::Event| {
            let Ok(ke) = ev.clone().dyn_into::<web_sys::KeyboardEvent>() else { return };
            match ke.key().as_str() {
                "ArrowRight" | "ArrowDown" | " " | "PageDown" => { ev.prevent_default(); go_next(); }
                "ArrowLeft" | "ArrowUp" | "PageUp" => { ev.prevent_default(); go_prev(); }
                "Home" => { ev.prevent_default(); set_idx.set(0); }
                "End" => {
                    ev.prevent_default();
                    let len = deck.with_untracked(|d| d.slides.len());
                    set_idx.set(len.saturating_sub(1));
                }
                "Escape" => { navigate(&format!("/d/{id}"), Default::default()); }
                _ => {}
            }
        });
        on_cleanup(move || handle.remove());
    }

    view! {
        <div
            class="deck-present"
            class:deck-present--presenter=is_presenter_view
            on:click=move |_| go_next()
        >
            <Show when=move || loaded.get() && deck.with(|d| !d.slides.is_empty())
                  fallback=|| view! { <div class="deck-present__empty">{crate::t!("deck-present-empty")}</div> }>
                <div class="deck-present__stage">
                    {move || {
                        let i = idx.get().min(deck.with(|d| d.slides.len().saturating_sub(1)));
                        deck.with(|d| d.slides.get(i).map(|s| render_deck_canvas(s, &d.theme)))
                    }}
                </div>
                <div class="deck-present__counter">
                    {move || format!("{} / {}", idx.get() + 1, deck.with(|d| d.slides.len()))}
                </div>
            </Show>
        </div>
    }
}
```

If `window_event_listener_untyped`'s import path differs, copy it from `deck_view.rs`'s existing use. `theme_class` is imported for the presenter-view task; drop the import if the compiler flags it unused here and re-add it in Task 7.

- [ ] **Step 6: Wire the route and the entry point**

`frontend/src/pages/mod.rs`: add `pub mod present;`.

`frontend/src/app.rs`: add **before** the `ParentRoute` (flat = sidebar-free, like `/login`):

```rust
                // Present mode is chrome-free: flat route, no AppShell.
                // Must precede the `d/:id/:slug` child route below or
                // "present" is swallowed as a slug.
                <Route path=path!("/d/:id/present") view=pages::present::PresentPage />
```

In `frontend/src/components/deck_view.rs`, add a Present button next to the existing "Add Text Frame" / theme picker controls (~:2133), navigating with the same `nav_bridge`/`use_navigate` mechanism the file or its neighbors already use:

```rust
                    <button
                        class="deck-present-btn"
                        on:click=move |_| crate::nav_bridge::go(&format!("/d/{}/present", doc_id_for_present))
                    >
                        {crate::t!("deck-present")}
                    </button>
```

(Clone `doc_id` into `doc_id_for_present` before the view block, as the file does for its other captured ids.)

Add to `frontend/locales/en-US/main.ftl`:

```
deck-present = Present
deck-present-empty = This deck has no slides yet.
```

Add to `frontend/style/presentation.css`:

```css
/* ─── Present mode (P2) ─────────────────────────────────────── */

/* Chrome-free stage. Fixed rather than 100vh so mobile browser URL
 * bars can't shrink the slide mid-presentation. */
.deck-present {
  position: fixed;
  inset: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  background: #000;
  cursor: pointer;
}

.deck-present__stage {
  width: min(100vw, calc(100vh * 16 / 9));
  max-width: 100vw;
}

/* The editor caps the canvas at 960px; presenting must fill the
 * stage instead. */
.deck-present__stage .deck-canvas {
  max-width: none;
  width: 100%;
  border-radius: 0;
  box-shadow: none;
}

.deck-present__counter {
  position: fixed;
  right: var(--space-md);
  bottom: var(--space-sm);
  color: rgba(255, 255, 255, 0.6);
  font-size: 0.85rem;
  pointer-events: none;
}

.deck-present__empty {
  color: rgba(255, 255, 255, 0.7);
}
```

- [ ] **Step 7: Verify**

Run: `cd frontend && cargo test 2>&1 | grep -E "test result: ok\. [0-9]{3,}|FAILED"` — pass.
Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -1` — clean.
Run: `cd frontend && trunk build 2>&1 | tail -1` — success.
Then follow `.claude/skills/verify/SKILL.md` to bring up the local stack, open a deck, click **Present**, and confirm: the slide fills the screen, Arrow/Space/PageDown advance, Arrow-Left/PageUp go back, Home/End jump, Escape returns to the editor. **Restart the API after `trunk build`** (CSP inline-script hashes are computed at startup).

- [ ] **Step 8: Commit**

```bash
git add frontend/src/presentation/nav.rs frontend/src/presentation/mod.rs frontend/src/pages/present.rs frontend/src/pages/mod.rs frontend/src/app.rs frontend/src/components/deck_view.rs frontend/style/presentation.css frontend/locales/en-US/main.ftl
git commit -m "feat(presentations): full-screen present mode with keyboard navigation"
```

---

### Task 7: Presenter view — next-slide preview, speaker notes, timer

**Files:**
- Modify: `frontend/src/pages/present.rs`
- Modify: `frontend/style/presentation.css`, `frontend/locales/en-US/main.ftl`

**Interfaces:**
- Consumes: `render_deck_canvas`, `render_frame_content` (`pub(crate)`, Task 6); `FrameRole::Notes`.
- Produces: `?presenter=1` renders current + next + notes + elapsed timer. Task 9 styles it for mobile; Task 10 asserts it.

- [ ] **Step 1: Write the failing timer test**

The only pure logic here is the clock format. Add to `frontend/src/pages/present.rs`:

```rust
/// Elapsed wall-clock as `M:SS` (or `H:MM:SS` past an hour) for the
/// presenter timer.
pub(crate) fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats_as_a_clock() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(9), "0:09");
        assert_eq!(format_elapsed(75), "1:15");
        assert_eq!(format_elapsed(3599), "59:59");
        assert_eq!(format_elapsed(3600), "1:00:00");
        assert_eq!(format_elapsed(3725), "1:02:05");
    }
}
```

Note: `pages/` is binary-only, so this runs under `cargo test` but not `cargo test --lib`. That is acceptable for one formatter; do **not** move the page module.

- [ ] **Step 2: Run to verify it fails, then implement `format_elapsed`, then re-run**

Run: `cd frontend && cargo test present 2>&1 | grep -E "test result|FAILED"` — fail, then PASS after Step 1's implementation compiles.

- [ ] **Step 3: Add the presenter layout**

In `PresentPage`, add the timer signal and start it on mount:

```rust
    let (elapsed, set_elapsed) = signal(0u64);
    if is_presenter_view() {
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                set_elapsed.update(|v| *v += 1);
            }
        });
    }
```

Render the presenter panel beside the stage, inside the `<Show>`, gated on `is_presenter_view`:

```rust
                <Show when=is_presenter_view>
                    <aside class="deck-present__panel">
                        <div class="deck-present__timer">{move || format_elapsed(elapsed.get())}</div>
                        <div class="deck-present__next">
                            <h3>{crate::t!("deck-present-next")}</h3>
                            {move || {
                                let n = idx.get() + 1;
                                deck.with(|d| d.slides.get(n).map(|s| render_deck_canvas(s, &d.theme)))
                            }}
                        </div>
                        <div class="deck-present__notes">
                            <h3>{crate::t!("deck-present-notes")}</h3>
                            {move || {
                                let i = idx.get();
                                deck.with(|d| {
                                    d.slides.get(i).map(|s| {
                                        s.frames
                                            .iter()
                                            .filter(|f| f.role == FrameRole::Notes)
                                            .map(|f| view! {
                                                <div class="deck-present__note">
                                                    {render_frame_content(&f.content)}
                                                </div>
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                })
                            }}
                        </div>
                    </aside>
                </Show>
```

Import `FrameRole` and `render_frame_content`. Clicking the panel must **not** advance the slide — add `on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()` to the `<aside>` (the stage's click-to-advance is on the root).

i18n additions:

```
deck-present-next = Next
deck-present-notes = Speaker notes
```

CSS additions:

```css
/* Presenter view: stage left, panel right. Plain flex row — the stage
 * keeps its own aspect box, so the panel just takes the remainder. */
.deck-present--presenter {
  flex-direction: row;
  align-items: stretch;
  justify-content: flex-start;
  cursor: default;
}

.deck-present--presenter .deck-present__stage {
  flex: 1 1 auto;
  align-self: center;
  width: auto;
  max-width: 65vw;
}

.deck-present__panel {
  flex: 0 0 30vw;
  min-width: 260px;
  overflow-y: auto;
  padding: var(--space-md);
  background: var(--color-surface);
  color: var(--color-text);
  cursor: default;
}

.deck-present__timer {
  font-size: 2rem;
  font-variant-numeric: tabular-nums;
  margin-bottom: var(--space-md);
}

.deck-present__next .deck-canvas {
  max-width: 100%;
  box-shadow: none;
}

.deck-present__note {
  border-inline-start: 3px solid var(--color-border);
  padding-inline-start: var(--space-sm);
  margin-block: var(--space-sm);
}
```

- [ ] **Step 4: Verify**

Run the three commands from Task 6 Step 7, then on the local stack open `/d/<id>/present?presenter=1` and confirm: timer counts, next-slide preview updates as you advance, `role=notes` frames appear in the notes column and **never** on the stage.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/pages/present.rs frontend/style/presentation.css frontend/locales/en-US/main.ftl
git commit -m "feat(presentations): presenter view with next-slide preview, notes, and timer"
```

---

### Task 8: Live follow-the-presenter

**Files:**
- Modify: `frontend/src/pages/present.rs`
- Modify: `frontend/style/presentation.css`, `frontend/locales/en-US/main.ftl`

**Interfaces:**
- Consumes: `RemoteCursor.presenting`, `send_awareness(..., presenting: Option<&str>)` (Task 5); `index_of_slide`, `slide_block_id` (Task 6); `CollabClient` — construct it by mirroring `frontend/src/pages/document.rs:919` verbatim (including its `ws-token` fetch and `set_on_awareness_update` wiring at :982), substituting this page's callbacks.
- Produces: present mode broadcasts the presenter's slide and can follow another presenter's.

- [ ] **Step 1: Write the failing follow-state tests**

The routing decision is pure — extract it. Add to `present.rs`:

```rust
/// Who this viewer can follow: everyone else currently broadcasting a
/// `presenting` slide. Self is excluded (you can't follow yourself),
/// and cursors with no `presenting` value are ordinary editors.
pub(crate) fn presenters<'a>(cursors: &'a [RemoteCursor], me: &str) -> Vec<&'a RemoteCursor> {
    cursors.iter().filter(|c| c.presenting.is_some() && c.user_id != me).collect()
}

/// The slide index a follower should be on, given the presenter's
/// broadcast id. `None` when not following, when the presenter is gone,
/// or when the id names a slide this deck no longer has (a concurrent
/// delete) — in every case the follower simply stays put.
pub(crate) fn followed_index(
    deck: &Deck,
    cursors: &[RemoteCursor],
    following: Option<&str>,
) -> Option<usize> {
    let target = following?;
    let cursor = cursors.iter().find(|c| c.user_id == target)?;
    let block_id = cursor.presenting.as_deref()?;
    index_of_slide(deck, block_id)
}

#[cfg(test)]
mod follow_tests {
    use super::*;
    use crate::collab::ws_client::RemoteCursor;
    use crate::presentation::model::{DeckSlide, DEFAULT_THEME};

    fn cursor(user: &str, presenting: Option<&str>) -> RemoteCursor {
        RemoteCursor {
            user_id: user.to_string(),
            name: format!("{user}-name"),
            color: "#fff".to_string(),
            cursor_block: None,
            selection_anchor_block: None,
            selection_head_block: None,
            typing_thread_id: None,
            presenting: presenting.map(|s| s.to_string()),
        }
    }

    fn deck(ids: &[&str]) -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: ids.iter().map(|id| DeckSlide {
                block_id: (*id).to_string(),
                layout: "blank".to_string(),
                background: None,
                frames: Vec::new(),
            }).collect(),
        }
    }

    #[test]
    fn presenters_excludes_self_and_non_presenters() {
        let cs = vec![cursor("me", Some("s1")), cursor("them", Some("s2")), cursor("editor", None)];
        let p = presenters(&cs, "me");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].user_id, "them");
    }

    #[test]
    fn followed_index_resolves_the_presenters_slide() {
        let d = deck(&["s1", "s2", "s3"]);
        let cs = vec![cursor("them", Some("s3"))];
        assert_eq!(followed_index(&d, &cs, Some("them")), Some(2));
        assert_eq!(followed_index(&d, &cs, None), None, "not following");
        assert_eq!(followed_index(&d, &cs, Some("ghost")), None, "presenter left");
        let cs_gone = vec![cursor("them", Some("deleted-slide"))];
        assert_eq!(followed_index(&d, &cs_gone, Some("them")), None, "unknown slide id");
    }
}
```

- [ ] **Step 2: Run to verify failure, implement, re-run**

Run: `cd frontend && cargo test follow_tests 2>&1 | grep -E "test result|FAILED"` — fail, then PASS.

- [ ] **Step 3: Wire the client**

In `PresentPage`, add signals `remote_cursors: RwSignal<Vec<RemoteCursor>>`, `(following, set_following): Option<String>`, and `(paused, set_paused): bool`. Construct a `CollabClient` exactly as `document.rs:919` does, install `set_on_awareness_update` (mirroring :982) to write `remote_cursors`, and:

- **Broadcast:** an Effect that, whenever `idx` changes, calls `send_awareness(user_id, name, color, None, None, None, None, slide_block_id(&deck, idx).as_deref())`. Awareness is dropped unless the socket is `Synced` (`ws_client.rs:1051`), so also re-send once on the sync transition — mirror however `document.rs` handles its first awareness send.
- **Follow:** an Effect that, when `following.is_some() && !paused`, sets `idx` from `followed_index(...)`.
- **Pause:** every manual navigation path (`go_next`, `go_prev`, Home/End, click) sets `set_paused.set(true)` when `following.is_some()`.
- **Rejoin:** the pill clears `paused`.

- [ ] **Step 4: Render the affordances**

```rust
                <Show when=move || !presenters(&remote_cursors.get(), &my_user_id).is_empty()>
                    <div class="deck-present__follow">
                        <Show
                            when=move || following.get().is_some() && paused.get()
                            fallback=move || view! {
                                <For each=move || presenters(&remote_cursors.get(), &my_user_id)
                                         .into_iter().map(|c| (c.user_id.clone(), c.name.clone())).collect::<Vec<_>>()
                                     key=|(id, _)| id.clone()
                                     children=move |(id, name)| {
                                        let id2 = id.clone();
                                        view! {
                                            <button class="deck-present__follow-btn"
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    set_following.set(Some(id2.clone()));
                                                    set_paused.set(false);
                                                }>
                                                {crate::t!("deck-present-follow", name = name)}
                                            </button>
                                        }
                                     } />
                            }
                        >
                            <button class="deck-present__rejoin"
                                on:click=move |ev: web_sys::MouseEvent| { ev.stop_propagation(); set_paused.set(false); }>
                                {crate::t!("deck-present-rejoin")}
                            </button>
                        </Show>
                    </div>
                </Show>
```

If the `t!` macro doesn't support named arguments, use a plain `format!` against a key that carries no interpolation (check an existing interpolated key with `grep -n "{ \$" frontend/locales/en-US/main.ftl` and mirror whatever the codebase does).

i18n:

```
deck-present-follow = Follow { $name }
deck-present-rejoin = Rejoin presenter
```

CSS:

```css
.deck-present__follow {
  position: fixed;
  left: var(--space-md);
  bottom: var(--space-md);
  display: flex;
  gap: var(--space-sm);
  z-index: 2;
}

.deck-present__follow-btn,
.deck-present__rejoin {
  padding: 0.4em 0.9em;
  border: 1px solid rgba(255, 255, 255, 0.4);
  border-radius: 999px;
  background: rgba(0, 0, 0, 0.55);
  color: #fff;
  cursor: pointer;
}
```

- [ ] **Step 5: Verify with two browser windows on the local stack**

Open the same deck's present route in two windows (log in as the same user is fine — `presenters` filters on `user_id`, so use **two different dev-login emails**). Advance in window A; window B shows "Follow …"; clicking it tracks A's slide. Navigate manually in B → follow pauses and the rejoin pill appears; clicking it resumes tracking. Close A → B's affordance disappears (awareness is ephemeral).

Also run the three verification commands from Task 6 Step 7.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/pages/present.rs frontend/style/presentation.css frontend/locales/en-US/main.ftl
git commit -m "feat(presentations): live follow-the-presenter over awareness"
```

---

### Task 9: Mobile view + present

`frontend/style/responsive.css` has zero deck rules today, and `presentation.css` has no `@media` blocks — the deck is entirely unresponsive.

**Files:**
- Modify: `frontend/style/presentation.css`
- Modify: `frontend/src/pages/present.rs` (touch navigation)

**Interfaces:**
- Consumes: `go_next` / `go_prev` (Task 6).
- Produces: usable present mode ≤640px; the deck **editor** stays desktop-only (design defers mobile editing).

- [ ] **Step 1: Add the responsive rules**

Append to `frontend/style/presentation.css` (media values are literals — `@media` can't read custom properties, matching `responsive.css:1-11`):

```css
/* ─── Mobile (P2: view + present only; editing stays desktop) ── */
@media (max-width: 640px) {
  /* Presenter view can't afford a side panel on a phone: stack it
   * under the stage and let the page scroll. */
  .deck-present--presenter {
    flex-direction: column;
    overflow-y: auto;
  }

  .deck-present--presenter .deck-present__stage {
    max-width: 100vw;
    width: 100%;
  }

  .deck-present__panel {
    flex: 1 1 auto;
    width: 100%;
    min-width: 0;
  }

  /* The editor's three-column grid collapses to the canvas alone —
   * the strip and pane are editing chrome. */
  .deck-view {
    grid-template-columns: 1fr;
    grid-template-areas: "canvas";
  }

  .deck-view__strip,
  .deck-view__pane {
    display: none;
  }
}
```

- [ ] **Step 2: Add touch navigation**

In `present.rs`, add horizontal swipe using the same coarse-pointer reality the codebase already assumes (`spreadsheet_view.rs:948` probes `(hover: none)`); no probe is needed here since touch events simply don't fire on a mouse:

```rust
    // Swipe: left → next, right → previous. 48px threshold, and the
    // gesture must be predominantly horizontal so a vertical scroll in
    // the presenter panel doesn't change slides.
    let (touch_start, set_touch_start) = signal::<Option<(f64, f64)>>(None);
```

with `on:touchstart` recording `(client_x, client_y)` of the first changed touch, and `on:touchend` computing `dx`/`dy` and calling `go_next()` / `go_prev()` when `dx.abs() > 48.0 && dx.abs() > dy.abs()`. Read the coordinates via `web_sys::TouchEvent::changed_touches().get(0)`.

- [ ] **Step 3: Verify**

Run the three commands from Task 6 Step 7. Then in a headed browser's device-emulation mode (or via the doctor's `devices["iPhone 13"]` context per the verify skill), confirm at 375×667: the stage fills the width, the presenter panel stacks below, and swipes change slides.

- [ ] **Step 4: Commit**

```bash
git add frontend/style/presentation.css frontend/src/pages/present.rs
git commit -m "feat(presentations): responsive present mode with swipe navigation"
```

---

### Task 10: `deck-present` doctor scenario + workflow step + full verification

**Files:**
- Modify: `scripts/frontend-doctor/doctor.js` (new scenario + dispatcher arm + usage string + `requiredSteps` entry — mirror `deck-blocks`, which is the closest sibling)
- Modify: `.github/workflows/playwright.yml` (step after `deck-blocks`)

**Interfaces:**
- Consumes: everything above.
- Produces: a committed regression net for present mode, wired into the nightly sweep.

- [ ] **Step 1: Read the sibling scenario first**

Read `scenarioDeckBlocks` in `scripts/frontend-doctor/doctor.js` end to end (dev-login → `chromium.launch` → `newContext({...DOCTOR_CONTEXT_DEFAULTS, recordHar})` → `seedAuth` → `instrument` → local `waitFor` → `steps` → try/catch → screenshots → `collector.scenario` → close). Mirror that shape exactly.

- [ ] **Step 2: Write `scenarioDeckPresent`**

Seed via `createDocViaApi(target, tokens.accessToken, "Present probe", "presentation")`, then drive the UI: open `/d/<id>`, add a `title-content` slide, type identifiable text into the body frame (`Ctrl+A` → `Delete` → type — typing over a select-all hits pre-existing issue #195), Escape, then click **Present**. Assert these steps:

- `presentRouteReached` — `page.url()` matches `/\/d\/[^/?#]+\/present$/`
- `stageRendersSlide` — `.deck-present__stage .deck-canvas` exists **and** its `boundingBox()` width exceeds 600 (the P1 launch bug was a 0×0 canvas; assert size, never mere presence)
- `slideTextVisible` — the typed text is inside `.deck-present__stage`
- `arrowAdvances` — press `ArrowRight`; `.deck-present__counter` text changes from `1 / N` to `2 / N`
- `arrowGoesBack` — press `ArrowLeft`; counter returns to `1 / N`
- `escapeReturnsToEditor` — press `Escape`; URL matches `/\/d\/[^/?#]+$/` and `.deck-view` is visible
- `presenterViewPanels` — `page.goto(url + "/present?presenter=1")`; `.deck-present__timer`, `.deck-present__next`, and `.deck-present__notes` all exist
- `timerAdvances` — read `.deck-present__timer`, `waitForTimeout(2200)`, read again, assert the strings differ
- `pdfExportDownloads` — `fetch(`${target}/api/v1/documents/${doc.id}/export/pdf`, { headers: { authorization: `Bearer ${tokens.accessToken}` } })`; assert `res.ok`, `content-type` is `application/pdf`, and the body starts with `%PDF-`
- `noConsoleErrors` — same filter as `deck-blocks` (which excuses `Failed to fetch`, aborted loads, and the handled 409 retry noise)

Register the scenario: dispatcher arm (`} else if (scenario === "deck-present") { await scenarioDeckPresent(ctx, collector); }`), the usage string, and a `requiredSteps["deck-present"]` array listing all ten step keys.

- [ ] **Step 3: Validate and run it**

Run: `node --check scripts/frontend-doctor/doctor.js` — OK.
Bring up the local stack per `.claude/skills/verify/SKILL.md` (restart the API after `trunk build`), then:
Run: `cd scripts/frontend-doctor && node doctor.js --scenario deck-present --base-url http://127.0.0.1:3100 --out /tmp/deck-present-out 2>&1 | grep -o '"ok":[a-z]*\|"steps":{[^}]*}'`
Expected: `"ok":true` with every step `true`. Run it **twice** — a scenario that passes once may be timing-dependent; both runs must be green before committing.

- [ ] **Step 4: Add the workflow step**

In `.github/workflows/playwright.yml`, after the `deck-blocks` step:

```yaml
      # Present mode (P2): full-screen stage at real size, keyboard
      # navigation, Escape back to the editor, presenter view panels +
      # timer, and PDF slide export over the API.
      - name: Run deck-present scenario
        id: deck-present
        if: always() && (steps.trash-flow.outcome == 'success' || steps.trash-flow.outcome == 'failure')
        working-directory: scripts/frontend-doctor
        run: |
          node doctor.js \
            --scenario deck-present \
            --base-url http://127.0.0.1:3000 \
            --out ../../artifacts/doctor/deck-present
```

- [ ] **Step 5: Full verification sweep**

Run: `cargo test --workspace 2>&1 | tail -5` — PASS (investigate any failure; do not weaken a test. `test_activity_feed_records_share` and the SCIM rate-limit test have both been observed as parallel-suite flakes — re-run the single test in isolation before concluding).
Run: `cargo test -p ogrenotes-collab --features pdf 2>&1 | grep "test result"` — PASS.
Run: `cd frontend && cargo test 2>&1 | grep -E "test result: ok\. [0-9]{3,}"` and `cargo test --lib 2>&1 | grep -cE "presentation::"` (non-zero, proving Task 1 holds).
Run: `cd frontend && cargo check --target wasm32-unknown-unknown 2>&1 | tail -1` — clean.
Run: `cd frontend && trunk build 2>&1 | tail -3` — success.
Run: `cd scripts/frontend-doctor && node doctor.js --scenario deck-basics …` and `--scenario deck-blocks …` — both still `"ok":true` (present mode changed shared renderers).

- [ ] **Step 6: Commit**

```bash
git add scripts/frontend-doctor/doctor.js .github/workflows/playwright.yml
git commit -m "test(presentations): deck-present doctor scenario in the nightly sweep"
```

---

## Post-plan notes for the reviewer

- **Deliberately out of scope** (design/presentations.md defers these to P3 or later): the feedback-prompt block, engagement analytics, polls, `.pptx` either direction, slide transitions/build animations, spatial comments, corporate/custom themes, and mobile deck *editing*.
- **Design drift to report, not fix:** the design doc names the present route `/doc/{id}/present`; the real route is `/d/:id/present`. Also, the design's "backend `themes.rs` … mirrored to the frontend with a duality test" lands here as backend-owns-colors + a source-text duality test (the frontend keeps colors in CSS, which is the only place the browser can use them).
- **Known limitation carried in from P1/frame-blocks:** slash-menu keyboard nav doesn't reach frame editors; present mode is unaffected (no menus).
- **Accepted risk:** `frontend/src/pages/` is binary-only, so `format_elapsed`, `presenters`, and `followed_index` run under `cargo test` but not CI's `cargo test --lib`. The doctor scenario is their CI-visible net. Moving `pages/` into the lib is a much larger change and is not attempted here.
