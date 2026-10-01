// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Text and outlines on the renderer's fixed, light data palettes. These
//! foregrounds stay dark in both document themes, unlike canvas text.

pub(crate) const DATA_TEXT: &str = "var(--mermaid-data-text, #1a1a1a)";
pub(crate) const DATA_MUTED_TEXT: &str = "var(--mermaid-data-text-muted, #444)";
pub(crate) const DATA_STROKE: &str = "var(--mermaid-data-stroke, #444)";

/// Bound label paint without global SVG IDs. Inline dimensions keep nested
/// viewports stable under the editor's responsive rules for SVG elements.
pub(crate) fn clip_label_to_rect(x: f64, y: f64, width: f64, height: f64, text: &str) -> String {
    if width <= 0.0 || height <= 0.0 || text.is_empty() {
        return String::new();
    }
    format!(
        r#"<svg x="{x:.3}" y="{y:.3}" width="{width:.3}" height="{height:.3}" viewBox="{x:.3} {y:.3} {width:.3} {height:.3}" overflow="hidden" style="overflow:hidden;max-width:none;width:{width:.3}px;height:{height:.3}px">{text}</svg>"#
    )
}
