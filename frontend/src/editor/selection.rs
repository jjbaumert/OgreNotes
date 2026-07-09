// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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

/// Compute the caret destination for a vertical arrow key when an atom
/// block (horizontal rule, image, embed) sits between the current text
/// block and its neighbor in the travel direction.
///
/// The browser's contenteditable cannot place a text caret across a void
/// element like `<hr>`, so native ArrowUp/ArrowDown gets stuck and the
/// cursor never crosses the rule (#78). This computes where the caret
/// *should* land — the nearest text position on the far side of the atom —
/// so the keydown handler can move it explicitly.
///
/// `dir < 0` is ArrowUp, `dir > 0` is ArrowDown. Returns `None` (let the
/// browser navigate normally) unless ALL of these hold:
///   * the selection is a collapsed cursor inside an inline-content block,
///   * the cursor sits at the start (up) / end (down) boundary of that block,
///   * an atom node is immediately adjacent in the travel direction, and
///   * there is a text block to land in beyond the atom.
pub fn arrow_over_atom(doc: &Node, sel: &Selection, dir: i32) -> Option<Selection> {
    let cross = atom_cross(doc, sel, dir)?;
    // Pure-model gate: only the block's exact start/end boundary is
    // *guaranteed* to be on the first/last visual line without DOM layout
    // info. The keydown handler additionally crosses from anywhere on the
    // first/last visual line using `block_edge` + DOM rects (#78).
    if sel.head() != cross.block_edge {
        return None;
    }
    Some(cross.target)
}

/// Where a vertical arrow should land when it crosses an adjacent atom
/// block, plus the model position of the current block's near edge
/// (`block_edge` — the block's start for ArrowUp, end for ArrowDown).
///
/// The caller decides *whether* to cross: the caret must be on the first
/// (up) / last (down) visual line of its block, which it confirms by
/// comparing the caret's DOM rect top against `block_edge`'s. `block_edge`
/// is always on that edge line, so equal tops ⇒ same line ⇒ cross.
pub struct AtomCross {
    pub target: Selection,
    pub block_edge: usize,
}

/// The boundary-free core of [`arrow_over_atom`]: returns the cross-atom
/// destination when the cursor sits in an inline-content block that has
/// an atom (hr / image / embed) immediately adjacent in the travel
/// direction and a text block beyond it — regardless of where in the
/// block the caret is. The caller gates on the visual line (#78).
pub fn atom_cross(doc: &Node, sel: &Selection, dir: i32) -> Option<AtomCross> {
    if !sel.empty() {
        return None;
    }
    let pos = sel.head();
    let rp = resolve(doc, pos)?;
    if !is_inline_content_node(rp.node_at(rp.depth, doc)) {
        return None;
    }

    let is_atom = |n: &Node| n.node_type().map(|t| t.is_atom()).unwrap_or(false);

    if dir < 0 {
        let block_edge = rp.start(rp.depth); // block's open boundary
        let before = block_edge.checked_sub(1)?;
        let brp = resolve(doc, before)?;
        if !brp.node_before(doc).is_some_and(is_atom) {
            return None;
        }
        let target = Selection::find_from(doc, before, -1)?;
        Some(AtomCross { target, block_edge })
    } else {
        let block_edge = rp.end(rp.depth, doc); // block's close boundary
        let after = block_edge + 1;
        let arp = resolve(doc, after)?;
        if !arp.node_after(doc).is_some_and(is_atom) {
            return None;
        }
        let target = Selection::find_from(doc, after, 1)?;
        Some(AtomCross { target, block_edge })
    }
}

/// #78: when the caret is at the very start of an inline-content block
/// whose immediately-previous sibling is an atom (horizontal rule, image,
/// embed), return that atom's `[from, to)` position range so Backspace
/// can delete just the atom. `join_backward` would instead skip the atom
/// and merge into the text block beyond it, leaving the rule floating.
/// Returns None unless the caret is a collapsed cursor exactly at the
/// block's content start with an atom directly before it.
pub fn atom_before_cursor_block(doc: &Node, sel: &Selection) -> Option<(usize, usize)> {
    if !sel.empty() {
        return None;
    }
    let pos = sel.head();
    let rp = resolve(doc, pos)?;
    if !is_inline_content_node(rp.node_at(rp.depth, doc)) {
        return None;
    }
    if pos != rp.start(rp.depth) {
        return None; // only at the block's content start
    }
    let before = rp.start(rp.depth).checked_sub(1)?;
    let brp = resolve(doc, before)?;
    let prev = brp.node_before(doc)?;
    if !prev.node_type().map(|t| t.is_atom()).unwrap_or(false) {
        return None;
    }
    let from = before.checked_sub(prev.node_size())?;
    Some((from, before))
}

/// Forward-delete mirror of [`atom_before_cursor_block`]: when the caret
/// is at the very end of an inline-content block whose immediately-next
/// sibling is an atom (horizontal rule, image, embed), return that atom's
/// `[from, to)` position range so Delete can remove just the atom.
/// `join_forward` can't target it (an atom is not a textblock, so
/// `find_block_at` past it returns None and the join fails) and a raw
/// `delete(pos, pos + 1)` deletes the block's close boundary instead,
/// leaving the atom undeletable from the block before it. Returns None
/// unless the caret is a collapsed cursor exactly at the block's content
/// end with an atom directly after it.
pub fn atom_after_cursor_block(doc: &Node, sel: &Selection) -> Option<(usize, usize)> {
    if !sel.empty() {
        return None;
    }
    let pos = sel.head();
    let rp = resolve(doc, pos)?;
    if !is_inline_content_node(rp.node_at(rp.depth, doc)) {
        return None;
    }
    if pos != rp.end(rp.depth, doc) {
        return None; // only at the block's content end
    }
    let after = pos + 1; // step over the block's close boundary
    let arp = resolve(doc, after)?;
    let next = arp.node_after(doc)?;
    if !next.node_type().map(|t| t.is_atom()).unwrap_or(false) {
        return None;
    }
    Some((after, after + next.node_size()))
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

    // ── arrow_over_atom (#78) ──

    fn doc_with_hr() -> Node {
        // doc > [para("Hi"), hr, para("Lo")]
        // positions: 1..3 "Hi", 3 end-Hi, 4 after-para1, 5 after-hr,
        //            6 start-Lo, 8 end-Lo
        Node::element_with_content(
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
        )
    }

    #[test]
    fn arrow_up_skips_hr_to_prev_block_end() {
        let doc = doc_with_hr();
        // Cursor at start of "Lo" (pos 6), ArrowUp → end of "Hi" (pos 3).
        let sel = arrow_over_atom(&doc, &Selection::cursor(6), -1).unwrap();
        assert_eq!(sel.head(), 3);
        assert!(sel.empty());
    }

    #[test]
    fn arrow_down_skips_hr_to_next_block_start() {
        let doc = doc_with_hr();
        // Cursor at end of "Hi" (pos 3), ArrowDown → start of "Lo" (pos 6).
        let sel = arrow_over_atom(&doc, &Selection::cursor(3), 1).unwrap();
        assert_eq!(sel.head(), 6);
        assert!(sel.empty());
    }

    #[test]
    fn arrow_up_not_at_block_start_is_none() {
        let doc = doc_with_hr();
        // Cursor mid-"Lo" (pos 7) — let the browser move within the block.
        assert!(arrow_over_atom(&doc, &Selection::cursor(7), -1).is_none());
    }

    #[test]
    fn arrow_down_not_at_block_end_is_none() {
        let doc = doc_with_hr();
        // Cursor at start of "Lo" (pos 6) is not the end — browser handles it.
        assert!(arrow_over_atom(&doc, &Selection::cursor(6), 1).is_none());
    }

    #[test]
    fn arrow_up_without_adjacent_atom_is_none() {
        // No HR between paragraphs — native nav already works, don't intercept.
        let doc = simple_doc();
        // Cursor at start of "World" (pos 8); block before is a paragraph.
        assert!(arrow_over_atom(&doc, &Selection::cursor(8), -1).is_none());
    }

    #[test]
    fn arrow_down_with_no_text_block_beyond_atom_is_none() {
        // doc > [para("Hi"), hr] — nothing to land on below the rule.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::HorizontalRule),
            ]),
        );
        // Cursor at end of "Hi" (pos 3): hr is after, but no text beyond it.
        assert!(arrow_over_atom(&doc, &Selection::cursor(3), 1).is_none());
    }

    #[test]
    fn arrow_up_with_no_text_block_beyond_atom_is_none() {
        // doc > [hr, para("Lo")] — nothing to land on above the rule.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        );
        // Cursor at start of "Lo" (pos 2): hr is before, but no text above it.
        assert!(arrow_over_atom(&doc, &Selection::cursor(2), -1).is_none());
    }

    #[test]
    fn arrow_over_atom_ignores_range_selection() {
        let doc = doc_with_hr();
        // A non-collapsed selection is never redirected.
        assert!(arrow_over_atom(&doc, &Selection::text(6, 8), -1).is_none());
    }

    // ── atom_cross: boundary-free core (#78) ──
    // The keydown handler gates these on the DOM visual line; the model
    // function itself fires from anywhere in the block.

    #[test]
    fn atom_cross_up_mid_block_targets_prev_block_end() {
        let doc = doc_with_hr();
        // Cursor mid-"Lo" (pos 7) — arrow_over_atom returns None here, but
        // atom_cross still resolves the cross target + the block's start.
        let cross = atom_cross(&doc, &Selection::cursor(7), -1).unwrap();
        assert_eq!(cross.target.head(), 3); // end of "Hi"
        assert_eq!(cross.block_edge, 6); // start of "Lo"
        assert!(arrow_over_atom(&doc, &Selection::cursor(7), -1).is_none());
    }

    #[test]
    fn atom_cross_down_mid_block_targets_next_block_start() {
        let doc = doc_with_hr();
        // Cursor mid-"Hi" (pos 2).
        let cross = atom_cross(&doc, &Selection::cursor(2), 1).unwrap();
        assert_eq!(cross.target.head(), 6); // start of "Lo"
        assert_eq!(cross.block_edge, 3); // end of "Hi"
    }

    #[test]
    fn atom_cross_without_adjacent_atom_is_none() {
        // No HR between paragraphs — nothing to cross.
        let doc = simple_doc();
        assert!(atom_cross(&doc, &Selection::cursor(9), -1).is_none());
    }

    #[test]
    fn atom_cross_ignores_range_selection() {
        let doc = doc_with_hr();
        assert!(atom_cross(&doc, &Selection::text(6, 8), -1).is_none());
    }

    // ── atom_before_cursor_block: Backspace over a rule (#78) ──

    #[test]
    fn atom_before_cursor_block_at_block_start_returns_hr_range() {
        let doc = doc_with_hr();
        // Caret at start of "Lo" (pos 6); the HR occupies [4, 5).
        assert_eq!(
            atom_before_cursor_block(&doc, &Selection::cursor(6)),
            Some((4, 5))
        );
    }

    #[test]
    fn atom_before_cursor_block_mid_block_is_none() {
        let doc = doc_with_hr();
        // Mid-"Lo" (pos 7) — Backspace deletes a char, not the rule.
        assert!(atom_before_cursor_block(&doc, &Selection::cursor(7)).is_none());
    }

    #[test]
    fn atom_before_cursor_block_without_atom_is_none() {
        // Two adjacent paragraphs, no rule between — let join_backward run.
        let doc = simple_doc();
        // Start of "World" (pos 8); previous sibling is a paragraph.
        assert!(atom_before_cursor_block(&doc, &Selection::cursor(8)).is_none());
    }

    #[test]
    fn atom_before_cursor_block_ignores_range() {
        let doc = doc_with_hr();
        assert!(atom_before_cursor_block(&doc, &Selection::text(6, 8)).is_none());
    }

    // ── atom_after_cursor_block: forward-delete over an atom ──

    fn doc_with_embed() -> Node {
        // doc > [para("Hi"), embed, para("Lo")] — same position layout
        // as doc_with_hr; the embed (YouTube/Vimeo) is a size-1 leaf atom.
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                ),
                Node::element(NodeType::Embed),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Lo")]),
                ),
            ]),
        )
    }

    #[test]
    fn atom_after_cursor_block_at_block_end_returns_embed_range() {
        let doc = doc_with_embed();
        // Caret at end of "Hi" (pos 3); the embed occupies [4, 5).
        assert_eq!(
            atom_after_cursor_block(&doc, &Selection::cursor(3)),
            Some((4, 5))
        );
    }

    #[test]
    fn atom_after_cursor_block_returns_hr_range() {
        let doc = doc_with_hr();
        // Caret at end of "Hi" (pos 3); the HR occupies [4, 5).
        assert_eq!(
            atom_after_cursor_block(&doc, &Selection::cursor(3)),
            Some((4, 5))
        );
    }

    #[test]
    fn atom_after_cursor_block_mid_block_is_none() {
        let doc = doc_with_embed();
        // Mid-"Hi" (pos 2) — Delete removes a char, not the embed.
        assert!(atom_after_cursor_block(&doc, &Selection::cursor(2)).is_none());
    }

    #[test]
    fn atom_after_cursor_block_without_atom_is_none() {
        // Two adjacent paragraphs, no atom between — let join_forward run.
        let doc = simple_doc();
        // End of "Hello" (pos 6); next sibling is a paragraph.
        assert!(atom_after_cursor_block(&doc, &Selection::cursor(6)).is_none());
    }

    #[test]
    fn atom_after_cursor_block_ignores_range() {
        let doc = doc_with_embed();
        assert!(atom_after_cursor_block(&doc, &Selection::text(1, 3)).is_none());
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
