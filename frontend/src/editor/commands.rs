use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::position::resolve;
use super::selection::Selection;
use super::state::{EditorState, Transaction};

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
    let pos = state.selection.from();
    let Some(rp) = resolve(&state.doc, pos) else {
        return vec![];
    };
    let parent = rp.node_at(rp.depth, &state.doc);
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
/// Only works on Heading nodes (not paragraphs -- changing node type
/// requires ReplaceAroundStep which is not implemented in the MVP).
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

    // Only Heading nodes can have their level changed via SetAttr
    if parent_type != Some(NodeType::Heading) {
        return false;
    }

    if let Some(dispatch) = dispatch {
        // Compute the position of the heading node within its parent's content.
        // rp.start(rp.depth) is the absolute start of the heading's content.
        // Subtract 1 for the opening boundary to get the node's position
        // in the grandparent's content.
        // Then subtract rp.start(rp.depth - 1) to make it relative to
        // the grandparent's content (which is what set_attr_at walks).
        // set_attr_at walks from the doc root using absolute positions.
        // rp.start(rp.depth) is the heading's content start;
        // subtract 1 for the opening boundary to get the node position.
        let abs_pos = rp.start(rp.depth) - 1;

        if let Ok(txn) = state.transaction().step(super::transform::Step::SetAttr {
            pos: abs_pos,
            attr: "level".to_string(),
            value: level.to_string(),
        }) {
            dispatch(txn);
        }
    }
    true
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
    fn set_heading_on_paragraph_returns_false() {
        let state = EditorState::create_default(simple_doc());
        let applicable = set_heading(1, &state, None);
        assert!(!applicable); // paragraphs can't change level
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
}
