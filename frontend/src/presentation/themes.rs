// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Built-in deck themes. Each theme is a named color palette
//! (`--deck-bg` / `--deck-heading-color` / `--deck-text-color` /
//! `--deck-accent`) defined as a `.deck-theme-<id>` class in
//! `style/presentation.css`; this module is only the id/label
//! registry + the id-to-class-name mapping the canvas view uses to
//! pick which class to apply to `.deck-canvas`.
//!
//! `Deck.theme` (see `presentation::model`) is a free-form `String`
//! read from the persisted `Doc.theme` attr, defaulting to
//! [`crate::presentation::model::DEFAULT_THEME`] ("slate") when
//! absent. It can drift from `DECK_THEMES` — an old deck could name
//! a theme that's since been retired, or (defensively) hold
//! attacker/garbage input from a malformed attr — so `theme_class`
//! must fall back rather than emit an unstyled/garbage class name.

use crate::presentation::model::DEFAULT_THEME;

/// One built-in deck theme: an id (persisted as `Doc.theme`) and an
/// i18n label for the theme picker UI.
pub struct DeckTheme {
    pub id: &'static str,
    pub label_key: &'static str,
}

/// The six built-in deck themes. Ids double as the CSS class-name
/// suffix consumed by `theme_class` — keep in sync with the
/// `.deck-theme-<id>` selectors in `style/presentation.css`.
pub const DECK_THEMES: &[DeckTheme] = &[
    DeckTheme { id: "slate", label_key: "deck-theme-slate-label" },
    DeckTheme { id: "paper", label_key: "deck-theme-paper-label" },
    DeckTheme { id: "midnight", label_key: "deck-theme-midnight-label" },
    DeckTheme { id: "ember", label_key: "deck-theme-ember-label" },
    DeckTheme { id: "forest", label_key: "deck-theme-forest-label" },
    DeckTheme { id: "ocean", label_key: "deck-theme-ocean-label" },
];

/// Map a `Deck.theme` id to its `.deck-theme-<id>` CSS class. An id
/// that isn't one of `DECK_THEMES` (missing, stale, or garbage)
/// falls back to `DEFAULT_THEME`'s class rather than emitting a
/// dangling class name the stylesheet has no rule for.
pub fn theme_class(theme_id: &str) -> String {
    let id = if DECK_THEMES.iter().any(|t| t.id == theme_id) {
        theme_id
    } else {
        DEFAULT_THEME
    };
    format!("deck-theme-{id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_theme_falls_back_to_default() {
        assert_eq!(theme_class("nope"), theme_class(DEFAULT_THEME));
        assert_eq!(DECK_THEMES.len(), 6);
    }

    #[test]
    fn known_theme_ids_round_trip_to_their_own_class() {
        for t in DECK_THEMES {
            assert_eq!(theme_class(t.id), format!("deck-theme-{}", t.id));
        }
    }
}
