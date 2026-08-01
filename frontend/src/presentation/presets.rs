// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Layout presets for the slide-deck canvas editor (Task 9-12).
//!
//! A [`LayoutPreset`] is a static geometry template — `LAYOUT_PRESETS`
//! ids mirror the `layout` allowlist in
//! `crates/collab/src/blocks/presentation.rs` and
//! `frontend/src/presentation/model.rs`'s `LAYOUTS` const (kept in
//! sync by hand; a mismatch would let `instantiate` build a slide
//! whose `layout` the server's `validate_slide_attrs` rejects).
//! [`instantiate`] turns a preset into a fresh [`DeckSlide`]: every
//! slide/frame gets a brand-new `blockId` via
//! `crate::editor::model::generate_block_id`, and each frame's seed
//! content is a single EMPTY `Heading` or `Paragraph` node.
//!
//! Placeholder text ("Click to add heading" / "Click to add text")
//! is deliberately NOT baked into the document: an earlier version
//! did (following `kanban.rs::build_default_node`'s literal-title
//! precedent), which made entering a fresh frame mean *editing the
//! placeholder prose* — the user had to delete it before typing.
//! The hint is a render-time affordance instead: `DeckView`'s
//! `render_frame_content` overlays a dimmed `deck-frame-placeholder`
//! on visually-empty frames, resolved via `crate::i18n::translate`
//! at render, so it also localizes without touching stored docs.

use crate::editor::model::{generate_block_id, Fragment, Node, NodeType};
use crate::presentation::model::{DeckFrame, DeckSlide, FrameRole, Rect};

/// One frame slot within a [`LayoutPreset`].
pub struct PresetFrame {
    /// Normalized `(x, y, w, h)`, fed through `Rect::clamped` at
    /// instantiation. All preset geometries are already in-range —
    /// clamping is defensive, not corrective — but going through
    /// the same clamp path as every other `Rect` construction site
    /// keeps this module from being a second source of truth for
    /// what "in range" means.
    pub rect: (f64, f64, f64, f64),
    /// `true` -> seed a `Heading` (level 1); `false` -> seed a
    /// `Paragraph`.
    pub heading: bool,
}

/// A named layout template: an id (persisted as `Slide.layout`), an
/// i18n label for the layout picker UI, and the frames a fresh
/// slide of this layout starts with.
pub struct LayoutPreset {
    pub id: &'static str,
    pub label_key: &'static str,
    pub frames: &'static [PresetFrame],
}

const TITLE_FRAMES: &[PresetFrame] = &[
    PresetFrame { rect: (0.1, 0.35, 0.8, 0.2), heading: true },
    PresetFrame { rect: (0.1, 0.58, 0.8, 0.1), heading: false },
];

const TITLE_CONTENT_FRAMES: &[PresetFrame] = &[
    PresetFrame { rect: (0.06, 0.06, 0.88, 0.12), heading: true },
    PresetFrame { rect: (0.06, 0.22, 0.88, 0.7), heading: false },
];

const TWO_COLUMN_FRAMES: &[PresetFrame] = &[
    PresetFrame { rect: (0.06, 0.06, 0.88, 0.12), heading: true },
    PresetFrame { rect: (0.06, 0.22, 0.42, 0.7), heading: false },
    PresetFrame { rect: (0.52, 0.22, 0.42, 0.7), heading: false },
];

const BLANK_FRAMES: &[PresetFrame] = &[];

/// Layout presets, ids matching `model::LAYOUTS` exactly: `title`,
/// `title-content`, `two-column`, `blank`.
pub const LAYOUT_PRESETS: &[LayoutPreset] = &[
    LayoutPreset { id: "title", label_key: "deck-layout-title-label", frames: TITLE_FRAMES },
    LayoutPreset {
        id: "title-content",
        label_key: "deck-layout-title-content-label",
        frames: TITLE_CONTENT_FRAMES,
    },
    LayoutPreset { id: "two-column", label_key: "deck-layout-two-column-label", frames: TWO_COLUMN_FRAMES },
    LayoutPreset { id: "blank", label_key: "deck-layout-blank-label", frames: BLANK_FRAMES },
];

/// Build a fresh [`DeckSlide`] from a preset: a new slide `blockId`,
/// and one [`DeckFrame`] per `PresetFrame` with a new frame
/// `blockId`, the preset's (clamped) rect, and seed content — a
/// single empty `Heading` or `Paragraph` node (the placeholder hint
/// is render-time, see the module docs).
pub fn instantiate(preset: &LayoutPreset) -> DeckSlide {
    let frames = preset
        .frames
        .iter()
        .map(|pf| {
            let (x, y, w, h) = pf.rect;
            DeckFrame {
                block_id: generate_block_id(),
                rect: Rect::clamped(x, y, w, h),
                z: 0,
                role: FrameRole::Content,
                content: seed_content(pf),
            }
        })
        .collect();

    DeckSlide {
        block_id: generate_block_id(),
        layout: preset.id.to_string(),
        background: None,
        frames,
    }
}

fn seed_content(pf: &PresetFrame) -> Fragment {
    // Seed an EMPTY node, not placeholder prose. The old behavior baked
    // the resolved "Click to add heading" string in as real document
    // text, so entering the frame meant *editing the placeholder* —
    // users had to delete it before typing. The hint is now a
    // render-time affordance: `DeckView` overlays a dimmed
    // `deck-frame-placeholder` on visually-empty frames (keyed off the
    // first child's node type) and the editor opens with a bare caret.
    // `NodeType::Heading`'s `default_attrs()` already seeds `level = "1"`
    // (see `NodeType::default_attrs` in `editor/model.rs`), so no extra
    // attr wiring is needed here.
    let node_type = if pf.heading { NodeType::Heading } else { NodeType::Paragraph };
    Fragment::from(vec![Node::element(node_type)])
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(instantiate(LAYOUT_PRESETS.iter().find(|p| p.id == "blank").unwrap())
            .frames
            .is_empty());
    }

    /// Two calls to `instantiate` on the same preset must never
    /// share a blockId — every slide/frame identity is fresh so
    /// yrs's `find_match` never conflates two independently
    /// inserted slides.
    /// Frames must seed EMPTY nodes — baked placeholder prose forced
    /// users to delete "Click to add heading" before typing (the
    /// launch-feedback bug this replaced).
    #[test]
    fn instantiate_seeds_empty_nodes_not_placeholder_prose() {
        let preset = LAYOUT_PRESETS.iter().find(|p| p.id == "title-content").unwrap();
        let s = instantiate(preset);
        let heading = &s.frames[0].content.children[0];
        assert_eq!(heading.node_type(), Some(NodeType::Heading));
        let body = &s.frames[1].content.children[0];
        assert_eq!(body.node_type(), Some(NodeType::Paragraph));
        for f in &s.frames {
            assert_eq!(f.content.children.len(), 1);
            assert_eq!(
                f.content.children[0].text_content(),
                "",
                "seed content must carry no text"
            );
        }
    }

    /// The empty seed nodes must survive the persist path: normalize +
    /// ydoc round-trip must keep the empty Heading (else the frame's
    /// heading-ness — and the render-time hint keyed off it — is lost
    /// after reload).
    #[test]
    fn empty_seed_frames_survive_normalize_and_ydoc_roundtrip() {
        use crate::editor::model::normalize_doc;
        use crate::editor::yrs_bridge::{doc_to_ydoc_bytes, ydoc_bytes_to_doc};
        use crate::presentation::model::{deck_from_doc, deck_to_doc, Deck, DEFAULT_THEME};

        let preset = LAYOUT_PRESETS.iter().find(|p| p.id == "title-content").unwrap();
        let deck = Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![instantiate(preset)],
        };
        let doc = normalize_doc(&deck_to_doc(&deck));
        let back = ydoc_bytes_to_doc(&doc_to_ydoc_bytes(&doc)).unwrap();
        let deck2 = deck_from_doc(&back);
        assert_eq!(deck2.slides[0].frames.len(), 2, "both frames survive");
        assert_eq!(
            deck2.slides[0].frames[0].content.children[0].node_type(),
            Some(NodeType::Heading),
            "empty heading survives the round-trip"
        );
    }

    #[test]
    fn instantiate_generates_fresh_block_ids_each_call() {
        let preset = LAYOUT_PRESETS.iter().find(|p| p.id == "title-content").unwrap();
        let a = instantiate(preset);
        let b = instantiate(preset);
        assert_ne!(a.block_id, b.block_id);
        for (fa, fb) in a.frames.iter().zip(b.frames.iter()) {
            assert_ne!(fa.block_id, fb.block_id);
        }
    }
}
