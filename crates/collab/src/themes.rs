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
                    block.contains(&format!("{var}: {want};")),
                    "theme {} : CSS {var} does not match the backend table value {want}",
                    t.id
                );
            }
        }
    }
}
