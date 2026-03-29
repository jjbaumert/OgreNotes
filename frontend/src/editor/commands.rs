use std::collections::HashMap;

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::position::resolve;
use super::selection::Selection;
use super::state::{find_block_at, find_container_at, find_container_of_type, find_item_at, EditorState, Transaction};
use super::transform::Step;

/// A command function signature.
/// When `dispatch` is None, the command checks applicability (returns true/false).
/// When `dispatch` is Some, the command creates and dispatches a transaction.
pub type CommandFn = fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool;

// ─── Mark Commands ──────────────────────────────────────────────

/// Toggle a mark on the current selection.
/// If the selection is a cursor, toggle stored marks.
/// If the selection is a range, add or remove the mark from the range.
/// Removes the mark only if ALL text in the range has it; otherwise adds.
pub fn toggle_mark(
    mark_type: MarkType,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let from = state.selection.from();
    let to = state.selection.to();

    // Check if the mark can be applied at the from position
    if let Some(rp) = resolve(&state.doc, from) {
        let parent = rp.node_at(rp.depth, &state.doc);
        if let Some(nt) = parent.node_type() {
            if !state.schema.can_apply_mark(nt, mark_type) {
                return false;
            }
        }
    }

    if state.selection.empty() {
        // Cursor: toggle stored marks
        if let Some(dispatch) = dispatch {
            let has_mark = state
                .stored_marks
                .as_ref()
                .map(|m| m.iter().any(|m| m.mark_type == mark_type))
                .unwrap_or_else(|| mark_active_at_cursor(state, mark_type));

            let new_stored = if has_mark {
                let current = state
                    .stored_marks
                    .clone()
                    .unwrap_or_else(|| marks_at_cursor(state));
                let filtered: Vec<Mark> = current
                    .into_iter()
                    .filter(|m| m.mark_type != mark_type)
                    .collect();
                Some(filtered)
            } else {
                let mut current = state
                    .stored_marks
                    .clone()
                    .unwrap_or_else(|| marks_at_cursor(state));
                current.push(Mark::new(mark_type));
                super::model::normalize_marks(&mut current);
                Some(current)
            };

            let txn = state.transaction().set_stored_marks(new_stored);
            dispatch(txn);
        }
        return true;
    }

    // Range selection: add or remove mark.
    // Remove only if ALL text in the range has the mark.
    if let Some(dispatch) = dispatch {
        let mark = Mark::new(mark_type);
        let all_have_mark = range_all_have_mark(&state.doc, from, to, mark_type);

        let result = if all_have_mark {
            state.transaction().remove_mark(from, to, mark)
        } else {
            state.transaction().add_mark(from, to, mark)
        };

        if let Ok(txn) = result {
            dispatch(txn);
        }
        // If step failed, don't dispatch (don't send no-op transaction)
    }
    true
}

/// Public version for toolbar state queries.
pub fn mark_active_at_cursor_public(state: &EditorState, mark_type: MarkType) -> bool {
    state
        .stored_marks
        .as_ref()
        .map(|m| m.iter().any(|m| m.mark_type == mark_type))
        .unwrap_or_else(|| mark_active_at_cursor(state, mark_type))
}

/// Check if a mark is active at the cursor position.
/// Inherits marks from the text node to the LEFT of the cursor (preceding text).
fn mark_active_at_cursor(state: &EditorState, mark_type: MarkType) -> bool {
    marks_at_cursor(state)
        .iter()
        .any(|m| m.mark_type == mark_type)
}

/// Get the marks at the cursor position.
/// Returns the marks of the text node to the left of (or containing) the cursor.
/// If the cursor is at the start of a paragraph (no preceding text), returns
/// the marks of the first text node.
fn marks_at_cursor(state: &EditorState) -> Vec<Mark> {
    marks_at_pos(&state.doc, state.selection.from())
}

/// Get the marks at a given document position.
/// Returns the marks of the text node to the left of (or containing) the position.
/// Used by both commands (for stored marks) and insert_text (for mark inheritance).
pub fn marks_at_pos(doc: &Node, pos: usize) -> Vec<Mark> {
    let Some(rp) = resolve(doc, pos) else {
        return vec![];
    };
    let parent = rp.node_at(rp.depth, doc);
    let offset = rp.parent_offset();

    // Walk children to find the text node at or just before the cursor
    let mut child_pos = 0;
    let mut last_text_marks: Option<Vec<Mark>> = None;

    for i in 0..parent.child_count() {
        let child = parent.child(i).unwrap();
        let child_size = child.node_size();
        let child_end = child_pos + child_size;

        if child.is_text() {
            if offset == child_pos && last_text_marks.is_some() {
                // Cursor is at the boundary between the previous text and this one.
                // Inherit marks from the left side (previous text node).
                return last_text_marks.unwrap();
            }
            if offset >= child_pos && offset < child_end {
                // Cursor is at or inside this text node
                return child.marks().to_vec();
            }
            // Track this text node as potential left-side marks
            last_text_marks = Some(child.marks().to_vec());
        } else if child_pos >= offset && last_text_marks.is_some() {
            return last_text_marks.unwrap();
        }

        child_pos = child_end;
    }

    last_text_marks.unwrap_or_default()
}

/// Check if ALL text in the range from..to has a given mark.
/// Returns false if any text node in the range lacks the mark.
/// Returns false if the range contains no text at all.
fn range_all_have_mark(node: &Node, from: usize, to: usize, mark_type: MarkType) -> bool {
    let mut found_text = false;
    let result = check_all_marks(node, from, to, mark_type, &mut found_text);
    result && found_text
}

fn check_all_marks(
    node: &Node,
    from: usize,
    to: usize,
    mark_type: MarkType,
    found_text: &mut bool,
) -> bool {
    match node {
        Node::Text { marks, text, .. } => {
            let len = super::model::char_len(text);
            if from < len && to > 0 {
                *found_text = true;
                return marks.iter().any(|m| m.mark_type == mark_type);
            }
            true // no overlap
        }
        Node::Element { content, node_type, .. } => {
            if node_type.is_leaf() {
                return true;
            }
            let mut pos = 0;
            for child in &content.children {
                let child_size = child.node_size();
                let child_end = pos + child_size;

                if child_end <= from {
                    pos = child_end;
                    continue;
                }
                if pos >= to {
                    break;
                }

                match child {
                    Node::Text { .. } => {
                        let rel_from = if from > pos { from - pos } else { 0 };
                        let rel_to = if to < child_end { to - pos } else { child_size };
                        if !check_all_marks(child, rel_from, rel_to, mark_type, found_text) {
                            return false;
                        }
                    }
                    Node::Element { node_type: ct, .. } => {
                        if !ct.is_leaf() {
                            let inner_from =
                                if from > pos + 1 { from - pos - 1 } else { 0 };
                            let inner_to = if to < child_end - 1 {
                                to - pos - 1
                            } else {
                                child.content_size()
                            };
                            if !check_all_marks(
                                child, inner_from, inner_to, mark_type, found_text,
                            ) {
                                return false;
                            }
                        }
                    }
                }
                pos = child_end;
            }
            true
        }
    }
}

// ─── Block Commands ─────────────────────────────────────────────

/// Set the heading level of the block containing the cursor.
/// Works on both Heading nodes (changes level) and Paragraph nodes (converts to Heading).
pub fn set_heading(
    level: u8,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    let Some(rp) = resolve(&state.doc, pos) else {
        return false;
    };

    let parent = rp.node_at(rp.depth, &state.doc);
    let parent_type = parent.node_type();

    match parent_type {
        Some(NodeType::Heading) => {
            // Already a heading -- change level via SetAttr
            if let Some(dispatch) = dispatch {
                let abs_pos = rp.start(rp.depth) - 1;
                if let Ok(txn) = state.transaction().step(Step::SetAttr {
                    pos: abs_pos,
                    attr: "level".to_string(),
                    value: level.to_string(),
                }) {
                    dispatch(txn);
                }
            }
            true
        }
        Some(NodeType::Paragraph) => {
            // Convert paragraph to heading via SetNodeType
            if let Some(dispatch) = dispatch {
                let abs_pos = rp.start(rp.depth) - 1;
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), level.to_string());
                if let Ok(txn) = state.transaction().step(Step::SetNodeType {
                    pos: abs_pos,
                    node_type: NodeType::Heading,
                    attrs,
                }) {
                    dispatch(txn);
                }
            }
            true
        }
        _ => false,
    }
}

/// Check if the cursor is in a paragraph (for toolbar state).
pub fn is_paragraph(state: &EditorState) -> bool {
    let pos = state.selection.from();
    let Some(rp) = resolve(&state.doc, pos) else {
        return false;
    };
    rp.node_at(rp.depth, &state.doc).node_type() == Some(NodeType::Paragraph)
}

/// Check if the cursor is in a heading and return the level.
pub fn heading_level(state: &EditorState) -> Option<u8> {
    let pos = state.selection.from();
    let rp = resolve(&state.doc, pos)?;
    let parent = rp.node_at(rp.depth, &state.doc);
    if parent.node_type() != Some(NodeType::Heading) {
        return None;
    }
    parent
        .attrs()
        .get("level")
        .and_then(|l| l.parse::<u8>().ok())
}

// ─── List and Container Commands ────────────────────────────────

/// Check if the cursor is inside a list of the given type.
/// Searches all ancestor containers, not just the innermost one.
pub fn is_in_list(state: &EditorState, list_type: NodeType) -> bool {
    let pos = state.selection.from();
    find_container_of_type(&state.doc, pos, list_type).is_some()
}

/// Check if the cursor is inside a blockquote.
/// Searches all ancestor containers, not just the innermost one.
pub fn is_in_blockquote(state: &EditorState) -> bool {
    let pos = state.selection.from();
    find_container_of_type(&state.doc, pos, NodeType::Blockquote).is_some()
}

/// Toggle a list: if already in this list type, unwrap. If in a different list
/// type, convert. If not in any list, wrap.
pub fn toggle_list(
    list_type: NodeType,
    item_type: NodeType,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    // Search for any list container (not just innermost container of any type)
    let container = find_container_of_type(&state.doc, pos, list_type)
        .or_else(|| {
            // Check if we're in a different list type
            [NodeType::BulletList, NodeType::OrderedList, NodeType::TaskList]
                .iter()
                .filter(|&&t| t != list_type)
                .find_map(|&t| find_container_of_type(&state.doc, pos, t))
        });

    match container {
        Some(ref c) if c.node_type == list_type => {
            // Already in this list type -- unwrap (lift out)
            if let Some(dispatch) = dispatch {
                if let Some(txn) = lift_from_container(state, c) {
                    dispatch(txn);
                }
            }
            true
        }
        Some(ref c) if is_list_type(c.node_type) => {
            // In a different list type -- convert the list type and item type
            if let Some(dispatch) = dispatch {
                if let Some(txn) = convert_list(state, c, list_type, item_type) {
                    dispatch(txn);
                }
            }
            true
        }
        _ => {
            // Not in a list -- wrap in one
            if let Some(dispatch) = dispatch {
                if let Some(txn) = wrap_in_list(state, list_type, item_type) {
                    dispatch(txn);
                }
            }
            true
        }
    }
}

/// Toggle blockquote: if in blockquote, unwrap. Otherwise wrap.
pub fn toggle_blockquote(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    // Search specifically for a blockquote ancestor (not just the innermost container)
    let container = find_container_of_type(&state.doc, pos, NodeType::Blockquote);

    if let Some(ref c) = container {
        if c.node_type == NodeType::Blockquote {
            // Already in blockquote -- lift out
            if let Some(dispatch) = dispatch {
                if let Some(txn) = lift_from_container(state, c) {
                    dispatch(txn);
                }
            }
            return true;
        }
    }

    // Not in blockquote -- wrap
    if let Some(dispatch) = dispatch {
        if let Some(txn) = wrap_in_blockquote(state) {
            dispatch(txn);
        }
    }
    true
}

/// Convert the current block to a paragraph.
/// If in a heading/code block, changes node type. If in a list/blockquote, lifts out.
pub fn set_paragraph(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    let Some(rp) = resolve(&state.doc, pos) else {
        return false;
    };

    let parent = rp.node_at(rp.depth, &state.doc);
    let parent_type = parent.node_type();

    match parent_type {
        Some(NodeType::Paragraph) => {
            // Already a paragraph. If inside a container, lift out.
            if let Some(c) = find_container_at(&state.doc, pos) {
                if let Some(dispatch) = dispatch {
                    if let Some(txn) = lift_from_container(state, &c) {
                        dispatch(txn);
                    }
                }
                return true;
            }
            true // already a bare paragraph, no-op
        }
        Some(NodeType::Heading) | Some(NodeType::CodeBlock) => {
            // Convert to paragraph via SetNodeType
            if let Some(dispatch) = dispatch {
                let abs_pos = rp.start(rp.depth) - 1;
                if let Ok(txn) = state.transaction().step(Step::SetNodeType {
                    pos: abs_pos,
                    node_type: NodeType::Paragraph,
                    attrs: HashMap::new(),
                }) {
                    dispatch(txn);
                }
            }
            true
        }
        _ => false,
    }
}

/// Insert a horizontal rule after the current block (or container if inside one).
pub fn insert_horizontal_rule(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    let Some(block) = find_block_at(&state.doc, pos) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        // If inside a container (list/blockquote), insert after the container,
        // not after the inner textblock.
        let insert_pos = if let Some(container) = find_container_at(&state.doc, pos) {
            container.offset + container.node_size
        } else {
            block.offset + block.node_size
        };

        let hr = Node::element(NodeType::HorizontalRule);
        let new_para = Node::element(NodeType::Paragraph);
        let slice = Slice::new(Fragment::from(vec![hr, new_para]), 0, 0);

        if let Ok(mut txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            // Place cursor in the new paragraph (after HR)
            txn.selection = Selection::cursor(insert_pos + 1 + 1); // HR size (1) + Para open (1)
            dispatch(txn);
        }
    }
    true
}

// ── Block command helpers ──

fn is_list_type(nt: NodeType) -> bool {
    matches!(
        nt,
        NodeType::BulletList | NodeType::OrderedList | NodeType::TaskList
    )
}

fn item_type_for_list(list_type: NodeType) -> NodeType {
    if list_type == NodeType::TaskList {
        NodeType::TaskItem
    } else {
        NodeType::ListItem
    }
}

/// Wrap the current textblock in a list (ListType > ItemType > existing block).
fn wrap_in_list(
    state: &EditorState,
    list_type: NodeType,
    item_type: NodeType,
) -> Option<Transaction> {
    let pos = state.selection.from();
    let block = find_block_at(&state.doc, pos)?;

    // Build wrapper: ListType[ItemType[]] with gap at depth 2
    let item_node = Node::element(item_type);
    let list_node = Node::element_with_content(list_type, Fragment::from(vec![item_node]));
    let wrapper = Slice::new(Fragment::from(vec![list_node]), 2, 2);

    let step = Step::ReplaceAround {
        from: block.offset,
        to: block.offset + block.node_size,
        gap_from: block.offset,
        gap_to: block.offset + block.node_size,
        insert: wrapper,
        structure: true,
    };

    state.transaction().step(step).ok()
}

/// Wrap the current textblock in a blockquote.
fn wrap_in_blockquote(state: &EditorState) -> Option<Transaction> {
    let pos = state.selection.from();
    let block = find_block_at(&state.doc, pos)?;

    // Build wrapper: Blockquote[] with gap at depth 1
    let bq_node = Node::element(NodeType::Blockquote);
    let wrapper = Slice::new(Fragment::from(vec![bq_node]), 1, 1);

    let step = Step::ReplaceAround {
        from: block.offset,
        to: block.offset + block.node_size,
        gap_from: block.offset,
        gap_to: block.offset + block.node_size,
        insert: wrapper,
        structure: true,
    };

    state.transaction().step(step).ok()
}

/// Lift the contents out of a container (unwrap).
/// Extracts all textblock children from the container and replaces the container with them.
fn lift_from_container(
    state: &EditorState,
    container: &super::state::ContainerInfo,
) -> Option<Transaction> {
    let from = container.offset;
    let to = container.offset + container.node_size;

    // The gap is the container's content (everything between its open and close boundaries).
    let gap_from = from + 1; // after container open boundary

    // For lists: container > item > content. Gap should be the inner content.
    // For blockquote: container > content. Gap is just the content.
    if is_list_type(container.node_type) {
        // List: gap needs to skip list boundary + item boundary
        // But we need to unwrap the WHOLE list, extracting all list items' children.
        // Use gap_from = from + 2 (skip list open + first item open)
        // and gap_to = to - 2 (skip last item close + list close)
        // This only works for single-item lists. For multi-item lists, we need
        // to extract each item's content and flatten.
        // Simpler approach: extract all textblock children and replace via Replace step.
        return lift_from_list(state, container);
    }

    // Blockquote: simple unwrap
    let gap_to = to - 1; // before container close boundary
    let step = Step::ReplaceAround {
        from,
        to,
        gap_from,
        gap_to,
        insert: Slice::empty(),
        structure: true,
    };

    state.transaction().step(step).ok()
}

/// Lift contents from a list by extracting all paragraphs from all list items.
fn lift_from_list(
    state: &EditorState,
    container: &super::state::ContainerInfo,
) -> Option<Transaction> {
    // Extract all children from all list items in the container
    let list_node = extract_node_at(&state.doc, container.offset)?;
    let mut paragraphs = Vec::new();

    for i in 0..list_node.child_count() {
        let item = list_node.child(i)?;
        // Each list item contains paragraphs and possibly nested blocks
        for j in 0..item.child_count() {
            paragraphs.push(item.child(j)?.clone());
        }
    }

    if paragraphs.is_empty() {
        paragraphs.push(Node::element(NodeType::Paragraph));
    }

    let from = container.offset;
    let to = container.offset + container.node_size;
    let slice = Slice::new(Fragment::from(paragraphs), 0, 0);

    state.transaction().step(Step::Replace { from, to, slice }).ok()
}

/// Convert a list from one type to another by changing the list node type
/// and item types.
fn convert_list(
    state: &EditorState,
    container: &super::state::ContainerInfo,
    new_list_type: NodeType,
    new_item_type: NodeType,
) -> Option<Transaction> {
    let list_pos = container.offset;

    // Step 1: Change the list container type
    let mut txn = state.transaction().step(Step::SetNodeType {
        pos: list_pos,
        node_type: new_list_type,
        attrs: HashMap::new(),
    }).ok()?;

    // Step 2: Change each list item's type if needed
    let old_item_type = item_type_for_list(container.node_type);
    if old_item_type != new_item_type {
        // Walk the list's children and change each item type
        let list_node = extract_node_at(&txn.doc, list_pos)?;
        let mut item_offset = list_pos + 1; // after list open boundary
        for i in 0..list_node.child_count() {
            let item = list_node.child(i)?;
            let item_size = item.node_size();
            let attrs = if new_item_type == NodeType::TaskItem {
                let mut a = HashMap::new();
                a.insert("checked".to_string(), "false".to_string());
                a
            } else {
                HashMap::new()
            };
            txn = txn.step(Step::SetNodeType {
                pos: item_offset,
                node_type: new_item_type,
                attrs,
            }).ok()?;
            item_offset += item_size;
        }
    }

    Some(txn)
}

/// Extract a node at a given offset in the doc's content.
pub(crate) fn extract_node_at(doc: &Node, offset: usize) -> Option<Node> {
    let Node::Element { content, .. } = doc else {
        return None;
    };
    extract_node_in_children(&content.children, offset, 0)
}

fn extract_node_in_children(children: &[Node], target_offset: usize, mut offset: usize) -> Option<Node> {
    for child in children {
        let child_size = child.node_size();
        if offset == target_offset {
            return Some(child.clone());
        }
        if let Node::Element { content, node_type, .. } = child {
            if !node_type.is_leaf() {
                let content_start = offset + 1;
                let content_end = offset + child_size - 1;
                if target_offset >= content_start && target_offset <= content_end {
                    return extract_node_in_children(&content.children, target_offset, content_start);
                }
            }
        }
        offset += child_size;
    }
    None
}

// ─── Text Commands ──────────────────────────────────────────────

/// Delete the current selection. No-op if selection is empty.
pub fn delete_selection(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if state.selection.empty() {
        return false;
    }

    if let Some(dispatch) = dispatch {
        if let Ok(txn) = state.transaction().delete_selection() {
            dispatch(txn);
        }
    }
    true
}

/// Select the entire document.
pub fn select_all(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if let Some(dispatch) = dispatch {
        let sel = Selection::all(&state.doc);
        let txn = state.transaction().set_selection(sel);
        dispatch(txn);
    }
    true
}

/// Insert a hard break (Shift+Enter).
pub fn insert_hard_break(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if let Some(dispatch) = dispatch {
        let from = state.selection.from();
        let to = state.selection.to();
        let br_node = Node::element(NodeType::HardBreak);
        let content = Fragment::from(vec![br_node]);
        let slice = Slice::new(content, 0, 0);

        if let Ok(txn) = state.transaction().replace(from, to, slice) {
            dispatch(txn);
        }
    }
    true
}

// ─── List Indent / Dedent ──────────────────────────────────────

/// Sink (indent) a list item: nest it inside the previous sibling's sub-list.
/// Pressing Tab in a list item produces:
///   Before: List > [Item("prev"), Item("cur")]
///   After:  List > [Item("prev", SubList > [Item("cur")])]
/// Returns false if not in a list item or the item is the first child (no prev sibling).
pub fn sink_list_item(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();

    // Must be inside a list item
    let Some(item) = find_item_at(&state.doc, pos) else {
        return false;
    };

    // Find the parent list that contains this item
    let Some(list_container) = find_container_at(&state.doc, pos) else {
        return false;
    };
    let list_node = match extract_node_at(&state.doc, list_container.offset) {
        Some(n) => n,
        None => return false,
    };

    // Find the item's index among list children and the previous sibling
    let mut item_index = None;
    let mut child_offset = list_container.offset + 1; // list content_start
    for i in 0..list_node.child_count() {
        if child_offset == item.offset {
            item_index = Some(i);
            break;
        }
        child_offset += list_node.child(i).map_or(0, |c| c.node_size());
    }
    let item_index = match item_index {
        Some(0) | None => return true, // first item — can't indent, but consume Tab
        Some(i) => i,
    };

    if dispatch.is_none() {
        return true; // command is applicable
    }

    let prev_sibling = match list_node.child(item_index - 1) {
        Some(n) => n.clone(),
        None => return false,
    };
    let cur_item = match list_node.child(item_index) {
        Some(n) => n.clone(),
        None => return false,
    };

    let cursor_in_item = pos - item.content_start;

    // Build the new previous sibling: append cur_item into a sub-list.
    // If prev already ends with a sub-list of the same type, append to it.
    // Otherwise, create a new sub-list.
    let mut prev_children: Vec<Node> = (0..prev_sibling.child_count())
        .filter_map(|i| prev_sibling.child(i).cloned())
        .collect();

    let list_type = list_container.node_type;
    let last_is_sublist = prev_children
        .last()
        .and_then(|n| n.node_type())
        .map_or(false, |nt| nt == list_type);

    // Compute sizes for cursor positioning BEFORE modifying prev_children
    let prefix_size: usize;
    let items_before_cur_in_sublist: usize;

    if last_is_sublist {
        let existing_sublist = prev_children.last().unwrap();
        items_before_cur_in_sublist = existing_sublist.node_size() - 2; // content size
        prefix_size = prev_children[..prev_children.len() - 1]
            .iter()
            .map(|c| c.node_size())
            .sum();

        // Append cur_item to existing sub-list
        if let Some(Node::Element {
            node_type,
            attrs,
            content,
            marks,
        }) = prev_children.pop()
        {
            let mut sub_children = content.children.clone();
            sub_children.push(cur_item);
            prev_children.push(Node::Element {
                node_type,
                attrs,
                content: Fragment::from(sub_children),
                marks,
            });
        }
    } else {
        prefix_size = prev_children.iter().map(|c| c.node_size()).sum();
        items_before_cur_in_sublist = 0;

        // Create a new sub-list containing cur_item
        let sub_list =
            Node::element_with_content(list_type, Fragment::from(vec![cur_item]));
        prev_children.push(sub_list);
    }

    let new_prev = Node::Element {
        node_type: prev_sibling.node_type().unwrap_or(NodeType::ListItem),
        attrs: prev_sibling.attrs().clone(),
        content: Fragment::from(prev_children),
        marks: vec![],
    };

    // Replace the prev sibling + current item with the merged prev sibling.
    let prev_offset = item.offset - prev_sibling.node_size();
    let end = item.offset + item.node_size;
    let slice = Slice::new(Fragment::from(vec![new_prev]), 0, 0);

    if let Some(dispatch) = dispatch {
        if let Ok(mut txn) = state.transaction().replace(prev_offset, end, slice) {
            // Place cursor inside cur_item in its new nested position:
            // prev_offset + 1 (item open) + prefix + 1 (sublist open)
            //   + items_before_cur + 1 (cur_item open) + cursor_in_item
            let new_cursor = prev_offset + 3 + prefix_size
                + items_before_cur_in_sublist + cursor_in_item;
            txn.selection = Selection::cursor(new_cursor);
            dispatch(txn);
        }
    }
    true
}

/// Lift (dedent) a list item: move it out of a nested sub-list into the parent list.
/// Pressing Shift-Tab in a nested list item produces:
///   Before: OuterList > [Item("prev", InnerList > [Item("cur")])]
///   After:  OuterList > [Item("prev"), Item("cur")]
/// Returns false if the item is not inside a nested list.
pub fn lift_list_item(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();

    // Must be inside a list item
    let Some(item) = find_item_at(&state.doc, pos) else {
        return false;
    };

    // The item's parent list must itself be nested inside another list item.
    // Find the inner list (direct parent of item).
    let Some(inner_list) = find_container_at(&state.doc, pos) else {
        return false;
    };

    // Check if this inner list is inside a parent item (i.e., nested).
    // The parent item would be at inner_list.offset - 1 (before the list open).
    // We need to find a list item that contains inner_list.offset.
    let inner_list_offset = inner_list.offset;
    // Search for an item that contains a position just outside the inner list
    let Some(parent_item) = find_item_at(&state.doc, inner_list_offset) else {
        return true; // not nested — can't dedent, but consume Shift-Tab
    };

    // Verify the inner list is actually inside the parent item
    if inner_list_offset < parent_item.offset
        || inner_list_offset >= parent_item.offset + parent_item.node_size
    {
        return true; // not properly nested — consume Shift-Tab
    }

    if dispatch.is_none() {
        return true; // command is applicable
    }

    // Strategy: extract the current item from the inner list.
    // Rebuild the parent item without cur_item in its sub-list, then
    // insert cur_item as a sibling after the parent item in the outer list.
    let inner_list_node = match extract_node_at(&state.doc, inner_list.offset) {
        Some(n) => n,
        None => return false,
    };

    // Find item index in inner list
    let mut item_index = 0;
    let mut child_offset = inner_list.offset + 1;
    for i in 0..inner_list_node.child_count() {
        if child_offset == item.offset {
            item_index = i;
            break;
        }
        child_offset += inner_list_node.child(i).map_or(0, |c| c.node_size());
    }

    let cur_item = match inner_list_node.child(item_index) {
        Some(n) => n.clone(),
        None => return false,
    };

    // Items before and after cur_item in the inner list
    let before_items: Vec<Node> = (0..item_index)
        .filter_map(|i| inner_list_node.child(i).cloned())
        .collect();
    let after_items: Vec<Node> = (item_index + 1..inner_list_node.child_count())
        .filter_map(|i| inner_list_node.child(i).cloned())
        .collect();

    // Rebuild the parent item's children:
    // - Keep everything before the inner list
    // - If before_items is non-empty, keep a trimmed inner list with just those
    // - Skip the original inner list
    // - (after_items will be attached to cur_item as a trailing sub-list)
    let parent_item_node = match extract_node_at(&state.doc, parent_item.offset) {
        Some(n) => n,
        None => return false,
    };

    let mut new_parent_children: Vec<Node> = Vec::new();
    let mut parent_child_offset = parent_item.content_start;
    for i in 0..parent_item_node.child_count() {
        let child = match parent_item_node.child(i) {
            Some(c) => c.clone(),
            None => continue,
        };
        if parent_child_offset == inner_list.offset {
            // This is the inner list — replace with trimmed version if needed
            if !before_items.is_empty() {
                let trimmed = Node::element_with_content(
                    inner_list.node_type,
                    Fragment::from(before_items.clone()),
                );
                new_parent_children.push(trimmed);
            }
        } else {
            new_parent_children.push(child.clone());
        }
        parent_child_offset += child.node_size();
    }

    let new_parent = Node::Element {
        node_type: parent_item.node_type,
        attrs: parent_item.attrs.clone(),
        content: Fragment::from(new_parent_children),
        marks: vec![],
    };

    // If there are items after cur_item, attach them as a trailing sub-list on cur_item
    let lifted_item = if after_items.is_empty() {
        cur_item
    } else {
        let trailing_list = Node::element_with_content(
            inner_list.node_type,
            Fragment::from(after_items),
        );
        let mut cur_children: Vec<Node> = (0..cur_item.child_count())
            .filter_map(|i| cur_item.child(i).cloned())
            .collect();
        cur_children.push(trailing_list);
        Node::Element {
            node_type: cur_item.node_type().unwrap_or(NodeType::ListItem),
            attrs: cur_item.attrs().clone(),
            content: Fragment::from(cur_children),
            marks: vec![],
        }
    };

    let cursor_in_item = pos - item.content_start;

    // Replace parent_item with [new_parent, lifted_item]
    let from = parent_item.offset;
    let end = parent_item.offset + parent_item.node_size;
    let new_parent_size = new_parent.node_size();
    let slice = Slice::new(
        Fragment::from(vec![new_parent, lifted_item]),
        0,
        0,
    );

    if let Some(dispatch) = dispatch {
        if let Ok(mut txn) = state.transaction().replace(from, end, slice) {
            // Place cursor inside lifted_item:
            // from + new_parent_size + 1 (lifted_item open) + cursor_in_item
            let new_cursor = from + new_parent_size + 1 + cursor_in_item;
            txn.selection = Selection::cursor(new_cursor);
            dispatch(txn);
        }
    }
    true
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::Fragment;
    use crate::editor::state::EditorState;
    use std::cell::RefCell;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    fn bold_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text_with_marks("Hello", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" world"),
                ]),
            )]),
        )
    }

    fn partial_bold_doc() -> Node {
        // "He" bold, "llo world" plain
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text_with_marks("He", vec![Mark::new(MarkType::Bold)]),
                    Node::text("llo world"),
                ]),
            )]),
        )
    }

    fn heading_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Heading,
                Fragment::from(vec![Node::text("Title")]),
            )]),
        )
    }

    fn run_command<F>(state: &EditorState, cmd: F) -> Option<Transaction>
    where
        F: Fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool,
    {
        let result: RefCell<Option<Transaction>> = RefCell::new(None);
        let dispatched = cmd(state, Some(&|txn| {
            *result.borrow_mut() = Some(txn);
        }));
        if dispatched {
            result.into_inner()
        } else {
            None
        }
    }

    // ── toggle_mark ──

    #[test]
    fn toggle_bold_on_range() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.child_count(), 2);
        assert!(para
            .child(0)
            .unwrap()
            .marks()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
        assert_eq!(para.child(0).unwrap().text_content(), "Hello");
    }

    #[test]
    fn toggle_bold_off_range() {
        let state = EditorState::create_default(bold_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "Hello world");
        assert!(para.child(0).unwrap().marks().is_empty());
    }

    #[test]
    fn toggle_bold_on_partial_range_adds() {
        // Only "He" is bold. Selecting "Hello" (1..6) and toggling Bold should ADD,
        // not remove, because not ALL text in the range is bold.
        let state = EditorState::create_default(partial_bold_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };

        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // All of "Hello" should now be bold
        let first = para.child(0).unwrap();
        assert!(first
            .marks()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn toggle_bold_cursor_sets_stored_marks() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(3),
            ..state
        };

        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert!(new_state
            .stored_marks
            .as_ref()
            .unwrap()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn toggle_bold_cursor_removes_stored_marks() {
        let state = EditorState::create_default(bold_doc());
        let state = EditorState {
            selection: Selection::cursor(3),
            ..state
        };

        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert!(!new_state
            .stored_marks
            .as_ref()
            .unwrap()
            .iter()
            .any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn toggle_mark_check_only() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let applicable = toggle_mark(MarkType::Bold, &state, None);
        assert!(applicable);
    }

    #[test]
    fn toggle_mark_in_code_block_returns_false() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("code")]),
            )]),
        );
        let state = EditorState::create_default(doc);
        let applicable = toggle_mark(MarkType::Bold, &state, None);
        assert!(!applicable);
    }

    #[test]
    fn mark_at_cursor_boundary_inherits_left() {
        // bold "Hello" + plain " world"
        // Cursor at position 6 (after "Hello", before " world") should inherit bold
        let state = EditorState::create_default(bold_doc());
        let state = EditorState {
            selection: Selection::cursor(6),
            ..state
        };
        assert!(mark_active_at_cursor(&state, MarkType::Bold));
    }

    #[test]
    fn mark_at_cursor_start_of_paragraph() {
        // Cursor at position 1 (start of paragraph content, before "H")
        let state = EditorState::create_default(bold_doc());
        let state = EditorState {
            selection: Selection::cursor(1),
            ..state
        };
        // At the start, before any text, we get marks of the first text node
        let marks = marks_at_cursor(&state);
        assert!(marks.iter().any(|m| m.mark_type == MarkType::Bold));
    }

    // ── set_heading ──

    #[test]
    fn set_heading_level() {
        let state = EditorState::create_default(heading_doc());
        let txn = run_command(&state, |s, d| set_heading(2, s, d)).unwrap();
        let new_state = state.apply(txn);
        let heading = new_state.doc.child(0).unwrap();
        assert_eq!(heading.attrs().get("level").unwrap(), "2");
    }

    #[test]
    fn set_heading_check_only() {
        let state = EditorState::create_default(heading_doc());
        let applicable = set_heading(2, &state, None);
        assert!(applicable);
    }

    #[test]
    fn set_heading_on_paragraph_converts() {
        let state = EditorState::create_default(simple_doc());
        let applicable = set_heading(1, &state, None);
        assert!(applicable); // paragraphs can now be converted to headings

        let txn = run_command(&state, |s, d| set_heading(1, s, d)).unwrap();
        let new_state = state.apply(txn);
        let heading = new_state.doc.child(0).unwrap();
        assert_eq!(heading.node_type(), Some(NodeType::Heading));
        assert_eq!(heading.attrs().get("level").unwrap(), "1");
        assert_eq!(heading.text_content(), "Hello world");
    }

    #[test]
    fn heading_level_query() {
        let state = EditorState::create_default(heading_doc());
        assert_eq!(heading_level(&state), Some(1));
    }

    #[test]
    fn is_paragraph_query() {
        let state = EditorState::create_default(simple_doc());
        assert!(is_paragraph(&state));

        let state = EditorState::create_default(heading_doc());
        assert!(!is_paragraph(&state));
    }

    // ── delete_selection ──

    #[test]
    fn delete_selection_command() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, delete_selection).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), " world");
    }

    #[test]
    fn delete_selection_empty_returns_false() {
        let state = EditorState::create_default(simple_doc());
        let applicable = delete_selection(&state, None);
        assert!(!applicable);
    }

    // ── select_all ──

    #[test]
    fn select_all_command() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, select_all).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.selection.from(), 0);
        assert_eq!(new_state.selection.to(), state.doc.content_size());
    }

    // ── insert_hard_break ──

    #[test]
    fn insert_hard_break_command() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(6),
            ..state
        };
        let txn = run_command(&state, insert_hard_break).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let has_br = (0..para.child_count())
            .any(|i| para.child(i).unwrap().node_type() == Some(NodeType::HardBreak));
        assert!(has_br);
    }

    // ── toggle_list ──

    #[test]
    fn toggle_bullet_list_wraps() {
        let state = EditorState::create_default(simple_doc());
        assert!(!is_in_list(&state, NodeType::BulletList));

        let txn = run_command(&state, |s, d| {
            toggle_list(NodeType::BulletList, NodeType::ListItem, s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);

        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::ListItem));
        let para = item.child(0).unwrap();
        assert_eq!(para.text_content(), "Hello world");

        assert!(is_in_list(&new_state, NodeType::BulletList));
    }

    #[test]
    fn toggle_bullet_list_unwraps() {
        // Start with a list doc
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Test")]),
        );
        let item = Node::element_with_content(NodeType::ListItem, Fragment::from(vec![para]));
        let list = Node::element_with_content(NodeType::BulletList, Fragment::from(vec![item]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![list]));

        let state = EditorState::create_default(doc);
        assert!(is_in_list(&state, NodeType::BulletList));

        let txn = run_command(&state, |s, d| {
            toggle_list(NodeType::BulletList, NodeType::ListItem, s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);

        let first = new_state.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "Test");
    }

    #[test]
    fn toggle_list_converts_type() {
        // Start with bullet list, convert to ordered list
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Test")]),
        );
        let item = Node::element_with_content(NodeType::ListItem, Fragment::from(vec![para]));
        let list = Node::element_with_content(NodeType::BulletList, Fragment::from(vec![item]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![list]));

        let state = EditorState::create_default(doc);
        let txn = run_command(&state, |s, d| {
            toggle_list(NodeType::OrderedList, NodeType::ListItem, s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);

        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::OrderedList));
    }

    // ── toggle_blockquote ──

    #[test]
    fn toggle_blockquote_wraps() {
        let state = EditorState::create_default(simple_doc());
        assert!(!is_in_blockquote(&state));

        let txn = run_command(&state, toggle_blockquote).unwrap();
        let new_state = state.apply(txn);

        let bq = new_state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        let para = bq.child(0).unwrap();
        assert_eq!(para.text_content(), "Hello world");
        assert!(is_in_blockquote(&new_state));
    }

    #[test]
    fn toggle_blockquote_unwraps() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Test")]),
        );
        let bq = Node::element_with_content(NodeType::Blockquote, Fragment::from(vec![para]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![bq]));

        let state = EditorState::create_default(doc);
        assert!(is_in_blockquote(&state));

        let txn = run_command(&state, toggle_blockquote).unwrap();
        let new_state = state.apply(txn);

        let first = new_state.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "Test");
    }

    // ── set_paragraph ──

    #[test]
    fn set_paragraph_from_heading() {
        let state = EditorState::create_default(heading_doc());
        let txn = run_command(&state, set_paragraph).unwrap();
        let new_state = state.apply(txn);

        let first = new_state.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "Title");
    }

    // ── insert_horizontal_rule ──

    #[test]
    fn insert_horizontal_rule_command() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, insert_horizontal_rule).unwrap();
        let new_state = state.apply(txn);

        // Doc should have: original paragraph, HR, new paragraph
        assert_eq!(new_state.doc.child_count(), 3);
        assert_eq!(
            new_state.doc.child(1).unwrap().node_type(),
            Some(NodeType::HorizontalRule)
        );
        assert_eq!(
            new_state.doc.child(2).unwrap().node_type(),
            Some(NodeType::Paragraph)
        );
    }

    // ── sink_list_item / lift_list_item ──

    fn two_item_list_doc() -> Node {
        // doc > bulletList > [listItem("first"), listItem("second")]
        Node::element_with_content(
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
        )
    }

    #[test]
    fn sink_list_item_not_applicable_outside_list() {
        let state = EditorState::create_default(simple_doc());
        assert!(!sink_list_item(&state, None));
    }

    #[test]
    fn sink_list_item_on_first_item_consumes_event() {
        let doc = two_item_list_doc();
        // Cursor in first item's paragraph content (pos 3)
        // Can't indent first item, but Tab should be consumed (not escape to UI)
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };
        assert!(sink_list_item(&state, None));
        // No transaction dispatched though
        let txn = run_command(&state, sink_list_item);
        assert!(txn.is_none(), "Should not dispatch a transaction for first item");
    }

    #[test]
    fn sink_list_item_indents_second_item() {
        let doc = two_item_list_doc();
        // Positions:
        //   0: BulletList open
        //   1: ListItem1 open
        //   2: Para open
        //   3..8: "first"
        //   8: Para close
        //   9: ListItem1 close
        //  10: ListItem2 open
        //  11: Para open
        //  12..18: "second"
        //  18: Para close
        //  19: ListItem2 close
        //  20: BulletList close
        // Cursor inside "second" at position 12
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, sink_list_item).unwrap();
        let new_state = state.apply(txn);

        // BulletList should now have 1 item
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 1);

        // That item should have: Paragraph("first") + BulletList > ListItem > Paragraph("second")
        let item = list.child(0).unwrap();
        assert_eq!(item.child_count(), 2);
        assert_eq!(item.child(0).unwrap().text_content(), "first");

        let sub_list = item.child(1).unwrap();
        assert_eq!(sub_list.node_type(), Some(NodeType::BulletList));
        assert_eq!(sub_list.child_count(), 1);
        assert_eq!(sub_list.child(0).unwrap().text_content(), "second");

        // Cursor should be inside "second" in the nested structure, not on "first"
        // After nesting: BulletList(0) > ListItem(1) > [Para("first")(2..9), BulletList(9) >
        //   ListItem(10) > Para("second")(11) > content at 12]
        // cursor_in_item was 12 - 11 = 1, new pos = 1 + 3 + 7 + 0 + 1 = 12
        assert_eq!(
            new_state.selection.from(),
            12,
            "Cursor should stay inside 'second', not jump to 'first'"
        );
    }

    #[test]
    fn lift_list_item_at_top_level_consumes_event() {
        let doc = two_item_list_doc();
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(doc)
        };
        // Second item is at top level — can't dedent, but Shift-Tab should be consumed
        assert!(lift_list_item(&state, None));
        // No transaction dispatched though
        let txn = run_command(&state, lift_list_item);
        assert!(txn.is_none(), "Should not dispatch a transaction at top level");
    }

    #[test]
    fn lift_list_item_dedents_nested_item() {
        // Build a nested structure:
        // doc > BulletList > ListItem > [Para("first"), BulletList > ListItem > Para("second")]
        let nested_doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![
                        Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("first")]),
                        ),
                        Node::element_with_content(
                            NodeType::BulletList,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::ListItem,
                                Fragment::from(vec![Node::element_with_content(
                                    NodeType::Paragraph,
                                    Fragment::from(vec![Node::text("second")]),
                                )]),
                            )]),
                        ),
                    ]),
                )]),
            )]),
        );
        // Positions:
        //  0: outer BulletList open
        //  1: outer ListItem open
        //  2: Para("first") open
        //  3..8: "first"
        //  8: Para close
        //  9: inner BulletList open
        // 10: inner ListItem open
        // 11: Para("second") open
        // 12..18: "second"
        // Cursor inside "second" at position 12
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(nested_doc)
        };
        assert!(lift_list_item(&state, None)); // should be applicable

        let txn = run_command(&state, lift_list_item).unwrap();
        let new_state = state.apply(txn);

        // Should now be: BulletList > [ListItem("first"), ListItem("second")]
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 2);
        assert_eq!(list.child(0).unwrap().text_content(), "first");
        assert_eq!(list.child(1).unwrap().text_content(), "second");

        // Cursor should be inside "second" after dedent
        // BulletList(0) > ListItem1(1..9) > ListItem2(10) > Para(11) > content at 12
        assert_eq!(
            new_state.selection.from(),
            12,
            "Cursor should stay inside 'second' after dedent"
        );
    }
}
