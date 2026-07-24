// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
///
/// For range selections that start exactly at a doc/structural
/// boundary (typically `Ctrl+A` which lands `from` at position 0 of
/// the doc), the resolved parent is the Doc node — which the schema
/// rightly rejects for inline marks. Peek one position further in to
/// see the first actual content block; if it allows the mark, allow
/// the operation. Code-block rejections are preserved because both
/// `from` and `from + 1` resolve inside the code block (so both
/// fail the schema check).
fn can_apply_mark_here(state: &EditorState, mark_type: MarkType) -> bool {
    let check = |pos: usize| -> Option<bool> {
        let rp = resolve(&state.doc, pos)?;
        let parent = rp.node_at(rp.depth, &state.doc);
        parent
            .node_type()
            .map(|nt| state.schema.can_apply_mark(nt, mark_type))
    };
    let from = state.selection.from();
    if check(from).unwrap_or(true) {
        return true;
    }
    // Range selections originating at a structural boundary need a
    // one-position peek inside the first content block. Cursor
    // selections stay strict — a cursor exactly at the doc boundary
    // isn't a position the user should mark.
    if !state.selection.empty() && from < state.selection.to() {
        if let Some(true) = check(from + 1) {
            return true;
        }
    }
    false
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
                // #143: applying sub/superscript strips its exclusive partner.
                if let Some(partner) = mark_type.exclusive_partner() {
                    current.retain(|m| m.mark_type != partner);
                }
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
            // #143: strip the mutually-exclusive partner (sub<->sup) before
            // applying, so a character never carries both. remove_mark consumes
            // the txn, so rebind it; if it fails (no-op), start fresh.
            let base = match mark_type.exclusive_partner() {
                Some(partner) => state
                    .transaction()
                    .remove_mark(from, to, Mark::new(partner))
                    .unwrap_or_else(|_| state.transaction()),
                None => state.transaction(),
            };
            base.add_mark(from, to, mark)
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

/// Language attribute of the code block containing the cursor.
/// `None` = not in a code block; `Some("")` = code block, no language.
pub fn code_block_language(state: &EditorState) -> Option<String> {
    let (nt, _) = resolve_parent_type(state)?;
    if nt != NodeType::CodeBlock {
        return None;
    }
    let rp = resolve(&state.doc, state.selection.from())?;
    Some(
        rp.node_at(rp.depth, &state.doc)
            .attrs()
            .get("language")
            .cloned()
            .unwrap_or_default(),
    )
}

/// Set (or clear, with `""`) the `language` attribute of the code
/// block containing the cursor. Same targeted `Step::SetAttr` shape
/// as `update_mermaid_source` — node identity and content untouched.
/// Returns `false` when the cursor is not in a code block.
pub fn set_code_block_language(
    lang: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((NodeType::CodeBlock, abs_pos)) = resolve_parent_type(state) else {
        return false;
    };
    if let Some(dispatch) = dispatch {
        if let Ok(txn) = state.transaction().step(Step::SetAttr {
            pos: abs_pos,
            attr: "language".to_string(),
            value: lang.to_string(),
        }) {
            dispatch(txn);
        }
    }
    true
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

/// Set the `text-align` of the textblock at the cursor. `align`
/// is one of "left", "center", "right" — anything else is rejected.
/// Passing "left" clears the attribute (left is the natural default
/// so we don't bother persisting it). Operates on the innermost
/// textblock returned by `find_block_at` — Paragraph / Heading /
/// CodeBlock / list-item-paragraphs all benefit from this. Returns
/// true when a transaction was dispatched (or could have been), so
/// the caller can update toolbar state.
pub fn set_alignment(
    align: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if !matches!(align, "left" | "center" | "right") {
        return false;
    }
    let pos = state.selection.from();
    let Some(block) = find_block_at(&state.doc, pos) else {
        return false;
    };
    // Don't apply to leaf or non-textblock containers.
    if !block.node_type.is_textblock() {
        return false;
    }
    let mut attrs = block.attrs.clone();
    if align == "left" {
        attrs.remove("align");
    } else {
        attrs.insert("align".to_string(), align.to_string());
    }
    // No-op when the requested alignment matches the current state.
    if attrs == block.attrs {
        return true;
    }
    if let Some(dispatch) = dispatch {
        if let Ok(txn) = state.transaction().step(Step::SetNodeType {
            pos: block.offset,
            node_type: block.node_type,
            attrs,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// #134: strip every inline mark from the current selection — the
/// "Clear Formatting" action. A bare cursor has nothing to clear, so it's a
/// no-op there. Removing a mark that isn't present is a safe no-op
/// (`apply_remove_mark` filters by mark type), so we remove every mark
/// type in one transaction without first checking which are present.
/// Block-level formatting (heading/list/alignment) is intentionally left
/// alone — this clears inline marks only.
pub fn clear_formatting(state: &EditorState, dispatch: Option<&dyn Fn(Transaction)>) -> bool {
    if state.selection.empty() {
        return false;
    }
    let from = state.selection.from();
    let to = state.selection.to();
    let Some(dispatch) = dispatch else {
        return true;
    };

    let mut txn = state.transaction();
    for mark_type in [
        MarkType::Bold,
        MarkType::Italic,
        MarkType::Underline,
        MarkType::Strike,
        MarkType::Code,
        MarkType::TextColor,
        MarkType::Highlight,
        MarkType::Subscript,
        MarkType::Superscript,
        MarkType::Link,
    ] {
        match txn.remove_mark(from, to, Mark::new(mark_type)) {
            Ok(t) => txn = t,
            // A valid selection range can't fail here; bail without
            // dispatching a partial transaction if it somehow does.
            Err(_) => return false,
        }
    }
    dispatch(txn);
    true
}

// ─── #147: find & replace ──────────────────────────────────────────

/// Select the model range `from..to` (no content change). Used by the
/// find/replace bar to navigate to a match — the editor renders the
/// selection and scrolls it into view.
pub fn select_range(
    from: usize,
    to: usize,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(dispatch) = dispatch else { return true };
    let mut txn = state.transaction();
    txn.selection = Selection::text(from, to);
    dispatch(txn);
    true
}

/// Build a slice carrying `text` (empty → an empty slice = pure delete).
fn text_slice(text: &str) -> Slice {
    if text.is_empty() {
        Slice::empty()
    } else {
        Slice::new(Fragment::from(vec![Node::text(text)]), 0, 0)
    }
}

/// Replace the model range `from..to` with `text`, leaving the cursor after
/// the inserted text. Goes through the transaction API so the edit syncs to
/// collaborators like any other.
pub fn replace_range(
    from: usize,
    to: usize,
    text: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(dispatch) = dispatch else { return true };
    if let Ok(mut txn) = state.transaction().replace(from, to, text_slice(text)) {
        txn.selection = Selection::cursor(from + text.chars().count());
        dispatch(txn);
        true
    } else {
        false
    }
}

/// #148: insert a document mention. Replaces `[from, to)` — the `@query`
/// trigger text — with `title`, carrying a `Link` mark to `href`, in a SINGLE
/// transaction so it's one undo step. Leaves the cursor just after the link.
/// `from`/`to`/the mark range are in the post-replace coordinate frame (the
/// replace step inserts `title` at `[from, from + title.chars().count())`,
/// which is exactly the range we mark).
pub fn insert_doc_link(
    from: usize,
    to: usize,
    title: &str,
    href: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if title.is_empty() {
        return false;
    }
    let Some(dispatch) = dispatch else { return true };
    let end = from + title.chars().count();
    let mark = Mark::new(MarkType::Link).with_attr("href", href);
    if let Ok(mut txn) = state
        .transaction()
        .replace(from, to, text_slice(title))
        .and_then(|t| t.add_mark(from, end, mark))
    {
        txn.selection = Selection::cursor(end);
        dispatch(txn);
        true
    } else {
        false
    }
}

/// #148: insert AI-generated text at `[from, to)`. Replaces the range
/// (typically the `@ask <prompt>` trigger text) with `text` in a SINGLE
/// transaction so undo is one step. Leaves the cursor at the end of the
/// inserted content.
///
/// The assistant returns Markdown (bullets, bold, headings, code
/// fences, links). The text is fed through
/// `crate::editor::markdown::parse_from_markdown` so it lands as
/// structured content — bullet lists render as lists, `**bold**` as
/// a Bold mark, `# heading` as a Heading node, etc. Plain prose with
/// no Markdown syntax parses into a single Paragraph, so unformatted
/// answers still land cleanly.
pub fn insert_ai_text(
    from: usize,
    to: usize,
    text: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if text.is_empty() {
        return false;
    }
    let Some(dispatch) = dispatch else { return true };
    let slice = crate::editor::markdown::parse_from_markdown(text);
    let slice_size = slice.size();
    let end = from + slice_size;
    if let Ok(mut txn) = state.transaction().replace(from, to, slice) {
        txn.selection = Selection::cursor(end);
        dispatch(txn);
        true
    } else {
        false
    }
}

/// #148: insert a user @-mention. Replaces `[from, to)` — the `@query`
/// trigger text — with a `NodeType::Mention` leaf atom carrying
/// `user_id` and `display` attributes, in a SINGLE transaction so it's
/// one undo step. Leaves the cursor just after the mention chip.
///
/// #148 slice 6: switched from text + `MarkType::Mention` to a
/// first-class node so partial-delete leaves the chip either intact or
/// gone — no more "@ali" corrupt-halfway states. Existing docs with
/// the legacy text+mark shape keep rendering / exporting correctly
/// because the read paths (export, extract_text, text_content) still
/// handle both. `MarkType::Mention` remains in the schema for that
/// dual-read compatibility; only the writer switches.
///
/// Position math: leaf atoms take one position (like Image / HR), so
/// `end = from + 1` regardless of `display.chars().count()`.
pub fn insert_user_mention(
    from: usize,
    to: usize,
    display: &str,
    user_id: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    if display.is_empty() || user_id.is_empty() {
        return false;
    }
    let Some(dispatch) = dispatch else { return true };
    let mut attrs = std::collections::HashMap::new();
    attrs.insert("user_id".to_string(), user_id.to_string());
    attrs.insert("display".to_string(), display.to_string());
    let mention = crate::editor::model::Node::element_with_attrs(
        crate::editor::model::NodeType::Mention,
        attrs,
        crate::editor::model::Fragment::empty(),
    );
    let slice = crate::editor::model::Slice::new(
        crate::editor::model::Fragment::from(vec![mention]),
        0,
        0,
    );
    let end = from + 1;
    if let Ok(mut txn) = state.transaction().replace(from, to, slice) {
        txn.selection = Selection::cursor(end);
        dispatch(txn);
        true
    } else {
        false
    }
}

/// Mentions spec §5 (Task 3): replace `[from, to)` — which must still
/// contain exactly `expected_text` (the pasted URL) — with a `DocMention`
/// atom. Returns `None` (abort, leave the plain URL in place) when the
/// document changed underneath in a way that invalidates the range: the
/// user kept typing, undid the paste, or another client's edit landed
/// there first. This is the concurrent-edit guard the async resolve races
/// against.
///
/// Unlike the other `insert_*`/`toggle_*` commands in this file, this one
/// returns `Option<Transaction>` directly rather than taking a dispatch
/// closure: the caller is always post-`await` code re-entering the
/// reactive world (`editor_component.rs`), which needs the transaction
/// itself to route through `apply_and_notify` — see that file's mention
/// paste resolve effect.
///
/// The returned transaction carries `meta("history", "merge")` so
/// `HistoryPlugin` folds it into the SAME undo entry as the original
/// paste-the-URL transaction: one undo removes the mention and restores
/// the raw URL, matching the paste's single edit from the user's
/// perspective.
pub fn replace_text_with_doc_mention(
    state: &EditorState,
    from: usize,
    to: usize,
    expected_text: &str,
    attrs: std::collections::HashMap<String, String>,
) -> Option<Transaction> {
    if text_between_positions(&state.doc, from, to) != expected_text {
        return None;
    }
    let mention = crate::editor::model::Node::element_with_attrs(
        crate::editor::model::NodeType::DocMention,
        attrs,
        crate::editor::model::Fragment::empty(),
    );
    let slice = crate::editor::model::Slice::new(
        crate::editor::model::Fragment::from(vec![mention]),
        0,
        0,
    );
    let mut txn = state.transaction().replace(from, to, slice).ok()?;
    txn = txn.set_meta("history", "merge");
    txn.selection = Selection::cursor(from + 1);
    Some(txn)
}

/// Scope of text extracted by `plain_text_from_state`. Callers use it
/// to phrase the AI prompt correctly — "summarize this selection" vs.
/// "summarize this document."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextScope {
    Selection,
    WholeDoc,
}

/// #148 v2 (AI wrappers): extract plain text from the current editor
/// state for use as an AI prompt input.
///
/// - Cursor selection (empty range) → whole doc's `text_content()`,
///   returned with `TextScope::WholeDoc`.
/// - Non-empty selection → text between selection.from() and
///   selection.to(), computed by walking the doc tree in step with
///   the position counter so element open/close boundaries (each
///   non-leaf takes +2 positions on top of its content) are
///   accounted for correctly. Returned with `TextScope::Selection`.
pub fn plain_text_from_state(state: &EditorState) -> (String, TextScope) {
    if state.selection.empty() {
        (state.doc.text_content(), TextScope::WholeDoc)
    } else {
        let from = state.selection.from();
        let to = state.selection.to().max(from);
        (text_between_positions(&state.doc, from, to), TextScope::Selection)
    }
}

/// Return the text between model positions `[from, to)` in `doc`.
/// Walks the doc's children with a running position counter starting
/// at 0 (matching `Node::text_before`'s zero-based iteration over
/// the doc's content). Each non-leaf element takes +1 position for
/// its open boundary and +1 for its close boundary; leaf elements
/// take +1 with no text output; text nodes contribute one position
/// per character.
fn text_between_positions(doc: &crate::editor::model::Node, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut cursor: usize = 0;
    if let crate::editor::model::Node::Element { content, .. } = doc {
        for child in &content.children {
            walk_extracting(child, &mut cursor, from, to, &mut out);
            if cursor >= to {
                break;
            }
        }
    }
    out
}

fn walk_extracting(
    node: &crate::editor::model::Node,
    cursor: &mut usize,
    from: usize,
    to: usize,
    out: &mut String,
) {
    use crate::editor::model::Node;
    match node {
        Node::Text { text, .. } => {
            let text_start = *cursor;
            let text_len = text.chars().count();
            let text_end = text_start + text_len;
            // Overlap of [text_start, text_end) with [from, to).
            if text_end > from && text_start < to {
                let a = text_start.max(from) - text_start;
                let b = text_end.min(to) - text_start;
                let sliced: String = text.chars().skip(a).take(b - a).collect();
                out.push_str(&sliced);
            }
            *cursor = text_end;
        }
        Node::Element {
            node_type,
            content,
            attrs,
            ..
        } => {
            if node_type.is_leaf() {
                // #148 slice 6 — Mention leaf contributes its
                // `display` attr to the extracted text if the
                // atom's position falls inside [from, to).
                let leaf_start = *cursor;
                let leaf_end = leaf_start + 1;
                if *node_type == crate::editor::model::NodeType::Mention
                    && leaf_end > from
                    && leaf_start < to
                {
                    if let Some(display) = attrs.get("display") {
                        out.push_str(display);
                    }
                }
                *cursor = leaf_end;
                return;
            }
            // Open boundary.
            *cursor += 1;
            for child in &content.children {
                walk_extracting(child, cursor, from, to, out);
                if *cursor >= to {
                    // The rest of this element is past the range;
                    // still need the close boundary so parent's
                    // position math stays correct.
                    break;
                }
            }
            // Close boundary — always executed even on early-exit.
            *cursor += 1;
        }
    }
}

/// Replace every range in `matches` with `text` in a single transaction.
/// `matches` must be in ascending document order (as `find::find_matches`
/// returns them); they're applied back-to-front so each earlier range keeps
/// its original position while later ones change length. Returns the count
/// replaced (0 if any step is rejected — nothing is dispatched then).
pub fn replace_all(
    matches: &[(usize, usize)],
    text: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> usize {
    if matches.is_empty() {
        return 0;
    }
    let Some(dispatch) = dispatch else { return matches.len() };
    let mut txn = state.transaction();
    for &(from, to) in matches.iter().rev() {
        match txn.replace(from, to, text_slice(text)) {
            Ok(t) => txn = t,
            Err(_) => return 0,
        }
    }
    dispatch(txn);
    matches.len()
}

/// Insert a horizontal rule after the current block (or container if inside one).
/// Insert an Embed atom after the current block. M-P6 piece B.
/// Carries url/provider/height attributes the editor view's
/// NodeType::Embed render branch uses for the sandboxed iframe.
/// Optional `title` becomes the iframe's title attribute when set.
///
/// Same insert-after-container pattern as `insert_horizontal_rule`:
/// if the caret is inside a list/blockquote, the embed lands after
/// the container, not interleaved with its inline content.
pub fn insert_embed(
    url: &str,
    provider: &str,
    height: u32,
    title: Option<&str>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let pos = state.selection.from();
    let Some(block) = find_block_at(&state.doc, pos) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        let insert_pos = if let Some(container) = find_container_at(&state.doc, pos) {
            container.offset + container.node_size
        } else {
            block.offset + block.node_size
        };

        let mut attrs = std::collections::HashMap::new();
        attrs.insert("url".to_string(), url.to_string());
        attrs.insert("provider".to_string(), provider.to_string());
        attrs.insert("height".to_string(), height.to_string());
        if let Some(t) = title {
            if !t.is_empty() {
                attrs.insert("title".to_string(), t.to_string());
            }
        }
        let embed = Node::element_with_attrs(NodeType::Embed, attrs, Fragment::empty());
        let new_para = Node::element(NodeType::Paragraph);
        let slice = Slice::new(Fragment::from(vec![embed, new_para]), 0, 0);

        if let Ok(mut txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            // Cursor lands in the new paragraph immediately after
            // the embed, matching the HR insert convention.
            txn.selection = Selection::cursor(insert_pos + 1 + 1);
            dispatch(txn);
        }
    }
    true
}

/// #136 — insert a live-app block by registry id. Looks up the
/// insert entry in `editor::blocks::BLOCK_INSERTS`, calls its
/// `build_default_node()` to construct the block, and drops it in
/// after the containing block (same anchor rule as `insert_embed`
/// and `insert_horizontal_rule`). Returns `false` if the id is
/// unknown so the caller can surface a bug.
pub fn insert_live_app(
    id: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some(entry) = super::blocks::insert_by_id(id) else {
        return false;
    };
    let pos = state.selection.from();
    let Some(block) = find_block_at(&state.doc, pos) else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        let insert_pos = if let Some(container) = find_container_at(&state.doc, pos) {
            container.offset + container.node_size
        } else {
            block.offset + block.node_size
        };

        let node = entry.build_default_node();
        // The follow-up paragraph gives the caret a landing spot
        // after an atom block, matching the HR / Embed convention.
        let new_para = Node::element(NodeType::Paragraph);
        let node_size = node.node_size();
        let slice = Slice::new(Fragment::from(vec![node, new_para]), 0, 0);

        if let Ok(mut txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            txn.selection = Selection::cursor(insert_pos + node_size + 1);
            dispatch(txn);
        }
    }
    true
}

// ─── #136 Calendar mutations ────────────────────────────────────
//
// The three commands below all operate on a Calendar container
// located by its structural `blockId` attribute. The click observer
// in `components/editor_component` reads the block-id off the DOM
// wrapper (data-block-id="…") when a user clicks a day cell or an
// event, and the resulting modal calls one of these to commit the
// change. `event_id` is the child CalendarEvent's blockId.
//
// Because Calendar is a leaf-atom-flavored container (isolating +
// atom in the schema) the model doesn't recurse into its children
// during normal editing — so we can't rely on find_block_at with a
// position. We walk the doc once looking for the matching blockId.

/// Walk `doc` looking for the Element node whose `blockId` attr
/// matches `block_id`. Returns the model position of that element
/// (its opening tag offset) plus a clone of the element. Used by
/// the calendar mutation commands and by tests.
pub fn find_element_by_block_id(
    doc: &Node,
    block_id: &str,
) -> Option<(usize, Node)> {
    fn walk(
        node: &Node,
        block_id: &str,
        offset: usize,
    ) -> Option<(usize, Node)> {
        if let Node::Element {
            attrs,
            content,
            ..
        } = node
        {
            if attrs
                .get("blockId")
                .map(String::as_str)
                == Some(block_id)
            {
                return Some((offset, node.clone()));
            }
            let mut child_offset = offset + 1;
            for child in &content.children {
                if let Some(hit) = walk(child, block_id, child_offset) {
                    return Some(hit);
                }
                child_offset += child.node_size();
            }
        }
        None
    }
    if let Node::Element { content, .. } = doc {
        let mut offset = 0;
        for child in &content.children {
            if let Some(hit) = walk(child, block_id, offset) {
                return Some(hit);
            }
            offset += child.node_size();
        }
    }
    None
}

/// Append a new CalendarEvent child to the Calendar identified by
/// `block_id`. `attrs` becomes the child's attribute bag (except
/// `blockId`, which is generated so yrs-bridge can align model↔yrs
/// on next sync). Returns `false` if the calendar can't be found.
///
/// Uses a targeted `Step::Replace` at the end of the calendar's
/// children fragment rather than rewriting the entire subtree —
/// under concurrent editing a peer's just-added event survives
/// because our step touches only the trailing insert position,
/// not the events that already exist.
pub fn add_calendar_event(
    block_id: &str,
    attrs: HashMap<String, String>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((cal_offset, cal_node)) = find_element_by_block_id(&state.doc, block_id) else {
        return false;
    };
    if !matches!(&cal_node, Node::Element { node_type: NodeType::Calendar, .. }) {
        return false;
    }

    if let Some(dispatch) = dispatch {
        // The child must carry a `blockId` because
        // CalendarEvent::needs_block_id() is true — yrs-bridge
        // synthesizes one on write, but seeding it here means the
        // DOM has a stable reference the click observer can use
        // immediately.
        let mut event_attrs = attrs;
        if !event_attrs.contains_key("blockId") {
            event_attrs.insert(
                "blockId".to_string(),
                super::model::generate_block_id(),
            );
        }
        let new_event = Node::element_with_attrs(
            NodeType::CalendarEvent,
            event_attrs,
            Fragment::empty(),
        );
        // Insert at end of children — position `cal_offset +
        // cal_size - 1` is the point just before the closing tag,
        // i.e. the tail of the content fragment.
        let cal_size = cal_node.node_size();
        let insert_pos = cal_offset + cal_size - 1;
        let slice = Slice::new(Fragment::from(vec![new_event]), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Attribute names the modal owns on a `CalendarEvent`. On save
/// we drop these from the existing attr bag and replace with the
/// modal's values; every OTHER attribute is preserved. This
/// protects forward-compat fields (e.g. a future `location`,
/// `externalId`, `lastModifiedAt`) from being silently wiped by
/// an older client whose modal doesn't know about them. Also
/// intentionally drops the disjoint pair (`startAt`/`endAt` vs
/// `startDate`/`endDate`) when the user toggles all-day so we
/// don't ship both shapes at once.
// (Historical: this list used to be a blanket "clear these
// before merge" set applied by `edit_calendar_event`. That
// stripped the event's `content` when a drag-only edit came
// through with just date fields. The clear is now conflict-
// aware — only the OPPOSITE date-shape's fields get removed —
// so no blanket set is needed. The constant is kept for the
// docstring reference below and CI's schema-parity tests.)
#[allow(dead_code)]
const MODAL_OWNED_EVENT_ATTRS: &[&str] = &[
    "color",
    "allDay",
    "startDate",
    "endDate",
    "startAt",
    "endAt",
    "content",
];

/// Replace attributes on a CalendarEvent identified by
/// `(block_id, event_id)`. Merges `new_attrs` on top of the
/// existing attribute bag so unknown / forward-compat fields are
/// preserved — see [`MODAL_OWNED_EVENT_ATTRS`].
///
/// Uses a targeted `Step::Replace` over only the matching child
/// node's range so a peer's concurrent add/remove elsewhere in
/// the calendar survives our edit.
pub fn edit_calendar_event(
    block_id: &str,
    event_id: &str,
    new_attrs: HashMap<String, String>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((child_offset, existing)) = find_calendar_child(&state.doc, block_id, event_id) else {
        return false;
    };
    let Node::Element {
        node_type: NodeType::CalendarEvent,
        attrs: existing_attrs,
        ..
    } = &existing
    else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        // Start from existing attrs. The only fields that MUST be
        // cleared before merge are the OPPOSITE date-shape's
        // fields when the caller is setting one shape — otherwise
        // toggling all-day would leave stale `startAt`/`endAt`
        // (or `startDate`/`endDate`) around and the render path
        // would pick the wrong pair. Everything else (color,
        // content, title, forward-compat fields) survives so a
        // drag that only shifts dates can't strip the event's
        // title (the 2026-07-04 report on Calendar drag).
        let mut merged = existing_attrs.clone();
        let sets_all_day_shape = new_attrs.contains_key("startDate")
            || new_attrs.contains_key("endDate");
        let sets_timed_shape = new_attrs.contains_key("startAt")
            || new_attrs.contains_key("endAt");
        if sets_all_day_shape {
            merged.remove("startAt");
            merged.remove("endAt");
        }
        if sets_timed_shape {
            merged.remove("startDate");
            merged.remove("endDate");
        }
        for (k, v) in &new_attrs {
            merged.insert(k.clone(), v.clone());
        }
        merged.insert("blockId".to_string(), event_id.to_string());
        let replacement = Node::element_with_attrs(
            NodeType::CalendarEvent,
            merged,
            Fragment::empty(),
        );
        let child_size = existing.node_size();
        let slice = Slice::new(Fragment::from(vec![replacement]), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: child_offset,
            to: child_offset + child_size,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Remove a CalendarEvent by `(block_id, event_id)`. Returns
/// `false` if the calendar or the event can't be found.
///
/// Uses a targeted delete over only the matching child's range —
/// same concurrent-edit rationale as [`edit_calendar_event`].
pub fn remove_calendar_event(
    block_id: &str,
    event_id: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((child_offset, existing)) = find_calendar_child(&state.doc, block_id, event_id) else {
        return false;
    };
    if !matches!(&existing, Node::Element { node_type: NodeType::CalendarEvent, .. }) {
        return false;
    }

    if let Some(dispatch) = dispatch {
        let child_size = existing.node_size();
        let slice = Slice::new(Fragment::empty(), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: child_offset,
            to: child_offset + child_size,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Merge `updates` into the Calendar container's own attribute
/// bag. Used by the view-toggle + prev/next/today navigation
/// buttons; keys already present in the container are
/// overwritten. Returns `false` if the calendar can't be found.
///
/// Dispatches one `Step::SetAttr` per changed attribute rather
/// than replacing the whole Calendar subtree — that preserves
/// every child event across the transaction, so a peer's
/// concurrent add during a view toggle survives.
pub fn update_calendar_attrs(
    block_id: &str,
    updates: HashMap<String, String>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((cal_offset, cal_node)) = find_element_by_block_id(&state.doc, block_id) else {
        return false;
    };
    if !matches!(&cal_node, Node::Element { node_type: NodeType::Calendar, .. }) {
        return false;
    }

    if let Some(dispatch) = dispatch {
        let mut txn = state.transaction();
        for (k, v) in updates {
            // Never let a caller overwrite the block id — it's
            // the yrs-bridge alignment anchor.
            if k == "blockId" {
                continue;
            }
            match txn.step(Step::SetAttr {
                pos: cal_offset,
                attr: k,
                value: v,
            }) {
                Ok(next) => txn = next,
                Err(_) => return true,
            }
        }
        dispatch(txn);
    }
    true
}

/// Walk `doc` looking for the CalendarEvent whose `blockId`
/// attribute matches `event_id` inside the Calendar container
/// identified by `block_id`. Returns the child's model position
/// (its opening tag offset) plus a clone of the node.
fn find_calendar_child(
    doc: &Node,
    block_id: &str,
    event_id: &str,
) -> Option<(usize, Node)> {
    let (cal_offset, cal_node) = find_element_by_block_id(doc, block_id)?;
    let Node::Element {
        node_type: NodeType::Calendar,
        content,
        ..
    } = &cal_node
    else {
        return None;
    };
    // Children start just past the container's opening tag.
    let mut child_offset = cal_offset + 1;
    for child in &content.children {
        if let Node::Element {
            node_type: NodeType::CalendarEvent,
            attrs,
            ..
        } = child
        {
            if attrs.get("blockId").map(String::as_str) == Some(event_id) {
                return Some((child_offset, child.clone()));
            }
        }
        child_offset += child.node_size();
    }
    None
}

// ─── Mermaid mutations ──────────────────────────────────────────
//
// Mermaid is a leaf atom (like Calendar/Kanban) whose only mutable
// state is its `source` attribute, edited via `MermaidModal` in
// `components/mermaid_modal.rs`. The delegated click listener in
// `components/editor_component.rs` reads the block-id off the DOM
// wrapper (`data-block-id`) and the modal's Save outcome calls this
// command to commit the new `source`.

/// Write a Mermaid block's `source` attribute by block id. Emits a
/// single `Step::SetAttr`, preserving the node's identity/position.
/// Returns `false` if no Mermaid element with `block_id` exists.
pub fn update_mermaid_source(
    block_id: &str,
    source: String,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((offset, node)) = find_element_by_block_id(&state.doc, block_id) else {
        return false;
    };
    if !matches!(&node, Node::Element { node_type: NodeType::Mermaid, .. }) {
        return false;
    }

    if let Some(dispatch) = dispatch {
        let mut txn = state.transaction();
        match txn.step(Step::SetAttr {
            pos: offset,
            attr: "source".to_string(),
            value: source,
        }) {
            Ok(next) => txn = next,
            Err(_) => return true,
        }
        dispatch(txn);
    }
    true
}

// ─── #137 Kanban mutations ──────────────────────────────────────
//
// The eight commands below all operate on a Kanban tree
// (Kanban → KanbanColumn → KanbanCard) located by structural
// blockIds. They follow the same targeted-step pattern the
// Calendar mutations use — no whole-subtree rewrites, so
// concurrent adds by peers survive. See design/live-app-blocks.md.

/// Modal-owned attribute names on a `KanbanCard`. On edit, we
/// drop these before merging the new values so a stale client
/// doesn't wipe forward-compat fields like a future `assignee`
/// or `dueDate` — same protection Calendar's
/// `MODAL_OWNED_EVENT_ATTRS` provides.
const MODAL_OWNED_CARD_ATTRS: &[&str] = &[
    "title", "content", "color",
    // Phase 4b/4c fields — MODAL owns these too so a Save with
    // an empty value actually clears the attribute rather than
    // preserving the last value.
    "dueAt", "labels", "assigneeId", "assigneeName",
];

/// Append a new KanbanColumn to the Kanban identified by
/// `block_id`. Uses a targeted insert at the tail of the
/// container's children fragment.
pub fn add_kanban_column(
    block_id: &str,
    title: String,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((cal_offset, cal_node)) = find_element_by_block_id(&state.doc, block_id) else {
        return false;
    };
    if !matches!(&cal_node, Node::Element { node_type: NodeType::Kanban, .. }) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        let mut attrs = HashMap::new();
        attrs.insert("blockId".into(), super::model::generate_block_id());
        attrs.insert("title".into(), title);
        let new_col = Node::element_with_attrs(
            NodeType::KanbanColumn,
            attrs,
            Fragment::empty(),
        );
        let cal_size = cal_node.node_size();
        let insert_pos = cal_offset + cal_size - 1;
        let slice = Slice::new(Fragment::from(vec![new_col]), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Rename a column by setting its `title` attribute. Uses
/// `Step::SetAttr` so the transaction touches only the one
/// attribute — a peer's concurrent card add on the same column
/// survives.
pub fn rename_kanban_column(
    _kanban_id: &str,
    column_id: &str,
    new_title: String,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((col_offset, col_node)) = find_element_by_block_id(&state.doc, column_id) else {
        return false;
    };
    if !matches!(&col_node, Node::Element { node_type: NodeType::KanbanColumn, .. }) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        if let Ok(txn) = state.transaction().step(Step::SetAttr {
            pos: col_offset,
            attr: "title".to_string(),
            value: new_title,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Phase 4a — set (or clear) a column's WIP limit. `limit=None`
/// removes the attribute (unlimited); `Some(n)` writes the
/// value. Uses `Step::SetAttr` so a peer's concurrent card add
/// on the same column survives the write.
pub fn set_kanban_column_wip_limit(
    column_id: &str,
    limit: Option<u32>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((col_offset, col_node)) = find_element_by_block_id(&state.doc, column_id) else {
        return false;
    };
    if !matches!(&col_node, Node::Element { node_type: NodeType::KanbanColumn, .. }) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        let value = match limit {
            None => String::new(),
            Some(n) => n.to_string(),
        };
        if let Ok(txn) = state.transaction().step(Step::SetAttr {
            pos: col_offset,
            attr: "wipLimit".to_string(),
            value,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Remove a column and everything it contained. Refuses when
/// the column can't be located.
pub fn remove_kanban_column(
    _kanban_id: &str,
    column_id: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((col_offset, col_node)) = find_element_by_block_id(&state.doc, column_id) else {
        return false;
    };
    if !matches!(&col_node, Node::Element { node_type: NodeType::KanbanColumn, .. }) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        let col_size = col_node.node_size();
        let slice = Slice::new(Fragment::empty(), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: col_offset,
            to: col_offset + col_size,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Append a new KanbanCard to the column identified by
/// `column_id`. Targeted insert at the tail of the column's
/// children fragment — same pattern as `add_kanban_column`.
pub fn add_kanban_card(
    column_id: &str,
    attrs: HashMap<String, String>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((col_offset, col_node)) = find_element_by_block_id(&state.doc, column_id) else {
        return false;
    };
    if !matches!(&col_node, Node::Element { node_type: NodeType::KanbanColumn, .. }) {
        return false;
    }
    // WIP enforcement (Phase 4a). If the column carries a
    // wipLimit attribute and it's already reached, reject the
    // add. Returns false so the caller can surface a warning.
    if column_is_at_wip_limit(&col_node) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        let mut card_attrs = attrs;
        if !card_attrs.contains_key("blockId") {
            card_attrs.insert(
                "blockId".to_string(),
                super::model::generate_block_id(),
            );
        }
        let new_card = Node::element_with_attrs(
            NodeType::KanbanCard,
            card_attrs,
            Fragment::empty(),
        );
        let col_size = col_node.node_size();
        let insert_pos = col_offset + col_size - 1;
        let slice = Slice::new(Fragment::from(vec![new_card]), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Replace a card's attributes while preserving any forward-
/// compat fields. Merges `new_attrs` on top of the existing bag
/// after clearing only `MODAL_OWNED_CARD_ATTRS`.
pub fn edit_kanban_card(
    card_id: &str,
    new_attrs: HashMap<String, String>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((card_offset, card_node)) = find_element_by_block_id(&state.doc, card_id) else {
        return false;
    };
    let Node::Element {
        node_type: NodeType::KanbanCard,
        attrs: existing_attrs,
        ..
    } = &card_node
    else {
        return false;
    };

    if let Some(dispatch) = dispatch {
        let mut merged = existing_attrs.clone();
        for name in MODAL_OWNED_CARD_ATTRS {
            merged.remove(*name);
        }
        for (k, v) in &new_attrs {
            merged.insert(k.clone(), v.clone());
        }
        merged.insert("blockId".to_string(), card_id.to_string());
        let replacement = Node::element_with_attrs(
            NodeType::KanbanCard,
            merged,
            Fragment::empty(),
        );
        let card_size = card_node.node_size();
        let slice = Slice::new(Fragment::from(vec![replacement]), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: card_offset,
            to: card_offset + card_size,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Remove a KanbanCard entirely.
pub fn remove_kanban_card(
    card_id: &str,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    let Some((card_offset, card_node)) = find_element_by_block_id(&state.doc, card_id) else {
        return false;
    };
    if !matches!(&card_node, Node::Element { node_type: NodeType::KanbanCard, .. }) {
        return false;
    }
    if let Some(dispatch) = dispatch {
        let card_size = card_node.node_size();
        let slice = Slice::new(Fragment::empty(), 0, 0);
        if let Ok(txn) = state.transaction().step(Step::Replace {
            from: card_offset,
            to: card_offset + card_size,
            slice,
        }) {
            dispatch(txn);
        }
    }
    true
}

/// Move a card from `from_column` to `to_column` at the tail of
/// the destination. Position-within-destination is Phase 3
/// (drag). Two Step operations in one transaction: delete from
/// source, insert at destination.
/// Move a KanbanCard between (or within) columns.
///
/// `to_index`:
///   - `None` → insert at the tail of the destination column
///     (Phase 2 default, matches "drop on the column but not on
///     any specific card").
///   - `Some(i)` → insert before the i-th child of the destination
///     column (0 = head, `column.child_count()` = tail; the caller
///     clamps out-of-range values). Used by the drag-drop path
///     when the drop landed on / above a specific card slot.
///
/// Same-column reorder is supported: pass the destination
/// column id as both the containing and destination column, and
/// give `to_index` in the pre-delete child indexing (the function
/// adjusts for the delete automatically, so the caller can compute
/// the target index off the DOM before the drag commits).
///
/// The card's containing column isn't a parameter — we locate the
/// card by `card_id` directly, so mis-matching from/to column ids
/// simply reduces to a cross-column move by whatever column
/// actually contains the card.
pub fn move_kanban_card(
    to_column_id: &str,
    card_id: &str,
    to_index: Option<usize>,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    // Locate the card + capture its attrs before we delete it.
    let Some((card_offset, card_node)) = find_element_by_block_id(&state.doc, card_id) else {
        return false;
    };
    let Node::Element {
        node_type: NodeType::KanbanCard,
        attrs: card_attrs,
        ..
    } = &card_node
    else {
        return false;
    };
    let card_size = card_node.node_size();

    // Locate the destination column BEFORE we mutate — the
    // delete-then-insert order below shifts positions of every
    // element after `card_offset`.
    let Some((to_offset, to_node)) = find_element_by_block_id(&state.doc, to_column_id) else {
        return false;
    };
    let Node::Element {
        node_type: NodeType::KanbanColumn,
        content: to_content,
        ..
    } = &to_node
    else {
        return false;
    };
    // Compute the child-index → offset-within-column mapping BEFORE
    // the mutate. Prefix sums so `child_offsets[i]` = model offset
    // (relative to the column's content start) of the i-th child.
    // `child_offsets[N]` = tail.
    let mut child_offsets: Vec<usize> = Vec::with_capacity(to_content.children.len() + 1);
    let mut running = 0usize;
    child_offsets.push(0);
    for child in &to_content.children {
        running += child.node_size();
        child_offsets.push(running);
    }
    // Same-column move (source card is inside the destination
    // column). Detect this once; both `effective_index` and
    // `post_delete_offsets` below need the source's own index.
    let same_column = to_offset < card_offset
        && card_offset < to_offset + to_node.node_size();
    // WIP enforcement (Phase 4a). Cross-column moves grow the
    // destination's card count; same-column moves don't. Reject
    // when the destination is at limit.
    if !same_column && column_is_at_wip_limit(&to_node) {
        return false;
    }
    let src_idx = if same_column {
        let src_rel = card_offset.saturating_sub(to_offset + 1);
        child_offsets.iter()
            .position(|o| *o == src_rel)
            .unwrap_or(0)
    } else {
        0
    };

    // Same-slot no-op guard — mirrors move_kanban_column. When
    // the caller requests a drop at the source's own slot, both
    // "before slot i" and "before slot i+1" flanking positions
    // resolve to the same visible order, and the tail path
    // (None) is a no-op only when the source is already the
    // last child. Skipping the dispatch here avoids a wasted
    // CRDT round-trip on every drop-back-onto-source drag.
    if same_column {
        let last_idx = child_offsets.len().saturating_sub(2);
        let is_noop = match to_index {
            Some(i) => i == src_idx || i == src_idx + 1,
            None => src_idx == last_idx,
        };
        if is_noop {
            return true;
        }
    }

    if let Some(dispatch) = dispatch {
        // Snapshot the card's attrs then build a fresh clone we
        // can insert at the destination.
        let card_clone = Node::element_with_attrs(
            NodeType::KanbanCard,
            card_attrs.clone(),
            Fragment::empty(),
        );
        // The delete-then-insert order shifts the child index of
        // every card AFTER the source card. Convert the caller's
        // pre-delete slot index to a post-delete slot index so
        // "drop before slot i" still lands at the visual position
        // the user aimed at.
        let effective_index = to_index.map(|i| {
            if same_column && i > src_idx { i - 1 } else { i }
        });
        // Delete from source.
        let mut txn = state.transaction();
        let empty = Slice::new(Fragment::empty(), 0, 0);
        let delete_step = Step::Replace {
            from: card_offset,
            to: card_offset + card_size,
            slice: empty,
        };
        txn = match txn.step(delete_step) {
            Ok(t) => t,
            // Schema/position rejection on the delete step —
            // surface as false so the caller can distinguish a
            // real move from a silent no-op.
            Err(_) => return false,
        };
        // Compute the destination insert position AFTER the
        // delete. If `to_offset > card_offset`, the destination
        // column shifted left by `card_size`. Otherwise it's
        // still at its original offset.
        let adjusted_to_offset = if to_offset > card_offset {
            to_offset - card_size
        } else {
            to_offset
        };
        let to_content_start = adjusted_to_offset + 1;
        // The destination column's SIZE also shrinks by card_size
        // when the source is inside it (same-column move). The
        // pre-delete `to_node.node_size()` overshoots by exactly
        // `card_size` in that case, and inserting there lands
        // BETWEEN the column's close boundary and the next
        // column's open boundary — i.e., at the Kanban's own
        // content level, as a sibling of columns. That's the
        // "cards disappeared on refresh" bug reported on doc
        // `AKlZqrcvBH4qdgfWqZqHW`: a same-column tail drop
        // orphaned the card at the Kanban level; on reload
        // `render_column` skipped it (not a KanbanColumn) and the
        // card became invisible.
        let to_size_post_delete = if same_column {
            to_node.node_size() - card_size
        } else {
            to_node.node_size()
        };
        let insert_pos = match effective_index {
            None => adjusted_to_offset + to_size_post_delete - 1,
            Some(i) => {
                // Rebuild `child_offsets` as it looks in the
                // POST-delete column: drop the source's entry and
                // shift entries past it down by `card_size`.
                let post_delete_offsets: Vec<usize> = if same_column {
                    let mut out = Vec::with_capacity(child_offsets.len() - 1);
                    for (idx, off) in child_offsets.iter().enumerate() {
                        if idx == src_idx { continue; }
                        if idx > src_idx {
                            out.push(off.saturating_sub(card_size));
                        } else {
                            out.push(*off);
                        }
                    }
                    out
                } else {
                    child_offsets.clone()
                };
                let clamped = i.min(post_delete_offsets.len().saturating_sub(1));
                to_content_start + post_delete_offsets[clamped]
            }
        };
        let slice = Slice::new(Fragment::from(vec![card_clone]), 0, 0);
        let insert_step = Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        };
        txn = match txn.step(insert_step) {
            Ok(t) => t,
            // Insert-side rejection: return false too. Leaves the
            // caller-facing side aware of the failure. NOTE: the
            // delete step has already been applied to the local
            // txn variable but was NOT dispatched — since we never
            // call `dispatch(txn)`, the editor state is unchanged.
            Err(_) => return false,
        };
        dispatch(txn);
    }
    true
}

/// Kanban v2 — reorder a column within its Kanban board.
///
/// `to_index` is the caller's target slot indexed against the
/// PRE-delete Kanban children (0 = head, `kanban.child_count()`
/// = tail; caller can pass out-of-range values and get clamp-to-
/// tail). Same pre/post-delete index shift as `move_kanban_card`.
///
/// Returns false on schema mismatch or step rejection.
pub fn move_kanban_column(
    column_id: &str,
    to_index: usize,
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    // Locate the column + capture its full subtree.
    let Some((col_offset, col_node)) = find_element_by_block_id(&state.doc, column_id) else {
        return false;
    };
    if !matches!(&col_node, Node::Element { node_type: NodeType::KanbanColumn, .. }) {
        return false;
    }
    let col_size = col_node.node_size();

    // Find the parent Kanban by walking children of the doc:
    // Kanban is a top-level block whose content contains this
    // column. `find_element_by_block_id` returns the column
    // directly, so we need to hunt the Kanban separately.
    let Some((kanban_offset, kanban_node)) = find_kanban_containing_offset(&state.doc, col_offset)
    else {
        return false;
    };
    let Node::Element {
        content: kanban_content,
        ..
    } = &kanban_node
    else {
        return false;
    };
    // child_offsets: prefix sums so [i] = offset of i-th child
    // relative to kanban content start. [N] = tail.
    let mut child_offsets: Vec<usize> = Vec::with_capacity(kanban_content.children.len() + 1);
    let mut running = 0usize;
    child_offsets.push(0);
    for child in &kanban_content.children {
        running += child.node_size();
        child_offsets.push(running);
    }
    let kanban_content_start = kanban_offset + 1;
    let src_rel = col_offset.saturating_sub(kanban_content_start);
    let src_idx = child_offsets.iter()
        .position(|o| *o == src_rel)
        .unwrap_or(0);
    // Post-delete: index that means "stay put" is src_idx (both
    // slot i and slot i+1 flanking the source resolve to the
    // same spot after the delete). No-op the whole call in that
    // case — same visible order, no dispatch, no CRDT churn.
    if to_index == src_idx || to_index == src_idx + 1 {
        return true;
    }

    if let Some(dispatch) = dispatch {
        // Build the clone (with children) BEFORE mutating.
        let column_clone = col_node.clone();
        // Pre → post delete index shift, mirroring
        // move_kanban_card.
        let effective_index = if to_index > src_idx { to_index - 1 } else { to_index };

        let mut txn = state.transaction();
        let empty = Slice::new(Fragment::empty(), 0, 0);
        let delete_step = Step::Replace {
            from: col_offset,
            to: col_offset + col_size,
            slice: empty,
        };
        txn = match txn.step(delete_step) {
            Ok(t) => t,
            Err(_) => return false,
        };
        // Post-delete: source column removed, positions past it
        // shift left by col_size. Build the post-delete offsets
        // table.
        let mut post_delete_offsets: Vec<usize> =
            Vec::with_capacity(child_offsets.len() - 1);
        for (idx, off) in child_offsets.iter().enumerate() {
            if idx == src_idx { continue; }
            if idx > src_idx {
                post_delete_offsets.push(off.saturating_sub(col_size));
            } else {
                post_delete_offsets.push(*off);
            }
        }
        let clamped = effective_index.min(post_delete_offsets.len().saturating_sub(1));
        let insert_pos = kanban_content_start + post_delete_offsets[clamped];
        let slice = Slice::new(Fragment::from(vec![column_clone]), 0, 0);
        let insert_step = Step::Replace {
            from: insert_pos,
            to: insert_pos,
            slice,
        };
        txn = match txn.step(insert_step) {
            Ok(t) => t,
            Err(_) => return false,
        };
        dispatch(txn);
    }
    true
}

/// True when the KanbanColumn's `wipLimit` attribute is a
/// positive integer AND the column's card count meets or
/// exceeds it. `wipLimit=0` and non-numeric values are
/// treated as "unlimited" (no enforcement).
fn column_is_at_wip_limit(col_node: &Node) -> bool {
    let Node::Element { attrs, content, node_type, .. } = col_node else {
        return false;
    };
    if *node_type != NodeType::KanbanColumn {
        return false;
    }
    let Some(limit_raw) = attrs.get("wipLimit") else { return false };
    let Ok(limit) = limit_raw.parse::<usize>() else { return false };
    if limit == 0 {
        return false;
    }
    let card_count = content.children.iter().filter(|c| {
        matches!(c, Node::Element { node_type: NodeType::KanbanCard, .. })
    }).count();
    card_count >= limit
}

/// Locate the Kanban block whose content range contains
/// `inner_offset`. Walks the doc's top-level children looking
/// for a NodeType::Kanban whose (content_start..content_end)
/// covers the position. Returns the Kanban's offset and node.
fn find_kanban_containing_offset(doc: &Node, inner_offset: usize) -> Option<(usize, Node)> {
    let Node::Element { content, .. } = doc else { return None };
    let mut off = 0usize;
    for child in &content.children {
        let size = child.node_size();
        if matches!(child, Node::Element { node_type: NodeType::Kanban, .. }) {
            let content_start = off + 1;
            let content_end = off + size - 1;
            if inner_offset >= content_start && inner_offset < content_end {
                return Some((off, child.clone()));
            }
        }
        off += size;
    }
    None
}

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

/// Tab command: table navigation takes priority, then list indent, then
/// fall back to inserting a literal tab character. The fallback prevents
/// the browser from escaping focus out of the editor when the cursor is
/// in a plain paragraph (issue #18).
/// The indent unit for the code block containing the cursor — `None`
/// when the cursor isn't in a code block. A missing or unresolved
/// `language` falls back to `DEFAULT_INDENT_UNIT` (4 spaces).
fn code_block_indent_unit(state: &EditorState) -> Option<&'static str> {
    let lang = code_block_language(state)?;
    Some(super::state::indent_unit_for_language_tag(Some(&lang)))
}

/// Remove one indent step from the start of the caret's line in a
/// code block: a full indent `unit`, or a single leading tab, or a
/// shorter run of leading spaces. `None` when the line isn't indented.
fn dedent_code_block_line(state: &EditorState, unit: &str) -> Option<Transaction> {
    let pos = state.selection.from();
    let block = find_block_at(&state.doc, pos)?;
    let text =
        Node::element_with_content(block.node_type, block.content.clone()).text_content();
    let chars: Vec<char> = text.chars().collect();
    let caret = pos.checked_sub(block.content_start)?.min(chars.len());
    let line_start = super::state::line_start_at(&chars, caret);
    let remove = if chars.get(line_start) == Some(&'\t') {
        1
    } else {
        let unit_len = unit.chars().count().max(1);
        chars[line_start..]
            .iter()
            .take_while(|&&c| c == ' ')
            .count()
            .min(unit_len)
    };
    if remove == 0 {
        return None;
    }
    let from = block.content_start + line_start;
    state.transaction().delete(from, from + remove).ok()
}

pub fn tab_command(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    // Innermost context wins: Tab inside a code block indents code by
    // the language's unit (4 spaces for Python, a hard tab for Go, 2
    // spaces for the web/config family), even when the block sits in
    // a list item or table cell.
    if let Some(unit) = code_block_indent_unit(state) {
        if let Some(d) = dispatch {
            if let Ok(txn) = state.transaction().insert_text(unit) {
                d(txn);
            }
        }
        return true;
    }
    if is_in_table(state) {
        return table_tab_forward(state, dispatch);
    }
    if sink_list_item(state, dispatch) {
        return true;
    }
    // Plain paragraph (or any other textblock that isn't a list item):
    // insert a tab character at the cursor. CSS `white-space: pre-wrap`
    // on `.editor-content` already renders tabs visibly.
    if let Some(d) = dispatch {
        if let Ok(txn) = state.transaction().insert_text("\t") {
            d(txn);
        }
    }
    true
}

/// Shift-Tab command: table navigation takes priority, then list dedent.
pub fn shift_tab_command(
    state: &EditorState,
    dispatch: Option<&dyn Fn(Transaction)>,
) -> bool {
    // Mirror of tab_command's code-block branch: dedent the caret's
    // line by one step. Consume the key even when there's nothing to
    // remove — Shift-Tab must never fall through to list-lifting or
    // browser focus navigation from inside a code block.
    if let Some(unit) = code_block_indent_unit(state) {
        if let Some(d) = dispatch {
            if let Some(txn) = dedent_code_block_line(state, unit) {
                d(txn);
            }
        }
        return true;
    }
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

    fn code_doc(language: Option<&str>, text: &str, cursor: usize) -> EditorState {
        let mut attrs = std::collections::HashMap::new();
        if let Some(lang) = language {
            attrs.insert("language".to_string(), lang.to_string());
        }
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text(text)]),
            )]),
        );
        EditorState {
            selection: Selection::cursor(cursor),
            ..EditorState::create_default(doc)
        }
    }

    fn apply_captured(
        state: &EditorState,
        f: impl Fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool,
    ) -> (bool, EditorState) {
        let captured: RefCell<Option<Transaction>> = RefCell::new(None);
        let dispatch = |txn: Transaction| {
            *captured.borrow_mut() = Some(txn);
        };
        let handled = f(state, Some(&dispatch));
        let new_state = match captured.into_inner() {
            Some(txn) => state.apply(txn),
            None => state.clone(),
        };
        (handled, new_state)
    }

    #[test]
    fn tab_in_python_code_block_inserts_four_spaces() {
        // caret at end of "class X:\n" → 1 (block open) + 9 chars
        let state = code_doc(Some("python"), "class X:\n", 10);
        let (handled, new_state) = apply_captured(&state, tab_command);
        assert!(handled);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "class X:\n    "
        );
        assert_eq!(new_state.selection.from(), 14, "caret after the 4 spaces");
    }

    #[test]
    fn tab_in_go_code_block_inserts_tab_char() {
        let state = code_doc(Some("go"), "x\n", 3);
        let (handled, new_state) = apply_captured(&state, tab_command);
        assert!(handled);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "x\n\t");
    }

    #[test]
    fn tab_in_unlabeled_code_block_inserts_four_spaces() {
        let state = code_doc(None, "x", 2);
        let (handled, new_state) = apply_captured(&state, tab_command);
        assert!(handled);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "x    ");
    }

    #[test]
    fn tab_in_paragraph_still_inserts_literal_tab() {
        // Regression: the pre-existing non-code-block fallback.
        let state = EditorState {
            selection: Selection::cursor(12),
            ..EditorState::create_default(simple_doc())
        };
        let (handled, new_state) = apply_captured(&state, tab_command);
        assert!(handled);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "Hello world\t"
        );
    }

    #[test]
    fn shift_tab_dedents_current_line_by_one_unit() {
        // caret at end of "class X:\n    pass" → 1 + 17
        let state = code_doc(Some("python"), "class X:\n    pass", 18);
        let (handled, new_state) = apply_captured(&state, shift_tab_command);
        assert!(handled);
        assert_eq!(
            new_state.doc.child(0).unwrap().text_content(),
            "class X:\npass"
        );
        assert_eq!(
            new_state.selection.from(),
            14,
            "caret shifts left with the line"
        );
    }

    #[test]
    fn shift_tab_removes_single_leading_tab() {
        let state = code_doc(Some("go"), "\tx", 3);
        let (handled, new_state) = apply_captured(&state, shift_tab_command);
        assert!(handled);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "x");
    }

    #[test]
    fn shift_tab_removes_short_space_run() {
        // 2 leading spaces < the 4-space unit: remove what's there.
        let state = code_doc(None, "  x", 4);
        let (handled, new_state) = apply_captured(&state, shift_tab_command);
        assert!(handled);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "x");
    }

    #[test]
    fn shift_tab_noop_on_unindented_line_but_consumes_key() {
        let state = code_doc(Some("python"), "a\nb", 4);
        let (handled, new_state) = apply_captured(&state, shift_tab_command);
        assert!(handled, "key must be consumed even with nothing to dedent");
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "a\nb");
    }

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    #[test]
    fn plain_text_from_state_whole_doc_on_cursor_selection() {
        let state = EditorState::create_default(simple_doc());
        let (text, scope) = plain_text_from_state(&state);
        assert_eq!(text, "Hello world");
        assert_eq!(scope, TextScope::WholeDoc);
    }

    #[test]
    fn plain_text_from_state_selection_extracts_word_across_paragraph_boundary() {
        // simple_doc = <doc><p>Hello world</p></doc>.
        // Position 0 = before <p>. Position 1 = start of p's text
        // (before 'H'). Position 1+i = between char i-1 and char i.
        // Selection covering "world" (chars 6..11) is positions
        // (7, 12). The proper walk returns "world" — the earlier
        // char-index slice implementation returned "orld" (missing
        // the leading 'w'), which is exactly the correctness bug
        // the reviewer surfaced.
        let state = EditorState::create_default(simple_doc());
        let selected = EditorState {
            selection: Selection::text(7, 12),
            ..state
        };
        let (text, scope) = plain_text_from_state(&selected);
        assert_eq!(scope, TextScope::Selection);
        assert_eq!(text, "world");
    }

    #[test]
    fn plain_text_from_state_selection_extracts_first_word() {
        // Positions 1..6 cover "Hello" (before 'H' through after 'o').
        let state = EditorState::create_default(simple_doc());
        let selected = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let (text, _) = plain_text_from_state(&selected);
        assert_eq!(text, "Hello");
    }

    #[test]
    fn plain_text_from_state_clamps_out_of_bounds_selection() {
        // A selection stretching past the doc's outer size gets
        // clamped rather than panicking. The walk simply runs out
        // of tree; whatever text falls into the overlap comes back.
        let state = EditorState::create_default(simple_doc());
        let selected = EditorState {
            selection: Selection::text(0, 999),
            ..state
        };
        let (text, scope) = plain_text_from_state(&selected);
        assert_eq!(scope, TextScope::Selection);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn plain_text_from_state_selection_crosses_paragraph_boundaries() {
        use crate::editor::model::{Fragment, Node, NodeType};
        // Two-paragraph doc:
        //   <doc>
        //     <p>Foo</p>   (node_size = 3 + 2 = 5, positions [0, 5))
        //     <p>Bar</p>   (node_size = 5, positions [5, 10))
        //   </doc>
        //
        // Position mapping per `Node::text_before`:
        //   pos 0 = p1's open boundary
        //   pos 1 = inside p1, before 'F'
        //   pos 2 = between 'F' and first 'o'
        //   pos 3 = between first 'o' and second 'o'
        //   pos 4 = inside p1, after the last 'o'
        //   pos 5 = p2's open boundary
        //   pos 6 = inside p2, before 'B'
        //   pos 7 = between 'B' and 'a'
        //   pos 8 = between 'a' and 'r'
        //   pos 9 = inside p2, after 'r'
        //
        // Selection [3, 8) covers "the second 'o'" + "Ba" — the
        // walk must skip the boundary positions between p1 and p2
        // and stitch the two text runs together.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Foo")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Bar")]),
                ),
            ]),
        );
        let state = EditorState::create_default(doc);
        let selected = EditorState {
            selection: Selection::text(3, 8),
            ..state
        };
        let (text, _) = plain_text_from_state(&selected);
        assert_eq!(text, "oBa");
    }

    #[test]
    fn plain_text_extractor_includes_mention_display() {
        // #148 slice 6 — <p>Hi <Mention/></p>. The walk mirrors
        // model::Node position math:
        //   cursor 0 → at p open boundary
        //   cursor 1 → inside p body, before 'H'
        //   text "Hi " runs 1..4 (3 chars)
        //   cursor 4 → at Mention (leaf), occupies [4, 5)
        //   cursor 5 → at p close boundary
        // So selection covering just the mention is [4, 5).
        use crate::editor::model::{Fragment, Node, NodeType};
        let mut mention_attrs = std::collections::HashMap::new();
        mention_attrs.insert("user_id".to_string(), "u-1".to_string());
        mention_attrs.insert("display".to_string(), "@alice".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("Hi "),
                    Node::element_with_attrs(
                        NodeType::Mention,
                        mention_attrs,
                        Fragment::empty(),
                    ),
                ]),
            )]),
        );
        // Whole-doc scope: cursor selection returns the full text_content().
        let state = EditorState::create_default(doc.clone());
        let (text, scope) = plain_text_from_state(&state);
        assert_eq!(scope, TextScope::WholeDoc);
        assert_eq!(text, "Hi @alice");

        // Selection [4, 5) covers just the Mention leaf.
        let selected = EditorState {
            selection: Selection::text(4, 5),
            ..EditorState::create_default(doc.clone())
        };
        let (text, _) = plain_text_from_state(&selected);
        assert_eq!(text, "@alice");

        // Selection [1, 5) covers "Hi " + Mention.
        let selected = EditorState {
            selection: Selection::text(1, 5),
            ..EditorState::create_default(doc)
        };
        let (text, _) = plain_text_from_state(&selected);
        assert_eq!(text, "Hi @alice");
    }

    #[test]
    fn text_content_of_mention_leaf_is_display() {
        // Direct model-side check: `Node::text_content` on a
        // Mention returns the `display` attr.
        use crate::editor::model::{Fragment, Node, NodeType};
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("user_id".to_string(), "u-42".to_string());
        attrs.insert("display".to_string(), "@bob".to_string());
        let mention = Node::element_with_attrs(
            NodeType::Mention,
            attrs,
            Fragment::empty(),
        );
        assert_eq!(mention.text_content(), "@bob");
    }

    #[test]
    fn tab_in_paragraph_inserts_tab_character() {
        // Cursor inside a plain paragraph — tab_command should fall
        // through table and list paths and insert a literal '\t'.
        let state = EditorState::create_default(simple_doc());
        let captured: RefCell<Option<Transaction>> = RefCell::new(None);
        let dispatch = |txn: Transaction| { *captured.borrow_mut() = Some(txn); };
        let handled = tab_command(&state, Some(&dispatch));
        assert!(handled, "tab_command must report handled to suppress focus escape");
        let txn = captured.into_inner().expect("a transaction must be dispatched");
        let after = state.apply(txn);
        let para = after.doc.child(0).unwrap();
        assert!(para.text_content().contains('\t'),
            "paragraph text must contain a tab character: {:?}", para.text_content());
    }

    #[test]
    fn tab_in_paragraph_returns_handled_without_dispatch() {
        // Query mode (dispatch=None) — must report applicability without
        // mutating anything.
        let state = EditorState::create_default(simple_doc());
        let handled = tab_command(&state, None);
        assert!(handled, "tab is always handled in a textblock context");
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
    fn clear_formatting_strips_all_inline_marks_in_range() {
        // #134: bold + italic on "Hello", then Clear Formatting over the
        // same range must leave the text with no inline marks.
        let state = EditorState::create_default(simple_doc());
        let state = EditorState { selection: Selection::text(1, 6), ..state };
        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d)).unwrap();
        let state = state.apply(txn);
        let state = EditorState { selection: Selection::text(1, 6), ..state };
        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Italic, s, d)).unwrap();
        let state = state.apply(txn);

        // Sanity: the marks landed.
        let para = state.doc.child(0).unwrap();
        assert!(para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Bold));

        // Clear formatting over the same range.
        let state = EditorState { selection: Selection::text(1, 6), ..state };
        let txn = run_command(&state, |s, d| clear_formatting(s, d)).unwrap();
        let new_state = state.apply(txn);

        let para = new_state.doc.child(0).unwrap();
        // Robust to text-node merging after marks are stripped: no child
        // text node may carry any mark, and the text is preserved.
        let any_marks =
            (0..para.child_count()).any(|i| !para.child(i).unwrap().marks().is_empty());
        assert!(!any_marks, "clear_formatting must strip every inline mark");
        assert_eq!(para.text_content(), "Hello world");
    }

    #[test]
    fn replace_range_swaps_the_matched_text() {
        // #147: "Hello world", replace (1,6)="Hello" → "Howdy".
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| replace_range(1, 6, "Howdy", s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Howdy world");
    }

    #[test]
    fn replace_all_replaces_every_match_back_to_front() {
        // doc text "aaaa"; matches for "aa" are (1,3) and (3,5); replacing
        // both with "b" yields "bb".
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("aaaa")]),
            )]),
        );
        let state = EditorState::create_default(doc);
        let txn = run_command(&state, |s, d| replace_all(&[(1, 3), (3, 5)], "b", s, d) > 0).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "bb");
    }

    #[test]
    fn clear_formatting_is_noop_on_empty_selection() {
        // Nothing to clear at a bare cursor.
        let state = EditorState::create_default(simple_doc());
        let state = EditorState { selection: Selection::cursor(2), ..state };
        assert!(!clear_formatting(&state, None));
    }

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
    fn subscript_replaces_superscript_on_range() {
        // #143: sub/superscript are mutually exclusive. Applying subscript over
        // a superscripted range must strip the superscript, not stack both.
        let state = EditorState::create_default(simple_doc());
        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Superscript, s, d)).unwrap();
        let state = state.apply(txn);

        let state = EditorState {
            selection: Selection::text(1, 6),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Subscript, s, d)).unwrap();
        let new_state = state.apply(txn);

        let marks = new_state.doc.child(0).unwrap().child(0).unwrap().marks().to_vec();
        assert!(
            marks.iter().any(|m| m.mark_type == MarkType::Subscript),
            "subscript should be applied"
        );
        assert!(
            !marks.iter().any(|m| m.mark_type == MarkType::Superscript),
            "superscript should have been stripped (mutual exclusion)"
        );
    }

    #[test]
    fn toggle_bold_on_full_doc_range_via_select_all() {
        // Ctrl+A in the contenteditable lands the DOM selection at
        // anchor=0, head=doc_size — `from` resolves to the doc-level
        // boundary, which the schema rightly rejects as a mark
        // location on its own. The can_apply_mark_here peek-one-
        // position-deeper guard is what makes this work; without it,
        // the command-palette-actions doctor scenario hits
        // `boldApplied: false` because toggle_mark returns early.
        let state = EditorState::create_default(simple_doc());
        let doc_size = state.doc.text_content().chars().count() + 2; // open + close
        let state = EditorState {
            selection: Selection::text(0, doc_size),
            ..state
        };
        let txn = run_command(&state, |s, d| toggle_mark(MarkType::Bold, s, d))
            .expect("toggle_mark must dispatch when the range covers content blocks");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert!(
            para.child(0).unwrap()
                .marks()
                .iter()
                .any(|m| m.mark_type == MarkType::Bold),
            "Bold mark must land on the paragraph's text even though `from`=0 resolves to the doc boundary",
        );
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

    // ── set_alignment ──

    #[test]
    fn set_alignment_center_writes_attr_on_paragraph() {
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| set_alignment("center", s, d)).unwrap();
        let new_state = state.apply(txn);
        let first = new_state.doc.child(0).unwrap();
        if let Node::Element { attrs, .. } = first {
            assert_eq!(attrs.get("align").map(String::as_str), Some("center"));
        } else {
            panic!("expected an element node");
        }
    }

    #[test]
    fn set_alignment_right_writes_attr_on_heading() {
        let state = EditorState::create_default(heading_doc());
        let txn = run_command(&state, |s, d| set_alignment("right", s, d)).unwrap();
        let new_state = state.apply(txn);
        let first = new_state.doc.child(0).unwrap();
        if let Node::Element { node_type, attrs, .. } = first {
            assert_eq!(*node_type, NodeType::Heading);
            assert_eq!(attrs.get("align").map(String::as_str), Some("right"));
        } else {
            panic!("expected an element node");
        }
    }

    #[test]
    fn set_alignment_left_clears_existing_align_attr() {
        // Pre-state: a paragraph already centered. set_alignment("left")
        // should remove the attr rather than set it to "left", so the
        // rendered DOM matches the natural-default state.
        let para_with_align = {
            let mut attrs = HashMap::new();
            attrs.insert("align".to_string(), "center".to_string());
            Node::Element {
                node_type: NodeType::Paragraph,
                attrs,
                content: Fragment::from(vec![Node::text("Hello world")]),
                marks: vec![],
            }
        };
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![para_with_align]),
        );
        let state = EditorState::create_default(doc);
        let txn = run_command(&state, |s, d| set_alignment("left", s, d)).unwrap();
        let new_state = state.apply(txn);
        let first = new_state.doc.child(0).unwrap();
        if let Node::Element { attrs, .. } = first {
            assert!(!attrs.contains_key("align"),
                "left should clear the attr, found {:?}", attrs);
        } else {
            panic!("expected an element node");
        }
    }

    #[test]
    fn set_alignment_rejects_invalid_value() {
        let state = EditorState::create_default(simple_doc());
        let dispatched = set_alignment("justify", &state, None);
        assert!(!dispatched, "values outside the allowlist must be rejected");
    }

    #[test]
    fn set_alignment_no_op_when_already_at_target() {
        // Calling set_alignment("center") on an already-centered block
        // should still report success (true) but not dispatch a
        // transaction. The implementation early-returns when the
        // computed attrs equal the existing attrs.
        let para_with_align = {
            let mut attrs = HashMap::new();
            attrs.insert("align".to_string(), "center".to_string());
            Node::Element {
                node_type: NodeType::Paragraph,
                attrs,
                content: Fragment::from(vec![Node::text("Hello")]),
                marks: vec![],
            }
        };
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![para_with_align]),
        );
        let state = EditorState::create_default(doc);
        let txn = run_command(&state, |s, d| set_alignment("center", s, d));
        assert!(txn.is_none(),
            "no transaction should be dispatched when align is unchanged");
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
    fn insert_doc_link_replaces_trigger_with_linked_title() {
        // simple_doc = paragraph "Hello world". Mimic replacing an "@h"
        // trigger occupying [1, 3) with a linked document title.
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| {
            insert_doc_link(1, 3, "Design Doc", "/d/abc123", s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // The trigger text is gone, replaced by the title + the remaining text.
        assert_eq!(para.text_content(), "Design Docllo world");
        // The first text node is the title and carries the Link mark.
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "Design Doc");
        let link = first.marks().iter().find(|m| m.mark_type == MarkType::Link);
        assert!(link.is_some(), "the inserted title must carry a Link mark");
        assert_eq!(link.unwrap().attrs.get("href").unwrap(), "/d/abc123");
        // The remaining "llo world" is NOT linked.
        let rest = para.child(1).unwrap();
        assert!(
            rest.marks().iter().all(|m| m.mark_type != MarkType::Link),
            "text after the mention must not inherit the link"
        );
        // Cursor sits just after the inserted link.
        assert_eq!(new_state.selection.from(), 1 + "Design Doc".chars().count());
    }

    #[test]
    fn insert_doc_link_empty_title_is_noop() {
        let state = EditorState::create_default(simple_doc());
        assert!(!insert_doc_link(1, 3, "", "/d/x", &state, None));
    }

    #[test]
    fn insert_user_mention_replaces_trigger_with_mention_node() {
        // Mimic replacing an "@a" trigger occupying [1, 3) with a
        // user mention. #148 slice 6: switched from text+mark to a
        // NodeType::Mention leaf. Delete now removes the chip as
        // one keystroke.
        use crate::editor::model::NodeType;
        let state = EditorState::create_default(simple_doc());
        let txn = run_command(&state, |s, d| {
            insert_user_mention(1, 3, "Alice", "u-42", s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // The trigger text is gone; para now holds Mention + the
        // remaining text after position 3 ("llo world").
        let mention = para.child(0).unwrap();
        assert_eq!(mention.node_type(), Some(NodeType::Mention));
        assert_eq!(mention.text_content(), "Alice");
        assert!(mention.is_leaf());
        // The Mention node's attrs carry both user_id and display.
        assert_eq!(mention.attrs().get("user_id").unwrap(), "u-42");
        assert_eq!(mention.attrs().get("display").unwrap(), "Alice");
        // The remaining text is untouched and carries no marks.
        let rest = para.child(1).unwrap();
        assert_eq!(rest.text_content(), "llo world");
        // Cursor sits just after the mention chip (leaf takes 1 pos).
        assert_eq!(new_state.selection.from(), 1 + 1);
        // Whole paragraph's text_content includes the display.
        assert_eq!(para.text_content(), "Alicello world");
    }

    #[test]
    fn insert_user_mention_empty_display_or_user_id_is_noop() {
        let state = EditorState::create_default(simple_doc());
        assert!(!insert_user_mention(1, 3, "", "u-1", &state, None));
        assert!(!insert_user_mention(1, 3, "Alice", "", &state, None));
    }

    // ── replace_text_with_doc_mention (mentions spec §5, Task 3) ──

    fn url_doc(url: &str) -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(url)]),
            )]),
        )
    }

    #[test]
    fn replace_text_with_doc_mention_swaps_in_the_atom_and_marks_merge() {
        let url = "https://notes.example/d/abc123";
        let state = EditorState::create_default(url_doc(url));
        let from = 1; // just inside <p>
        let to = from + url.chars().count();

        let mut attrs = HashMap::new();
        attrs.insert("doc_id".to_string(), "abc123".to_string());
        attrs.insert("url".to_string(), url.to_string());
        attrs.insert("title".to_string(), "Target Doc".to_string());

        let txn = replace_text_with_doc_mention(&state, from, to, url, attrs).unwrap();
        assert_eq!(txn.meta.get("history").map(String::as_str), Some("merge"));

        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let mention = para.child(0).unwrap();
        assert_eq!(mention.node_type(), Some(NodeType::DocMention));
        assert!(mention.is_leaf());
        assert_eq!(mention.attrs().get("doc_id").unwrap(), "abc123");
        assert_eq!(mention.attrs().get("title").unwrap(), "Target Doc");
        // Cursor lands just after the atom (leaf takes one position).
        assert_eq!(new_state.selection.from(), from + 1);
        // The URL text is gone — replaced wholesale.
        assert_eq!(para.text_content(), "Target Doc");
    }

    #[test]
    fn replace_text_with_doc_mention_aborts_when_text_no_longer_matches() {
        // Concurrent-edit guard: [from,to) no longer holds `expected_text`
        // (user kept typing, undid, or a remote edit landed there first).
        let url = "https://notes.example/d/abc123";
        let state = EditorState::create_default(url_doc(url));
        let from = 1;
        let to = from + url.chars().count();
        assert!(replace_text_with_doc_mention(&state, from, to, "not the url anymore", HashMap::new())
            .is_none());
    }

    #[test]
    fn replace_text_with_doc_mention_aborts_on_out_of_range_positions() {
        let url = "https://notes.example/d/abc123";
        let state = EditorState::create_default(url_doc(url));
        assert!(replace_text_with_doc_mention(&state, 0, 1000, url, HashMap::new()).is_none());
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

    // ── code_block_language / set_code_block_language ──

    fn code_block_doc_lang(language: Option<&str>) -> Node {
        let mut attrs = std::collections::HashMap::new();
        if let Some(lang) = language {
            attrs.insert("language".to_string(), lang.to_string());
        }
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::CodeBlock,
                attrs,
                Fragment::from(vec![Node::text("fn main() {}")]),
            )]),
        )
    }

    #[test]
    fn code_block_language_reads_attr() {
        let state = EditorState::create_default(code_block_doc_lang(Some("rust")));
        // cursor into the code block's text (pos 1 = before 'f')
        let state = EditorState { selection: Selection::cursor(1), ..state };
        assert_eq!(code_block_language(&state).as_deref(), Some("rust"));
    }

    #[test]
    fn code_block_language_empty_when_unset_none_when_outside() {
        let state = EditorState::create_default(code_block_doc_lang(None));
        let inside = EditorState { selection: Selection::cursor(1), ..state };
        assert_eq!(code_block_language(&inside).as_deref(), Some(""));

        let para = EditorState::create_default(simple_doc());
        let outside = EditorState { selection: Selection::cursor(1), ..para };
        assert_eq!(code_block_language(&outside), None);
    }

    #[test]
    fn set_code_block_language_dispatches_set_attr() {
        let state = EditorState::create_default(code_block_doc_lang(None));
        let state = EditorState { selection: Selection::cursor(1), ..state };
        let txn = run_command(&state, |s, d| set_code_block_language("python", s, d)).unwrap();
        let new_state = state.apply(txn);
        let updated = EditorState { selection: Selection::cursor(1), ..new_state };
        assert_eq!(code_block_language(&updated).as_deref(), Some("python"));
    }

    #[test]
    fn set_code_block_language_refuses_outside_code_block() {
        let state = EditorState::create_default(simple_doc());
        let state = EditorState { selection: Selection::cursor(1), ..state };
        assert!(!set_code_block_language("python", &state, None));
        let txn = run_command(&state, |s, d| set_code_block_language("python", s, d));
        assert!(txn.is_none());
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

    // ── move_kanban_card (Phase 3 drag support) ──

    fn kanban_doc_two_columns() -> Node {
        // Doc > Kanban > [ColA(cardA1, cardA2, cardA3), ColB(cardB1)]
        // Card ids chosen to match assertions.
        fn card(id: &str) -> Node {
            let mut attrs = HashMap::new();
            attrs.insert("blockId".into(), id.into());
            attrs.insert("title".into(), id.into());
            Node::element_with_attrs(NodeType::KanbanCard, attrs, Fragment::empty())
        }
        fn col(id: &str, cards: Vec<Node>) -> Node {
            let mut attrs = HashMap::new();
            attrs.insert("blockId".into(), id.into());
            attrs.insert("title".into(), id.into());
            Node::element_with_attrs(NodeType::KanbanColumn, attrs, Fragment::from(cards))
        }
        let mut kanban_attrs = HashMap::new();
        kanban_attrs.insert("blockId".into(), "kanban".into());
        let kanban = Node::element_with_attrs(
            NodeType::Kanban,
            kanban_attrs,
            Fragment::from(vec![
                col("A", vec![card("A1"), card("A2"), card("A3")]),
                col("B", vec![card("B1")]),
            ]),
        );
        Node::element_with_content(NodeType::Doc, Fragment::from(vec![kanban]))
    }

    /// Column child order under `column_id` as a Vec of `blockId`
    /// strings, for readable assertions.
    fn col_card_ids(doc: &Node, column_id: &str) -> Vec<String> {
        let (_, col_node) = super::find_element_by_block_id(doc, column_id).unwrap();
        let Node::Element { content, .. } = col_node else { panic!() };
        content.children.iter().map(|c| {
            let Node::Element { attrs, .. } = c else { return String::new() };
            attrs.get("blockId").cloned().unwrap_or_default()
        }).collect()
    }

    #[test]
    fn move_kanban_card_cross_column_tail() {
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("B", "A2", None, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "A"), vec!["A1", "A3"]);
        assert_eq!(col_card_ids(&new_state.doc, "B"), vec!["B1", "A2"]);
    }

    #[test]
    fn move_kanban_card_cross_column_head() {
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("B", "A2", Some(0), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "A"), vec!["A1", "A3"]);
        assert_eq!(col_card_ids(&new_state.doc, "B"), vec!["A2", "B1"]);
    }

    #[test]
    fn move_kanban_card_same_column_forward() {
        // A1, A2, A3 → move A1 to slot 2 → A2, A1, A3.
        // With pre-delete indexing: slot 2 means "before original A3",
        // which is index 2. After deleting A1, A3 is at index 1, so
        // move_kanban_card must adjust internally.
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("A", "A1", Some(2), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "A"), vec!["A2", "A1", "A3"]);
    }

    #[test]
    fn move_kanban_card_same_column_backward() {
        // A1, A2, A3 → move A3 to slot 0 → A3, A1, A2.
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("A", "A3", Some(0), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "A"), vec!["A3", "A1", "A2"]);
    }

    /// Regression: a drag-only edit that shifted `startDate`/
    /// `endDate` (no `content`, no `color`) used to strip the
    /// event's `content` (the title/description text) because
    /// `edit_calendar_event` did a blanket clear of
    /// `MODAL_OWNED_EVENT_ATTRS` before merging. Observed on
    /// Calendar drag 2026-07-04.
    #[test]
    fn edit_calendar_event_partial_shift_preserves_title_and_color() {
        use crate::editor::model::Fragment;
        fn mk() -> Node {
            let mut kanban_attrs: HashMap<String, String> = HashMap::new();
            kanban_attrs.insert("blockId".into(), "kan".into());
            let mut cal_attrs: HashMap<String, String> = HashMap::new();
            cal_attrs.insert("blockId".into(), "cal1".into());
            cal_attrs.insert("view".into(), "month".into());
            cal_attrs.insert("cursor".into(), "2026-07".into());
            cal_attrs.insert("timezone".into(), "UTC".into());
            let mut ev_attrs: HashMap<String, String> = HashMap::new();
            ev_attrs.insert("blockId".into(), "ev1".into());
            ev_attrs.insert("content".into(), "Team meeting".into());
            ev_attrs.insert("color".into(), "orange".into());
            ev_attrs.insert("allDay".into(), "true".into());
            ev_attrs.insert("startDate".into(), "2026-07-04".into());
            ev_attrs.insert("endDate".into(), "2026-07-04".into());
            let ev = Node::element_with_attrs(
                NodeType::CalendarEvent, ev_attrs, Fragment::empty(),
            );
            let cal = Node::element_with_attrs(
                NodeType::Calendar, cal_attrs, Fragment::from(vec![ev]),
            );
            Node::element_with_content(NodeType::Doc, Fragment::from(vec![cal]))
        }
        let state = EditorState::create_default(mk());
        // Drag delta: just the new dates + allDay flag (mirrors
        // exactly what `drag_compute_commit` builds for an
        // AllDay Move).
        let mut delta: HashMap<String, String> = HashMap::new();
        delta.insert("allDay".into(), "true".into());
        delta.insert("startDate".into(), "2026-07-11".into());
        delta.insert("endDate".into(), "2026-07-11".into());
        let txn = run_command(&state, |s, d|
            edit_calendar_event("cal1", "ev1", delta.clone(), s, d)).unwrap();
        let new_state = state.apply(txn);
        // Find the event, assert content + color survived, dates
        // updated.
        let cal = new_state.doc.child(0).unwrap();
        let Node::Element { content, .. } = cal else { panic!() };
        let ev = &content.children[0];
        let Node::Element { attrs, .. } = ev else { panic!() };
        assert_eq!(attrs.get("content").map(|s| s.as_str()), Some("Team meeting"),
            "content must survive a date-only edit");
        assert_eq!(attrs.get("color").map(|s| s.as_str()), Some("orange"),
            "color must survive a date-only edit");
        assert_eq!(attrs.get("startDate").map(|s| s.as_str()), Some("2026-07-11"));
        assert_eq!(attrs.get("endDate").map(|s| s.as_str()), Some("2026-07-11"));
    }

    /// Regression: flipping all-day off (drag on a timed event
    /// via the modal, or manual all-day toggle) must clear the
    /// opposite date-shape so the schema doesn't carry both
    /// `startAt/endAt` and `startDate/endDate` simultaneously.
    #[test]
    fn edit_calendar_event_all_day_toggle_clears_opposite_shape() {
        use crate::editor::model::Fragment;
        fn mk_timed() -> Node {
            let mut cal_attrs: HashMap<String, String> = HashMap::new();
            cal_attrs.insert("blockId".into(), "cal1".into());
            let mut ev_attrs: HashMap<String, String> = HashMap::new();
            ev_attrs.insert("blockId".into(), "ev1".into());
            ev_attrs.insert("allDay".into(), "false".into());
            ev_attrs.insert("startAt".into(), "2026-07-04T09:00:00Z".into());
            ev_attrs.insert("endAt".into(), "2026-07-04T10:00:00Z".into());
            ev_attrs.insert("content".into(), "Meeting".into());
            let ev = Node::element_with_attrs(
                NodeType::CalendarEvent, ev_attrs, Fragment::empty(),
            );
            let cal = Node::element_with_attrs(
                NodeType::Calendar, cal_attrs, Fragment::from(vec![ev]),
            );
            Node::element_with_content(NodeType::Doc, Fragment::from(vec![cal]))
        }
        let state = EditorState::create_default(mk_timed());
        // Modal Save flipping to all-day mode.
        let mut delta: HashMap<String, String> = HashMap::new();
        delta.insert("allDay".into(), "true".into());
        delta.insert("startDate".into(), "2026-07-04".into());
        delta.insert("endDate".into(), "2026-07-04".into());
        let txn = run_command(&state, |s, d|
            edit_calendar_event("cal1", "ev1", delta.clone(), s, d)).unwrap();
        let new_state = state.apply(txn);
        let cal = new_state.doc.child(0).unwrap();
        let Node::Element { content, .. } = cal else { panic!() };
        let ev = &content.children[0];
        let Node::Element { attrs, .. } = ev else { panic!() };
        assert!(!attrs.contains_key("startAt"), "stale timed shape must be removed");
        assert!(!attrs.contains_key("endAt"), "stale timed shape must be removed");
        assert_eq!(attrs.get("startDate").map(|s| s.as_str()), Some("2026-07-04"));
        // Content still preserved.
        assert_eq!(attrs.get("content").map(|s| s.as_str()), Some("Meeting"));
    }

    // ── Mermaid mutations ──

    fn state_with_mermaid(block_id: &str, source: &str) -> EditorState {
        let mut attrs: HashMap<String, String> = HashMap::new();
        attrs.insert("blockId".into(), block_id.into());
        attrs.insert("source".into(), source.into());
        let mermaid = Node::element_with_attrs(NodeType::Mermaid, attrs, Fragment::empty());
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![mermaid]));
        EditorState::create_default(doc)
    }

    #[test]
    fn update_mermaid_source_sets_attr() {
        let state = state_with_mermaid("m1", "pie\n\"A\": 1");
        let txn = run_command(&state, |s, d| {
            update_mermaid_source("m1", "pie\n\"B\": 2".into(), s, d)
        })
        .unwrap();
        let new_state = state.apply(txn);
        let mermaid = new_state.doc.child(0).unwrap();
        let Node::Element { attrs, .. } = mermaid else { panic!() };
        assert_eq!(
            attrs.get("source").map(|s| s.as_str()),
            Some("pie\n\"B\": 2"),
        );
    }

    #[test]
    fn update_mermaid_source_unknown_block_id_returns_false() {
        let state = state_with_mermaid("m1", "pie\n\"A\": 1");
        let dispatched = run_command(&state, |s, d| {
            update_mermaid_source("missing", "pie\n\"B\": 2".into(), s, d)
        });
        assert!(dispatched.is_none(), "unknown block_id must not dispatch");
    }

    /// Regression: `run_command` treats a `false` return as "no
    /// dispatch" regardless of whether the closure invoked the
    /// callback, so this also proves the early-return path itself
    /// returns `false` (checked directly, bypassing the harness).
    #[test]
    fn update_mermaid_source_unknown_block_id_returns_false_direct() {
        let state = state_with_mermaid("m1", "pie\n\"A\": 1");
        let ok = update_mermaid_source("missing", "pie\n\"B\": 2".into(), &state, None);
        assert!(!ok);
    }

    /// Regression: same-column drop with `to_index = None` (tail)
    /// used `to_node.node_size()` unadjusted for the pending
    /// delete, so `insert_pos` overshot the destination column's
    /// close boundary. The card landed at the Kanban's own
    /// content level as a sibling of the columns — a schema
    /// violation. On reload, `render_column` skipped the
    /// orphaned card (not a KanbanColumn), so the card became
    /// invisible. Observed on doc `AKlZqrcvBH4qdgfWqZqHW`.
    #[test]
    fn move_kanban_card_same_column_tail_stays_in_column() {
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("A", "A1", None, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "A"), vec!["A2", "A3", "A1"]);
        // The Kanban's direct children must remain columns only —
        // no orphaned cards floating at the board level.
        let kanban = new_state.doc.child(0).unwrap();
        let crate::editor::model::Node::Element { content, .. } = kanban else {
            panic!("first child not an element");
        };
        for c in &content.children {
            let crate::editor::model::Node::Element { node_type, .. } = c else {
                panic!("kanban child is text");
            };
            assert_eq!(
                *node_type,
                NodeType::KanbanColumn,
                "kanban child must be a column, got {:?}",
                node_type,
            );
        }
    }

    // ── edit_kanban_card modal-owned attrs (Phase 4b/4c) ──

    fn kanban_doc_with_card_attrs(extra: &[(&str, &str)]) -> Node {
        let mut attrs: HashMap<String, String> = HashMap::new();
        attrs.insert("blockId".into(), "C1".into());
        attrs.insert("title".into(), "T".into());
        for (k, v) in extra {
            attrs.insert(k.to_string(), v.to_string());
        }
        let card = Node::element_with_attrs(NodeType::KanbanCard, attrs, Fragment::empty());
        let mut col_attrs: HashMap<String, String> = HashMap::new();
        col_attrs.insert("blockId".into(), "A".into());
        col_attrs.insert("title".into(), "A".into());
        let col = Node::element_with_attrs(NodeType::KanbanColumn, col_attrs, Fragment::from(vec![card]));
        let mut kanban_attrs: HashMap<String, String> = HashMap::new();
        kanban_attrs.insert("blockId".into(), "K".into());
        let kanban = Node::element_with_attrs(NodeType::Kanban, kanban_attrs, Fragment::from(vec![col]));
        Node::element_with_content(NodeType::Doc, Fragment::from(vec![kanban]))
    }

    fn card_attr(doc: &Node, card_id: &str, name: &str) -> Option<String> {
        let (_, card) = super::find_element_by_block_id(doc, card_id)?;
        let Node::Element { attrs, .. } = card else { return None };
        attrs.get(name).cloned()
    }

    #[test]
    fn edit_kanban_card_writes_due_labels_assignee() {
        let state = EditorState::create_default(kanban_doc_with_card_attrs(&[]));
        let mut new_attrs: HashMap<String, String> = HashMap::new();
        new_attrs.insert("title".into(), "T".into());
        new_attrs.insert("dueAt".into(), "2026-07-15".into());
        new_attrs.insert("labels".into(), "bug|red;ux|blue".into());
        new_attrs.insert("assigneeId".into(), "user1".into());
        new_attrs.insert("assigneeName".into(), "Ada".into());
        let txn = run_command(&state, |s, d|
            edit_kanban_card("C1", new_attrs.clone(), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(card_attr(&new_state.doc, "C1", "dueAt").as_deref(),
            Some("2026-07-15"));
        assert_eq!(card_attr(&new_state.doc, "C1", "labels").as_deref(),
            Some("bug|red;ux|blue"));
        assert_eq!(card_attr(&new_state.doc, "C1", "assigneeId").as_deref(),
            Some("user1"));
        assert_eq!(card_attr(&new_state.doc, "C1", "assigneeName").as_deref(),
            Some("Ada"));
    }

    #[test]
    fn edit_kanban_card_clears_omitted_modal_owned_fields() {
        // Card already has dueAt + assignee. Modal Save with no
        // dueAt / assignee in new_attrs should CLEAR them (they're
        // in MODAL_OWNED_CARD_ATTRS, so `edit_kanban_card` drops
        // them before merging).
        let state = EditorState::create_default(kanban_doc_with_card_attrs(&[
            ("dueAt", "2026-01-01"),
            ("assigneeId", "old"),
            ("assigneeName", "Old"),
        ]));
        let mut new_attrs: HashMap<String, String> = HashMap::new();
        new_attrs.insert("title".into(), "T".into());
        // No dueAt, no assignee — modal wants to clear them.
        let txn = run_command(&state, |s, d|
            edit_kanban_card("C1", new_attrs.clone(), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert!(card_attr(&new_state.doc, "C1", "dueAt").is_none(),
            "dueAt must be cleared when omitted from new_attrs");
        assert!(card_attr(&new_state.doc, "C1", "assigneeId").is_none(),
            "assigneeId must be cleared when omitted");
    }

    #[test]
    fn edit_kanban_card_preserves_forward_compat_attrs() {
        // A future attr like `externalId` that the modal doesn't
        // know about must survive an edit.
        let state = EditorState::create_default(kanban_doc_with_card_attrs(&[
            ("externalId", "trello-42"),
        ]));
        let mut new_attrs: HashMap<String, String> = HashMap::new();
        new_attrs.insert("title".into(), "T".into());
        new_attrs.insert("dueAt".into(), "2026-07-15".into());
        let txn = run_command(&state, |s, d|
            edit_kanban_card("C1", new_attrs.clone(), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(card_attr(&new_state.doc, "C1", "externalId").as_deref(),
            Some("trello-42"));
        assert_eq!(card_attr(&new_state.doc, "C1", "dueAt").as_deref(),
            Some("2026-07-15"));
    }

    // ── wipLimit enforcement (Phase 4a) ──

    fn kanban_doc_with_wip_limit(limit: usize, cards_in_a: usize) -> Node {
        fn card(id: &str) -> Node {
            let mut attrs = HashMap::new();
            attrs.insert("blockId".into(), id.into());
            attrs.insert("title".into(), id.into());
            Node::element_with_attrs(NodeType::KanbanCard, attrs, Fragment::empty())
        }
        let mut a_attrs = HashMap::new();
        a_attrs.insert("blockId".into(), "A".into());
        a_attrs.insert("title".into(), "A".into());
        a_attrs.insert("wipLimit".into(), limit.to_string());
        let a_cards: Vec<Node> = (0..cards_in_a)
            .map(|i| card(&format!("A{i}")))
            .collect();
        let col_a = Node::element_with_attrs(
            NodeType::KanbanColumn, a_attrs, Fragment::from(a_cards),
        );
        let mut b_attrs = HashMap::new();
        b_attrs.insert("blockId".into(), "B".into());
        b_attrs.insert("title".into(), "B".into());
        let col_b = Node::element_with_attrs(
            NodeType::KanbanColumn, b_attrs, Fragment::from(vec![card("B1")]),
        );
        let mut kanban_attrs = HashMap::new();
        kanban_attrs.insert("blockId".into(), "kanban".into());
        let kanban = Node::element_with_attrs(
            NodeType::Kanban,
            kanban_attrs,
            Fragment::from(vec![col_a, col_b]),
        );
        Node::element_with_content(NodeType::Doc, Fragment::from(vec![kanban]))
    }

    #[test]
    fn add_kanban_card_blocked_when_column_at_wip_limit() {
        let state = EditorState::create_default(kanban_doc_with_wip_limit(2, 2));
        let mut attrs: HashMap<String, String> = HashMap::new();
        attrs.insert("title".into(), "new".into());
        let result = add_kanban_card("A", attrs, &state, Some(&|_| panic!("no dispatch")));
        assert!(!result, "add must fail when column at wipLimit");
    }

    #[test]
    fn add_kanban_card_succeeds_when_column_under_wip_limit() {
        let state = EditorState::create_default(kanban_doc_with_wip_limit(3, 2));
        let captured: RefCell<Option<Transaction>> = RefCell::new(None);
        let mut attrs: HashMap<String, String> = HashMap::new();
        attrs.insert("title".into(), "new".into());
        let result = add_kanban_card("A", attrs, &state,
            Some(&|txn| { *captured.borrow_mut() = Some(txn); }));
        assert!(result);
        assert!(captured.into_inner().is_some(),
            "add must dispatch a transaction");
    }

    #[test]
    fn move_kanban_card_blocked_when_destination_at_wip_limit() {
        let state = EditorState::create_default(kanban_doc_with_wip_limit(2, 2));
        // Move B1 → A (at limit).
        let result = move_kanban_card("A", "B1", None, &state,
            Some(&|_| panic!("no dispatch")));
        assert!(!result, "cross-column move must fail into a full column");
    }

    #[test]
    fn move_kanban_card_same_column_allowed_at_wip_limit() {
        // Same-column reorder does not increase count → allowed
        // even at limit.
        let state = EditorState::create_default(kanban_doc_with_wip_limit(2, 2));
        let txn = run_command(&state, |s, d|
            move_kanban_card("A", "A0", Some(2), s, d)).unwrap();
        let _new_state = state.apply(txn);
    }

    #[test]
    fn wip_limit_zero_treated_as_unlimited() {
        let state = EditorState::create_default(kanban_doc_with_wip_limit(0, 5));
        let captured: RefCell<Option<Transaction>> = RefCell::new(None);
        let mut attrs: HashMap<String, String> = HashMap::new();
        attrs.insert("title".into(), "new".into());
        let result = add_kanban_card("A", attrs, &state,
            Some(&|txn| { *captured.borrow_mut() = Some(txn); }));
        assert!(result);
        assert!(captured.into_inner().is_some(),
            "wipLimit=0 must not block adds");
    }

    // ── move_kanban_column (Phase 4a) ──

    fn kanban_columns_ids(doc: &Node) -> Vec<String> {
        let kanban = doc.child(0).unwrap();
        let crate::editor::model::Node::Element { content, .. } = kanban else { panic!() };
        content.children.iter().map(|c| {
            let crate::editor::model::Node::Element { attrs, .. } = c else { return String::new() };
            attrs.get("blockId").cloned().unwrap_or_default()
        }).collect()
    }

    #[test]
    fn move_kanban_column_forward() {
        // Cols A, B: move A to slot 2 (tail). Expect [B, A].
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_column("A", 2, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(kanban_columns_ids(&new_state.doc), vec!["B", "A"]);
    }

    #[test]
    fn move_kanban_column_backward() {
        // Cols A, B: move B to slot 0. Expect [B, A].
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_column("B", 0, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(kanban_columns_ids(&new_state.doc), vec!["B", "A"]);
    }

    #[test]
    fn move_kanban_column_noop_stays_put() {
        // Cols A, B: move A to slot 0 (its current slot). Expect
        // [A, B] — no change.
        let state = EditorState::create_default(kanban_doc_two_columns());
        let handled = move_kanban_column("A", 0, &state, Some(&|_| panic!("no dispatch")));
        assert!(handled, "no-op still reports handled");
    }

    #[test]
    fn move_kanban_column_out_of_range_clamps_to_tail() {
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_column("A", 999, s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(kanban_columns_ids(&new_state.doc), vec!["B", "A"]);
    }

    #[test]
    fn move_kanban_card_same_slot_is_noop() {
        // Drop at own current slot — both "before self" and
        // "before self + 1" resolve to the same visible order,
        // so no dispatch must fire.
        let state = EditorState::create_default(kanban_doc_two_columns());
        // A2 is at index 1. Dropping at Some(1) means "before A2"
        // and Some(2) means "before A3". Both leave A2 where it is.
        let handled_before_self = move_kanban_card(
            "A", "A2", Some(1), &state, Some(&|_| panic!("no dispatch")),
        );
        assert!(handled_before_self,
            "same-slot drop reports handled (no-op) without dispatch");
        let handled_after_self = move_kanban_card(
            "A", "A2", Some(2), &state, Some(&|_| panic!("no dispatch")),
        );
        assert!(handled_after_self,
            "adjacent-slot drop also reports handled without dispatch");
    }

    #[test]
    fn move_kanban_card_same_column_tail_when_already_tail_is_noop() {
        // A3 is the last card. Tail drop (None) on its own column
        // is a no-op — must not dispatch.
        let state = EditorState::create_default(kanban_doc_two_columns());
        let handled = move_kanban_card(
            "A", "A3", None, &state, Some(&|_| panic!("no dispatch")),
        );
        assert!(handled);
    }

    #[test]
    fn move_kanban_card_out_of_range_index_clamps_to_tail() {
        let state = EditorState::create_default(kanban_doc_two_columns());
        let txn = run_command(&state, |s, d|
            move_kanban_card("B", "A2", Some(999), s, d)).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(col_card_ids(&new_state.doc, "B"), vec!["B1", "A2"]);
    }
}
