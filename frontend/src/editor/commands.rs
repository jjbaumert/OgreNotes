use std::collections::HashMap;

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::position::resolve;
use super::selection::Selection;
use super::state::{find_block_at, find_container_at, find_container_of_type, find_item_at, find_table_at, EditorState, Transaction};
use super::transform::Step;

/// A command function signature.
/// When `dispatch` is None, the command checks applicability (returns true/false).
/// When `dispatch` is Some, the command creates and dispatches a transaction.
pub type CommandFn = fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool;

// ─── Shared Helpers ─────────────────────────────────────────────

/// Check if a mark type can be applied at the selection's from position.
fn can_apply_mark_here(state: &EditorState, mark_type: MarkType) -> bool {
    let Some(rp) = resolve(&state.doc, state.selection.from()) else {
        return true; // can't resolve → don't block
    };
    let parent = rp.node_at(rp.depth, &state.doc);
    match parent.node_type() {
        Some(nt) => state.schema.can_apply_mark(nt, mark_type),
        None => true,
    }
}

/// Resolve the cursor position to the parent node type and its absolute position.
/// Returns `(parent_node_type, abs_pos_of_parent)`.
fn resolve_parent_type(state: &EditorState) -> Option<(NodeType, usize)> {
    let rp = resolve(&state.doc, state.selection.from())?;
    let parent = rp.node_at(rp.depth, &state.doc);
    let nt = parent.node_type()?;
    let start = rp.start(rp.depth);
    let abs_pos = start.checked_sub(1)?;
    Some((nt, abs_pos))
}

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
    if !can_apply_mark_here(state, mark_type) {
        return false;
    }

    let from = state.selection.from();
    let to = state.selection.to();

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

/// Toggle a mark with specific attributes on the current selection.
/// Used for link marks which carry an href attribute.
/// If the mark is already present, removes it. Otherwise adds with the given attrs.
pub fn toggle_link(
    href: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if state.selection.empty() || !can_apply_mark_here(state, MarkType::Link) {
        return false;
    }

    let from = state.selection.from();
    let to = state.selection.to();

    if let Some(dispatch) = dispatch {
        let result = if range_all_have_mark(&state.doc, from, to, MarkType::Link) {
            state.transaction().remove_mark(from, to, Mark::new(MarkType::Link))
        } else {
            let mark = Mark::new(MarkType::Link).with_attr("href", href);
            state.transaction().add_mark(from, to, mark)
        };
        if let Ok(txn) = result {
            dispatch(txn);
        }
    }
    true
}

/// Toggle a color-based mark (TextColor or Highlight) on the current selection.
/// If `color` is empty, removes the mark. Otherwise adds/replaces it.
pub fn toggle_color_mark(
    mark_type: MarkType,
    color: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if state.selection.empty() || !can_apply_mark_here(state, mark_type) {
        return false;
    }

    let from = state.selection.from();
    let to = state.selection.to();

    if let Some(dispatch) = dispatch {
        let result = if color.is_empty() {
            state.transaction().remove_mark(from, to, Mark::new(mark_type))
        } else {
            let add_mark = Mark::new(mark_type).with_attr("color", color);
            state.transaction()
                .remove_mark(from, to, Mark::new(mark_type))
                .and_then(|t| t.add_mark(from, to, add_mark))
        };
        if let Ok(txn) = result {
            dispatch(txn);
        }
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
                marks.iter().any(|m| m.mark_type == mark_type)
            } else {
                true
            }
        }
        Node::Element { content, node_type, .. } => {
            if node_type.is_leaf() {
                return true;
            }
            let mut pos = 0;
            for child in &content.children {
                let child_size = child.node_size();
                let child_end = pos + child_size;

                if child_end <= from { pos = child_end; continue; }
                if pos >= to { break; }

                // Compute range relative to child's content.
                // Text nodes have no open/close boundaries; elements skip 1 on each side.
                let (rel_from, rel_to) = if child.is_text() {
                    (from.saturating_sub(pos), to.min(child_end) - pos)
                } else if child.node_type().map_or(true, |nt| nt.is_leaf()) {
                    pos = child_end;
                    continue;
                } else {
                    let content_start = pos + 1;
                    let content_end = child_end - 1;
                    (from.saturating_sub(content_start), to.min(content_end) - content_start)
                };

                if !check_all_marks(child, rel_from, rel_to, mark_type, found_text) {
                    return false;
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
    let Some((parent_type, abs_pos)) = resolve_parent_type(state) else {
        return false;
    };

    match parent_type {
        NodeType::Heading => {
            if let Some(dispatch) = dispatch {
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
        NodeType::Paragraph => {
            if let Some(dispatch) = dispatch {
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

/// Convert the current block to a code block.
pub fn set_code_block(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((parent_type, abs_pos)) = resolve_parent_type(state) else {
        return false;
    };

    match parent_type {
        NodeType::CodeBlock => true,
        NodeType::Paragraph | NodeType::Heading => {
            if let Some(dispatch) = dispatch {
                if let Ok(txn) = state.transaction().step(Step::SetNodeType {
                    pos: abs_pos,
                    node_type: NodeType::CodeBlock,
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

/// Check if the cursor is in a paragraph (for toolbar state).
pub fn is_paragraph(state: &EditorState) -> bool {
    resolve_parent_type(state).map_or(false, |(nt, _)| nt == NodeType::Paragraph)
}

/// Check if the cursor is in a heading and return the level.
pub fn heading_level(state: &EditorState) -> Option<u8> {
    let (nt, _) = resolve_parent_type(state)?;
    if nt != NodeType::Heading { return None; }
    let rp = resolve(&state.doc, state.selection.from())?;
    rp.node_at(rp.depth, &state.doc)
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

/// Check if the cursor is in a code block.
pub fn is_in_code_block(state: &EditorState) -> bool {
    resolve_parent_type(state).map_or(false, |(nt, _)| nt == NodeType::CodeBlock)
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
    let Some((parent_type, abs_pos)) = resolve_parent_type(state) else {
        return false;
    };

    match parent_type {
        NodeType::Paragraph => {
            // Already a paragraph. If inside a container, lift out.
            if let Some(c) = find_container_at(&state.doc, state.selection.from()) {
                if let Some(dispatch) = dispatch {
                    if let Some(txn) = lift_from_container(state, &c) {
                        dispatch(txn);
                    }
                }
                return true;
            }
            true
        }
        NodeType::Heading | NodeType::CodeBlock => {
            if let Some(dispatch) = dispatch {
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

/// Collect all children of a node into a Vec.
fn collect_children(node: &Node) -> Vec<Node> {
    (0..node.child_count())
        .filter_map(|i| node.child(i).cloned())
        .collect()
}

/// Find the index of a child at a given offset within a container node.
fn find_child_index_at(container_node: &Node, container_offset: usize, target_offset: usize) -> Option<usize> {
    let mut offset = container_offset + 1; // skip container open boundary
    for i in 0..container_node.child_count() {
        if offset == target_offset {
            return Some(i);
        }
        offset += container_node.child(i).map_or(0, |c| c.node_size());
    }
    None
}

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

/// Wrap selected textblocks in a list.
/// If the selection spans multiple blocks, all blocks become list items in one list.
fn wrap_in_list(
    state: &EditorState,
    list_type: NodeType,
    item_type: NodeType,
) -> Option<Transaction> {
    let from = state.selection.from();
    let to = state.selection.to();

    // Find all top-level blocks that overlap with the selection
    let blocks = find_blocks_in_range(&state.doc, from, to);

    if blocks.is_empty() {
        return None;
    }

    if blocks.len() == 1 {
        // Single block: use ReplaceAround for efficiency
        let block = &blocks[0];
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

        return state.transaction().step(step).ok();
    }

    // Multiple blocks: wrap each as a ListItem, then replace the range with a single list
    let first_offset = blocks[0].offset;
    let last_end = blocks.last().unwrap().offset + blocks.last().unwrap().node_size;

    let mut items = Vec::new();
    for block in &blocks {
        // Clone the block and wrap it in a ListItem
        let block_node = extract_node_at(&state.doc, block.offset)?;
        items.push(Node::element_with_content(
            item_type,
            Fragment::from(vec![block_node]),
        ));
    }

    let list = Node::element_with_content(list_type, Fragment::from(items));
    let slice = Slice::new(Fragment::from(vec![list]), 0, 0);

    state.transaction().replace(first_offset, last_end, slice).ok()
}

/// Find all top-level blocks (textblocks and containers) that overlap with [from, to].
fn find_blocks_in_range(
    doc: &Node,
    from: usize,
    to: usize,
) -> Vec<super::state::BlockInfo> {
    let Node::Element { content, .. } = doc else {
        return Vec::new();
    };

    let mut results = Vec::new();
    let mut offset = 0;

    for child in &content.children {
        let child_size = child.node_size();
        let child_end = offset + child_size;

        if let Node::Element { node_type, attrs, content: child_content, .. } = child {
            let overlaps = child_end > from && offset < to;
            if overlaps && (node_type.is_textblock() || !node_type.is_leaf()) {
                results.push(super::state::BlockInfo {
                    offset,
                    node_size: child_size,
                    content_start: offset + 1,
                    node_type: *node_type,
                    attrs: attrs.clone(),
                    content: child_content.clone(),
                });
            }
        }

        offset += child_size;
    }

    results
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

    let item_index = match find_child_index_at(&list_node, list_container.offset, item.offset) {
        Some(0) | None => return true, // first item — can't indent, but consume Tab
        Some(i) => i,
    };

    if dispatch.is_none() {
        return true;
    }

    let prev_sibling = list_node.child(item_index - 1).cloned().unwrap();
    let cur_item = list_node.child(item_index).cloned().unwrap();
    let cursor_in_item = pos - item.content_start;

    // Build the new previous sibling: append cur_item into a sub-list.
    // If prev already ends with a sub-list of the same type, append to it.
    // Otherwise, create a new sub-list.
    let mut prev_children = collect_children(&prev_sibling);

    let list_type = list_container.node_type;
    let last_is_sublist = prev_children
        .last()
        .and_then(|n| n.node_type())
        .map_or(false, |nt| nt == list_type);

    // Compute sizes for cursor positioning BEFORE modifying prev_children
    let (prefix_size, items_before_cur_in_sublist) = if last_is_sublist {
        let existing = prev_children.last().unwrap();
        let before = existing.node_size() - 2; // content size of existing sub-list
        let prefix: usize = prev_children[..prev_children.len() - 1]
            .iter().map(|c| c.node_size()).sum();

        // Append cur_item to existing sub-list
        let old_sublist = prev_children.pop().unwrap();
        let mut sub_children = collect_children(&old_sublist);
        sub_children.push(cur_item);
        prev_children.push(old_sublist.copy_with_content(Fragment::from(sub_children)));

        (prefix, before)
    } else {
        let prefix: usize = prev_children.iter().map(|c| c.node_size()).sum();
        prev_children.push(
            Node::element_with_content(list_type, Fragment::from(vec![cur_item])),
        );
        (prefix, 0)
    };

    let new_prev = prev_sibling.copy_with_content(Fragment::from(prev_children));

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
    let Some(inner_list_node) = extract_node_at(&state.doc, inner_list.offset) else {
        return false;
    };
    let item_index = find_child_index_at(&inner_list_node, inner_list.offset, item.offset)
        .unwrap_or(0);
    let Some(cur_item) = inner_list_node.child(item_index).cloned() else {
        return false;
    };

    // Split inner list children around cur_item
    let before_items: Vec<Node> = (0..item_index)
        .filter_map(|i| inner_list_node.child(i).cloned())
        .collect();
    let after_items: Vec<Node> = (item_index + 1..inner_list_node.child_count())
        .filter_map(|i| inner_list_node.child(i).cloned())
        .collect();

    // Rebuild parent item: replace the inner list with a trimmed version (or remove it)
    let Some(parent_item_node) = extract_node_at(&state.doc, parent_item.offset) else {
        return false;
    };
    let new_parent_children: Vec<Node> = collect_children(&parent_item_node)
        .into_iter()
        .enumerate()
        .filter_map(|(i, child)| {
            let child_offset = parent_item.content_start
                + (0..i).filter_map(|j| parent_item_node.child(j)).map(|c| c.node_size()).sum::<usize>();
            if child_offset == inner_list.offset {
                // Replace inner list with trimmed version, or drop if empty
                if before_items.is_empty() { None }
                else { Some(Node::element_with_content(inner_list.node_type, Fragment::from(before_items.clone()))) }
            } else {
                Some(child)
            }
        })
        .collect();

    let new_parent = parent_item_node.copy_with_content(Fragment::from(new_parent_children));

    // If there are items after cur_item, attach them as a trailing sub-list
    let lifted_item = if after_items.is_empty() {
        cur_item
    } else {
        let trailing = Node::element_with_content(inner_list.node_type, Fragment::from(after_items));
        let mut children = collect_children(&cur_item);
        children.push(trailing);
        cur_item.copy_with_content(Fragment::from(children))
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

// ─── Table Commands ────────────────────────────────────────────

/// Check if the cursor is inside a table.
pub fn is_in_table(state: &EditorState) -> bool {
    find_table_at(&state.doc, state.selection.from()).is_some()
}

/// Insert a table with the given number of rows and columns after the current block.
pub fn insert_table(
    rows: usize,
    cols: usize,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if rows == 0 || cols == 0 {
        return false;
    }

    if let Some(dispatch) = dispatch {
        let pos = state.selection.from();
        let block = find_block_at(&state.doc, pos);
        let container = find_container_at(&state.doc, pos);

        // Insert after the outermost container (or block)
        let insert_pos = if let Some(c) = container {
            c.offset + c.node_size
        } else if let Some(b) = block {
            b.offset + b.node_size
        } else {
            state.doc.content_size()
        };

        // Build table: rows × cols, each cell has one empty paragraph
        let table_rows: Vec<Node> = (0..rows)
            .map(|_| {
                let cells: Vec<Node> = (0..cols)
                    .map(|_| {
                        Node::element_with_content(
                            NodeType::TableCell,
                            Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                        )
                    })
                    .collect();
                Node::element_with_content(NodeType::TableRow, Fragment::from(cells))
            })
            .collect();
        let table = Node::element_with_content(NodeType::Table, Fragment::from(table_rows));
        let slice = Slice::new(Fragment::from(vec![table]), 0, 0);

        if let Ok(mut txn) = state.transaction().replace(insert_pos, insert_pos, slice) {
            // Cursor inside first cell: table(+1) + row(+1) + cell(+1) + para(+1) = +4
            txn.selection = Selection::cursor(insert_pos + 4);
            dispatch(txn);
        }
    }
    true
}

/// Add a row after the current row in the table.
pub fn add_row(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        // Build a new row with the same number of cells
        let cells: Vec<Node> = (0..info.num_cols)
            .map(|_| {
                Node::element_with_content(
                    NodeType::TableCell,
                    Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                )
            })
            .collect();
        let new_row = Node::element_with_content(NodeType::TableRow, Fragment::from(cells));
        let insert_pos = info.row_offset + info.row_node_size;
        let slice = Slice::new(Fragment::from(vec![new_row]), 0, 0);

        if let Ok(mut txn) = state.transaction().replace(insert_pos, insert_pos, slice) {
            // Cursor in first cell of new row: row(+1) + cell(+1) + para(+1) = +3
            txn.selection = Selection::cursor(insert_pos + 3);
            dispatch(txn);
        }
    }
    true
}

/// Delete the current row. If it's the only row, delete the entire table.
pub fn delete_row(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        if info.num_rows <= 1 {
            // Only row → delete entire table, insert empty paragraph
            let para = Node::element(NodeType::Paragraph);
            let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
            if let Ok(mut txn) = state.transaction().replace(
                info.table_offset,
                info.table_offset + info.table_node_size,
                slice,
            ) {
                txn.selection = Selection::cursor(info.table_offset + 1);
                dispatch(txn);
            }
        } else {
            // Delete just this row
            if let Ok(mut txn) = state.transaction().delete(
                info.row_offset,
                info.row_offset + info.row_node_size,
            ) {
                // Place cursor in the cell at the same column in adjacent row
                if let Some(sel) = Selection::find_from(&txn.doc, info.row_offset, 1)
                    .or_else(|| Selection::find_from(&txn.doc, info.row_offset, -1))
                {
                    txn.selection = sel;
                }
                dispatch(txn);
            }
        }
    }
    true
}

/// Add a column after the current column in every row (process bottom-to-top).
pub fn add_column(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        let table_node = extract_node_at(&state.doc, info.table_offset);
        let Some(table) = table_node else { return false };

        let col_idx = info.cell_index;
        let mut txn = state.transaction();

        // Process rows bottom-to-top so position shifts don't affect earlier rows
        let rows: Vec<_> = (0..table.child_count()).collect();
        for &row_i in rows.iter().rev() {
            let Some(row) = table.child(row_i) else { continue };
            // Find the cell at col_idx in this row
            let mut cell_offset = info.table_offset + 1; // table content start
            // Walk to this row
            for ri in 0..row_i {
                if let Some(r) = table.child(ri) {
                    cell_offset += r.node_size();
                }
            }
            cell_offset += 1; // row open boundary
            // Walk to the cell after col_idx
            for ci in 0..=col_idx.min(row.child_count().saturating_sub(1)) {
                if let Some(c) = row.child(ci) {
                    cell_offset += c.node_size();
                }
            }
            // Insert new empty cell at cell_offset
            let new_cell = Node::element_with_content(
                NodeType::TableCell,
                Fragment::from(vec![Node::element(NodeType::Paragraph)]),
            );
            let slice = Slice::new(Fragment::from(vec![new_cell]), 0, 0);
            match txn.step(Step::Replace {
                from: cell_offset,
                to: cell_offset,
                slice,
            }) {
                Ok(t) => txn = t,
                Err(_) => return false,
            }
        }

        dispatch(txn);
    }
    true
}

/// Delete the current column from every row (process bottom-to-top).
/// If it's the last column, delete the entire table.
pub fn delete_column(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        if info.num_cols <= 1 {
            // Last column → delete entire table
            let para = Node::element(NodeType::Paragraph);
            let slice = Slice::new(Fragment::from(vec![para]), 0, 0);
            if let Ok(mut txn) = state.transaction().replace(
                info.table_offset,
                info.table_offset + info.table_node_size,
                slice,
            ) {
                txn.selection = Selection::cursor(info.table_offset + 1);
                dispatch(txn);
            }
        } else {
            let table_node = extract_node_at(&state.doc, info.table_offset);
            let Some(table) = table_node else { return false };

            let col_idx = info.cell_index;
            let mut txn = state.transaction();

            // Process rows bottom-to-top
            let rows: Vec<_> = (0..table.child_count()).collect();
            for &row_i in rows.iter().rev() {
                let Some(row) = table.child(row_i) else { continue };
                if col_idx >= row.child_count() { continue; }

                // Find the cell at col_idx in this row
                let mut cell_start = info.table_offset + 1;
                for ri in 0..row_i {
                    if let Some(r) = table.child(ri) {
                        cell_start += r.node_size();
                    }
                }
                cell_start += 1; // row open boundary
                for ci in 0..col_idx {
                    if let Some(c) = row.child(ci) {
                        cell_start += c.node_size();
                    }
                }
                let cell_size = row.child(col_idx).map(|c| c.node_size()).unwrap_or(0);
                if cell_size > 0 {
                    match txn.step(Step::Replace {
                        from: cell_start,
                        to: cell_start + cell_size,
                        slice: Slice::empty(),
                    }) {
                        Ok(t) => txn = t,
                        Err(_) => return false,
                    }
                }
            }

            // Find valid cursor position in the result
            if let Some(sel) = Selection::find_from(&txn.doc, info.cell_offset.min(txn.doc.content_size()), 1) {
                txn.selection = sel;
            }
            dispatch(txn);
        }
    }
    true
}

/// Select all content in the current table row.
/// Creates a text selection spanning from the first cell's content to the last cell's content.
pub fn select_table_row(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        // First cell starts at row_offset + 1 (row open) + 1 (cell open) + 1 (para open)
        let row_content_start = info.row_offset + 3;
        // Last cell ends at row_offset + row_node_size - 1 (row close) - 1 (cell close) - 1 (para close)
        let row_content_end = info.row_offset + info.row_node_size - 3;
        let txn = state.transaction()
            .set_selection(Selection::text(row_content_start, row_content_end));
        dispatch(txn);
    }
    true
}

/// Select all content in the current table column.
/// Creates a text selection spanning from the first row's cell to the last row's cell in this column.
pub fn select_table_column(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        let table = extract_node_at(&state.doc, info.table_offset);
        let Some(table) = table else { return false };

        // Find the cell at col_idx in the first row
        let first_row_offset = info.table_offset + 1; // table content start
        let Some(first_row) = table.child(0) else { return false };
        let mut first_cell_offset = first_row_offset + 1; // first row content start
        for ci in 0..info.cell_index {
            if let Some(c) = first_row.child(ci) {
                first_cell_offset += c.node_size();
            }
        }
        let first_cell_content = first_cell_offset + 2; // cell open + para open

        // Find the cell at col_idx in the last row
        let last_row_idx = table.child_count().saturating_sub(1);
        let mut last_row_offset = info.table_offset + 1;
        for ri in 0..last_row_idx {
            if let Some(r) = table.child(ri) {
                last_row_offset += r.node_size();
            }
        }
        let Some(last_row) = table.child(last_row_idx) else { return false };
        let mut last_cell_offset = last_row_offset + 1;
        for ci in 0..info.cell_index {
            if let Some(c) = last_row.child(ci) {
                last_cell_offset += c.node_size();
            }
        }
        let last_cell_size = last_row.child(info.cell_index)
            .map(|c| c.node_size()).unwrap_or(4);
        let last_cell_content_end = last_cell_offset + last_cell_size - 2; // cell close + para close

        let txn = state.transaction()
            .set_selection(Selection::text(first_cell_content, last_cell_content_end));
        dispatch(txn);
    }
    true
}

/// Delete the contents of all cells in a table-spanning selection.
/// Instead of removing structural elements, replaces each cell's content with an empty paragraph.
/// Returns true if the selection spans table cells and was handled.
pub fn delete_table_selection(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let from = state.selection.from();
    let to = state.selection.to();
    if from == to { return false; }

    // Check if both ends are in the same table
    let from_table = find_table_at(&state.doc, from);
    let to_table = find_table_at(&state.doc, to);
    let (Some(ft), Some(tt)) = (&from_table, &to_table) else { return false };
    if ft.table_offset != tt.table_offset { return false; }

    // Only handle if the selection spans multiple cells
    if ft.cell_offset == tt.cell_offset { return false; }

    if let Some(dispatch) = dispatch {
        let table = extract_node_at(&state.doc, ft.table_offset);
        let Some(table) = table else { return false };

        // Walk all cells in the table, clear those that overlap with the selection
        let mut txn = state.transaction();
        let mut row_offset = ft.table_offset + 1;

        // Collect all cell ranges to clear (from last to first to avoid position shifts)
        let mut cells_to_clear: Vec<(usize, usize)> = Vec::new();
        for ri in 0..table.child_count() {
            let Some(row) = table.child(ri) else { continue };
            let mut cell_offset = row_offset + 1;
            for ci in 0..row.child_count() {
                let Some(cell) = row.child(ci) else { continue };
                let cell_size = cell.node_size();
                let cell_content_start = cell_offset + 1;
                let cell_content_end = cell_offset + cell_size - 1;

                // Check if this cell overlaps with the selection
                if cell_content_end > from && cell_content_start < to {
                    cells_to_clear.push((cell_content_start, cell_content_end));
                }
                cell_offset += cell_size;
            }
            row_offset += row.node_size();
        }

        // Clear cells from last to first
        for (cs, ce) in cells_to_clear.into_iter().rev() {
            let empty_para = Node::element(NodeType::Paragraph);
            let slice = Slice::new(Fragment::from(vec![empty_para]), 0, 0);
            match txn.step(Step::Replace { from: cs, to: ce, slice }) {
                Ok(t) => txn = t,
                Err(_) => return false,
            }
        }

        // Place cursor in the first affected cell
        if let Some(sel) = Selection::find_from(&txn.doc, from, 1) {
            txn.selection = sel;
        }
        dispatch(txn);
    }
    true
}

/// Tab in table: move cursor to next cell. At last cell, add a new row.
pub fn table_tab_forward(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        // Next cell in same row
        if info.cell_index + 1 < info.num_cols {
            let next_cell_offset = info.cell_offset + info.cell_node_size;
            // +1 to enter the cell, +1 to enter its paragraph
            let txn = state.transaction().set_selection(Selection::cursor(next_cell_offset + 2));
            dispatch(txn);
        }
        // First cell of next row
        else if info.row_index + 1 < info.num_rows {
            let next_row_offset = info.row_offset + info.row_node_size;
            // +1 row open, +1 cell open, +1 para open = +3
            let txn = state.transaction().set_selection(Selection::cursor(next_row_offset + 3));
            dispatch(txn);
        }
        // Last cell of last row → add a new row and move into it
        else {
            add_row(state, Some(&|txn| dispatch(txn)));
        }
    }
    true
}

/// Shift-Tab in table: move cursor to previous cell.
pub fn table_tab_backward(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(info) = find_table_at(&state.doc, state.selection.from()) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        if info.cell_index > 0 || info.row_index > 0 {
            // Find previous cell: walk backward from current cell_offset
            // The previous cell ends at cell_offset, so its content is just before
            if let Some(sel) = Selection::find_from(&state.doc, info.cell_offset.saturating_sub(1), -1) {
                let txn = state.transaction().set_selection(sel);
                dispatch(txn);
            }
        }
        // At first cell of first row → do nothing (consume the key)
    }
    true
}

/// Tab command: table navigation takes priority, then list indent.
pub fn tab_command(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if is_in_table(state) {
        return table_tab_forward(state, dispatch);
    }
    sink_list_item(state, dispatch)
}

/// Shift-Tab command: table navigation takes priority, then list dedent.
pub fn shift_tab_command(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if is_in_table(state) {
        return table_tab_backward(state, dispatch);
    }
    lift_list_item(state, dispatch)
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

    // ── toggle_link ──

    #[test]
    fn toggle_link_adds_href() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_link("https://example.com", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        let link = first.marks().iter().find(|m| m.mark_type == MarkType::Link);
        assert!(link.is_some());
        assert_eq!(link.unwrap().attrs.get("href").unwrap(), "https://example.com");
    }

    #[test]
    fn toggle_link_removes_existing() {
        // Doc with linked text
        let link = Mark::new(MarkType::Link).with_attr("href", "https://example.com");
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks("Hello", vec![link])]),
            )]),
        );
        let state = EditorState::create_default(doc);
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_link("https://example.com", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert!(para.child(0).unwrap().marks().is_empty());
    }

    #[test]
    fn toggle_link_cursor_returns_false() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::cursor(3),
            ..state
        };
        assert!(!toggle_link("https://example.com", &state, None));
    }

    #[test]
    fn toggle_link_in_code_block_returns_false() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("code")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::text(1, 5),
            ..EditorState::create_default(doc)
        };
        assert!(!toggle_link("https://example.com", &state, None));
    }

    // ── toggle_color_mark ──

    #[test]
    fn toggle_color_mark_adds_color() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_color_mark(MarkType::TextColor, "#E53935", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        let color_mark = first.marks().iter().find(|m| m.mark_type == MarkType::TextColor);
        assert!(color_mark.is_some());
        assert_eq!(color_mark.unwrap().attrs.get("color").unwrap(), "#E53935");
    }

    #[test]
    fn toggle_color_mark_removes_with_empty_color() {
        let color = Mark::new(MarkType::TextColor).with_attr("color", "#E53935");
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks("Hello", vec![color])]),
            )]),
        );
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, |s, d| toggle_color_mark(MarkType::TextColor, "", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert!(!para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::TextColor));
    }

    #[test]
    fn toggle_color_mark_replaces_color() {
        let color = Mark::new(MarkType::TextColor).with_attr("color", "#E53935");
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks("Hello", vec![color])]),
            )]),
        );
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, |s, d| toggle_color_mark(MarkType::TextColor, "#1E88E5", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let color_mark = para.child(0).unwrap().marks().iter()
            .find(|m| m.mark_type == MarkType::TextColor).cloned();
        assert!(color_mark.is_some());
        assert_eq!(color_mark.unwrap().attrs.get("color").unwrap(), "#1E88E5");
    }

    #[test]
    fn toggle_highlight_adds_color() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_color_mark(MarkType::Highlight, "#FFF176", s, d)).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let mark = para.child(0).unwrap().marks().iter()
            .find(|m| m.mark_type == MarkType::Highlight).cloned();
        assert!(mark.is_some());
        assert_eq!(mark.unwrap().attrs.get("color").unwrap(), "#FFF176");
    }

    #[test]
    fn toggle_color_mark_cursor_returns_false() {
        let state = EditorState::create_default(simple_doc());
        assert!(!toggle_color_mark(MarkType::TextColor, "#E53935", &state, None));
    }

    #[test]
    fn toggle_color_mark_in_code_block_returns_false() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("code")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::text(1, 5),
            ..EditorState::create_default(doc)
        };
        assert!(!toggle_color_mark(MarkType::TextColor, "#E53935", &state, None));
    }

    // ── set_code_block ──

    fn code_block_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text("fn main() {}")]),
            )]),
        )
    }

    #[test]
    fn set_code_block_from_paragraph() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, set_code_block).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::CodeBlock));
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello world");
    }

    #[test]
    fn set_code_block_from_heading() {
        let state = EditorState::create_default(heading_doc());
        let txn = run_command(&state, set_code_block).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::CodeBlock));
    }

    #[test]
    fn set_code_block_already_code_block() {
        let state = EditorState::create_default(code_block_doc());
        // Applicable (returns true) but no transaction dispatched
        assert!(set_code_block(&state, None));
        let txn = run_command(&state, set_code_block);
        assert!(txn.is_none());
    }

    #[test]
    fn set_code_block_on_list_returns_false() {
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
        let state = EditorState {
            selection: Selection::cursor(3),
            ..EditorState::create_default(doc)
        };
        // Cursor resolves inside Paragraph inside ListItem — set_code_block
        // should work on the paragraph (it matches Paragraph arm)
        let applicable = set_code_block(&state, None);
        assert!(applicable);
    }

    // ── is_in_code_block ──

    #[test]
    fn is_in_code_block_true() {
        let state = EditorState::create_default(code_block_doc());
        assert!(is_in_code_block(&state));
    }

    #[test]
    fn is_in_code_block_false() {
        let state = EditorState::create_default(simple_doc());
        assert!(!is_in_code_block(&state));
    }

    // ── set_paragraph (additional cases) ──

    #[test]
    fn set_paragraph_from_code_block() {
        let state = EditorState::create_default(code_block_doc());
        let txn = run_command(&state, set_paragraph).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "fn main() {}");
    }

    #[test]
    fn set_paragraph_already_bare_paragraph() {
        let state = EditorState::create_default(simple_doc());
        // Applicable (returns true) but should be a no-op
        assert!(set_paragraph(&state, None));
    }

    #[test]
    fn set_paragraph_from_paragraph_in_blockquote_lifts() {
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
        let state = EditorState {
            selection: Selection::cursor(2),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, set_paragraph).unwrap();
        let new_state = state.apply(txn);
        let first = new_state.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph));
        assert_eq!(first.text_content(), "quoted");
    }

    // ── toggle_list with TaskList ──

    #[test]
    fn toggle_task_list_wraps() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| {
            toggle_list(NodeType::TaskList, NodeType::TaskItem, s, d)
        }).unwrap();
        let new_state = state.apply(txn);

        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::TaskList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::TaskItem));
        assert_eq!(item.attrs().get("checked").unwrap(), "false");
    }

    #[test]
    fn toggle_list_converts_bullet_to_task() {
        let para = Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("Test")]),
        );
        let item = Node::element_with_content(NodeType::ListItem, Fragment::from(vec![para]));
        let list = Node::element_with_content(NodeType::BulletList, Fragment::from(vec![item]));
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![list]));

        let state = EditorState::create_default(doc);
        let txn = run_command(&state, |s, d| {
            toggle_list(NodeType::TaskList, NodeType::TaskItem, s, d)
        }).unwrap();
        let new_state = state.apply(txn);

        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::TaskList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::TaskItem));
        assert_eq!(item.attrs().get("checked").unwrap(), "false");
    }

    // ── insert_horizontal_rule inside container ──

    #[test]
    fn insert_horizontal_rule_in_blockquote() {
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
        let state = EditorState {
            selection: Selection::cursor(2),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, insert_horizontal_rule).unwrap();
        let new_state = state.apply(txn);

        // HR should be inserted AFTER the blockquote (not inside it)
        let has_hr = (0..new_state.doc.child_count())
            .any(|i| new_state.doc.child(i).unwrap().node_type() == Some(NodeType::HorizontalRule));
        assert!(has_hr, "HR should be inserted after the container");
    }

    // ── sink_list_item into existing sub-list ──

    #[test]
    fn sink_list_item_appends_to_existing_sublist() {
        // doc > BulletList > [
        //   ListItem > [Para("first"), BulletList > [ListItem > Para("nested")]],
        //   ListItem > Para("third")
        // ]
        // Indenting "third" should append it to the existing sub-list, not create a new one.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
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
                                        Fragment::from(vec![Node::text("nested")]),
                                    )]),
                                )]),
                            ),
                        ]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("third")]),
                        )]),
                    ),
                ]),
            )]),
        );
        // Positions:
        //  0: outer BulletList
        //  1: ListItem1
        //  2: Para("first") open
        //  3..8: "first"
        //  8: Para close
        //  9: inner BulletList open
        // 10: inner ListItem open
        // 11: Para("nested") open
        // 12..18: "nested"
        // 18: Para close
        // 19: inner ListItem close
        // 20: inner BulletList close
        // 21: ListItem1 close
        // 22: ListItem2 open
        // 23: Para("third") open
        // 24..29: "third"
        let state = EditorState {
            selection: Selection::cursor(24),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, sink_list_item).unwrap();
        let new_state = state.apply(txn);

        // Outer list should now have 1 item
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.child_count(), 1);

        let item = list.child(0).unwrap();
        // Should have: Para("first") + BulletList with 2 items (nested + third)
        let sub_list = item.child(1).unwrap();
        assert_eq!(sub_list.node_type(), Some(NodeType::BulletList));
        assert_eq!(sub_list.child_count(), 2, "should append to existing sub-list, not create new");
        assert_eq!(sub_list.child(0).unwrap().text_content(), "nested");
        assert_eq!(sub_list.child(1).unwrap().text_content(), "third");
    }

    // ── lift_list_item with trailing items ──

    #[test]
    fn lift_list_item_with_trailing_siblings() {
        // doc > BulletList > ListItem > [Para("parent"),
        //   BulletList > [ListItem > Para("a"), ListItem > Para("b"), ListItem > Para("c")]]
        // Lifting "b" should produce:
        // BulletList > [ListItem > [Para("parent"), BulletList > [ListItem > Para("a")]],
        //               ListItem > [Para("b"), BulletList > [ListItem > Para("c")]]]
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![
                        Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("parent")]),
                        ),
                        Node::element_with_content(
                            NodeType::BulletList,
                            Fragment::from(vec![
                                Node::element_with_content(
                                    NodeType::ListItem,
                                    Fragment::from(vec![Node::element_with_content(
                                        NodeType::Paragraph,
                                        Fragment::from(vec![Node::text("a")]),
                                    )]),
                                ),
                                Node::element_with_content(
                                    NodeType::ListItem,
                                    Fragment::from(vec![Node::element_with_content(
                                        NodeType::Paragraph,
                                        Fragment::from(vec![Node::text("b")]),
                                    )]),
                                ),
                                Node::element_with_content(
                                    NodeType::ListItem,
                                    Fragment::from(vec![Node::element_with_content(
                                        NodeType::Paragraph,
                                        Fragment::from(vec![Node::text("c")]),
                                    )]),
                                ),
                            ]),
                        ),
                    ]),
                )]),
            )]),
        );

        // Find position inside "b":
        // 0: outer BulletList
        // 1: outer ListItem
        // 2: Para("parent") open, 3..9: "parent", 9: close
        // 10: inner BulletList
        // 11: ListItem("a") open, 12: Para open, 13: "a", 14: Para close, 15: LI close
        // 16: ListItem("b") open, 17: Para open, 18: "b", 19: Para close, 20: LI close
        // 21: ListItem("c") open, ...
        let state = EditorState {
            selection: Selection::cursor(18),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, lift_list_item).unwrap();
        let new_state = state.apply(txn);

        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.child_count(), 2);

        // First item: "parent" + sub-list with just "a"
        let first_item = list.child(0).unwrap();
        assert_eq!(first_item.child(0).unwrap().text_content(), "parent");
        let trimmed_sublist = first_item.child(1).unwrap();
        assert_eq!(trimmed_sublist.child_count(), 1);
        assert_eq!(trimmed_sublist.child(0).unwrap().text_content(), "a");

        // Second item: "b" + trailing sub-list with "c"
        let second_item = list.child(1).unwrap();
        assert_eq!(second_item.child(0).unwrap().text_content(), "b");
        let trailing = second_item.child(1).unwrap();
        assert_eq!(trailing.node_type(), Some(NodeType::BulletList));
        assert_eq!(trailing.child_count(), 1);
        assert_eq!(trailing.child(0).unwrap().text_content(), "c");
    }

    // ── Table commands ──

    fn table_doc(rows: usize, cols: usize) -> Node {
        let table_rows: Vec<Node> = (0..rows).map(|r| {
            let cells: Vec<Node> = (0..cols).map(|c| {
                Node::element_with_content(
                    NodeType::TableCell,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text(&format!("r{}c{}", r + 1, c + 1))]),
                    )]),
                )
            }).collect();
            Node::element_with_content(NodeType::TableRow, Fragment::from(cells))
        }).collect();
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Table,
                Fragment::from(table_rows),
            )]),
        )
    }

    #[test]
    fn is_in_table_true() {
        let doc = table_doc(2, 2);
        // Position 4 is inside first cell's paragraph content
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        assert!(is_in_table(&state));
    }

    #[test]
    fn is_in_table_false() {
        let state = EditorState::create_default(simple_doc());
        assert!(!is_in_table(&state));
    }

    #[test]
    fn insert_table_creates_3x3() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| insert_table(3, 3, s, d)).unwrap();
        let new_state = state.apply(txn);
        // Should have original paragraph + table
        assert_eq!(new_state.doc.child_count(), 2);
        let table = new_state.doc.child(1).unwrap();
        assert_eq!(table.node_type(), Some(NodeType::Table));
        assert_eq!(table.child_count(), 3); // 3 rows
        let row = table.child(0).unwrap();
        assert_eq!(row.child_count(), 3); // 3 cells per row
    }

    #[test]
    fn add_row_to_table() {
        let doc = table_doc(2, 3);
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, add_row).unwrap();
        let new_state = state.apply(txn);
        let table = new_state.doc.child(0).unwrap();
        assert_eq!(table.child_count(), 3, "should have 3 rows after add_row");
    }

    #[test]
    fn delete_row_from_table() {
        let doc = table_doc(3, 2);
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, delete_row).unwrap();
        let new_state = state.apply(txn);
        let table = new_state.doc.child(0).unwrap();
        assert_eq!(table.child_count(), 2, "should have 2 rows after delete_row");
    }

    #[test]
    fn delete_only_row_removes_table() {
        let doc = table_doc(1, 2);
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, delete_row).unwrap();
        let new_state = state.apply(txn);
        // Table should be replaced with a paragraph
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn join_backward_at_cell_boundary_fails() {
        let doc = table_doc(2, 2);
        // Cursor at start of first cell's paragraph content
        // table(0)+1, row(1)+1, cell(2)+1, para(3)+1 = 4
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        // join_backward should fail (isolation)
        assert!(state.transaction().join_backward().is_err());
    }

    #[test]
    fn tab_in_table_moves_to_next_cell() {
        let doc = table_doc(2, 3);
        // Cursor in first cell
        let state = EditorState {
            selection: Selection::cursor(4),
            ..EditorState::create_default(doc)
        };
        let txn = run_command(&state, table_tab_forward).unwrap();
        let new_state = state.apply(txn);
        // Should be in the second cell now (pos > first cell end)
        assert!(new_state.selection.from() > 4 + 4, // past first cell
            "cursor should move to next cell, got {}",
            new_state.selection.from());
    }
}
