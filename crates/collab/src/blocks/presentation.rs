//! Presentation deck block — Slide/Frame attr validation and export
//! helpers. See design/presentations.md and blocks/calendar.rs (the
//! reference implementation this mirrors).

use std::collections::HashMap;
use crate::schema::NodeType;
use super::{BlockValidationError, LiveAppBlock};

pub struct PresentationBlock;
pub static PRESENTATION: PresentationBlock = PresentationBlock;

pub const LAYOUTS: &[&str] = &["title", "title-content", "two-column", "blank"];
pub const ROLES: &[&str] = &["content", "notes"];
pub const SLIDE_ATTR_NAMES: &[&str] = &["layout", "background"];
pub const FRAME_ATTR_NAMES: &[&str] = &["x", "y", "w", "h", "z", "role"];
const BACKGROUND_MAX_LEN: usize = 200;

impl LiveAppBlock for PresentationBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Slide, NodeType::Frame]
    }
    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        match node_type {
            NodeType::Slide => validate_slide_attrs(attrs),
            NodeType::Frame => validate_frame_attrs(attrs),
            other => Err(err(other, "node_type", "not a presentation node")),
        }
    }
}

fn err(node_type: NodeType, field: &'static str, reason: &str) -> BlockValidationError {
    BlockValidationError { node_type, field: field.into(), reason: reason.into() }
}

/// Accept-verbatim-or-reject. NEVER rewrite a value: the write gate's
/// canonicalization diff (validate_writes.rs) treats any change as a
/// violation, and legitimate frame drags would trip it. Readers clamp.
fn validate_frame_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    for key in ["x", "y", "w", "h"] {
        if let Some(v) = attrs.get(key) {
            let f: f64 = v.parse().map_err(|_| err(NodeType::Frame, "geometry",
                &format!("{key} is not a number: {v}")))?;
            let ok = f.is_finite() && (0.0..=1.0).contains(&f)
                && !((key == "w" || key == "h") && f == 0.0);
            if !ok {
                return Err(err(NodeType::Frame, "geometry",
                    &format!("{key} out of range 0..=1: {v}")));
            }
        }
    }
    if let Some(v) = attrs.get("z") {
        v.parse::<i64>().map_err(|_| err(NodeType::Frame, "z",
            &format!("z is not an integer: {v}")))?;
    }
    if let Some(v) = attrs.get("role") {
        if !ROLES.contains(&v.as_str()) {
            return Err(err(NodeType::Frame, "role", &format!("unknown role: {v}")));
        }
    }
    Ok(attrs.clone())
}

fn validate_slide_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    if let Some(v) = attrs.get("layout") {
        if !LAYOUTS.contains(&v.as_str()) {
            return Err(err(NodeType::Slide, "layout", &format!("unknown layout: {v}")));
        }
    }
    if let Some(v) = attrs.get("background") {
        if v.len() > BACKGROUND_MAX_LEN {
            return Err(err(NodeType::Slide, "background", "background too long"));
        }
    }
    Ok(attrs.clone())
}

// ── export helpers (called from export.rs match arms, Task 4) ──

pub fn html_tag(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Slide => "section",
        _ => "div", // Frame
    }
}

/// Pre-escaped attr string with a LEADING space (calendar.rs contract).
pub fn html_attrs(node_type: NodeType, attrs: &HashMap<String, String>) -> String {
    let names = match node_type {
        NodeType::Slide => SLIDE_ATTR_NAMES,
        _ => FRAME_ATTR_NAMES,
    };
    let mut out = String::new();
    for name in names {
        if let Some(v) = attrs.get(*name) {
            out.push_str(&format!(" data-{}=\"{}\"", name, crate::export::html_escape(v)));
        }
    }
    match node_type {
        NodeType::Slide => out.push_str(" class=\"deck-slide\""),
        _ => out.push_str(" class=\"deck-frame\""),
    }
    out
}

pub fn markdown_placeholder(node_type: NodeType, _attrs: &HashMap<String, String>) -> String {
    match node_type {
        NodeType::Slide => String::new(), // heading emitted by the export arm (needs the slide number)
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn frame_accepts_valid_geometry_verbatim() {
        let a = attrs(&[("x", "0.25"), ("y", "0"), ("w", "0.5"), ("h", "0.333"),
                        ("z", "2"), ("role", "content")]);
        let out = PRESENTATION.validate_attrs(NodeType::Frame, &a).unwrap();
        assert_eq!(out, a); // byte-identical echo — never canonicalize
    }

    #[test]
    fn frame_rejects_out_of_range_geometry() {
        for bad in [("x", "1.5"), ("x", "-0.1"), ("w", "0"), ("h", "nan"), ("x", "abc")] {
            let a = attrs(&[bad]);
            assert!(PRESENTATION.validate_attrs(NodeType::Frame, &a).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn frame_rejects_bad_role_and_z() {
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[("role", "banner")])).is_err());
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[("z", "2.5")])).is_err());
    }

    #[test]
    fn frame_accepts_absent_attrs() {
        // Absent attrs are fine — readers apply defaults (x=0,y=0,w=1,h=1,z=0,role=content).
        assert!(PRESENTATION.validate_attrs(NodeType::Frame, &attrs(&[])).is_ok());
    }

    #[test]
    fn slide_validates_layout_and_background() {
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("layout", "two-column")])).is_ok());
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("layout", "pyramid")])).is_err());
        let long = "x".repeat(300);
        assert!(PRESENTATION.validate_attrs(NodeType::Slide,
            &attrs(&[("background", &long)])).is_err()); // cap 200 chars
    }
}
