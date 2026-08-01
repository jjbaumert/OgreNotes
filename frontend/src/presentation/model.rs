// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Presentation deck working model — a client-side `Deck` tree that
//! mirrors the persisted `Doc -> Slide -> Frame -> blocks` shape
//! (see `design/presentations.md`) but is easier for the canvas
//! editor (Task 9-12) to work with directly than walking `Node`.
//!
//! `deck_from_doc` / `deck_to_doc` convert between the two shapes.
//! The server (`crates/collab/src/blocks/presentation.rs`) validates
//! attrs on write and REJECTS out-of-range geometry rather than
//! clamping; `Rect::clamped` here is the defensive reader-side
//! complement, so `deck_to_doc` must only ever emit values that
//! already pass server validation.
//!
//! Geometry formatting: `x`/`y`/`w`/`h` are always serialized with
//! `format!("{:.4}", v)` (fixed 4 decimal places, never trimmed).
//! This is the one canonical representation: since `Rect::clamped`
//! output is what gets formatted, and re-parsing a 4-decimal string
//! and reformatting it to 4 decimals reproduces the same digits,
//! `deck_from_doc` -> `deck_to_doc` is idempotent at the string
//! level — a value read from attrs and written back unchanged
//! produces the identical attr string, so yrs sync never churns on
//! an unedited frame.

use std::collections::HashMap;

use crate::editor::model::{Fragment, Node, NodeType};

/// Built-in theme id used when a Doc has no `theme` attr (or it's a
/// non-presentation doc being defensively read as one).
pub const DEFAULT_THEME: &str = "slate";

/// `slideSize` used when a Doc has no `slideSize` attr.
const DEFAULT_SLIDE_SIZE: &str = "16:9";

/// Layout preset used when a Slide has no `layout` attr, or has one
/// that isn't in `LAYOUTS`.
const DEFAULT_LAYOUT: &str = "blank";

/// Valid `layout` preset ids. Mirrors `LAYOUTS` in
/// `crates/collab/src/blocks/presentation.rs` (`validate_slide_attrs`)
/// — the server rejects any other value, so a reader that lets an
/// unrecognized layout through would produce a `Deck` that
/// `deck_to_doc` can't legally persist.
const LAYOUTS: &[&str] = &["title", "title-content", "two-column", "blank"];

/// Max `background` length in chars. Mirrors `BACKGROUND_MAX_LEN` in
/// `crates/collab/src/blocks/presentation.rs`.
const BACKGROUND_MAX_LEN: usize = 200;

/// Minimum frame width/height (normalized 0..1). A frame can never
/// clamp down to a zero-area rect — that would make it unselectable
/// and unresizable on the canvas.
pub const MIN_FRAME_DIM: f64 = 0.02;

/// A frame's geometry, normalized to 0..1 of the slide. Always
/// clamped: `x`/`y` in `0.0..=1.0`, `w`/`h` in `MIN_FRAME_DIM..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// Clamp raw (possibly garbage — NaN, infinite, out-of-range)
    /// numbers into a valid unit-square rect. Never panics, never
    /// produces a zero-or-negative-area rect.
    pub fn clamped(x: f64, y: f64, w: f64, h: f64) -> Rect {
        let clamp_pos = |v: f64| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        let clamp_dim = |v: f64| if v.is_finite() { v.clamp(MIN_FRAME_DIM, 1.0) } else { MIN_FRAME_DIM };
        Rect {
            x: clamp_pos(x),
            y: clamp_pos(y),
            w: clamp_dim(w),
            h: clamp_dim(h),
        }
    }
}

/// Whether a frame holds on-canvas content or speaker notes (rendered
/// only in presenter view, never on the canvas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRole {
    Content,
    Notes,
}

/// A positioned frame on a slide, holding ordinary block content.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckFrame {
    pub block_id: String,
    pub rect: Rect,
    pub z: i64,
    pub role: FrameRole,
    pub content: Fragment,
}

/// One slide in a deck.
#[derive(Debug, Clone, PartialEq)]
pub struct DeckSlide {
    pub block_id: String,
    pub layout: String,
    pub background: Option<String>,
    pub frames: Vec<DeckFrame>,
}

/// A presentation deck: the client-side working model mirroring the
/// persisted `Doc -> Slide -> Frame` tree (`design/presentations.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct Deck {
    pub theme: String,
    pub slide_size: String,
    pub slides: Vec<DeckSlide>,
}

/// Format a clamped geometry value as the canonical 4-decimal attr
/// string. See the module doc comment for the idempotency argument.
fn fmt_geom(v: f64) -> String {
    format!("{:.4}", v)
}

fn parse_geom(attrs: &HashMap<String, String>, key: &str, default: f64) -> f64 {
    attrs.get(key).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default)
}

/// Build a `Deck` from a `Doc` node, walking `Slide` children and
/// each slide's `Frame` children. Non-`Slide` children of the doc,
/// and non-`Frame` children of a slide, are skipped defensively
/// (they should never occur on a `docType == "presentation"` doc,
/// but this reader must never panic on a malformed tree).
pub fn deck_from_doc(doc: &Node) -> Deck {
    let Node::Element { node_type: NodeType::Doc, attrs, content, .. } = doc else {
        return Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: DEFAULT_SLIDE_SIZE.to_string(),
            slides: vec![],
        };
    };

    let theme = attrs.get("theme").cloned().unwrap_or_else(|| DEFAULT_THEME.to_string());
    let slide_size = attrs.get("slideSize").cloned().unwrap_or_else(|| DEFAULT_SLIDE_SIZE.to_string());

    let slides = content
        .children
        .iter()
        .filter_map(|child| {
            let Node::Element { node_type: NodeType::Slide, attrs, content, .. } = child else {
                return None;
            };
            Some(slide_from_node(attrs, content))
        })
        .collect();

    Deck { theme, slide_size, slides }
}

fn slide_from_node(attrs: &HashMap<String, String>, content: &Fragment) -> DeckSlide {
    let block_id = attrs.get("blockId").cloned().unwrap_or_default();
    // Unrecognized layout ids (missing, or not in the server's allowlist)
    // read as the default — never pass through verbatim, or deck_to_doc
    // would re-emit a value the server's validate_slide_attrs rejects.
    let layout = match attrs.get("layout") {
        Some(v) if LAYOUTS.contains(&v.as_str()) => v.clone(),
        _ => DEFAULT_LAYOUT.to_string(),
    };
    // An over-length background is garbage, not a style choice — drop it
    // rather than truncate. Truncating would silently mutate the value,
    // which defeats "readers clamp/reject, they never rewrite"; dropping
    // instead falls back to the same `None` that a missing attr produces.
    let background = attrs
        .get("background")
        .cloned()
        .filter(|v| v.len() <= BACKGROUND_MAX_LEN); // byte len, matches the server's `v.len()` check

    let frames = content
        .children
        .iter()
        .filter_map(|child| {
            let Node::Element { node_type: NodeType::Frame, attrs, content, .. } = child else {
                return None;
            };
            Some(frame_from_node(attrs, content))
        })
        .collect();

    DeckSlide { block_id, layout, background, frames }
}

fn frame_from_node(attrs: &HashMap<String, String>, content: &Fragment) -> DeckFrame {
    let block_id = attrs.get("blockId").cloned().unwrap_or_default();
    let rect = Rect::clamped(
        parse_geom(attrs, "x", 0.0),
        parse_geom(attrs, "y", 0.0),
        parse_geom(attrs, "w", 1.0),
        parse_geom(attrs, "h", 1.0),
    );
    let z = attrs.get("z").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    let role = match attrs.get("role").map(String::as_str) {
        Some("notes") => FrameRole::Notes,
        _ => FrameRole::Content,
    };

    DeckFrame { block_id, rect, z, role, content: content.clone() }
}

/// Build a `Doc` node from a `Deck`. Every slide/frame's `block_id`
/// is written back verbatim into `attrs["blockId"]` — never
/// regenerated — so `yrs`'s `find_match` (which aligns nodes by
/// blockId) sees no spurious inserts/deletes on an unedited deck.
/// Attrs that equal their reader-side default are omitted (`role`
/// only written for `Notes`, `z` only written when non-zero,
/// `background` only written when `Some`) to keep attr diffs
/// minimal.
pub fn deck_to_doc(deck: &Deck) -> Node {
    let mut doc_attrs = HashMap::new();
    doc_attrs.insert("theme".to_string(), deck.theme.clone());
    doc_attrs.insert("slideSize".to_string(), deck.slide_size.clone());

    let slides = deck.slides.iter().map(slide_to_node).collect();
    Node::element_with_attrs(NodeType::Doc, doc_attrs, Fragment::from(slides))
}

fn slide_to_node(slide: &DeckSlide) -> Node {
    let mut attrs = HashMap::new();
    attrs.insert("blockId".to_string(), slide.block_id.clone());
    attrs.insert("layout".to_string(), slide.layout.clone());
    if let Some(bg) = &slide.background {
        attrs.insert("background".to_string(), bg.clone());
    }

    let frames = slide.frames.iter().map(frame_to_node).collect();
    Node::element_with_attrs(NodeType::Slide, attrs, Fragment::from(frames))
}

fn frame_to_node(frame: &DeckFrame) -> Node {
    let mut attrs = HashMap::new();
    attrs.insert("blockId".to_string(), frame.block_id.clone());
    attrs.insert("x".to_string(), fmt_geom(frame.rect.x));
    attrs.insert("y".to_string(), fmt_geom(frame.rect.y));
    attrs.insert("w".to_string(), fmt_geom(frame.rect.w));
    attrs.insert("h".to_string(), fmt_geom(frame.rect.h));
    if frame.z != 0 {
        attrs.insert("z".to_string(), frame.z.to_string());
    }
    if frame.role == FrameRole::Notes {
        attrs.insert("role".to_string(), "notes".to_string());
    }

    Node::element_with_attrs(NodeType::Frame, attrs, frame.content.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{generate_block_id, Fragment, Node};

    fn fixture_deck() -> Deck {
        Deck {
            theme: "midnight".to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![
                DeckSlide {
                    block_id: generate_block_id(),
                    layout: "title".to_string(),
                    background: Some("accent-1".to_string()),
                    frames: vec![
                        DeckFrame {
                            block_id: generate_block_id(),
                            rect: Rect::clamped(0.1, 0.2, 0.5, 0.3),
                            z: 0,
                            role: FrameRole::Content,
                            content: Fragment::empty(),
                        },
                        DeckFrame {
                            block_id: generate_block_id(),
                            rect: Rect::clamped(0.0, 0.6, 1.0, 0.35),
                            z: 2,
                            role: FrameRole::Notes,
                            content: Fragment::empty(),
                        },
                    ],
                },
                DeckSlide {
                    block_id: generate_block_id(),
                    layout: "blank".to_string(),
                    background: None,
                    frames: vec![DeckFrame {
                        block_id: generate_block_id(),
                        rect: Rect::clamped(0.0, 0.0, 1.0, 1.0),
                        z: 0,
                        role: FrameRole::Content,
                        content: Fragment::empty(),
                    }],
                },
            ],
        }
    }

    /// A hand-built `Doc` node with a `Frame` missing its geometry
    /// attrs entirely, plus garbage (unparseable / NaN / out-of-range)
    /// values elsewhere, to exercise `deck_from_doc`'s defaulting and
    /// clamping without going through `deck_to_doc` first.
    fn fixture_doc_with_missing_and_garbage_attrs() -> Node {
        use std::collections::HashMap;

        let frame_no_geometry = Node::element_with_attrs(
            crate::editor::model::NodeType::Frame,
            HashMap::from([("blockId".to_string(), "frameNoGeom".to_string())]),
            Fragment::empty(),
        );
        let frame_garbage = Node::element_with_attrs(
            crate::editor::model::NodeType::Frame,
            HashMap::from([
                ("blockId".to_string(), "frameGarbage".to_string()),
                ("x".to_string(), "not-a-number".to_string()),
                ("y".to_string(), "NaN".to_string()),
                ("w".to_string(), "99".to_string()),
                ("h".to_string(), "-5".to_string()),
                ("z".to_string(), "not-an-int".to_string()),
                ("role".to_string(), "bogus-role".to_string()),
            ]),
            Fragment::empty(),
        );
        let slide = Node::element_with_attrs(
            crate::editor::model::NodeType::Slide,
            HashMap::from([
                ("blockId".to_string(), "slide1".to_string()),
                ("layout".to_string(), "pyramid".to_string()), // not in LAYOUTS
                ("background".to_string(), "x".repeat(300)), // over BACKGROUND_MAX_LEN
            ]),
            Fragment::from(vec![frame_no_geometry, frame_garbage]),
        );
        Node::element_with_attrs(
            crate::editor::model::NodeType::Doc,
            HashMap::new(),
            Fragment::from(vec![slide]),
        )
    }

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
        // a Doc with no theme reads as DEFAULT_THEME; garbage numbers clamp;
        // a Slide with an unrecognized layout reads as "blank"; a Slide with
        // an over-length background reads as None (dropped, not truncated).
        let doc = fixture_doc_with_missing_and_garbage_attrs();
        let deck = deck_from_doc(&doc);
        assert_eq!(deck.theme, DEFAULT_THEME);
        assert_eq!(deck.slides[0].frames[0].rect, Rect::clamped(0.0, 0.0, 1.0, 1.0));
        assert_eq!(deck.slides[0].layout, "blank");
        assert_eq!(deck.slides[0].background, None);

        // The second frame's every attr is garbage in a different way
        // (unparseable x, NaN y, out-of-range w/h, non-integer z, unknown
        // role) — assert the fully-clamped/defaulted result, not just that
        // it doesn't panic.
        let garbage_frame = &deck.slides[0].frames[1];
        assert_eq!(garbage_frame.rect, Rect { x: 0.0, y: 0.0, w: 1.0, h: MIN_FRAME_DIM });
        assert_eq!(garbage_frame.z, 0);
        assert_eq!(garbage_frame.role, FrameRole::Content);
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
}
