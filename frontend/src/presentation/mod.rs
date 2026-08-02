// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Presentation deck feature — client-side working model + (later)
//! canvas editor. See `design/presentations.md`.

pub mod geometry;
pub mod liveness;
pub mod model;
pub mod nav;
pub mod presets;
pub mod themes;

pub use geometry::{
    apply_drag, next_frame_id, nudge, previous_frame_id, snap, snap_resize, Axis, Corner, DragKind, Guide,
};
pub use liveness::{should_keep_warm, should_trigger_reconnect};
pub use model::{
    Deck, DeckFrame, DeckSlide, FrameRole, Rect, DEFAULT_THEME, MIN_FRAME_DIM, deck_from_doc,
    deck_to_doc,
};
pub use nav::{index_of_slide, next_index, prev_index, slide_block_id};
pub use presets::{instantiate, LayoutPreset, PresetFrame, LAYOUT_PRESETS};
pub use themes::{theme_class, DeckTheme, DECK_THEMES};
