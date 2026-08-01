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
//! content is a single `Heading` or `Paragraph` node carrying the
//! *resolved* i18n placeholder text.
//!
//! Baking a resolved string in at instantiate time (rather than
//! storing the key as an attr for a renderer to resolve later)
//! follows the precedent set by
//! `frontend/src/editor/blocks/kanban.rs`'s `build_default_node`,
//! which seeds its three default columns with literal English
//! titles ("To Do", "In Progress", "Done") baked directly into the
//! `KanbanColumn` node's `title` attr — never re-resolved at render
//! time. `calendar.rs`'s `build_default_node` has no analogous
//! placeholder-text case (Calendar seeds only machine attrs, no
//! prose), so Kanban is the closer precedent for "insert-time
//! content baking". The one difference here is the source of the
//! literal text: Kanban hardcodes English, we resolve it through
//! `crate::i18n::translate` so a non-English locale (once
//! translated) seeds localized placeholder text instead.

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
    /// i18n key for this frame's seed placeholder text.
    pub placeholder_key: &'static str,
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
    PresetFrame { rect: (0.1, 0.35, 0.8, 0.2), placeholder_key: "deck-placeholder-title", heading: true },
    PresetFrame { rect: (0.1, 0.58, 0.8, 0.1), placeholder_key: "deck-placeholder-subtitle", heading: false },
];

const TITLE_CONTENT_FRAMES: &[PresetFrame] = &[
    PresetFrame { rect: (0.06, 0.06, 0.88, 0.12), placeholder_key: "deck-placeholder-heading", heading: true },
    PresetFrame { rect: (0.06, 0.22, 0.88, 0.7), placeholder_key: "deck-placeholder-body", heading: false },
];

const TWO_COLUMN_FRAMES: &[PresetFrame] = &[
    PresetFrame { rect: (0.06, 0.06, 0.88, 0.12), placeholder_key: "deck-placeholder-heading", heading: true },
    PresetFrame { rect: (0.06, 0.22, 0.42, 0.7), placeholder_key: "deck-placeholder-column", heading: false },
    PresetFrame { rect: (0.52, 0.22, 0.42, 0.7), placeholder_key: "deck-placeholder-column", heading: false },
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
/// single `Heading` or `Paragraph` node wrapping the resolved i18n
/// placeholder text.
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
    let text = crate::i18n::translate(pf.placeholder_key, None);
    // `NodeType::Heading`'s `default_attrs()` already seeds `level = "1"`
    // (see `NodeType::default_attrs` in `editor/model.rs`), so no extra
    // attr wiring is needed here.
    let node_type = if pf.heading { NodeType::Heading } else { NodeType::Paragraph };
    let node = Node::element_with_content(node_type, Fragment::from(vec![Node::text(&text)]));
    Fragment::from(vec![node])
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
