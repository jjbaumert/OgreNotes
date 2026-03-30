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

    /// Extract the selected content as a Slice for clipboard operations.
    /// Strips empty boundary artifacts that can appear when the browser selection
    /// lands on a block boundary (e.g., an empty Heading captured at the edge).
    pub fn selected_slice(&self) -> Slice {
        let from = self.selection.from();
        let to = self.selection.to();
        if from == to {
            return Slice::empty();
        }
        let slice = self.doc.slice(from, to);
        // Filter out empty non-leaf blocks at the edges of the slice
        let trimmed: Vec<Node> = slice
            .content
            .children
            .into_iter()
            .filter(|node| match node {
                Node::Text { text, .. } => !text.is_empty(),
                Node::Element { node_type, .. } => {
                    node_type.is_leaf() || !node.text_content().is_empty()
                }
            })
            .collect();
        if trimmed.is_empty() {
            return Slice::empty();
        }
        Slice::new(Fragment::from(trimmed), 0, 0)
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
    /// When the selection spans multiple blocks, merges the remaining content
    /// of the first and last blocks into a single block.
    /// For cross-item selections within a list, operates at the item level.
    pub fn delete_selection(self) -> Result<Self, StepError> {
        let from = self.selection.from();
        let to = self.selection.to();
        if from == to {
            return Ok(self);
        }

        // Check if from and to are in different blocks
        let from_block = find_block_at(&self.doc, from);
        let to_block = find_block_at(&self.doc, to);

        if let (Some(fb), Some(tb)) = (&from_block, &to_block) {
            if fb.offset != tb.offset {
                // Cross-block selection. Check if both blocks are in list items.
                let from_item = find_item_at(&self.doc, from);
                let to_item = find_item_at(&self.doc, to);

                if let (Some(fi), Some(ti)) = (&from_item, &to_item) {
                    if fi.offset != ti.offset {
                        // Cross-item selection: merge at the item level.
                        // Keep content before selection in first item,
                        // content after selection in last item, merge into one item.
                        let before_offset = from - fb.content_start;
                        let after_offset = to - tb.content_start;
                        let before_content = fb.content.cut(0, before_offset);
                        let after_content = tb.content.cut(after_offset, tb.content.size());
                        let merged_content = before_content.append_fragment(after_content);
                        let merged_para = Node::Element {
                            node_type: fb.node_type,
                            attrs: fb.attrs.clone(),
                            content: merged_content,
                            marks: vec![],
                        };
                        let merged_item = Node::element_with_content(
                            fi.node_type,
                            Fragment::from(vec![merged_para]),
                        );

                        let replace_from = fi.offset;
                        let replace_to = ti.offset + ti.node_size;
                        let slice = Slice::new(Fragment::from(vec![merged_item]), 0, 0);
                        let mut txn = self.replace(replace_from, replace_to, slice)?;
                        txn.selection = Selection::cursor(from);
                        return Ok(txn);
                    }
                }

                // Cross-block but not cross-item (or not in a list):
                // merge the two blocks directly
                let before_offset = from - fb.content_start;
                let after_offset = to - tb.content_start;
                let before_content = fb.content.cut(0, before_offset);
                let after_content = tb.content.cut(after_offset, tb.content.size());
                let merged_content = before_content.append_fragment(after_content);
                let merged = Node::Element {
                    node_type: fb.node_type,
                    attrs: fb.attrs.clone(),
                    content: merged_content,
                    marks: vec![],
                };

                let replace_from = fb.offset;
                let replace_to = tb.offset + tb.node_size;
                let slice = Slice::new(Fragment::from(vec![merged]), 0, 0);
                let mut txn = self.replace(replace_from, replace_to, slice)?;
                txn.selection = Selection::cursor(from);
                return Ok(txn);
            }
        }

        // Same block or couldn't find blocks: simple delete
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

        // Check if we're inside a list item — if so, handle empty items specially
        if let Some(item) = find_item_at(&txn.doc, pos) {
            // Check if the current list item is empty (paragraph with no text)
            let item_text: String = item.content.children.iter()
                .map(|c| c.text_content())
                .collect();
            let item_is_empty = item_text.trim().is_empty();

            if item_is_empty {
                // Empty list item on Enter: dedent if nested, exit list if top-level.
                let container = find_container_at(&txn.doc, pos);
                let is_nested = container.as_ref().map_or(false, |c| {
                    find_item_at(&txn.doc, c.offset).is_some()
                });

                if is_nested {
                    // Nested empty item: dedent (same as Shift-Tab)
                    return txn.lift_from_list();
                }

                // Top-level empty item: remove from list, insert empty paragraph after list.
                if let Some(ref c) = container {
                    let is_first = item.offset == c.offset + 1;
                    let is_only = item.node_size == c.node_size - 2; // list open + close

                    if is_only {
                        // Only item in list: replace entire list with empty paragraph
                        let para = Node::element(NodeType::Paragraph);
                        let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
                        let mut result = txn.replace(c.offset, c.offset + c.node_size, slice)?;
                        result.selection = Selection::cursor(c.offset + 1);
                        result.stored_marks = None;
                        return Ok(result);
                    }

                    // Remove this item from the list, add empty paragraph after the list
                    let item_end = item.offset + item.node_size;
                    let list_end = c.offset + c.node_size;

                    // Delete the empty item
                    let mut result = txn.delete(item.offset, item_end)?;

                    // After deletion, the list end shifted. Insert paragraph after the list.
                    let new_list_end = list_end - item.node_size;
                    let para = Node::element(NodeType::Paragraph);
                    let para_slice = Slice::new(Fragment::from(vec![para]), 0, 0);
                    result = result.replace(new_list_end, new_list_end, para_slice)?;
                    result.selection = Selection::cursor(new_list_end + 1);
                    result.stored_marks = None;
                    return Ok(result);
                }
            }

            let first_para = Node::Element {
                node_type: block.node_type,
                attrs: block.attrs.clone(),
                content: before_content,
                marks: vec![],
            };
            let second_para =
                Node::element_with_content(NodeType::Paragraph, after_content);

            // Find the index of the split paragraph within the list item's children
            let mut para_index = 0;
            let mut child_offset = item.content_start;
            for (i, child) in item.content.children.iter().enumerate() {
                if child_offset == block.offset {
                    para_index = i;
                    break;
                }
                child_offset += child.node_size();
            }

            // First item: children before the split paragraph + first half
            let mut first_children: Vec<Node> =
                item.content.children[..para_index].to_vec();
            first_children.push(first_para);
            let first_item = Node::Element {
                node_type: item.node_type,
                attrs: item.attrs.clone(),
                content: Fragment::from(first_children),
                marks: vec![],
            };
            let first_item_size = first_item.node_size();

            // Second item: second half + children after the split paragraph
            let mut second_children = vec![second_para];
            second_children
                .extend(item.content.children[para_index + 1..].iter().cloned());
            let second_item = Node::element_with_content(
                item.node_type,
                Fragment::from(second_children),
            );

            let slice =
                Slice::new(Fragment::from(vec![first_item, second_item]), 0, 0);
            let mut txn =
                txn.replace(item.offset, item.offset + item.node_size, slice)?;
            // Cursor inside second item's first paragraph content:
            // item.offset + first_item_size + 1 (item open) + 1 (para open)
            txn.selection =
                Selection::cursor(item.offset + first_item_size + 2);
            txn.stored_marks = None;
            Ok(txn)
        } else {
            // Not in a list item — split at the paragraph level
            let first = Node::Element {
                node_type: block.node_type,
                attrs: block.attrs,
                content: before_content,
                marks: vec![],
            };
            let first_size = first.node_size();
            let second =
                Node::element_with_content(NodeType::Paragraph, after_content);

            let slice = Slice::new(Fragment::from(vec![first, second]), 0, 0);
            let from = block.offset;
            let end = block.offset + block.node_size;

            let mut txn = txn.replace(from, end, slice)?;
            txn.selection = Selection::cursor(from + first_size + 1);
            txn.stored_marks = None;
            Ok(txn)
        }
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

        // Find the previous sibling textblock by searching just before this block.
        // Try block.offset - 1 first (adjacent textblock), then block.offset - 2
        // (handles the case where the previous element is a container like a list —
        // offset-1 is the container's close boundary, offset-2 is inside its content).
        if block.offset == 0 {
            return Err(StepError("no previous block to join with".into()));
        }
        let prev = find_block_at(&self.doc, block.offset - 1)
            .or_else(|| {
                if block.offset >= 2 {
                    find_block_at(&self.doc, block.offset - 2)
                } else {
                    None
                }
            })
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

    /// Lift the current list item out of its list.
    /// Used when backspace is pressed at the start of a list item and
    /// there's no previous textblock to join with.
    ///
    /// Behavior:
    /// - If the item is the first (or only) item and the list is at the doc level:
    ///   unwrap it to a plain paragraph before the remaining list.
    /// - If the item is not the first item: join its content with the previous item.
    pub fn lift_from_list(self) -> Result<Self, StepError> {
        let pos = self.selection.from();

        // Must be at the start of a block inside a list item
        let block = find_block_at(&self.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;
        if pos != block.content_start {
            return Err(StepError("cursor not at block start".into()));
        }

        let item = find_item_at(&self.doc, pos)
            .ok_or_else(|| StepError("not in a list item".into()))?;
        let container = find_container_at(&self.doc, pos)
            .ok_or_else(|| StepError("not in a container".into()))?;

        // Check if this is the first item in the list by position
        let is_first_item = item.offset == container.offset + 1;

        if is_first_item {
            // First item: unwrap from the list.
            // Extract the paragraph content out as a standalone paragraph
            // and keep the remaining items (if any) in the list.
            let para = Node::element_with_content(
                NodeType::Paragraph,
                block.content.clone(),
            );

            // Check if there are remaining items after this one in the list
            let item_end = item.offset + item.node_size;
            let container_content_end = container.offset + container.node_size - 1;
            let has_remaining = item_end < container_content_end;

            if has_remaining {
                // Replace just this item with the paragraph; keep remaining items in the list
                // Strategy: replace from container start through this item's end
                // with the paragraph + a new list containing remaining items
                // Simpler: replace just the first item with a paragraph BEFORE the list,
                // and remove the item from inside the list.
                //
                // Replace the range [container.offset .. item_end] with just the paragraph.
                // This removes the list open + first item, leaving remaining items orphaned.
                // Instead, replace the whole list and rebuild:
                // [paragraph, list(remaining_items)]
                //
                // We can't easily extract remaining items without the list content.
                // Use doc.slice to get remaining list content.
                let remaining_slice = self.doc.slice(item_end, container_content_end);
                let remaining_list = Node::element_with_content(
                    container.node_type,
                    remaining_slice.content,
                );
                let replacement = vec![para, remaining_list];
                let slice = Slice::new(Fragment::from(replacement), 0, 0);
                let mut txn = self.replace(
                    container.offset,
                    container.offset + container.node_size,
                    slice,
                )?;
                txn.selection = Selection::cursor(container.offset + 1);
                Ok(txn)
            } else {
                // Only item: replace the entire list with just the paragraph
                let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
                let mut txn = self.replace(
                    container.offset,
                    container.offset + container.node_size,
                    slice,
                )?;
                txn.selection = Selection::cursor(container.offset + 1);
                Ok(txn)
            }
        } else {
            // Non-first item: merge current item's content into the previous item.
            // Find previous item by walking the list's children directly.
            let list_slice = self.doc.slice(container.offset, container.offset + container.node_size);
            let list_node = list_slice.content.children.first()
                .ok_or_else(|| StepError("expected list element".into()))?;

            // Find previous item index and offset
            let mut prev_item_idx = 0;
            let mut child_offset = container.offset + 1;
            for i in 0..list_node.child_count() {
                let child = list_node.child(i)
                    .ok_or_else(|| StepError("missing list child".into()))?;
                if child_offset == item.offset {
                    break;
                }
                prev_item_idx = i;
                child_offset += child.node_size();
            }

            let prev_item_node = list_node.child(prev_item_idx)
                .ok_or_else(|| StepError("no previous item".into()))?;
            let prev_item_offset = item.offset - prev_item_node.node_size();

            // Merge: take prev item's children, append current block content to last paragraph
            let mut merged_children: Vec<Node> = Vec::new();
            for i in 0..prev_item_node.child_count() {
                if let Some(child) = prev_item_node.child(i) {
                    merged_children.push(child.clone());
                }
            }

            if let Some(last) = merged_children.last_mut() {
                if let Node::Element { content: last_content, .. } = last {
                    let merged = last_content.clone().append_fragment(block.content.clone());
                    *last_content = merged;
                }
            }

            // If current item has additional children beyond the first paragraph, append them
            for i in 1..item.content.children.len() {
                merged_children.push(item.content.children[i].clone());
            }

            // Cursor: end of the original prev item's last paragraph content.
            // prev_item_offset + 1 (item open) + original children sizes - 1 (last para close)
            // = prev_item_offset + prev_item_node.node_size() - 2
            let cursor_pos = prev_item_offset + prev_item_node.node_size() - 2;

            let merged_item = Node::element_with_content(
                item.node_type,
                Fragment::from(merged_children),
            );

            let slice = Slice::new(Fragment::from(vec![merged_item]), 0, 0);
            let mut txn = self.replace(prev_item_offset, item.offset + item.node_size, slice)?;
            txn.selection = Selection::cursor(cursor_pos);
            Ok(txn)
        }
    }

    /// Join the current block with the next block.
    /// Used when delete forward is pressed at the end of a block.
    /// Merges the next block's content into the end of the current block.
    pub fn join_forward(self) -> Result<Self, StepError> {
        let pos = self.selection.from();
        let block = find_block_at(&self.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        let block_content_end = block.offset + block.node_size - 1;

        // Only join if cursor is at the very end of the block's content
        if pos != block_content_end {
            return Err(StepError("cursor not at block end".into()));
        }

        // Find the next sibling textblock by searching just after this block
        let after_block = block.offset + block.node_size;
        let next = find_block_at(&self.doc, after_block + 1)
            .ok_or_else(|| StepError("no next textblock".into()))?;

        // Merge: replace both blocks with one block (current type) containing combined content
        let merged_content = block.content.append_fragment(next.content);
        let merged = Node::Element {
            node_type: block.node_type,
            attrs: block.attrs,
            content: merged_content,
            marks: vec![],
        };
        let cursor_pos = pos; // cursor stays where it was

        let from = block.offset;
        let end = next.offset + next.node_size;
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

    /// Delete the word before the cursor (Ctrl+Backspace).
    /// Scans backward from cursor to find word boundary, then deletes the range.
    pub fn delete_word_backward(self) -> Result<Self, StepError> {
        let pos = self.selection.from();
        let to = self.selection.to();

        // If there's a selection, just delete it
        if pos != to {
            return self.delete(pos, to);
        }

        let block = find_block_at(&self.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        let text: String = block.content.children.iter().map(|c| c.text_content()).collect();
        let offset = pos - block.content_start;
        let chars: Vec<char> = text.chars().collect();

        if offset == 0 {
            // At start of block — try joining backward instead
            return self.join_backward();
        }

        // Scan backward: skip whitespace, then skip word chars
        let mut i = offset;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }

        let delete_from = block.content_start + i;
        self.delete(delete_from, pos)
    }

    /// Delete the word after the cursor (Ctrl+Delete).
    /// Scans forward from cursor to find word boundary, then deletes the range.
    pub fn delete_word_forward(self) -> Result<Self, StepError> {
        let pos = self.selection.from();
        let to = self.selection.to();

        // If there's a selection, just delete it
        if pos != to {
            return self.delete(pos, to);
        }

        let block = find_block_at(&self.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        let text: String = block.content.children.iter().map(|c| c.text_content()).collect();
        let offset = pos - block.content_start;
        let chars: Vec<char> = text.chars().collect();
        let len = chars.len();

        if offset >= len {
            // At end of block — try joining forward instead
            return self.join_forward();
        }

        // Scan forward: skip word chars, then skip whitespace
        let mut i = offset;
        while i < len && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < len && chars[i].is_whitespace() {
            i += 1;
        }

        let delete_to = block.content_start + i;
        self.delete(pos, delete_to)
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

pub(crate) struct BlockInfo {
    pub offset: usize,
    pub node_size: usize,
    pub content_start: usize,
    pub node_type: NodeType,
    pub attrs: HashMap<String, String>,
    pub content: Fragment,
}

/// Info about a container node (list or blockquote) that wraps a textblock.
pub(crate) struct ContainerInfo {
    /// Offset of the container in its parent's content (absolute position).
    pub offset: usize,
    /// Total node size of the container.
    pub node_size: usize,
    /// The container's node type.
    pub node_type: NodeType,
}

/// Find the innermost text-containing block node at the given position.
/// Recurses into container blocks (lists, blockquote) to find the
/// Paragraph/Heading/CodeBlock that actually holds inline content.
pub(crate) fn find_block_at(doc: &Node, pos: usize) -> Option<BlockInfo> {
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

/// Find the list item (ListItem or TaskItem) that contains the position, if any.
pub(crate) fn find_item_at(doc: &Node, pos: usize) -> Option<BlockInfo> {
    let Node::Element { content, .. } = doc else {
        return None;
    };
    find_item_in_children(&content.children, pos, 0)
}

fn find_item_in_children(
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
                    if matches!(node_type, NodeType::ListItem | NodeType::TaskItem) {
                        // Check for a deeper nested item first
                        if let Some(inner) = find_item_in_children(
                            &child_content.children,
                            pos,
                            content_start,
                        ) {
                            return Some(inner);
                        }
                        // No deeper item — this is the innermost
                        return Some(BlockInfo {
                            offset,
                            node_size: child_size,
                            content_start,
                            node_type: *node_type,
                            attrs: attrs.clone(),
                            content: child_content.clone(),
                        });
                    }
                    // Recurse deeper
                    return find_item_in_children(
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

/// Find the nearest container (list or blockquote) ancestor at a given position.
/// Searches through the doc tree to find if the textblock at `pos` is inside
/// a container like BulletList, OrderedList, TaskList, or Blockquote.
pub(crate) fn find_container_at(doc: &Node, pos: usize) -> Option<ContainerInfo> {
    let Node::Element { content, .. } = doc else {
        return None;
    };
    find_container_in_children(&content.children, pos, 0)
}

fn find_container_in_children(
    children: &[Node],
    pos: usize,
    mut offset: usize,
) -> Option<ContainerInfo> {
    for child in children {
        let child_size = child.node_size();
        if let Node::Element {
            node_type,
            content: child_content,
            ..
        } = child
        {
            if !node_type.is_leaf() {
                let content_start = offset + 1;
                let content_end = offset + child_size - 1;
                if pos >= content_start && pos <= content_end {
                    if is_container_type(*node_type) {
                        // Found a container. Check if there's a deeper container inside.
                        if let Some(inner) = find_container_in_children(
                            &child_content.children,
                            pos,
                            content_start,
                        ) {
                            return Some(inner);
                        }
                        // No deeper container -- this is the innermost one.
                        return Some(ContainerInfo {
                            offset,
                            node_size: child_size,
                            node_type: *node_type,
                        });
                    }
                    // Not a container -- recurse to find one deeper
                    return find_container_in_children(
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

/// Find the innermost container of a specific type at a given position.
/// Unlike `find_container_at` which returns the innermost container of ANY type,
/// this searches for the innermost container matching `target_type` specifically.
/// This correctly handles nested containers like Blockquote[BulletList[...]].
pub(crate) fn find_container_of_type(doc: &Node, pos: usize, target_type: NodeType) -> Option<ContainerInfo> {
    let Node::Element { content, .. } = doc else {
        return None;
    };
    find_typed_container_in_children(&content.children, pos, 0, target_type)
}

fn find_typed_container_in_children(
    children: &[Node],
    pos: usize,
    mut offset: usize,
    target_type: NodeType,
) -> Option<ContainerInfo> {
    for child in children {
        let child_size = child.node_size();
        if let Node::Element {
            node_type,
            content: child_content,
            ..
        } = child
        {
            if !node_type.is_leaf() {
                let content_start = offset + 1;
                let content_end = offset + child_size - 1;
                if pos >= content_start && pos <= content_end {
                    // Check if there's a deeper match inside
                    let inner = find_typed_container_in_children(
                        &child_content.children,
                        pos,
                        content_start,
                        target_type,
                    );
                    if inner.is_some() {
                        return inner;
                    }
                    // No deeper match -- check if THIS node matches
                    if *node_type == target_type {
                        return Some(ContainerInfo {
                            offset,
                            node_size: child_size,
                            node_type: *node_type,
                        });
                    }
                    return None; // pos is inside this child but it's not the target type
                }
            }
        }
        offset += child_size;
    }
    None
}

fn is_container_type(nt: NodeType) -> bool {
    matches!(
        nt,
        NodeType::BulletList
            | NodeType::OrderedList
            | NodeType::TaskList
            | NodeType::Blockquote
    )
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
    fn split_block_in_list_creates_two_list_items() {
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
        // The BulletList should now have two ListItems
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.child_count(), 2);
        let item1 = list.child(0).unwrap();
        assert_eq!(item1.node_type(), Some(NodeType::ListItem));
        assert_eq!(item1.text_content(), "He");
        let item2 = list.child(1).unwrap();
        assert_eq!(item2.node_type(), Some(NodeType::ListItem));
        assert_eq!(item2.text_content(), "llo");
        // Cursor should be inside the second item's paragraph
        // item1: offset 1, size = 1 + (1+2+1) + 1 = 6
        // item2: offset 7, content_start 8, para at 8, para content_start 9
        // So cursor at position 9... let's verify:
        // Actually: item1 at offset 1, first_item_size for ListItem("He"):
        //   ListItem open(1) + Paragraph("He")(1+2+1=4) + ListItem close(1) = 6
        // cursor = item.offset + first_item_size + 2 = 1 + 6 + 2 = 9
        assert_eq!(new_state.selection.from(), 9);
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

    // ── join_forward ──

    #[test]
    fn join_forward_merges_paragraphs() {
        let state = EditorState::create_default(two_para_doc());
        // Cursor at end of first paragraph's content
        // para1: offset 0, size 7, content_end = 6
        let state = EditorState {
            selection: Selection::cursor(6),
            ..state
        };
        let txn = state.transaction().join_forward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "HelloWorld");
        // Cursor should stay at position 6
        assert_eq!(new_state.selection.from(), 6);
    }

    #[test]
    fn join_forward_at_last_block_returns_error() {
        let state = EditorState::create_default(simple_doc());
        // "Hello world" = 11 chars, para content_end = 12
        let state = EditorState {
            selection: Selection::cursor(12),
            ..state
        };
        assert!(state.transaction().join_forward().is_err());
    }

    #[test]
    fn join_forward_not_at_block_end_returns_error() {
        let state = EditorState::create_default(two_para_doc());
        let state = EditorState {
            selection: Selection::cursor(3), // middle of first paragraph
            ..state
        };
        assert!(state.transaction().join_forward().is_err());
    }

    // ── paste list items into empty list item ──

    #[test]
    fn replace_empty_list_item_with_items() {
        // doc > BulletList > ListItem > Paragraph(empty)
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                )]),
            )]),
        );
        // BulletList(0) > ListItem(1) > Para(2..3) > empty
        // ListItem: offset=1, node_size=4
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };

        // Pasted items: two ListItems
        let items = vec![
            Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("item1")]),
                )]),
            ),
            Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("item2")]),
                )]),
            ),
        ];
        let item_slice = Slice::new(Fragment::from(items), 0, 0);

        // Replace the empty item (offset 1, size 4) with the two new items
        let txn = state.transaction().replace(1, 5, item_slice).unwrap();
        let new_state = state.apply(txn);

        // Should still have one BulletList with two items
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList), "Should remain a BulletList");
        assert_eq!(list.child_count(), 2, "Should have 2 items");
        assert_eq!(list.child(0).unwrap().text_content(), "item1");
        assert_eq!(list.child(1).unwrap().text_content(), "item2");
    }

    #[test]
    fn replace_empty_item_at_end_of_list() {
        // doc > BulletList > [ListItem("existing"), ListItem(empty)]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("existing")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                    ),
                ]),
            )]),
        );
        // BulletList(0), ListItem1(1) size=12, ListItem2(13) size=4
        // ListItem2: offset=13, content_start=14, Para(14) size=2
        // Cursor in empty para at position 15
        let state = EditorState {
            selection: Selection::cursor(15),
            ..EditorState::create_default(doc)
        };

        let item = find_item_at(&state.doc, 15).unwrap();
        assert_eq!(item.offset, 13);
        assert_eq!(item.node_size, 4);

        // Replace the empty item with two new items
        let items = vec![
            Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("pasted1")]),
                )]),
            ),
            Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("pasted2")]),
                )]),
            ),
        ];
        let item_slice = Slice::new(Fragment::from(items), 0, 0);
        let txn = state.transaction().replace(13, 17, item_slice).unwrap();
        let new_state = state.apply(txn);

        // Should be ONE list with 3 items
        assert_eq!(new_state.doc.child_count(), 1, "Should be one block (BulletList)");
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 3, "Should have 3 items: existing + 2 pasted");
        assert_eq!(list.child(0).unwrap().text_content(), "existing");
        assert_eq!(list.child(1).unwrap().text_content(), "pasted1");
        assert_eq!(list.child(2).unwrap().text_content(), "pasted2");
    }

    // ── lift_from_list ──

    #[test]
    fn lift_sole_item_from_list() {
        // doc > BulletList > ListItem > Paragraph("text")
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("text")]),
                    )]),
                )]),
            )]),
        );
        // Cursor at start of paragraph content (position 3)
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().lift_from_list().unwrap();
        let new_state = state.apply(txn);

        // Should be a plain paragraph, no list
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "text");
    }

    #[test]
    fn lift_first_item_keeps_remaining_list() {
        // doc > BulletList > [ListItem("first"), ListItem("second")]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("first")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("second")]),
                        )]),
                    ),
                ]),
            )]),
        );
        // Cursor at start of "first" (position 3)
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().lift_from_list().unwrap();
        let new_state = state.apply(txn);

        // Should be: Paragraph("first") + BulletList > ListItem("second")
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "first");
        let list = new_state.doc.child(1).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 1);
        assert_eq!(list.child(0).unwrap().text_content(), "second");
    }

    #[test]
    fn backspace_joins_second_list_item_with_first() {
        // doc > BulletList > [ListItem("first"), ListItem("second")]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("first")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("second")]),
                        )]),
                    ),
                ]),
            )]),
        );
        // Cursor at start of "second" content
        // ListItem1(1): size=8, ListItem2(9): Para(10), content_start=11
        // Wait: "first" is 5 chars. Para = 1+5+1=7. ListItem = 1+7+1=9.
        // ListItem1: offset=1, size=9. ListItem2: offset=10, Para at 11, content_start=12.
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().lift_from_list().unwrap();
        let new_state = state.apply(txn);

        // Should join "second" with "first" in one item
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 1);
        assert_eq!(list.child(0).unwrap().text_content(), "firstsecond");
    }

    #[test]
    fn backspace_joins_third_item_with_second() {
        // doc > BulletList > [ListItem("asdf1"), ListItem("asdf2"), ListItem("asdf3")]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf1")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf2")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf3")]),
                        )]),
                    ),
                ]),
            )]),
        );
        // Positions: "asdf1"=5 chars, Para=7, ListItem=9
        // BulletList(0) > ListItem1(1, size=9) > ListItem2(10, size=9) > ListItem3(19, size=9)
        // ListItem3(19) > Para(20) > content_start=21
        let state = EditorState {
            selection: Selection::cursor(21),
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().lift_from_list().unwrap();
        let new_state = state.apply(txn);

        // Should join "asdf3" into "asdf2", leaving 2 items
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 2, "Should have 2 items after join");
        assert_eq!(list.child(0).unwrap().text_content(), "asdf1");
        assert_eq!(list.child(1).unwrap().text_content(), "asdf2asdf3");
    }

    // ── delete_selection across blocks ──

    #[test]
    fn delete_selection_cross_block_merges() {
        let state = EditorState::create_default(two_para_doc());
        // Select from position 3 (after "He") to position 10 (after "Wo")
        let state = EditorState {
            selection: Selection::text(3, 10),
            ..state
        };
        let txn = state.transaction().delete_selection().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1, "Should merge into one paragraph");
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Herld");
        assert_eq!(new_state.selection.from(), 3);
    }

    #[test]
    fn delete_selection_same_block_simple_delete() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = state.transaction().delete_selection().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), " world");
    }

    #[test]
    fn delete_selection_entire_second_block() {
        let state = EditorState::create_default(two_para_doc());
        // Select from end of para1 (position 6) to end of para2 (position 13)
        let state = EditorState {
            selection: Selection::text(6, 13),
            ..state
        };
        let txn = state.transaction().delete_selection().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello");
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
