// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Frontend live-app block plugin surface — see
//! `design/live-app-blocks.md`. Backend counterpart:
//! `crates/collab/src/blocks/`.
//!
//! Two trait registries:
//!
//! - [`BLOCK_VIEWS`] — renderers looked up by `view::render_node`.
//!   Adding a new block = one module here implementing
//!   [`LiveAppBlockView`] + one line in the const.
//! - [`BLOCK_INSERTS`] — insert-menu entries. Consumed by the `/`
//!   slash-command menu, the block-menu, and the command palette
//!   so a new widget appears in all three surfaces at once.

use std::collections::HashMap;

use web_sys::{Document, Node as DomNode};

use super::model::NodeType;

pub mod calendar;
pub mod kanban;
pub mod mermaid;

/// Renders one live-app block node to a DOM subtree.
///
/// `view::render_node` walks its match arms first (for legacy node
/// types); if nothing matches, it delegates to the block whose
/// [`Self::node_types`] contains the node's type. The trait is
/// object-safe (all methods take concrete parameters) so lookup
/// via [`view_for`] returns a `&dyn` pointer.
pub trait LiveAppBlockView: Sync + 'static {
    fn node_types(&self) -> &'static [NodeType];

    /// Build a DOM node for this block. The returned element becomes
    /// the block's outer rendering; the caller does not descend into
    /// `content` — the block owns its subtree.
    fn render(
        &self,
        doc: &Document,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
        // `content` is the block's child nodes (e.g. Calendar's events).
        // Renderers walk this themselves — the caller does NOT recurse.
        content: &super::model::Fragment,
    ) -> Option<DomNode>;
}

/// Describes an inline-insert entry for a live-app block. Consumed
/// by all insert surfaces (`/` menu, block-menu, palette).
pub trait LiveAppBlockInsert: Sync + 'static {
    /// Stable id (kebab-case) used by ToolbarCommand routing and
    /// slash-menu keying. Must be unique across BLOCK_INSERTS.
    fn id(&self) -> &'static str;
    /// Fluent key for the user-visible label. Convention:
    /// `insert-<id>-label`. See `frontend/locales/en-US/main.ftl`.
    fn label_key(&self) -> &'static str;
    /// Fluent key for the one-line description shown in the palette
    /// and slash menu. Convention: `insert-<id>-description`.
    fn description_key(&self) -> &'static str;
    /// Emoji glyph. Keeps the insert surface icon-consistent
    /// without new sprite/svg wiring.
    fn icon(&self) -> &'static str;
    /// Build the default block Node to insert. The caller places
    /// it at the current cursor position via the transform pipeline.
    fn build_default_node(&self) -> super::model::Node;
}

/// Every registered live-app block view. Add a line here when
/// adding a new block.
pub const BLOCK_VIEWS: &[&(dyn LiveAppBlockView + 'static)] =
    &[&calendar::CalendarView, &kanban::KanbanView, &mermaid::MermaidView];

/// Every registered live-app block insert entry. Read by every
/// insert surface — new entries show up in all three at once.
pub const BLOCK_INSERTS: &[&(dyn LiveAppBlockInsert + 'static)] =
    &[&calendar::CalendarInsert, &kanban::KanbanInsert, &mermaid::MermaidInsert];

/// Look up the view for a given NodeType. Returns `None` for core
/// editor types with no live-app block owner.
pub fn view_for(node_type: NodeType) -> Option<&'static (dyn LiveAppBlockView + 'static)> {
    BLOCK_VIEWS
        .iter()
        .copied()
        .find(|b| b.node_types().contains(&node_type))
}

/// Look up an insert entry by id (kebab-case string). Used by
/// `ToolbarCommand::InsertLiveApp(id)` dispatch and by the slash
/// menu when the user activates an item.
pub fn insert_by_id(id: &str) -> Option<&'static (dyn LiveAppBlockInsert + 'static)> {
    BLOCK_INSERTS.iter().copied().find(|b| b.id() == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every NodeType is claimed by at most one block. Two blocks
    /// claiming the same NodeType would make `view_for` order-
    /// dependent — a Kanban render silently taking over Calendar's
    /// grid on load. Backend has the equivalent test at
    /// `crates/collab/src/blocks/mod.rs::tests::node_type_ownership_is_disjoint`;
    /// keeping the invariant on both sides means the CI cascade
    /// catches the failure regardless of which side introduces
    /// the collision.
    #[test]
    fn view_node_type_ownership_is_disjoint() {
        let mut seen: HashSet<NodeType> = HashSet::new();
        for view in BLOCK_VIEWS {
            for nt in view.node_types() {
                assert!(
                    seen.insert(*nt),
                    "{nt:?} claimed by multiple live-app block views"
                );
            }
        }
    }

    /// Every insert entry's id is unique. Duplicates would make
    /// `insert_by_id` (and the auto-registered palette entries in
    /// `commands/mod.rs`) route ambiguously — a second block
    /// registering `id() == \"calendar\"` would silently overwrite
    /// the palette entry and become unreachable via block-menu.
    #[test]
    fn insert_ids_are_unique() {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for insert in BLOCK_INSERTS {
            assert!(
                seen.insert(insert.id()),
                "insert id {:?} registered twice",
                insert.id()
            );
        }
    }

    /// A block that owns some NodeType has a corresponding view.
    /// (No view without an insert entry is required — some blocks
    /// might be non-user-insertable — but if there's no view,
    /// render_node falls through to a bare Element.)
    #[test]
    fn every_view_owns_at_least_one_node_type() {
        for view in BLOCK_VIEWS {
            assert!(
                !view.node_types().is_empty(),
                "view registered with empty node_types"
            );
        }
    }
}
