use super::model::{Node, NodeType};
use super::position::resolve;
use super::transform::StepMap;

/// A selection in the document.
#[derive(Debug, Clone, PartialEq)]
pub enum Selection {
    /// A text selection with anchor and head positions.
    /// When anchor == head, this is a cursor (empty selection).
    Text(TextSelection),
    /// A selection of an entire atom node (e.g., an image or horizontal rule).
    NodeSel(NodeSelection),
    /// A selection of the entire document.
    All(AllSelection),
}

impl Selection {
    /// The left edge of the selection (minimum of anchor and head).
    pub fn from(&self) -> usize {
        match self {
            Selection::Text(s) => s.from(),
            Selection::NodeSel(s) => s.from,
            Selection::All(s) => s.from,
        }
    }

    /// The right edge of the selection (maximum of anchor and head).
    pub fn to(&self) -> usize {
        match self {
            Selection::Text(s) => s.to(),
            Selection::NodeSel(s) => s.to,
            Selection::All(s) => s.to,
        }
    }

    /// The anchor position (where the selection started, fixed end).
    pub fn anchor(&self) -> usize {
        match self {
            Selection::Text(s) => s.anchor,
            Selection::NodeSel(s) => s.from,
            Selection::All(s) => s.from,
        }
    }

    /// The head position (where the selection ends, movable end).
    pub fn head(&self) -> usize {
        match self {
            Selection::Text(s) => s.head,
            Selection::NodeSel(s) => s.to,
            Selection::All(s) => s.to,
        }
    }

    /// Whether the selection is empty (cursor with no range).
    pub fn empty(&self) -> bool {
        self.from() == self.to()
    }

    /// Map this selection through a step map.
    pub fn map(&self, mapping: &StepMap) -> Selection {
        match self {
            Selection::Text(s) => Selection::Text(TextSelection {
                anchor: mapping.map(s.anchor, -1),
                head: mapping.map(s.head, 1),
            }),
            Selection::NodeSel(s) => Selection::NodeSel(NodeSelection {
                from: mapping.map(s.from, -1),
                to: mapping.map(s.to, 1),
            }),
            Selection::All(s) => Selection::All(AllSelection {
                from: mapping.map(s.from, -1),
                to: mapping.map(s.to, 1),
            }),
        }
    }

    /// Create a cursor (empty text selection) at a position.
    pub fn cursor(pos: usize) -> Self {
        Selection::Text(TextSelection {
            anchor: pos,
            head: pos,
        })
    }

    /// Create a text selection between two positions.
    pub fn text(anchor: usize, head: usize) -> Self {
        Selection::Text(TextSelection { anchor, head })
    }

    /// Create a node selection around the atom node at `pos`.
    /// `pos` is the position before the node in its parent's content.
    /// Returns `None` if no atom node exists at that position.
    pub fn node(doc: &Node, pos: usize) -> Option<Self> {
        let rp = resolve(doc, pos)?;
        let node = rp.node_after(doc)?;

        // Only atom nodes (HorizontalRule, Image) can be node-selected
        if !node.node_type().map(|t| t.is_atom()).unwrap_or(false) {
            return None;
        }

        let size = node.node_size();
        Some(Selection::NodeSel(NodeSelection {
            from: pos,
            to: pos + size,
        }))
    }

    /// Create a selection of the entire document.
    pub fn all(doc: &Node) -> Self {
        Selection::All(AllSelection {
            from: 0,
            to: doc.content_size(),
        })
    }

    /// Find the nearest valid cursor position from a given position.
    /// Searches forward (dir=1) or backward (dir=-1).
    /// Returns a cursor selection at the first valid text position found.
    pub fn find_from(doc: &Node, pos: usize, dir: i32) -> Option<Self> {
        let content_size = doc.content_size();
        if pos > content_size {
            return None;
        }

        // Try to resolve at the given position
        if let Some(rp) = resolve(doc, pos) {
            let parent = rp.node_at(rp.depth, doc);
            if is_inline_content_node(parent) {
                return Some(Selection::cursor(pos));
            }
        }

        // Search in the given direction for a valid position.
        // Skip by child node sizes when possible to avoid O(n) scanning.
        let mut search_pos = pos;
        let max_iterations = content_size + 1; // safety bound
        for _ in 0..max_iterations {
            if dir > 0 {
                if search_pos >= content_size {
                    return None;
                }
                search_pos += 1;
            } else {
                if search_pos == 0 {
                    return None;
                }
                search_pos -= 1;
            }

            if let Some(rp) = resolve(doc, search_pos) {
                let parent = rp.node_at(rp.depth, doc);
                if is_inline_content_node(parent) {
                    return Some(Selection::cursor(search_pos));
                }
            }
        }
        None
    }
}

/// Check if a node is a container for inline content (text).
/// This includes Paragraph, Heading, and CodeBlock.
/// ListItem and TaskItem do NOT directly hold inline content --
/// they contain Paragraphs which do.
fn is_inline_content_node(node: &Node) -> bool {
    match node {
        Node::Element { node_type, .. } => matches!(
            node_type,
            NodeType::Paragraph | NodeType::Heading | NodeType::CodeBlock
        ),
        _ => false,
    }
}

// ─── TextSelection ──────────────────────────────────────────────

/// A text selection between two positions.
/// When anchor == head, this is a cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSelection {
    /// Where the selection started (fixed end).
    pub anchor: usize,
    /// Where the selection ends (movable end, follows cursor).
    pub head: usize,
}

impl TextSelection {
    /// The left edge of the selection.
    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    /// The right edge of the selection.
    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    /// Whether the selection is empty (cursor).
    pub fn empty(&self) -> bool {
        self.anchor == self.head
    }
}

// ─── NodeSelection ──────────────────────────────────────────────

/// A selection of an entire atom node (e.g., an image or horizontal rule).
/// Only nodes where `NodeType::is_atom()` returns true can be node-selected.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSelection {
    /// Position before the selected node.
    pub from: usize,
    /// Position after the selected node.
    pub to: usize,
}

// ─── AllSelection ───────────────────────────────────────────────

/// A selection of the entire document content.
#[derive(Debug, Clone, PartialEq)]
pub struct AllSelection {
    pub from: usize,
    pub to: usize,
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::*;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("World")]),
                ),
            ]),
        )
    }

    // ── Cursor ──

    #[test]
    fn cursor_creation() {
        let sel = Selection::cursor(1);
        assert_eq!(sel.anchor(), 1);
        assert_eq!(sel.head(), 1);
        assert_eq!(sel.from(), 1);
        assert_eq!(sel.to(), 1);
        assert!(sel.empty());
    }

    // ── TextSelection ──

    #[test]
    fn text_selection_range() {
        let sel = Selection::text(1, 6);
        assert_eq!(sel.anchor(), 1);
        assert_eq!(sel.head(), 6);
        assert_eq!(sel.from(), 1);
        assert_eq!(sel.to(), 6);
        assert!(!sel.empty());
    }

    #[test]
    fn text_selection_reversed() {
        let sel = Selection::text(6, 1);
        assert_eq!(sel.anchor(), 6);
        assert_eq!(sel.head(), 1);
        assert_eq!(sel.from(), 1);
        assert_eq!(sel.to(), 6);
        assert!(!sel.empty());
    }

    // ── NodeSelection ──

    #[test]
    fn node_selection_hr() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        );
        let sel = Selection::node(&doc, 4).unwrap();
        assert_eq!(sel.from(), 4);
        assert_eq!(sel.to(), 5);
        assert!(!sel.empty());
    }

    #[test]
    fn node_selection_rejects_paragraph() {
        let doc = simple_doc();
        // Position 0 is before the first paragraph, which is NOT an atom
        let sel = Selection::node(&doc, 0);
        assert!(sel.is_none());
    }

    #[test]
    fn node_selection_rejects_text_position() {
        let doc = simple_doc();
        // Position 3 is inside text
        let sel = Selection::node(&doc, 3);
        assert!(sel.is_none());
    }

    #[test]
    fn node_selection_image() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element(NodeType::Image),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("text")]),
                ),
            ]),
        );
        // Image is at position 0, size 1
        let sel = Selection::node(&doc, 0).unwrap();
        assert_eq!(sel.from(), 0);
        assert_eq!(sel.to(), 1);
    }

    // ── AllSelection ──

    #[test]
    fn all_selection() {
        let doc = simple_doc();
        let sel = Selection::all(&doc);
        assert_eq!(sel.from(), 0);
        assert_eq!(sel.to(), 14);
        assert!(!sel.empty());
    }

    // ── Selection equality ──

    #[test]
    fn selection_equality() {
        let a = Selection::cursor(5);
        let b = Selection::cursor(5);
        assert_eq!(a, b);

        let c = Selection::cursor(3);
        assert_ne!(a, c);

        let d = Selection::text(1, 6);
        let e = Selection::text(1, 6);
        assert_eq!(d, e);
    }

    // ── Mapping ──

    #[test]
    fn cursor_maps_through_insert() {
        let sel = Selection::cursor(5);
        let map = StepMap::new(3, 0, 4);
        let mapped = sel.map(&map);
        assert_eq!(mapped.anchor(), 9);
        assert_eq!(mapped.head(), 9);
    }

    #[test]
    fn range_maps_through_delete() {
        let sel = Selection::text(2, 8);
        let map = StepMap::new(0, 2, 0);
        let mapped = sel.map(&map);
        assert_eq!(mapped.from(), 0);
        assert_eq!(mapped.to(), 6);
    }

    #[test]
    fn cursor_maps_through_insert_before() {
        let sel = Selection::cursor(10);
        let map = StepMap::new(5, 0, 3);
        let mapped = sel.map(&map);
        assert_eq!(mapped.head(), 13);
    }

    #[test]
    fn cursor_maps_through_insert_after() {
        let sel = Selection::cursor(3);
        let map = StepMap::new(5, 0, 3);
        let mapped = sel.map(&map);
        assert_eq!(mapped.head(), 3);
    }

    #[test]
    fn node_sel_map_bias_correct() {
        // Insert at the exact position of a node selection's from
        let sel = Selection::NodeSel(NodeSelection { from: 5, to: 6 });
        let map = StepMap::new(5, 0, 3); // insert 3 chars at position 5
        let mapped = sel.map(&map);
        // from should stick left (bias -1): stays at 5
        assert_eq!(mapped.from(), 5);
        // to should stick right (bias +1): 6 + 3 = 9
        assert_eq!(mapped.to(), 9);
    }

    // ── find_from ──

    #[test]
    fn find_from_forward() {
        let doc = simple_doc();
        let sel = Selection::find_from(&doc, 0, 1).unwrap();
        assert_eq!(sel.head(), 1);
    }

    #[test]
    fn find_from_backward() {
        let doc = simple_doc();
        let sel = Selection::find_from(&doc, 7, -1).unwrap();
        assert_eq!(sel.head(), 6);
    }

    #[test]
    fn find_from_at_valid_position() {
        let doc = simple_doc();
        let sel = Selection::find_from(&doc, 3, 1).unwrap();
        assert_eq!(sel.head(), 3);
    }

    #[test]
    fn find_from_out_of_bounds() {
        let doc = simple_doc();
        assert!(Selection::find_from(&doc, 100, 1).is_none());
    }

    #[test]
    fn find_from_at_doc_end_backward() {
        let doc = simple_doc();
        let sel = Selection::find_from(&doc, 14, -1).unwrap();
        assert_eq!(sel.head(), 13);
    }

    #[test]
    fn find_from_in_list_document() {
        // doc > bulletList > listItem > paragraph("item")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("item")]),
                    )]),
                )]),
            )]),
        );
        // Position 0 = before bulletList at doc level
        // Searching forward: pos 1 = ul level, pos 2 = li level,
        // pos 3 = paragraph content start (depth 3, inline content)
        let sel = Selection::find_from(&doc, 0, 1).unwrap();
        // Verify it's inside the paragraph
        let rp = resolve(&doc, sel.head()).unwrap();
        assert_eq!(rp.depth, 3); // doc > ul > li > paragraph
        assert!(super::is_inline_content_node(rp.node_at(rp.depth, &doc)));
    }

    // ── Mapping: additional cases ──

    #[test]
    fn range_maps_through_insert_inside() {
        // Selection spans 2..10, insert 3 chars at position 5 (inside the range)
        let sel = Selection::text(2, 10);
        let map = StepMap::new(5, 0, 3);
        let mapped = sel.map(&map);
        // anchor (2) is before insert → unchanged
        assert_eq!(mapped.from(), 2);
        // head (10) is after insert → shifted by 3
        assert_eq!(mapped.to(), 13);
    }

    #[test]
    fn reversed_range_maps_correctly() {
        // Reversed selection: anchor=10, head=2
        let sel = Selection::text(10, 2);
        let map = StepMap::new(5, 0, 3); // insert 3 at pos 5
        let mapped = sel.map(&map);
        // anchor (10) → 13 (shifted), head (2) → 2 (before insert)
        assert_eq!(mapped.anchor(), 13);
        assert_eq!(mapped.head(), 2);
        assert_eq!(mapped.from(), 2);
        assert_eq!(mapped.to(), 13);
    }

    #[test]
    fn all_selection_maps() {
        let sel = Selection::All(AllSelection { from: 0, to: 14 });
        let map = StepMap::new(5, 0, 3); // insert 3 at pos 5
        let mapped = sel.map(&map);
        assert_eq!(mapped.from(), 0);
        assert_eq!(mapped.to(), 17);
    }

    #[test]
    fn range_maps_through_replacement() {
        // Replace 2 chars at position 3 with 5 chars (net +3)
        let sel = Selection::text(1, 10);
        let map = StepMap::new(3, 2, 5);
        let mapped = sel.map(&map);
        assert_eq!(mapped.from(), 1);
        assert_eq!(mapped.to(), 13);
    }

    // ── find_from: additional cases ──

    #[test]
    fn find_from_skips_leaf_nodes() {
        // doc > paragraph("A") > HR > paragraph("B")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("A")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("B")]),
                ),
            ]),
        );
        // Position 3 = after first paragraph close. Search forward should skip HR
        // and land inside second paragraph.
        let sel = Selection::find_from(&doc, 3, 1).unwrap();
        let rp = resolve(&doc, sel.head()).unwrap();
        let parent = rp.node_at(rp.depth, &doc);
        assert!(super::is_inline_content_node(parent));
        assert!(sel.head() > 4, "should be past the HR");
    }

    #[test]
    fn find_from_backward_at_zero_returns_none() {
        let doc = simple_doc();
        assert!(Selection::find_from(&doc, 0, -1).is_none());
    }

    #[test]
    fn find_from_empty_doc() {
        // Doc with a single empty paragraph
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::empty(),
            )]),
        );
        // Should find the cursor position inside the empty paragraph
        let sel = Selection::find_from(&doc, 0, 1).unwrap();
        assert_eq!(sel.head(), 1); // inside the paragraph
    }

    #[test]
    fn find_from_only_leaf_nodes() {
        // Doc with only an HR — no valid text cursor position
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element(NodeType::HorizontalRule)]),
        );
        // Forward from 0: HR is at pos 0, size 1 — no textblock to land in
        assert!(Selection::find_from(&doc, 0, 1).is_none());
    }

    // ── NodeSelection: edge case ──

    #[test]
    fn node_selection_at_doc_end() {
        let doc = simple_doc();
        // Position 14 = content_size, past all nodes
        assert!(Selection::node(&doc, 14).is_none());
    }

    // ── empty() edge case ──

    #[test]
    fn all_selection_empty_doc() {
        // Doc with no children: content_size = 0
        let doc = Node::element_with_content(NodeType::Doc, Fragment::empty());
        let sel = Selection::all(&doc);
        assert_eq!(sel.from(), 0);
        assert_eq!(sel.to(), 0);
        assert!(sel.empty());
    }
}
