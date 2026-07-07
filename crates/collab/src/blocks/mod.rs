// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Live-app block plugin surface — see design/live-app-blocks.md.
//!
//! Each block owns one or more NodeType variants (typically a container
//! plus a child variant) and lives in its own module here. The trait
//! documents the shape every block must satisfy; it's object-safe so
//! callers can look up the block for a given NodeType via
//! [`block_for`] and dispatch validation / metadata through the
//! `&dyn` pointer.
//!
//! HTML export dispatch stays as a `match` in `export.rs` because the
//! yrs `ReadTxn` generic prevents object-safe extraction of render
//! logic. Adding a new block therefore touches: (a) a new module here,
//! (b) one match arm in `resolve_html_tag`, `render_html_attrs`, and
//! `render_node_markdown` — each arm delegates to a helper function
//! the module exports. See `blocks/calendar.rs` for the reference.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::schema::NodeType;
use ogrenotes_common::metrics::{counter, MetricKey};

pub mod calendar;
pub mod kanban;
pub mod validate_writes;

pub use validate_writes::{
    collect_liveapp_index, diff_liveapp_deletions, repair_liveapp_attrs,
    validate_liveapp_writes, validate_liveapp_writes_scoped,
    walk_doc as walk_liveapp_violations, LiveAppDeletion, LiveAppIndex,
    LiveAppViolation, RepairReport, WalkScope,
};

/// Three-state rollout knob for the LiveApp-attribute pre-apply
/// gate. Read from the `LIVEAPP_STRICT_VALIDATION` env var at
/// server startup; passed per-call into `Room::apply_update_gated`
/// and to the REST full-state upload path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveAppValidationMode {
    /// Skip the gate. Pre-2a behavior.
    Off,
    /// Run the gate, emit `_would_reject_total` metric on
    /// violation, apply anyway. Rollout default.
    Log,
    /// Run the gate, emit `_rejected_total`, and refuse to
    /// apply — returns an ApplyUpdate error to the caller.
    Reject,
}

impl LiveAppValidationMode {
    /// Parse from the env var value. Unknown values fall back
    /// to `Log` — during rollout we favor observability over
    /// silent-off (which would mask a config typo).
    pub fn from_env_value(v: Option<&str>) -> Self {
        match v.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("off") => Self::Off,
            Some("reject") => Self::Reject,
            _ => Self::Log,
        }
    }
}

/// Emit one metric per violation and, in `Reject` mode, return
/// the first violation so the caller can shape it into their
/// own error type (`DocError` for the WS path, `ApiError` for
/// REST).
///
/// `extra_tags` — additional dimensions for the metric, e.g.
/// `[("path", "ws")]` or `[("path", "rest")]`. Node type + field
/// tags are added automatically from each violation. Shared to
/// keep the two call sites (WS + REST) from drifting on tag
/// schema or metric naming.
pub fn emit_violations_and_should_reject<'a>(
    violations: &'a [LiveAppViolation],
    mode: LiveAppValidationMode,
    extra_tags: &[(&'static str, &str)],
) -> Option<&'a LiveAppViolation> {
    let name = match mode {
        LiveAppValidationMode::Reject => "liveapp.pre_apply_validation_rejected_total",
        _ => "liveapp.pre_apply_validation_would_reject_total",
    };
    for v in violations {
        let mut tags: Vec<(&'static str, &str)> = extra_tags.to_vec();
        tags.push(("node_type", v.node_type.tag_name()));
        tags.push(("field", v.field.as_ref()));
        counter::inc(MetricKey::new(name, &tags));
    }
    if mode == LiveAppValidationMode::Reject {
        violations.first()
    } else {
        None
    }
}

/// A per-attribute validation failure surfaced back to the caller
/// (typically the HTML importer or a paste handler). Kept opaque —
/// callers wrap it in their own error variant.
///
/// `field` uses `Cow` so both the built-in validators (returning
/// static string literals) and the strict pre-apply gate layer
/// (returning runtime attr-key names when an oversized value was
/// silently clamped) can populate it without allocation on the
/// common path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockValidationError {
    pub node_type: NodeType,
    pub field: Cow<'static, str>,
    pub reason: String,
}

impl std::fmt::Display for BlockValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: invalid `{}` — {}",
            self.node_type.tag_name(),
            self.field,
            self.reason
        )
    }
}

impl std::error::Error for BlockValidationError {}

/// Convention every live-app block module implements. The trait is
/// intentionally minimal — heavy render logic stays inline in
/// `export.rs` (see the module doc for why). Blocks own: the set of
/// NodeType variants they represent, and attribute validation on the
/// import / paste path.
pub trait LiveAppBlock: Sync + 'static {
    /// Every NodeType this block owns. `block_for(nt)` returns the
    /// unique block whose `node_types()` list contains `nt`.
    fn node_types(&self) -> &'static [NodeType];

    /// Validate + canonicalize a node's attribute bag. Called on
    /// the import path (paste, HTML import) so bad values from
    /// untrusted sources don't land in the CRDT. The interactive
    /// write path trusts the client because the client is under
    /// our control. Returns the canonicalized attrs (e.g.
    /// lowercased enums, normalized dates).
    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError>;
}

/// Every registered live-app block. Add a line here when adding
/// a new block. The compile-time list is small enough that a
/// linear scan by NodeType is fine.
pub const BLOCKS: &[&(dyn LiveAppBlock + 'static)] =
    &[&calendar::CALENDAR, &kanban::KANBAN];

/// Look up the block that owns a given NodeType, or `None` if the
/// NodeType is a core editor type (paragraph, heading, embed, etc.)
/// with no live-app block owning it.
pub fn block_for(node_type: NodeType) -> Option<&'static (dyn LiveAppBlock + 'static)> {
    BLOCKS
        .iter()
        .copied()
        .find(|b| b.node_types().contains(&node_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_for_calendar_resolves_to_calendar_block() {
        let b = block_for(NodeType::Calendar).expect("Calendar has a block");
        assert!(b.node_types().contains(&NodeType::CalendarEvent));
    }

    #[test]
    fn block_for_core_type_is_none() {
        assert!(block_for(NodeType::Paragraph).is_none());
        assert!(block_for(NodeType::Embed).is_none());
    }

    #[test]
    fn every_block_owns_at_least_one_node_type() {
        for b in BLOCKS {
            assert!(
                !b.node_types().is_empty(),
                "block registered with empty node_types"
            );
        }
    }

    #[test]
    fn node_type_ownership_is_disjoint() {
        // A NodeType belongs to at most one block. Overlap would make
        // `block_for` order-dependent.
        let mut seen: std::collections::HashSet<NodeType> = std::collections::HashSet::new();
        for b in BLOCKS {
            for nt in b.node_types() {
                assert!(
                    seen.insert(*nt),
                    "{nt:?} claimed by multiple live-app blocks"
                );
            }
        }
    }
}
