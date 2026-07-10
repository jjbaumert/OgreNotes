// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Guards `frontend/style/main.css`'s `--tok-*` custom properties
//! against drifting from `color_for` — the doc comment on
//! `color_for` calls the crate the "single source of truth" for
//! these colors; this test is what makes that true. See
//! architecture.md's *Cross-target schema agreement*.

use ogrenotes_highlight::{color_for, Theme, TokenKind};

const CSS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../frontend/style/main.css"
));

const KINDS: [TokenKind; 7] = [
    TokenKind::Keyword, TokenKind::Type, TokenKind::String,
    TokenKind::Comment, TokenKind::Number, TokenKind::Function,
    TokenKind::Meta,
];

/// Value assigned to `--{var_name}` inside the first `{ ... }` block
/// whose selector text is exactly `block_selector`.
fn css_var(css: &str, block_selector: &str, var_name: &str) -> Option<String> {
    let block_start = css.find(block_selector)?;
    let body_start = css[block_start..].find('{')? + block_start + 1;
    let body_end = css[body_start..].find('}')? + body_start;
    let needle = format!("--{var_name}:");
    let rel = css[body_start..body_end].find(&needle)? + needle.len();
    Some(css[body_start..body_end][rel..].split(';').next()?.trim().to_string())
}

#[test]
fn css_custom_properties_match_color_for() {
    for kind in KINDS {
        let cls = kind.css_class().unwrap();
        let light = css_var(CSS, ":root {", cls)
            .unwrap_or_else(|| panic!("no --{cls} in :root"));
        assert_eq!(light, color_for(kind, Theme::Light).unwrap(), "light --{cls} drifted");
        let dark = css_var(CSS, ":root[data-theme=\"dark\"]", cls)
            .unwrap_or_else(|| panic!("no --{cls} in :root[data-theme=\"dark\"]"));
        assert_eq!(dark, color_for(kind, Theme::Dark).unwrap(), "dark --{cls} drifted");
    }
}
