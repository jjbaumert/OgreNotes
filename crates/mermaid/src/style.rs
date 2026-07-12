//! Shared styling vocabulary for node/edge diagrams (flowchart, class,
//! state, ER): the CSS-injection allowlist, named `classDef` styles, and
//! resolution of a node's effective `style="…"` (class + inline +
//! luminance-based auto-contrast text color).

/// The style allowlist is the CSS-injection boundary: only these props,
/// and only benign value characters, survive into an emitted `style`
/// attribute. Everything else is dropped silently — styling is cosmetic
/// and mermaid's vocabulary is huge, so erroring would be hostile.
pub(crate) const STYLE_PROPS: &[&str] = &[
    "fill", "stroke", "stroke-width", "stroke-dasharray",
    "color", "font-weight", "font-style", "opacity",
];

/// Sanitize a comma-separated `prop:value` list against the allowlist,
/// returning `prop:value;`-joined survivors (possibly empty).
pub(crate) fn sanitize_style(styles: &str) -> String {
    let mut kept = Vec::new();
    for pair in styles.split(',') {
        let Some((prop, value)) = pair.split_once(':') else { continue };
        let (prop, value) = (prop.trim(), value.trim());
        let value_ok = value.chars().all(|c| c.is_ascii_alphanumeric() || " #.,%-".contains(c));
        if STYLE_PROPS.contains(&prop) && value_ok && !value.is_empty() {
            kept.push(format!("{prop}:{value}"));
        }
    }
    kept.join(";")
}

#[derive(Debug, Clone)]
pub(crate) struct ClassDef {
    pub name: String,
    pub style: String, // already sanitized at parse time
}

/// A node's effective style: the first assigned class with a non-empty
/// style (declared order), then the node's inline `style` layered on top
/// (CSS last-declaration-wins gives override semantics), then a
/// luminance-derived text `color` when a `fill` is set without one.
/// Returns `None` when nothing applies (the unstyled render path).
pub(crate) fn resolve(classes: &[String], inline: Option<&str>, defs: &[ClassDef]) -> Option<String> {
    let class_style = classes
        .iter()
        .find_map(|c| defs.iter().find(|d| &d.name == c && !d.style.is_empty()))
        .map(|d| d.style.as_str());
    let mut combined = match (class_style, inline) {
        (Some(c), Some(i)) => format!("{c};{i}"),
        (Some(c), None) => c.to_string(),
        (None, Some(i)) => i.to_string(),
        (None, None) => return None,
    };
    if let Some(color) = auto_contrast_text(&combined) {
        combined.push_str(";color:");
        combined.push_str(color);
    }
    Some(combined)
}

/// Black or white text for the style's `fill`, chosen by luminance — only
/// when a hex `fill` is present and no explicit `color` is set.
fn auto_contrast_text(style: &str) -> Option<&'static str> {
    if prop_value(style, "color").is_some() {
        return None;
    }
    let (r, g, b) = parse_hex(prop_value(style, "fill")?)?;
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    Some(if lum > 140.0 { "#000" } else { "#fff" })
}

/// Value of `prop` in a `prop:value;prop:value` string (last wins).
fn prop_value<'a>(style: &'a str, prop: &str) -> Option<&'a str> {
    style
        .split(';')
        .filter_map(|p| p.split_once(':'))
        .filter(|(k, _)| k.trim() == prop)
        .last()
        .map(|(_, v)| v.trim())
}

/// Parse `#rgb` / `#rrggbb` into (r,g,b). `None` for any other form.
fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    let (r, g, b) = match h.len() {
        3 => (dup(h.get(0..1)?), dup(h.get(1..2)?), dup(h.get(2..3)?)),
        6 => (h.get(0..2)?.to_string(), h.get(2..4)?.to_string(), h.get(4..6)?.to_string()),
        _ => return None,
    };
    Some((
        u8::from_str_radix(&r, 16).ok()?,
        u8::from_str_radix(&g, 16).ok()?,
        u8::from_str_radix(&b, 16).ok()?,
    ))
}

fn dup(nibble: &str) -> String {
    format!("{nibble}{nibble}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_drops_unknown_and_unsafe() {
        // allowlisted survives; unknown prop and unsafe value dropped.
        assert_eq!(sanitize_style("fill:#f00,stroke:#333"), "fill:#f00;stroke:#333");
        assert_eq!(sanitize_style("fill:red,evil:url(x),onclick:alert"), "fill:red");
        assert_eq!(sanitize_style("fill:\"</style>"), "");
    }

    #[test]
    fn resolve_class_then_inline_then_autocontrast() {
        let defs = vec![ClassDef { name: "hot".into(), style: "fill:#f00".into() }];
        // class only -> fill + auto-contrast (dark red -> white text)
        assert_eq!(
            resolve(&["hot".into()], None, &defs).as_deref(),
            Some("fill:#f00;color:#fff")
        );
        // inline overrides via CSS last-wins ordering; explicit color wins,
        // so no auto-contrast is added.
        assert_eq!(
            resolve(&["hot".into()], Some("fill:#ffffcc;color:#111"), &defs).as_deref(),
            Some("fill:#f00;fill:#ffffcc;color:#111")
        );
        // light fill, no color -> black text
        assert_eq!(
            resolve(&[], Some("fill:#ffffcc"), &defs).as_deref(),
            Some("fill:#ffffcc;color:#000")
        );
        // nothing -> None (unstyled path)
        assert_eq!(resolve(&[], None, &defs), None);
        // non-hex fill -> no auto-contrast
        assert_eq!(resolve(&[], Some("fill:red"), &defs).as_deref(), Some("fill:red"));
    }
}
