use std::collections::HashMap;
use std::sync::Arc;

use super::model::{Fragment, Mark, Node, NodeType, Slice};
use super::schema::{default_schema, Schema};
use super::selection::Selection;
use super::transform::{Step, StepError, StepMap};

/// The complete, immutable editor state at a point in time.
/// New states are produced by applying transactions.
#[derive(Debug, Clone)]
pub struct EditorState {
    /// The current document.
    pub doc: Node,
    /// The current selection.
    pub selection: Selection,
    /// Marks to apply to the next typed text (set by toggle commands).
    pub stored_marks: Option<Vec<Mark>>,
    /// The document schema (shared via Arc -- never cloned per transaction).
    pub schema: Arc<Schema>,
    // TODO: plugins: Vec<Box<dyn Plugin>> -- see design/rich-text-editor.md §5
}

impl EditorState {
    /// Create an initial editor state from a document and schema.
    /// Places the cursor at the first valid text position.
    pub fn create(doc: Node, schema: Schema) -> Self {
        let selection =
            Selection::find_from(&doc, 0, 1).unwrap_or_else(|| Selection::cursor(0));

        Self {
            doc,
            selection,
            stored_marks: None,
            schema: Arc::new(schema),
        }
    }

    /// Create an initial state with the default schema.
    pub fn create_default(doc: Node) -> Self {
        Self::create(doc, default_schema())
    }

    /// Create an initial state with an empty document and default schema.
    pub fn empty() -> Self {
        Self::create_default(Node::empty_doc())
    }

    /// Apply a transaction to produce a new state.
    pub fn apply(&self, txn: Transaction) -> Self {
        Self {
            doc: txn.doc,
            selection: txn.selection,
            stored_marks: txn.stored_marks,
            schema: Arc::clone(&self.schema),
        }
    }

    /// Start building a transaction from this state.
    pub fn transaction(&self) -> Transaction {
        Transaction::new(self)
    }
}

/// A transaction describes a state change.
/// It accumulates steps that modify the document, and tracks
/// selection and stored mark changes.
#[derive(Debug, Clone)]
pub struct Transaction {
    /// The document after all steps have been applied.
    pub doc: Node,
    /// Steps applied in this transaction.
    pub steps: Vec<Step>,
    /// Step maps for position mapping.
    pub maps: Vec<StepMap>,
    /// The new selection (mapped through steps).
    pub selection: Selection,
    /// Stored marks override (None = no change).
    pub stored_marks: Option<Vec<Mark>>,
    /// Whether the document was modified.
    pub doc_changed: bool,
    /// Whether to scroll the selection into view.
    pub scroll_into_view: bool,
    /// Arbitrary metadata.
    pub meta: HashMap<String, String>,
}

impl Transaction {
    /// Create a new transaction from an editor state.
    fn new(state: &EditorState) -> Self {
        Self {
            doc: state.doc.clone(),
            steps: Vec::new(),
            maps: Vec::new(),
            selection: state.selection.clone(),
            stored_marks: state.stored_marks.clone(),
            doc_changed: false,
            scroll_into_view: false,
            meta: HashMap::new(),
        }
    }

    /// Apply a step to the transaction's document.
    pub fn step(mut self, step: Step) -> Result<Self, StepError> {
        let (new_doc, map) = step.apply(&self.doc)?;
        self.selection = self.selection.map(&map);
        self.doc = new_doc;
        self.steps.push(step);
        self.maps.push(map);
        self.doc_changed = true;
        Ok(self)
    }

    /// Insert content at a position.
    pub fn insert(self, pos: usize, content: Fragment) -> Result<Self, StepError> {
        self.step(Step::Replace {
            from: pos,
            to: pos,
            slice: Slice::new(content, 0, 0),
        })
    }

    /// Delete content between two positions.
    pub fn delete(self, from: usize, to: usize) -> Result<Self, StepError> {
        if from == to {
            return Ok(self);
        }
        self.step(Step::Replace {
            from,
            to,
            slice: Slice::empty(),
        })
    }

    /// Replace content between two positions with a slice.
    pub fn replace(self, from: usize, to: usize, slice: Slice) -> Result<Self, StepError> {
        self.step(Step::Replace { from, to, slice })
    }

    /// Replace the current selection with a slice.
    pub fn replace_selection(self, slice: Slice) -> Result<Self, StepError> {
        let from = self.selection.from();
        let to = self.selection.to();
        self.replace(from, to, slice)
    }

    /// Delete the current selection.
    pub fn delete_selection(self) -> Result<Self, StepError> {
        let from = self.selection.from();
        let to = self.selection.to();
        self.delete(from, to)
    }

    /// Insert text at the current cursor position, replacing any selection.
    /// Applies stored marks to the inserted text if set.
    /// No-ops on empty text.
    pub fn insert_text(self, text: &str) -> Result<Self, StepError> {
        if text.is_empty() {
            return Ok(self);
        }

        let from = self.selection.from();
        let to = self.selection.to();

        // Build text node with appropriate marks:
        // - Some(marks): use explicitly set stored marks (from toggle commands)
        // - None: inherit marks from the text at the cursor position
        let text_node = if let Some(ref marks) = self.stored_marks {
            if marks.is_empty() {
                Node::text(text)
            } else {
                Node::text_with_marks(text, marks.clone())
            }
        } else {
            let inherited = super::commands::marks_at_pos(&self.doc, from);
            if inherited.is_empty() {
                Node::text(text)
            } else {
                Node::text_with_marks(text, inherited)
            }
        };

        let content = Fragment::from(vec![text_node]);
        let content_size = content.size();

        // Replace selection with the text content
        let mut txn = self.replace(from, to, Slice::new(content, 0, 0))?;

        // Place cursor at end of inserted text using mapped position
        let cursor_pos = txn.map_pos(from, 1).min(from + content_size);
        txn.selection = Selection::cursor(cursor_pos);
        txn.stored_marks = None; // consumed

        Ok(txn)
    }

    /// Add a mark to text in a range.
    pub fn add_mark(self, from: usize, to: usize, mark: Mark) -> Result<Self, StepError> {
        self.step(Step::AddMark { from, to, mark })
    }

    /// Remove a mark from text in a range.
    pub fn remove_mark(self, from: usize, to: usize, mark: Mark) -> Result<Self, StepError> {
        self.step(Step::RemoveMark { from, to, mark })
    }

    /// Change the type of a block node at position `pos`.
    pub fn set_node_type(
        self,
        pos: usize,
        node_type: NodeType,
        attrs: HashMap<String, String>,
    ) -> Result<Self, StepError> {
        self.step(Step::SetNodeType { pos, node_type, attrs })
    }

    /// Split the block at the current cursor position.
    /// Creates a new paragraph after the current block, moving any content
    /// after the cursor into it.
    pub fn split_block(self) -> Result<Self, StepError> {
        let pos = self.selection.from();
        let to = self.selection.to();

        // If there's a selection, delete it first
        let txn = if pos != to {
            self.delete(pos, to)?
        } else {
            self
        };

        let pos = txn.selection.from();
        let block = find_block_at(&txn.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        let inner_pos = pos - block.content_start;
        let before_content = block.content.cut(0, inner_pos);
        let after_content = block.content.cut(inner_pos, block.content.size());

        let first = Node::Element {
            node_type: block.node_type,
            attrs: block.attrs,
            content: before_content,
            marks: vec![],
        };
        let first_size = first.node_size();
        let second = Node::element_with_content(NodeType::Paragraph, after_content);

        let slice = Slice::new(Fragment::from(vec![first, second]), 0, 0);
        let from = block.offset;
        let end = block.offset + block.node_size;

        let mut txn = txn.replace(from, end, slice)?;
        txn.selection = Selection::cursor(from + first_size + 1);
        txn.stored_marks = None;
        Ok(txn)
    }

    /// Join the current block with the previous block.
    /// Used when backspace is pressed at the start of a block.
    /// Merges the current block's content into the end of the previous block.
    pub fn join_backward(self) -> Result<Self, StepError> {
        let pos = self.selection.from();
        let block = find_block_at(&self.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        // Only join if cursor is at the very start of the block's content
        if pos != block.content_start {
            return Err(StepError("cursor not at block start".into()));
        }

        // Find the previous sibling textblock by searching just before this block
        if block.offset == 0 {
            return Err(StepError("no previous block to join with".into()));
        }
        let prev = find_block_at(&self.doc, block.offset - 1)
            .ok_or_else(|| StepError("no previous textblock".into()))?;

        // Merge: replace both blocks with one block (prev type) containing combined content
        let merged_content = prev.content.append_fragment(block.content);
        let merged = Node::Element {
            node_type: prev.node_type,
            attrs: prev.attrs,
            content: merged_content,
            marks: vec![],
        };
        let cursor_pos = prev.offset + prev.node_size - 1; // end of prev's original content

        let from = prev.offset;
        let end = block.offset + block.node_size;
        let slice = Slice::new(Fragment::from(vec![merged]), 0, 0);

        let mut txn = self.replace(from, end, slice)?;
        txn.selection = Selection::cursor(cursor_pos);
        Ok(txn)
    }

    /// Set the selection explicitly.
    pub fn set_selection(mut self, selection: Selection) -> Self {
        self.selection = selection;
        self
    }

    /// Set stored marks (marks to apply to next input).
    pub fn set_stored_marks(mut self, marks: Option<Vec<Mark>>) -> Self {
        self.stored_marks = marks;
        self
    }

    /// Set metadata on the transaction.
    pub fn set_meta(mut self, key: &str, value: &str) -> Self {
        self.meta.insert(key.to_string(), value.to_string());
        self
    }

    /// Mark that the view should scroll the selection into view.
    pub fn scroll_into_view(mut self) -> Self {
        self.scroll_into_view = true;
        self
    }

    /// Map a position through all steps in this transaction.
    pub fn map_pos(&self, pos: usize, bias: i32) -> usize {
        let mut result = pos;
        for map in &self.maps {
            result = map.map(result, bias);
        }
        result
    }
}

// ─── Helpers ───────────────────────────────────────────────────

struct BlockInfo {
    offset: usize,
    node_size: usize,
    content_start: usize,
    node_type: NodeType,
    attrs: HashMap<String, String>,
    content: Fragment,
}

/// Find the innermost text-containing block node at the given position.
/// Recurses into container blocks (lists, blockquote) to find the
/// Paragraph/Heading/CodeBlock that actually holds inline content.
fn find_block_at(doc: &Node, pos: usize) -> Option<BlockInfo> {
    let Node::Element { content, .. } = doc else {
        return None;
    };
    find_block_in_children(&content.children, pos, 0)
}

fn find_block_in_children(
    children: &[Node],
    pos: usize,
    mut offset: usize,
) -> Option<BlockInfo> {
    for child in children {
        let child_size = child.node_size();
        if let Node::Element {
            node_type,
            attrs,
            content: child_content,
            ..
        } = child
        {
            if !node_type.is_leaf() {
                let content_start = offset + 1;
                let content_end = offset + child_size - 1;
                if pos >= content_start && pos <= content_end {
                    if node_type.is_textblock() {
                        return Some(BlockInfo {
                            offset,
                            node_size: child_size,
                            content_start,
                            node_type: *node_type,
                            attrs: attrs.clone(),
                            content: child_content.clone(),
                        });
                    }
                    // Container block — recurse into its children
                    return find_block_in_children(
                        &child_content.children,
                        pos,
                        content_start,
                    );
                }
            }
        }
        offset += child_size;
    }
    None
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{MarkType, NodeType};

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    fn two_para_doc() -> Node {
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

    // ── State creation ──

    #[test]
    fn create_empty_state() {
        let state = EditorState::empty();
        assert_eq!(state.doc, Node::empty_doc());
        assert_eq!(state.selection.head(), 1);
        assert!(state.selection.empty());
        assert!(state.stored_marks.is_none());
    }

    #[test]
    fn create_state_with_doc() {
        let state = EditorState::create_default(simple_doc());
        assert_eq!(state.selection.head(), 1);
        assert!(state.selection.empty());
    }

    #[test]
    fn create_state_with_two_paras() {
        let state = EditorState::create_default(two_para_doc());
        assert_eq!(state.selection.head(), 1);
    }

    // ── Transaction basics ──

    #[test]
    fn transaction_insert_text() {
        let state = EditorState::create_default(simple_doc());
        let txn = state.transaction().insert_text("Hi ").unwrap();

        assert!(txn.doc_changed);
        assert_eq!(txn.steps.len(), 1);

        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Hi Hello world");
        assert_eq!(new_state.selection.head(), 4);
    }

    #[test]
    fn transaction_insert_text_empty_is_noop() {
        let state = EditorState::create_default(simple_doc());
        let stored = Some(vec![Mark::new(MarkType::Bold)]);
        let state_with_marks = EditorState {
            stored_marks: stored.clone(),
            ..state
        };

        let txn = state_with_marks.transaction().insert_text("").unwrap();
        assert!(!txn.doc_changed);
        assert_eq!(txn.steps.len(), 0);
        // Stored marks should NOT be consumed on empty insert
        assert!(txn.stored_marks.is_some());
    }

    #[test]
    fn transaction_insert_text_replaces_selection() {
        let state = EditorState::create_default(simple_doc());
        // Select "Hello" (1..6)
        let state_with_sel = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let txn = state_with_sel.transaction().insert_text("Goodbye").unwrap();
        let new_state = state_with_sel.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Goodbye world");
        // Cursor should be after "Goodbye" (1 + 7 = 8)
        assert_eq!(new_state.selection.head(), 8);
    }

    #[test]
    fn transaction_delete_selection() {
        let state = EditorState::create_default(simple_doc());
        let state_with_sel = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let txn = state_with_sel.transaction().delete_selection().unwrap();
        let new_state = state_with_sel.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), " world");
    }

    #[test]
    fn transaction_replace_selection() {
        let state = EditorState::create_default(simple_doc());
        let state_with_sel = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let slice = Slice::new(Fragment::from(vec![Node::text("Goodbye")]), 0, 0);
        let txn = state_with_sel.transaction().replace_selection(slice).unwrap();
        let new_state = state_with_sel.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Goodbye world");
    }

    #[test]
    fn transaction_no_change_on_empty_delete() {
        let state = EditorState::create_default(simple_doc());
        let state_with_cursor = EditorState {
            selection: Selection::cursor(3),
            ..state
        };

        let txn = state_with_cursor.transaction().delete_selection().unwrap();
        assert!(!txn.doc_changed);
    }

    // ── Marks ──

    #[test]
    fn transaction_add_mark() {
        let state = EditorState::create_default(simple_doc());
        let txn = state
            .transaction()
            .add_mark(1, 6, Mark::new(MarkType::Bold))
            .unwrap();

        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.child_count(), 2);
        assert!(para
            .child(0)
            .unwrap()
            .marks()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn transaction_stored_marks_applied() {
        let state = EditorState::create_default(simple_doc());
        let txn = state
            .transaction()
            .set_stored_marks(Some(vec![Mark::new(MarkType::Bold)]))
            .insert_text("X")
            .unwrap();

        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "X");
        assert!(first
            .marks()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
        assert!(new_state.stored_marks.is_none());
    }

    #[test]
    fn transaction_stored_marks_empty_vec() {
        let state = EditorState::create_default(simple_doc());
        // Empty stored marks vec = insert plain text (no marks)
        let txn = state
            .transaction()
            .set_stored_marks(Some(vec![]))
            .insert_text("X")
            .unwrap();

        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // "X" merges with "Hello world" since both have no marks
        assert_eq!(para.text_content(), "XHello world");
        // First child has no marks
        assert!(para.child(0).unwrap().marks().is_empty());
    }

    #[test]
    fn insert_text_inherits_marks_from_cursor_position() {
        // Doc with bold "Hello" + plain " world"
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text_with_marks("Hello", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" world"),
                ]),
            )]),
        );
        let state = EditorState::create_default(doc);
        // Cursor at position 3 (inside "Hello", which is bold)
        let state = EditorState {
            selection: Selection::cursor(3),
            ..state
        };

        // Insert "X" with no stored marks — should inherit bold
        let txn = state.transaction().insert_text("X").unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // "HeXllo" should all be bold
        let first = para.child(0).unwrap();
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
        assert!(first.text_content().contains('X'));
    }

    #[test]
    fn insert_text_no_marks_in_plain_text() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(3),
            ..state
        };
        let txn = state.transaction().insert_text("X").unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // All text should be plain
        assert!(para.child(0).unwrap().marks().is_empty());
    }

    // ── Selection tracking ──

    #[test]
    fn transaction_selection_maps_through_insert() {
        let state = EditorState::create_default(simple_doc());
        let state_at_6 = EditorState {
            selection: Selection::cursor(6),
            ..state
        };

        let txn = state_at_6
            .transaction()
            .insert(1, Fragment::from(vec![Node::text("XY")]))
            .unwrap();

        assert_eq!(txn.selection.head(), 8);
    }

    #[test]
    fn transaction_set_selection() {
        let state = EditorState::create_default(simple_doc());
        let txn = state.transaction().set_selection(Selection::cursor(5));
        let new_state = state.apply(txn);
        assert_eq!(new_state.selection.head(), 5);
    }

    // ── Chaining ──

    #[test]
    fn transaction_chain_multiple_steps() {
        let state = EditorState::create_default(simple_doc());
        let txn = state
            .transaction()
            .delete(1, 7)
            .unwrap()
            .insert(1, Fragment::from(vec![Node::text("Goodbye ")]))
            .unwrap();

        assert_eq!(txn.steps.len(), 2);
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Goodbye world");
    }

    // ── Metadata ──

    #[test]
    fn transaction_metadata() {
        let state = EditorState::create_default(simple_doc());
        let txn = state
            .transaction()
            .set_meta("source", "keyboard")
            .set_meta("key", "a");

        assert_eq!(txn.meta.get("source").unwrap(), "keyboard");
        assert_eq!(txn.meta.get("key").unwrap(), "a");
    }

    // ── Map position ──

    #[test]
    fn transaction_map_pos() {
        let state = EditorState::create_default(simple_doc());
        let txn = state
            .transaction()
            .insert(1, Fragment::from(vec![Node::text("ABC")]))
            .unwrap();

        assert_eq!(txn.map_pos(1, 1), 4);
        assert_eq!(txn.map_pos(5, 1), 8);
        assert_eq!(txn.map_pos(0, 1), 0);
    }

    // ── Scroll into view ──

    #[test]
    fn transaction_scroll_into_view() {
        let state = EditorState::create_default(simple_doc());
        let txn = state.transaction().scroll_into_view();
        assert!(txn.scroll_into_view);
    }

    // ── Schema sharing ──

    #[test]
    fn schema_shared_via_arc() {
        let state = EditorState::create_default(simple_doc());
        let txn = state.transaction().insert_text("X").unwrap();
        let new_state = state.apply(txn);
        // Schema is shared, not cloned
        assert!(Arc::ptr_eq(&state.schema, &new_state.schema));
    }

    // ── split_block ──

    #[test]
    fn split_block_clears_stored_marks() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(6),
            stored_marks: Some(vec![Mark::new(MarkType::Bold)]),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert!(new_state.stored_marks.is_none());
    }

    #[test]
    fn split_block_at_end_creates_empty_paragraph() {
        let state = EditorState::create_default(simple_doc());
        // Cursor at end of "Hello world" (position 12)
        let state = EditorState {
            selection: Selection::cursor(12),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello world");
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "");
    }

    #[test]
    fn split_block_mid_text() {
        let state = EditorState::create_default(simple_doc());
        // Cursor after "Hello" (position 6)
        let state = EditorState {
            selection: Selection::cursor(6),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello");
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), " world");
    }

    // ── find_block_at ──

    #[test]
    fn find_block_at_flat_paragraph() {
        let doc = simple_doc();
        let block = find_block_at(&doc, 3).unwrap();
        assert_eq!(block.node_type, NodeType::Paragraph);
        assert_eq!(block.offset, 0);
        assert_eq!(block.content_start, 1);
    }

    #[test]
    fn find_block_at_in_list() {
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
        // Positions: doc content[0] = BulletList open
        //   BulletList content[1] = ListItem open
        //     ListItem content[2] = Paragraph open
        //       Paragraph content starts at 3
        //       "item" occupies positions 3..7
        let block = find_block_at(&doc, 4).unwrap();
        assert_eq!(block.node_type, NodeType::Paragraph);
        assert_eq!(block.content_start, 3);
    }

    #[test]
    fn find_block_at_in_blockquote() {
        // doc > blockquote > paragraph("quoted")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("quoted")]),
                )]),
            )]),
        );
        // Blockquote open at 0, content_start at 1
        // Paragraph open at 1, content_start at 2
        let block = find_block_at(&doc, 3).unwrap();
        assert_eq!(block.node_type, NodeType::Paragraph);
        assert_eq!(block.content_start, 2);
    }

    #[test]
    fn split_block_in_list_produces_two_paragraphs() {
        // doc > bulletList > listItem > paragraph("Hello")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("Hello")]),
                    )]),
                )]),
            )]),
        );
        // Paragraph content starts at position 3, "Hello" at 3..8
        // Cursor at position 5 (after "He")
        let state = EditorState::create_default(doc);
        let state = EditorState {
            selection: Selection::cursor(5),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        // The ListItem should now have two paragraphs
        let list = new_state.doc.child(0).unwrap();
        let item = list.child(0).unwrap();
        assert_eq!(item.child_count(), 2);
        assert_eq!(item.child(0).unwrap().text_content(), "He");
        assert_eq!(item.child(1).unwrap().text_content(), "llo");
    }

    // ── join_backward ──

    #[test]
    fn join_backward_merges_paragraphs() {
        let state = EditorState::create_default(two_para_doc());
        // Cursor at start of second paragraph's content
        // para1: offset 0, size 7 (open + "Hello" + close), content_start 1
        // para2: offset 7, size 7, content_start 8
        let state = EditorState {
            selection: Selection::cursor(8),
            ..state
        };
        let txn = state.transaction().join_backward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "HelloWorld");
        // Cursor should be where the join happened (after "Hello")
        assert_eq!(new_state.selection.from(), 6);
    }

    #[test]
    fn join_backward_at_first_block_returns_error() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(1), // start of first paragraph content
            ..state
        };
        assert!(state.transaction().join_backward().is_err());
    }

    #[test]
    fn join_backward_not_at_block_start_returns_error() {
        let state = EditorState::create_default(two_para_doc());
        let state = EditorState {
            selection: Selection::cursor(9), // middle of second paragraph
            ..state
        };
        assert!(state.transaction().join_backward().is_err());
    }

    #[test]
    fn split_block_on_empty_paragraph() {
        // Empty doc has one empty paragraph, cursor at position 1
        let state = EditorState::empty();
        assert_eq!(state.selection.from(), 1);
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
    }
}
