// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Presentation deck feature — client-side working model + (later)
//! canvas editor. See `design/presentations.md`.

pub mod model;
pub mod presets;
pub mod themes;

pub use model::{
    Deck, DeckFrame, DeckSlide, FrameRole, Rect, DEFAULT_THEME, MIN_FRAME_DIM, deck_from_doc,
    deck_to_doc,
};
pub use presets::{instantiate, LayoutPreset, PresetFrame, LAYOUT_PRESETS};
pub use themes::{theme_class, DeckTheme, DECK_THEMES};
