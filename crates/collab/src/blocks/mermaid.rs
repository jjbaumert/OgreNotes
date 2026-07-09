// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mermaid diagram live-app block. `NodeType::Mermaid` is a leaf
//! carrying its diagram source in a `source` attribute. Rendered to
//! SVG by `ogrenotes-mermaid` on the export path; validation here just
//! caps length and preserves `blockId`.

use std::collections::HashMap;

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct MermaidBlock;
pub static MERMAID: MermaidBlock = MermaidBlock;

/// Max diagram source length (chars). Over-cap is hard-rejected so an
/// interactive write is not silently clamped (which the write gate
/// would flag as a canonicalization violation).
///
/// Re-exported from `ogrenotes_mermaid` — that crate is the single
/// source of truth, shared with the frontend modal's client-side guard,
/// so this path stays stable for existing callers.
pub use ogrenotes_mermaid::MAX_SOURCE_LEN;

/// Single source of truth for the attribute names the export path
/// iterates. Mirrors `CALENDAR_ATTR_NAMES` / `CARD_ATTR_NAMES`.
pub const MERMAID_ATTR_NAMES: &[&str] = &["source"];

impl LiveAppBlock for MermaidBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Mermaid]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        if node_type != NodeType::Mermaid {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("node_type"),
                reason: format!("MermaidBlock cannot validate {}", node_type.tag_name()),
            });
        }
        let mut out = HashMap::new();
        let source = attrs.get("source").map(String::as_str).unwrap_or("");
        if source.trim().is_empty() {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: "mermaid source must not be empty".to_string(),
            });
        }
        if source.chars().count() > MAX_SOURCE_LEN {
            return Err(BlockValidationError {
                node_type,
                field: std::borrow::Cow::Borrowed("source"),
                reason: format!("source exceeds {MAX_SOURCE_LEN} chars"),
            });
        }
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
        let out = MERMAID
            .validate_attrs(NodeType::Mermaid, &attrs(&[("source", "pie\n\"A\": 1")]))
            .unwrap();
        assert_eq!(out.get("source").map(String::as_str), Some("pie\n\"A\": 1"));
    }

    #[test]
    fn empty_source_rejected() {
        assert!(MERMAID.validate_attrs(NodeType::Mermaid, &attrs(&[("source", "   ")])).is_err());
    }

    #[test]
    fn oversized_source_rejected() {
        let big = "x".repeat(MAX_SOURCE_LEN + 1);
        assert!(MERMAID.validate_attrs(NodeType::Mermaid, &attrs(&[("source", &big)])).is_err());
    }

    /// `MAX_SOURCE_LEN` is now a re-export of `ogrenotes_mermaid::MAX_SOURCE_LEN`
    /// (single source of truth shared with the frontend modal's guard).
    /// Pin the value and the boundary so the relocation stays transparent
    /// to callers of this path.
    #[test]
    fn max_source_len_reexport_matches_shared_constant() {
        assert_eq!(MAX_SOURCE_LEN, 20_000);
        assert_eq!(MAX_SOURCE_LEN, ogrenotes_mermaid::MAX_SOURCE_LEN);
        // Exactly at the cap is still accepted.
        let at_cap = "x".repeat(MAX_SOURCE_LEN);
        assert!(MERMAID.validate_attrs(NodeType::Mermaid, &attrs(&[("source", &at_cap)])).is_ok());
    }

    #[test]
    fn preserves_block_id() {
        let out = MERMAID
            .validate_attrs(NodeType::Mermaid, &attrs(&[("source", "pie\n\"A\": 1"), ("blockId", "abc")]))
            .unwrap();
        assert_eq!(out.get("blockId").map(String::as_str), Some("abc"));
    }

    #[test]
    fn wrong_node_type_rejected() {
        assert!(MERMAID.validate_attrs(NodeType::Paragraph, &attrs(&[("source", "x")])).is_err());
    }
}
