// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Equations. `NodeType::MathBlock` (display) and `NodeType::MathInline`
//! (`$…$` in running text) are leaves carrying LaTeX math in a `source`
//! attribute, rendered to MathML by
//! `ogrenotes-math` on the export path and in the editor. Validation
//! here caps length and preserves `blockId`; like Mermaid, it doesn't
//! parse the source, so a stricter renderer later can't make a stored
//! document unsaveable (a bad equation shows its error and source).

use std::collections::HashMap;

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct MathBlockDef;
pub static MATH: MathBlockDef = MathBlockDef;

/// Max equation source length (chars), hard-rejected rather than
/// clamped. Re-exported from `ogrenotes_math`, the single source of
/// truth shared with the editor's client-side guard.
pub use ogrenotes_math::MAX_SOURCE_LEN;

impl LiveAppBlock for MathBlockDef {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::MathBlock, NodeType::MathInline]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        if !self.node_types().contains(&node_type) {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("node_type"),
                reason: format!("MathBlockDef cannot validate {}", node_type.tag_name()),
            });
        }
        let source = attrs.get("source").map(String::as_str).unwrap_or("");
        if source.trim().is_empty() {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: "equation source must not be empty".to_string(),
            });
        }
        if source.chars().count() > MAX_SOURCE_LEN {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: format!("source exceeds {MAX_SOURCE_LEN} chars"),
            });
        }
        let mut out = HashMap::new();
        // Echo unchanged so the write gate sees no canonicalization diff.
        out.insert("source".to_string(), source.to_string());
        // Preserve the CRDT anchor.
        if let Some(bid) = attrs.get("blockId") {
            out.insert("blockId".to_string(), bid.clone());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn valid_source_echoes_unchanged() {
        let out = MATH
            .validate_attrs(NodeType::MathBlock, &attrs(&[("source", "x^2"), ("blockId", "b1")]))
            .unwrap();
        assert_eq!(out.get("source").map(String::as_str), Some("x^2"));
        assert_eq!(out.get("blockId").map(String::as_str), Some("b1"));
        assert_eq!(out.len(), 2, "unknown attributes are dropped");
    }

    #[test]
    fn unparseable_source_is_still_accepted() {
        // Rendering errors are shown in place, never a write rejection.
        assert!(MATH.validate_attrs(NodeType::MathBlock, &attrs(&[("source", "\\foo{")])).is_ok());
    }

    #[test]
    fn empty_and_oversized_sources_rejected() {
        assert!(MATH.validate_attrs(NodeType::MathBlock, &attrs(&[("source", "  ")])).is_err());
        assert!(MATH.validate_attrs(NodeType::MathBlock, &attrs(&[])).is_err());
        let big = "x".repeat(MAX_SOURCE_LEN + 1);
        assert!(MATH.validate_attrs(NodeType::MathBlock, &attrs(&[("source", &big)])).is_err());
    }

    #[test]
    fn inline_equations_share_the_gate() {
        let out = MATH.validate_attrs(NodeType::MathInline, &attrs(&[("source", "a^2")])).unwrap();
        assert_eq!(out.get("source").map(String::as_str), Some("a^2"));
        assert!(MATH.validate_attrs(NodeType::MathInline, &attrs(&[("source", "")])).is_err());
    }

    #[test]
    fn wrong_node_type_rejected() {
        assert!(MATH.validate_attrs(NodeType::Paragraph, &attrs(&[("source", "x")])).is_err());
    }
}
