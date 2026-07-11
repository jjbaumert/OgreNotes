// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::HashMap;
use std::sync::Arc;

use super::model::{Fragment, Mark, Node, NodeType, Slice};
use super::schema::{default_schema, Schema};
use super::selection::Selection;
use super::transform::{Step, StepError, StepMap};

/// Start of the line containing char-index `pos` within `chars` — the
/// index just past the nearest preceding `'\n'`, or 0 on the first
/// line. Shared by `split_block`'s code-block auto-indent and
/// triple-Enter escape, and by `commands::dedent_code_block_line`
/// (Shift-Tab): all answer "where does the caret's line start" against
/// a char-indexed code-block text buffer and must agree.
pub(crate) fn line_start_at(chars: &[char], pos: usize) -> usize {
    chars[..pos]
        .iter()
        .rposition(|&c| c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Indent unit for a code block's `language` attr tag —
/// `DEFAULT_INDENT_UNIT` when the tag is absent/empty/unresolved.
/// Shared by `split_block`'s auto-indent and
/// `commands::code_block_indent_unit` (Tab/Shift-Tab): Enter and Tab
/// must insert the same string for the same block.
pub(crate) fn indent_unit_for_language_tag(tag: Option<&str>) -> &'static str {
    tag.and_then(ogrenotes_highlight::Language::from_tag)
        .map(|l| l.indent_unit())
        .unwrap_or(ogrenotes_highlight::DEFAULT_INDENT_UNIT)
}

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
    /// Does a cheap top-level structural check: if any direct child of Doc is
    /// a bare Text node (from a corrupted undo), normalizes the document.
    /// Also ensures the selection is inside a valid textblock after normalization.
    pub fn apply(&self, txn: Transaction) -> Self {
        let doc = if needs_normalize(&txn.doc) {
            super::model::normalize_doc(&txn.doc)
        } else {
            txn.doc
        };
        // If the cursor (empty selection) is outside any textblock (e.g., at
        // position 0 after Ctrl+A delete + normalize added an empty paragraph),
        // find the nearest valid cursor position. Only for cursors — range
        // selections like select_all intentionally span doc boundaries.
        let selection = if txn.selection.empty()
            && find_block_at(&doc, txn.selection.from()).is_none()
        {
            Selection::find_from(&doc, txn.selection.from(), 1)
                .unwrap_or(txn.selection)
        } else {
            txn.selection
        };
        Self {
            doc,
            selection,
            stored_marks: txn.stored_marks,
            schema: Arc::clone(&self.schema),
        }
    }

    /// Start building a transaction from this state.
    pub fn transaction(&self) -> Transaction {
        Transaction::new(self)
    }
}

/// Cheap check on Doc's direct children and their immediate children.
/// Returns true if the document has structural violations that normalize_doc would fix.
fn needs_normalize(doc: &Node) -> bool {
    let Node::Element { content, node_type, .. } = doc else { return false };
    if *node_type != NodeType::Doc { return false; }
    content.children.iter().any(|child| match child {
        Node::Text { .. } => true, // bare text under Doc
        Node::Element { node_type, content: child_content, .. } => {
            // Orphaned structural nodes under Doc
            matches!(node_type,
                NodeType::ListItem | NodeType::TaskItem
                | NodeType::TableRow | NodeType::TableCell | NodeType::TableHeader
            )
            // Empty lists/tables
            || (matches!(node_type,
                NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList
                | NodeType::Table
            ) && child_content.children.is_empty())
            // Block elements nested inside textblocks (e.g., <p> inside <p>)
            || (node_type.is_textblock() && child_content.children.iter().any(|gc| {
                matches!(gc, Node::Element { node_type: nt, .. } if nt.is_block() && !nt.is_inline())
            }))
        }
    })
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
                // Cursor at the merge point, but clamped to inside the merged block
                // (from=0 on a select-all would be before the paragraph open boundary)
                let cursor = from.max(replace_from + 1);
                txn.selection = Selection::cursor(cursor);
                return Ok(txn);
            }
        }

        // Same block or couldn't find blocks: simple delete.
        // If everything is deleted, ensure the doc has at least one paragraph
        // and the cursor is inside it (not at the doc boundary).
        let mut txn = self.delete(from, to)?;
        if txn.doc.child_count() == 0 || txn.doc.text_content().is_empty() {
            // Normalize: ensure at least one empty paragraph
            txn.doc = super::model::normalize_doc(&txn.doc);
            // Place cursor inside the first paragraph
            if let Some(valid) = Selection::find_from(&txn.doc, 0, 1) {
                txn.selection = valid;
            }
        }
        Ok(txn)
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

        // Clamp position to a valid inside-block range. `content_size`
        // is the "just past the end of the last child" boundary, which
        // is a legitimate model position but NOT inside any block —
        // find_block_at returns None there. Clamp to `content_size - 1`
        // so an end-of-doc caret (typical after typing at the tail
        // of the last block, or an auto-heading input rule) snaps
        // back to the last position inside the last block, which is
        // where the split should happen. `max.saturating_sub(1)` is
        // safe for a wholly empty doc (max = 0) — split_block on that
        // still fails via find_block_at, which is correct.
        let max = txn.doc.content_size();
        let pos = txn.selection.from().min(max.saturating_sub(1));
        let block = find_block_at(&txn.doc, pos)
            .ok_or_else(|| StepError("cursor not in a block".into()))?;

        let inner_pos = pos - block.content_start;
        let before_content = block.content.cut(0, inner_pos);
        let after_content = block.content.cut(inner_pos, block.content.size());

        // Enter inside a code block extends it with a literal newline
        // (design/rich-text-editor.md's `newlineInCode`) instead of
        // splitting — a code block holds text lines, not sibling
        // blocks. Exception: Enter on a trailing empty line (the text
        // ends with '\n' and the caret sits at the very end) exits to
        // a paragraph below — the same double-Enter escape the
        // blockquote and empty-list-item branches below implement.
        if block.node_type == NodeType::CodeBlock {
            let text: String = block
                .content
                .children
                .iter()
                .map(|c| c.text_content())
                .collect();
            let chars: Vec<char> = text.chars().collect();
            let at_end = inner_pos == block.content.size();
            let clamped = inner_pos.min(chars.len());
            let line_start = line_start_at(&chars, clamped);

            // Triple-Enter escape (user-tuned): Enter at the very end
            // of TWO consecutive whitespace-only trailing lines
            // (auto-indent leaves spaces on "empty" lines) strips both
            // and exits to a paragraph below. One empty line is not
            // enough — the second Enter just adds another line, the
            // third breaks free.
            let is_ws = |c: &char| *c == ' ' || *c == '\t';
            if at_end
                && line_start > 0
                && chars[line_start..].iter().all(|c| is_ws(c))
            {
                let prev_line_start = line_start_at(&chars, line_start - 1);
                let prev_line_ws_only =
                    chars[prev_line_start..line_start - 1].iter().all(is_ws);
                // `prev_line_start > 0` requires a THIRD line-start
                // boundary before the two blank trailing lines — i.e.
                // genuinely three Enters' worth of lines. Without it,
                // a code block that started completely empty (toolbar-
                // created, or a fence rule with nothing typed) counts
                // its own pre-existing first line as one blank line and
                // the escape fires on the SECOND Enter, not the third.
                if prev_line_ws_only && prev_line_start > 0 {
                    // Delete from the newline that opens the first
                    // empty line (or from the content start when the
                    // whole block is just the two empty lines).
                    let del_from =
                        block.content_start + prev_line_start.saturating_sub(1);
                    let del_to = block.content_start + chars.len();
                    let removed = del_to - del_from;
                    let mut result = txn.delete(del_from, del_to)?;
                    let block_end = block.offset + block.node_size - removed;
                    let para = Node::element(NodeType::Paragraph);
                    let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
                    result = result.replace(block_end, block_end, slice)?;
                    result.selection = Selection::cursor(block_end + 1);
                    result.stored_marks = None;
                    return Ok(result);
                }
            }

            // newlineInCode with editor-style auto-indent: keep the
            // current line's leading whitespace, plus one language
            // indent unit when the text before the caret ends with a
            // block opener (':' for Python-style suites, '{' for brace
            // languages).
            let indent_unit =
                indent_unit_for_language_tag(block.attrs.get("language").map(String::as_str));
            let current_indent: String = chars[line_start..clamped]
                .iter()
                .take_while(|&&c| c == ' ' || c == '\t')
                .collect();
            let opener = chars[line_start..clamped]
                .iter()
                .rev()
                .find(|c| !c.is_whitespace());
            let mut insert = String::from("\n");
            insert.push_str(&current_indent);
            if matches!(opener, Some(':') | Some('{')) {
                insert.push_str(indent_unit);
            }
            let insert_len = insert.chars().count();
            let nl = Slice::new(Fragment::from(vec![Node::text(&insert)]), 0, 0);
            let mut result = txn.replace(pos, pos, nl)?;
            result.selection = Selection::cursor(pos + insert_len);
            result.stored_marks = None;
            return Ok(result);
        }

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
            // Empty paragraph inside a blockquote: pressing Enter exits the
            // blockquote (so two Enters at the end of a blockquote line
            // revert the trailing empty line back to a plain paragraph).
            let block_is_empty_paragraph =
                block.node_type == NodeType::Paragraph
                    && block.content.size() == 0;
            if block_is_empty_paragraph {
                if let Some(container) = find_container_at(&txn.doc, pos) {
                    if container.node_type == NodeType::Blockquote {
                        let is_only =
                            block.node_size == container.node_size - 2;
                        if is_only {
                            // Only paragraph in blockquote: replace the
                            // entire blockquote with a plain paragraph.
                            let para = Node::element(NodeType::Paragraph);
                            let slice =
                                Slice::new(Fragment::from(vec![para]), 0, 0);
                            let mut result = txn.replace(
                                container.offset,
                                container.offset + container.node_size,
                                slice,
                            )?;
                            result.selection =
                                Selection::cursor(container.offset + 1);
                            result.stored_marks = None;
                            return Ok(result);
                        }
                        // Remove the empty paragraph from the blockquote and
                        // insert a plain paragraph after the blockquote.
                        let block_end = block.offset + block.node_size;
                        let bq_end = container.offset + container.node_size;
                        let mut result = txn.delete(block.offset, block_end)?;
                        let new_bq_end = bq_end - block.node_size;
                        let para = Node::element(NodeType::Paragraph);
                        let para_slice =
                            Slice::new(Fragment::from(vec![para]), 0, 0);
                        result = result.replace(
                            new_bq_end,
                            new_bq_end,
                            para_slice,
                        )?;
                        result.selection =
                            Selection::cursor(new_bq_end + 1);
                        result.stored_marks = None;
                        return Ok(result);
                    }
                }
            }

            // Not in a list item — split at the block level.
            //
            // When the cursor is at the very start of the block, insert a
            // plain empty paragraph above and keep the original block (with
            // its formatting and attrs) below — so pressing Enter at the
            // start of a heading moves it down a line without dropping the
            // heading style. Otherwise keep the original block on top and
            // put the trailing content into a new paragraph below.
            let (first, second) = if inner_pos == 0 {
                let first = Node::element_with_content(
                    NodeType::Paragraph,
                    before_content,
                );
                let second = Node::Element {
                    node_type: block.node_type,
                    attrs: block.attrs,
                    content: after_content,
                    marks: vec![],
                };
                (first, second)
            } else {
                let first = Node::Element {
                    node_type: block.node_type,
                    attrs: block.attrs,
                    content: before_content,
                    marks: vec![],
                };
                let second = Node::element_with_content(
                    NodeType::Paragraph,
                    after_content,
                );
                (first, second)
            };
            let first_size = first.node_size();

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

        // If we're inside a table cell, don't escape — backspace at the start
        // of a cell's first paragraph should be a no-op.
        if let Some(table_info) = find_table_at(&self.doc, pos) {
            if pos == table_info.cell_content_start + 1 {
                // Cursor at start of cell's first paragraph — don't join
                return Err(StepError("at table cell boundary".into()));
            }
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

        // Mirror of join_forward's code-block guard: Backspace at the
        // start of a code block never dissolves it into the previous
        // block. An empty textblock above is removed; a non-empty one
        // just steps the caret out to its end.
        if block.node_type == NodeType::CodeBlock && prev.node_type != NodeType::CodeBlock {
            if prev.content.size() == 0 {
                let mut txn =
                    self.delete(prev.offset, prev.offset + prev.node_size)?;
                txn.selection =
                    Selection::cursor(block.content_start - prev.node_size);
                return Ok(txn);
            }
            let mut txn = self;
            txn.selection = Selection::cursor(prev.offset + prev.node_size - 1);
            return Ok(txn);
        }

        // If prev sits inside a different container than the current block
        // (e.g. prev is the last paragraph of a blockquote and block is a
        // doc-level paragraph after it), a single replace spanning both
        // would also swallow the container's close token and corrupt the
        // doc. Handle that as two steps: append block's inline content to
        // the end of prev's content, then delete the now-empty block.
        let prev_end = prev.offset + prev.node_size;
        let prev_content_end = prev.offset + prev.node_size - 1;
        let cursor_pos = prev_content_end;

        if prev_end == block.offset {
            // Same parent: merge both blocks into one (prev's type/attrs).
            let merged_content = prev.content.append_fragment(block.content);
            let merged = Node::Element {
                node_type: prev.node_type,
                attrs: prev.attrs,
                content: merged_content,
                marks: vec![],
            };
            let from = prev.offset;
            let end = block.offset + block.node_size;
            let slice = Slice::new(Fragment::from(vec![merged]), 0, 0);
            let mut txn = self.replace(from, end, slice)?;
            txn.selection = Selection::cursor(cursor_pos);
            Ok(txn)
        } else {
            let inserted = block.content.size();
            let txn = if inserted > 0 {
                let inline_slice = Slice::new(block.content.clone(), 0, 0);
                self.replace(prev_content_end, prev_content_end, inline_slice)?
            } else {
                self
            };
            let new_block_offset = block.offset + inserted;
            let mut txn = txn.delete(
                new_block_offset,
                new_block_offset + block.node_size,
            )?;
            txn.selection = Selection::cursor(cursor_pos);
            Ok(txn)
        }
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

        // Code blocks are sturdy: forward-delete never dissolves one
        // into the caret's block (2026-07-11 repro — Delete in the
        // empty paragraph the triple-Enter escape leaves behind turned
        // the next code block into plain text). An empty textblock
        // before the code block is simply removed; a non-empty one
        // just steps the caret into the block.
        if next.node_type == NodeType::CodeBlock && block.node_type != NodeType::CodeBlock {
            if block.content.size() == 0 {
                let mut txn =
                    self.delete(block.offset, block.offset + block.node_size)?;
                txn.selection =
                    Selection::cursor(next.content_start - block.node_size);
                return Ok(txn);
            }
            let mut txn = self;
            txn.selection = Selection::cursor(next.content_start);
            return Ok(txn);
        }

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

pub struct BlockInfo {
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
pub fn find_block_at(doc: &Node, pos: usize) -> Option<BlockInfo> {
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

/// Remap a selection made against `old_doc` so it still points to the
/// same logical block-relative position after the document has been
/// replaced by `new_doc` — e.g., a CRDT update arrived from a peer
/// and the editor's model got rebuilt from scratch.
///
/// Without this remap, absolute indices drift by one per character the
/// peer inserts anywhere above the selection: the fresh EditorState
/// keeps the old numeric positions but the content has shifted. Users
/// see their highlight slide sideways as someone types above it.
///
/// Each endpoint is encoded as `(block_id, offset_within_block)` using
/// the OLD doc, then resolved back to an absolute position in the NEW
/// doc. Anchor and head are handled independently so right-to-left
/// selections keep their direction. Endpoints whose block lost its
/// `blockId` attribute or whose block was removed by the remote update
/// fall back to clamp-to-length — the best we can do without more info.
pub fn remap_selection_across_doc_swap(
    old_doc: &Node,
    old_selection: &Selection,
    new_doc: &Node,
) -> Selection {
    let anchor = old_selection.anchor();
    let head = old_selection.head();

    let to_block_relative = |pos: usize| -> Option<(String, u32)> {
        let block = find_block_at(old_doc, pos)?;
        let id = block.attrs.get("blockId")?.clone();
        let off = pos.saturating_sub(block.content_start) as u32;
        Some((id, off))
    };
    let to_absolute = |block_id: &str, off: u32| -> Option<usize> {
        let start = new_doc.find_block_content_start(block_id)?;
        Some(start + off as usize)
    };

    let max = new_doc.content_size();
    let remap = |pos: usize| -> usize {
        to_block_relative(pos)
            .and_then(|(id, off)| to_absolute(&id, off))
            .unwrap_or(pos)
            .min(max)
    };

    let new_anchor = remap(anchor);
    let new_head = remap(head);
    if new_anchor == new_head {
        Selection::cursor(new_anchor)
    } else {
        Selection::text(new_anchor, new_head)
    }
}

/// Remap one **block-relative** anchor `[start, end)` (an inline comment's
/// span within block `block_id`) through a document swap described by
/// `maps`. Returns the new `(start, end)` if it moved and stays valid,
/// else `None` (no change, or the anchor collapsed / its block vanished).
///
/// Block-relative storage means an edit in *another* block is a no-op
/// here; only a length change *before* the offset inside the anchor's own
/// block shifts it. Convert to absolute via the old block's content-start,
/// push through `maps` (start right-biased, end left-biased so a
/// same-position insert doesn't swallow the span), convert back via the
/// new block's content-start. Companion to [`remap_selection_across_doc_swap`];
/// the thread-iterating wrapper is `components::comment_highlights::remap_thread_anchors`.
pub fn remap_block_anchor(
    block_id: &str,
    start: u32,
    end: u32,
    maps: &[crate::editor::transform::StepMap],
    old_doc: &Node,
    new_doc: &Node,
) -> Option<(u32, u32)> {
    let old_content_start = old_doc.find_block_content_start(block_id)?;
    let new_content_start = new_doc.find_block_content_start(block_id)?;

    let mut abs_start = old_content_start + start as usize;
    let mut abs_end = old_content_start + end as usize;
    for map in maps {
        abs_start = map.map(abs_start, 1);
        abs_end = map.map(abs_end, -1);
    }

    let new_start = abs_start.saturating_sub(new_content_start) as u32;
    let new_end = abs_end.saturating_sub(new_content_start) as u32;
    if new_start < new_end && (new_start != start || new_end != end) {
        Some((new_start, new_end))
    } else {
        None
    }
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
            | NodeType::Table
    )
}

/// Info about the table context at a given position.
pub(crate) struct TableInfo {
    pub table_offset: usize,
    pub table_node_size: usize,
    pub row_offset: usize,
    pub row_index: usize,
    pub row_node_size: usize,
    pub cell_offset: usize,
    pub cell_index: usize,
    pub cell_node_size: usize,
    pub cell_content_start: usize,
    pub cell_node_type: NodeType,
    pub num_rows: usize,
    pub num_cols: usize,
}

/// Find the table, row, and cell containing the given position.
pub(crate) fn find_table_at(doc: &Node, pos: usize) -> Option<TableInfo> {
    let Node::Element { content, .. } = doc else { return None };
    find_table_in_children(&content.children, pos, 0)
}

fn find_table_in_children(children: &[Node], pos: usize, mut offset: usize) -> Option<TableInfo> {
    for child in children {
        let child_size = child.node_size();
        let Node::Element { node_type, content: child_content, .. } = child else {
            offset += child_size;
            continue;
        };
        if node_type.is_leaf() {
            offset += child_size;
            continue;
        }
        let content_start = offset + 1;
        let content_end = offset + child_size - 1;
        if pos < content_start || pos > content_end {
            offset += child_size;
            continue;
        }

        if *node_type == NodeType::Table {
            let num_rows = child_content.children.len();
            let mut row_offset = content_start;
            for (row_idx, row) in child_content.children.iter().enumerate() {
                let row_size = row.node_size();
                let row_content_start = row_offset + 1;
                let row_content_end = row_offset + row_size - 1;
                if pos >= row_content_start && pos <= row_content_end {
                    if let Node::Element { content: row_content, .. } = row {
                        let num_cols = row_content.children.len();
                        let mut cell_offset = row_content_start;
                        for (cell_idx, cell) in row_content.children.iter().enumerate() {
                            let cell_size = cell.node_size();
                            let cell_cs = cell_offset + 1;
                            let cell_ce = cell_offset + cell_size - 1;
                            if pos >= cell_cs && pos <= cell_ce {
                                let cell_nt = cell.node_type().unwrap_or(NodeType::TableCell);
                                return Some(TableInfo {
                                    table_offset: offset,
                                    table_node_size: child_size,
                                    row_offset,
                                    row_index: row_idx,
                                    row_node_size: row_size,
                                    cell_offset,
                                    cell_index: cell_idx,
                                    cell_node_size: cell_size,
                                    cell_content_start: cell_cs,
                                    cell_node_type: cell_nt,
                                    num_rows,
                                    num_cols,
                                });
                            }
                            cell_offset += cell_size;
                        }
                    }
                }
                row_offset += row_size;
            }
            return None; // inside table but not inside a cell
        }

        // Not a table — recurse into container
        return find_table_in_children(&child_content.children, pos, content_start);
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
        assert_eq!(state.doc.node_type(), Some(NodeType::Doc));
        assert_eq!(state.doc.child_count(), 1);
        assert_eq!(state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
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

    // ── remap_selection_across_doc_swap (remote-update selection fix) ──

    /// Two paragraphs `<p blockId=b1>Hello</p><p blockId=b2>World</p>`.
    /// Positions: p1 content-start = 1, text "Hello" spans 1..6, p1 ends
    /// at 7. p2 content-start = 8, text "World" spans 8..13.
    fn two_para_doc_with_ids(p1_text: &str, p2_text: &str) -> Node {
        use crate::editor::model::Fragment;
        let mut b1 = std::collections::HashMap::new();
        b1.insert("blockId".to_string(), "b1".to_string());
        let mut b2 = std::collections::HashMap::new();
        b2.insert("blockId".to_string(), "b2".to_string());
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    b1,
                    Fragment::from(vec![Node::text(p1_text)]),
                ),
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    b2,
                    Fragment::from(vec![Node::text(p2_text)]),
                ),
            ]),
        )
    }

    #[test]
    fn remap_selection_survives_insert_in_block_above() {
        // Regression for the reported bug: B types on a line above A's
        // highlight; without remap, A's selection slides by one per
        // character typed. With remap, the block-relative (block_id,
        // offset) coordinates stay identical, so the selection stays
        // anchored to the same logical text in block b2.
        let old_doc = two_para_doc_with_ids("Hello", "World");
        // Select "Wor" inside p2 — absolute 8..11 (p2 content-start is 8).
        let old_sel = Selection::text(8, 11);

        // Remote update: six chars appended to p1 ("Hello" → "Hello there").
        // p2 content-start shifts from 8 to 14, so "Wor" now at 14..17.
        let new_doc = two_para_doc_with_ids("Hello there", "World");

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        assert_eq!(remapped.anchor(), 14);
        assert_eq!(remapped.head(), 17);
    }

    #[test]
    fn remap_selection_preserves_cursor_not_range() {
        let old_doc = two_para_doc_with_ids("Hello", "World");
        let old_sel = Selection::cursor(10); // in p2 at offset 2 (after "Wo")
        let new_doc = two_para_doc_with_ids("Hello!!", "World"); // p1 + "!!"

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        assert!(remapped.empty(), "cursor must stay a cursor, not become a range");
        // p2 content-start shifts from 8 to 10; offset 2 ⇒ absolute 12.
        assert_eq!(remapped.head(), 12);
    }

    #[test]
    fn remap_selection_preserves_anchor_head_direction() {
        // Right-to-left selection: anchor > head. Direction must survive.
        let old_doc = two_para_doc_with_ids("Hello", "World");
        let old_sel = Selection::text(11, 8); // anchor after "Wor", head before "W"
        let new_doc = two_para_doc_with_ids("Hello there", "World");

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        assert_eq!(remapped.anchor(), 17);
        assert_eq!(remapped.head(), 14);
        assert!(remapped.anchor() > remapped.head());
    }

    #[test]
    fn remap_selection_anchor_and_head_in_different_blocks() {
        // Selection spans the paragraph boundary: anchor in p1, head in p2.
        // Each endpoint remaps independently against its own block_id.
        let old_doc = two_para_doc_with_ids("Hello", "World");
        let old_sel = Selection::text(3, 11); // "llo...Wor" across blocks
        let new_doc = two_para_doc_with_ids("Hello there", "World");

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        // p1 unchanged: offset 2 ⇒ absolute 3 still.
        assert_eq!(remapped.anchor(), 3);
        // p2 content-start moved from 8 to 14; offset 3 ⇒ absolute 17.
        assert_eq!(remapped.head(), 17);
    }

    #[test]
    fn remap_selection_falls_back_when_block_missing() {
        // Block was removed by the remote update. We can't preserve the
        // selection's logical position — best we can do is clamp so the
        // cursor stays on screen and doesn't panic.
        let old_doc = two_para_doc_with_ids("Hello", "World");
        let old_sel = Selection::cursor(10); // in p2 "Wo|rld"
        // New doc lacks b2 entirely.
        let mut b1 = std::collections::HashMap::new();
        b1.insert("blockId".to_string(), "b1".to_string());
        let new_doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Paragraph,
                b1,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        // Must clamp and not panic; landing anywhere inside the new doc is fine.
        assert!(remapped.head() <= new_doc.content_size());
    }

    #[test]
    fn remap_selection_falls_back_on_legacy_no_blockid() {
        // Old-format doc with no blockId on paragraphs — `find_block_at`
        // returns the block but there's no id to round-trip. Behaviour
        // degrades to clamp-to-length (the pre-fix path).
        let old_doc = simple_doc(); // no blockIds
        let old_sel = Selection::cursor(5);
        let new_doc = simple_doc();

        let remapped = remap_selection_across_doc_swap(&old_doc, &old_sel, &new_doc);
        assert_eq!(remapped.head(), 5);
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
    fn split_block_in_code_block_inserts_newline() {
        // Enter inside a code block extends it (newlineInCode), never
        // splits it — the user stays in the block on the next line.
        // Updated for auto-indent (2026-07-10, user-requested): the
        // ':' block opener adds one indent unit on the new line.
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "python".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("class PythonClass:")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(19), // end of the text (1 + 18)
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1, "block must not split");
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(block.text_content(), "class PythonClass:\n    ");
        assert_eq!(block.attrs().get("language").unwrap(), "python");
        assert_eq!(new_state.selection.from(), 24, "caret after the indent");
    }

    #[test]
    fn split_block_mid_code_block_inserts_newline_between_lines() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("ab")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(2), // between 'a' and 'b'
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "a\nb");
        assert_eq!(new_state.selection.from(), 3);
    }

    fn python_code_block(text: &str) -> EditorState {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "python".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text(text)]),
            )]),
        );
        let pos = 1 + crate::editor::model::char_len(text);
        EditorState {
            selection: Selection::cursor(pos),
            ..EditorState::create_default(doc)
        }
    }

    #[test]
    fn split_block_after_colon_line_adds_one_indent_unit() {
        // Python block opener: Enter after "class A:" auto-indents the
        // new line by one unit (4 spaces for Python).
        let state = python_code_block("class A:");
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "class A:\n    "
        );
        assert_eq!(new_state.selection.from(), 1 + 13, "caret after the indent");
    }

    #[test]
    fn split_block_preserves_and_extends_nested_indent() {
        // Enter after an indented ":"-line keeps the current indent and
        // adds one more unit: 4 → 8.
        let state = python_code_block("class A:\n    def set_x(self):");
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        let text = new_state.doc.child(0).unwrap().text_content();
        assert!(
            text.ends_with("\n        "),
            "expected 8-space continuation, got {text:?}"
        );
    }

    #[test]
    fn split_block_preserves_indent_without_block_opener() {
        // A plain indented line continues at the same depth.
        let state = python_code_block("    x = 1");
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "    x = 1\n    "
        );
    }

    #[test]
    fn split_block_after_open_brace_adds_indent_unit() {
        // Brace languages: '{' is the block opener.
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "rust".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("fn main() {")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "fn main() {\n    "
        );
    }

    #[test]
    fn split_block_second_enter_adds_another_empty_line() {
        // Triple-Enter escape (user-requested 2026-07-10): ONE
        // whitespace-only trailing line is not enough to exit — the
        // second Enter just adds another empty line at the same
        // indent.
        let state = python_code_block("class A:\n    ");
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1, "must NOT exit yet");
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "class A:\n    \n    "
        );
    }

    #[test]
    fn split_block_from_empty_code_block_takes_three_enters_to_exit() {
        // Review finding (2026-07-10): a block that never had typed
        // content must not count its own pre-existing first line as a
        // user-typed blank — the escape still takes three Enters.
        let mut state = python_code_block("");
        for enters in 1..=2 {
            let txn = state.transaction().split_block().unwrap();
            state = state.apply(txn);
            assert_eq!(state.doc.child_count(), 1, "Enter {enters} must not exit");
        }
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child_count(), 2, "Enter 3 must exit");
        assert_eq!(
            state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    #[test]
    fn split_block_third_enter_exits_stripping_both_empty_lines() {
        // Two whitespace-only trailing lines + Enter = break free; both
        // empty lines are stripped on the way out.
        let state = python_code_block("class A:\n    \n    ");
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(
            block.text_content(),
            "class A:",
            "both whitespace lines stripped"
        );
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    #[test]
    fn typing_python_class_body_auto_indents_like_an_editor() {
        // End-to-end typing sequence (user acceptance case):
        //   ```python → class A: ⏎ def set_x(self): ⏎ self.x=3
        // Enter never leaves the block; each ':' line indents one more
        // unit; the result reads like an editor laid it out.
        let mut state = python_code_block("");
        for (i, keys) in ["class A:", "def set_x(self):", "self.x=3"]
            .iter()
            .enumerate()
        {
            if i > 0 {
                let txn = state.transaction().split_block().unwrap();
                state = state.apply(txn);
            }
            let txn = state.transaction().insert_text(keys).unwrap();
            state = state.apply(txn);
        }
        assert_eq!(state.doc.child_count(), 1, "still one code block");
        assert_eq!(
            state.doc.child(0).unwrap().text_content(),
            "class A:\n    def set_x(self):\n        self.x=3"
        );
    }

    #[test]
    fn split_block_on_code_block_trailing_empty_line_exits() {
        // Triple-Enter escape (updated 2026-07-10): exit fires on the
        // Enter pressed at the end of TWO empty trailing lines, and
        // removes both on the way out.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("code\n\n")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(7), // after both '\n' (1 + 6)
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(block.text_content(), "code", "both trailing newlines removed");
        let para = new_state.doc.child(1).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(para.text_content(), "");
        // caret inside the new paragraph: block(6 chars → hmm computed) —
        // assert via containment instead of a magic number:
        let caret = new_state.selection.from();
        let block0_end = 1 + new_state.doc.child(0).unwrap().node_size();
        assert!(caret > block0_end - 1, "caret must be past the code block");
    }

    #[test]
    fn split_block_in_code_block_with_selection_replaces_with_newline() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("aXXb")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::text(2, 4), // the "XX"
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "a\nb");
    }

    #[test]
    fn split_block_at_start_of_heading_preserves_formatting() {
        // doc > heading("Hello")
        // Pressing Enter at the start of a heading should leave an empty
        // paragraph above and keep the heading (with its content) below.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let state = EditorState::create_default(doc);
        // Heading content_start is 1 (after the heading open token)
        let state = EditorState {
            selection: Selection::cursor(1),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(
            new_state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Heading)
        );
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "Hello");
        // Cursor stays with the content (start of heading content, now at pos 3)
        assert_eq!(new_state.selection.from(), 3);
    }

    #[test]
    fn split_block_at_end_of_heading_creates_paragraph_below() {
        // Sanity: pressing Enter at the END of a heading still creates a
        // plain paragraph below (the existing well-known behavior).
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let state = EditorState::create_default(doc);
        // End of "Hello" inside heading: content_start (1) + 5
        let state = EditorState {
            selection: Selection::cursor(6),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(
            new_state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Heading)
        );
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "");
    }

    /// Regression: pressing Enter with the caret at `doc.content_size`
    /// (past the last block's close boundary) used to fail with
    /// "cursor not in a block" because `find_block_at` correctly
    /// reports None for that position — no block *contains* it. This
    /// happened in practice when the `# ` input rule converted the
    /// initial paragraph into a Heading whose caret then ended up
    /// one position past the heading's inside-content-end. See the
    /// 2026-07-04 `[editor:enter] split_block failed` report on doc
    /// `TD36n0qmHXgh_wjb1nSbQ`. `split_block` now clamps to
    /// `content_size - 1` so the "past the end" caret snaps to the
    /// last position inside the last block, which is what the user
    /// intended.
    #[test]
    fn split_block_at_content_size_snaps_into_last_block() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let state = EditorState::create_default(doc);
        // Caret at content_size (7 = heading node_size = 2 + 5). This
        // is *past* the heading's content_end (which is 6). Before the
        // fix, split_block would return Err("cursor not in a block").
        let state = EditorState {
            selection: Selection::cursor(state.doc.content_size()),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(
            new_state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Heading)
        );
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "");
    }

    #[test]
    fn split_block_twice_at_end_of_blockquote_exits_to_paragraph() {
        // doc > blockquote > paragraph("Hi")
        // First Enter at end of "Hi" leaves an empty paragraph inside the
        // blockquote. The second Enter on that empty paragraph should
        // remove it from the blockquote and insert a plain paragraph after.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                )]),
            )]),
        );
        // Positions: blockquote open at 0, para open at 1,
        // "Hi" at 2..4, para close at 4, bq close at 5
        // End of "Hi" content is position 4
        let state = EditorState::create_default(doc);
        let state = EditorState {
            selection: Selection::cursor(4),
            ..state
        };
        // First Enter: split inside the blockquote
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        // blockquote should now contain two paragraphs
        let bq = state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        assert_eq!(bq.child_count(), 2);
        assert_eq!(bq.child(0).unwrap().text_content(), "Hi");
        assert_eq!(bq.child(1).unwrap().text_content(), "");

        // Second Enter on the empty paragraph: should exit the blockquote
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child_count(), 2);
        let bq = state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        assert_eq!(bq.child_count(), 1);
        assert_eq!(bq.child(0).unwrap().text_content(), "Hi");
        let trailing = state.doc.child(1).unwrap();
        assert_eq!(trailing.node_type(), Some(NodeType::Paragraph));
        assert_eq!(trailing.text_content(), "");
    }

    #[test]
    fn split_block_on_only_empty_paragraph_in_blockquote_unwraps() {
        // doc > blockquote > paragraph("")
        // Pressing Enter on the only (empty) paragraph in a blockquote should
        // replace the whole blockquote with a plain paragraph.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element(NodeType::Paragraph)]),
            )]),
        );
        // Cursor at start of empty paragraph content (position 2)
        let state = EditorState::create_default(doc);
        let state = EditorState {
            selection: Selection::cursor(2),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child_count(), 1);
        assert_eq!(
            state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(state.doc.child(0).unwrap().text_content(), "");
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
    fn backspace_at_block_start_over_hr_deletes_only_the_rule() {
        // doc > [para("Hi"), hr, para("Lo")]; caret at start of "Lo" (pos 6).
        // #78: deleting the atom range from atom_before_cursor_block must
        // remove the rule and leave both paragraphs intact — not merge them.
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
        let state = EditorState {
            selection: Selection::cursor(6),
            ..EditorState::create_default(doc)
        };
        let (from, to) = crate::editor::selection::atom_before_cursor_block(
            &state.doc, &state.selection,
        )
        .unwrap();
        let txn = state.transaction().delete(from, to).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hi");
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "Lo");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    #[test]
    fn forward_delete_at_block_end_over_embed_deletes_only_the_embed() {
        // doc > [para("Hi"), embed, para("Lo")]; caret at end of "Hi" (pos 3).
        // Forward-delete over a following atom (the YouTube embed) must remove
        // just the embed and leave both paragraphs intact — not merge them and
        // not leave the embed floating. `join_forward` can't reach the atom (an
        // embed is not a textblock) and delete(pos, pos+1) hits the block-close
        // boundary, so `atom_after_cursor_block` supplies the atom's range.
        let doc = Node::element_with_content(
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
        );
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };
        // join_forward must fail here (the next sibling is an atom, not a
        // textblock) — that's why the atom-after special case is needed.
        assert!(state.transaction().join_forward().is_err());
        let (from, to) = crate::editor::selection::atom_after_cursor_block(
            &state.doc, &state.selection,
        )
        .unwrap();
        let txn = state.transaction().delete(from, to).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hi");
        assert_eq!(new_state.doc.child(1).unwrap().text_content(), "Lo");
        assert_eq!(
            new_state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    #[test]
    fn join_backward_after_exit_blockquote_preserves_container() {
        // Reproduces the bug where pressing Enter twice at the end of a
        // blockquote (which exits the blockquote) and then backspace
        // corrupted the doc by replacing across the blockquote's close
        // token.  Backspace on the doc-level empty paragraph after the
        // blockquote should pull the cursor back into the last paragraph
        // of the blockquote without destroying the blockquote.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hi")]),
                )]),
            )]),
        );
        let state = EditorState::create_default(doc);
        // Cursor at end of "Hi" inside the blockquote (pos 4).
        let state = EditorState {
            selection: Selection::cursor(4),
            ..state
        };
        // Enter, Enter — second Enter exits the blockquote.
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        // Doc should now be: Blockquote[Para["Hi"]], Para[""]
        assert_eq!(state.doc.child_count(), 2);
        assert_eq!(
            state.doc.child(0).unwrap().node_type(),
            Some(NodeType::Blockquote),
        );
        assert_eq!(
            state.doc.child(1).unwrap().node_type(),
            Some(NodeType::Paragraph),
        );

        // Backspace at the start of the trailing empty paragraph.
        let txn = state.transaction().join_backward().unwrap();
        let state = state.apply(txn);

        // Blockquote must still be intact, with its single paragraph "Hi".
        assert_eq!(state.doc.child_count(), 1);
        let bq = state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        assert_eq!(bq.child_count(), 1);
        assert_eq!(bq.child(0).unwrap().text_content(), "Hi");
        // Cursor is at the end of "Hi" inside the blockquote.
        assert_eq!(state.selection.from(), 4);
    }

    #[test]
    fn join_backward_with_content_into_blockquote_merges_inline() {
        // Sister to the previous test: if the trailing paragraph has
        // content (not just empty), backspace should pull that content
        // into the last paragraph of the blockquote.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Blockquote,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("Hi")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("foo")]),
                ),
            ]),
        );
        let state = EditorState::create_default(doc);
        // Cursor at start of "foo" content. Blockquote spans 0..6 (open,
        // para open, "Hi", para close, bq close), trailing para opens at 6,
        // its content_start is 7.
        let state = EditorState {
            selection: Selection::cursor(7),
            ..state
        };
        let txn = state.transaction().join_backward().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child_count(), 1);
        let bq = state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        assert_eq!(bq.child_count(), 1);
        assert_eq!(bq.child(0).unwrap().text_content(), "Hifoo");
        // Cursor should land at the boundary between "Hi" and "foo".
        assert_eq!(state.selection.from(), 4);
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

    fn para_and_code_doc(para_text: &str, code_text: &str) -> Node {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "python".to_string());
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    if para_text.is_empty() {
                        Fragment::from(vec![])
                    } else {
                        Fragment::from(vec![Node::text(para_text)])
                    },
                ),
                Node::element_with_attrs(
                    NodeType::CodeBlock,
                    attrs,
                    Fragment::from(vec![Node::text(code_text)]),
                ),
            ]),
        )
    }

    #[test]
    fn join_forward_before_code_block_removes_empty_paragraph_not_the_block() {
        // Bug repro (2026-07-11): Delete in the empty paragraph between
        // blocks dissolved the code block into plain text. The block is
        // sturdy: the empty paragraph goes, the block and its text stay,
        // the caret lands at the block's content start.
        let state = EditorState {
            selection: Selection::cursor(1), // inside the empty paragraph
            ..EditorState::create_default(para_and_code_doc("", "code"))
        };
        let txn = state.transaction().join_forward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock), "block survives");
        assert_eq!(block.text_content(), "code", "text intact");
        assert_eq!(block.attrs().get("language").unwrap(), "python");
        assert_eq!(new_state.selection.from(), 1, "caret at code content start");
    }

    #[test]
    fn join_forward_before_code_block_with_text_steps_into_block() {
        // Non-empty paragraph before the block: nothing merges; the
        // caret just steps into the block.
        let state = EditorState {
            selection: Selection::cursor(3), // end of "hi"
            ..EditorState::create_default(para_and_code_doc("hi", "code"))
        };
        let txn = state.transaction().join_forward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2, "no merge");
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "hi");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::CodeBlock)
        );
        // paragraph "hi" node_size 4 → code block offset 4, content start 5
        assert_eq!(new_state.selection.from(), 5, "caret steps into the block");
    }

    #[test]
    fn join_forward_at_end_of_code_block_pulls_paragraph_text_in() {
        // Pin the non-destructive direction: Delete at the end of a
        // code block absorbs the following paragraph's text AS code.
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("language".to_string(), "python".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::CodeBlock,
                    attrs,
                    Fragment::from(vec![Node::text("code")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("hi")]),
                ),
            ]),
        );
        let state = EditorState {
            selection: Selection::cursor(5), // end of "code"
            ..EditorState::create_default(doc)
        };
        let txn = state.transaction().join_forward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(block.text_content(), "codehi");
    }

    #[test]
    fn join_backward_at_code_block_start_removes_empty_paragraph_above() {
        // Backspace at the code block's start with an empty paragraph
        // above: the paragraph goes, the block survives.
        let state = EditorState {
            selection: Selection::cursor(3), // code content start (P size 2)
            ..EditorState::create_default(para_and_code_doc("", "code"))
        };
        let txn = state.transaction().join_backward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 1);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock), "block survives");
        assert_eq!(block.text_content(), "code");
        assert_eq!(new_state.selection.from(), 1, "caret stays at code start");
    }

    #[test]
    fn join_backward_at_code_block_start_with_text_above_steps_out() {
        let state = EditorState {
            selection: Selection::cursor(5), // code content start (P "hi" size 4)
            ..EditorState::create_default(para_and_code_doc("hi", "code"))
        };
        let txn = state.transaction().join_backward().unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child_count(), 2, "no merge");
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::CodeBlock)
        );
        assert_eq!(new_state.selection.from(), 3, "caret at end of paragraph text");
    }

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

    // ── Regression: select + backspace + type puts cursor before inserted text ──

    #[test]
    fn select_backspace_then_type_cursor_after_inserted_text() {
        // Reproduce: type "1234", Enter, then select text, backspace, type "a"
        // The cursor should be AFTER the "a", not before it.

        // Step 1: Start with "1234" in a paragraph
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("1234")]),
            )]),
        );
        let state = EditorState::create_default(doc);

        // Step 2: Split block (Enter) at end of "1234" (position 5)
        let state = EditorState {
            selection: Selection::cursor(5),
            ..state
        };
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        // Now: Para("1234") + Para("")
        assert_eq!(state.doc.child_count(), 2);
        assert_eq!(state.doc.child(0).unwrap().text_content(), "1234");

        // Step 3: Select "1234" (positions 1..5) and delete
        let state = EditorState {
            selection: Selection::text(1, 5),
            ..state
        };
        let txn = state.transaction().delete_selection().unwrap();
        let state = state.apply(txn);
        // Now: Para("") + Para("") or just one empty Para
        let cursor_after_delete = state.selection.from();

        // Step 4: Type "a" at the cursor position after deletion
        let state = EditorState {
            selection: Selection::cursor(cursor_after_delete),
            ..state
        };
        let txn = state.transaction().insert_text("a").unwrap();
        let new_state = state.apply(txn);

        // The "a" should be in the document
        assert!(new_state.doc.text_content().contains("a"),
            "doc should contain 'a', got: '{}'", new_state.doc.text_content());

        // Cursor should be AFTER "a", not before it
        let cursor = new_state.selection.from();
        let cursor_text_before: String = {
            // Get text content up to cursor position
            let block = find_block_at(&new_state.doc, cursor);
            if let Some(b) = block {
                let offset = cursor - b.content_start;
                b.content.cut(0, offset).children.iter()
                    .map(|c| c.text_content()).collect()
            } else {
                String::new()
            }
        };
        assert!(cursor_text_before.contains("a"),
            "cursor at pos={cursor} should be AFTER 'a', but text before cursor is '{}'. \
             Full doc text: '{}'",
            cursor_text_before, new_state.doc.text_content());
    }

    #[test]
    fn select_all_in_block_backspace_then_type() {
        // Simpler version: single paragraph "abcde", select all, backspace, type "x"
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("abcde")]),
            )]),
        );
        let state = EditorState::create_default(doc);

        // Select all text in paragraph (1..6)
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        // Delete selection
        let txn = state.transaction().delete_selection().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child(0).unwrap().text_content(), "");
        assert_eq!(state.selection.from(), 1, "cursor should be at para content start after delete");

        // Type "x"
        let txn = state.transaction().insert_text("x").unwrap();
        let new_state = state.apply(txn);

        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "x");
        // Cursor should be at position 2 (after "x": para_open=1, "x"=1 char)
        assert_eq!(new_state.selection.from(), 2,
            "cursor should be after 'x' at position 2, got {}",
            new_state.selection.from());
        assert!(new_state.selection.empty(), "should be a cursor, not a range");
    }

    #[test]
    fn type_1234_enter_select_backspace_type_a() {
        // Exact user scenario: type "1234", Enter, select "1234", backspace, type "a"
        // Cursor should be AFTER the "a".

        // Start with "1234" paragraph
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("1234")]),
            )]),
        );
        let state = EditorState::create_default(doc);

        // Press Enter at end of "1234" → two paragraphs
        let state = EditorState { selection: Selection::cursor(5), ..state };
        let txn = state.transaction().split_block().unwrap();
        let state = state.apply(txn);
        assert_eq!(state.doc.child_count(), 2);
        assert_eq!(state.doc.child(0).unwrap().text_content(), "1234");
        // Cursor should be in second paragraph
        let cursor_after_enter = state.selection.from();
        assert!(cursor_after_enter > 5, "cursor should be in second paragraph, got {cursor_after_enter}");

        // Select "1234" in first paragraph (positions 1..5)
        let state = EditorState { selection: Selection::text(1, 5), ..state };
        let txn = state.transaction().delete_selection().unwrap();
        let state = state.apply(txn);
        let cursor_after_delete = state.selection.from();
        assert_eq!(cursor_after_delete, 1,
            "cursor should be at start of first (now empty) paragraph after delete, got {cursor_after_delete}");

        // Type "a"
        let state = EditorState { selection: Selection::cursor(cursor_after_delete), ..state };
        let txn = state.transaction().insert_text("a").unwrap();
        let new_state = state.apply(txn);

        let cursor_final = new_state.selection.from();
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "a");
        assert_eq!(cursor_final, 2,
            "cursor should be at position 2 (after 'a'), got {cursor_final}");
        assert!(new_state.selection.empty(), "should be a cursor, not a range");
    }

    // ── remap_block_anchor (comment anchors across a remote doc swap) ──

    fn anchor_doc(blocks: &[(&str, &str)]) -> Node {
        let children = blocks
            .iter()
            .map(|(id, text)| {
                let mut attrs = std::collections::HashMap::new();
                attrs.insert("blockId".to_string(), id.to_string());
                Node::element_with_attrs(
                    NodeType::Paragraph,
                    attrs,
                    Fragment::from(vec![Node::text(text)]),
                )
            })
            .collect();
        Node::element_with_content(NodeType::Doc, Fragment::from(children))
    }

    #[test]
    fn remap_block_anchor_other_block_is_noop() {
        // Anchor on block b; the edit ("HeXllo") is in block a — block-
        // relative offsets in b are untouched.
        let old = anchor_doc(&[("a", "Hello"), ("b", "World")]);
        let new = anchor_doc(&[("a", "HeXllo"), ("b", "World")]);
        let map = crate::editor::transform::step_map_for_doc_swap(&old, &new);
        assert_eq!(
            remap_block_anchor("b", 0, 5, std::slice::from_ref(&map), &old, &new),
            None,
        );
    }

    #[test]
    fn remap_block_anchor_same_block_insert_before_shifts() {
        // Anchor "lo" (3..5) in a; a char inserted at offset 0 shifts it +1.
        let old = anchor_doc(&[("a", "Hello")]);
        let new = anchor_doc(&[("a", "XHello")]);
        let map = crate::editor::transform::step_map_for_doc_swap(&old, &new);
        assert_eq!(
            remap_block_anchor("a", 3, 5, std::slice::from_ref(&map), &old, &new),
            Some((4, 6)),
        );
    }

    #[test]
    fn remap_block_anchor_same_block_insert_after_is_noop() {
        // Anchor "He" (0..2); insert at offset 4 is after it — no shift.
        let old = anchor_doc(&[("a", "Hello")]);
        let new = anchor_doc(&[("a", "HellXo")]);
        let map = crate::editor::transform::step_map_for_doc_swap(&old, &new);
        assert_eq!(
            remap_block_anchor("a", 0, 2, std::slice::from_ref(&map), &old, &new),
            None,
        );
    }
}
