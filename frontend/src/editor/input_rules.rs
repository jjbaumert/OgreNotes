use std::collections::HashMap;

use super::model::{Fragment, Mark, MarkType, Node, NodeType, Slice};
use super::selection::Selection;
use super::state::{find_block_at, EditorState, Transaction};
use super::transform::Step;

/// An input rule that matches text typed at the end of a line and transforms it.
pub struct InputRule {
    /// Description for debugging.
    pub name: &'static str,
    /// The trigger pattern (matched against the text before the cursor + the just-typed char).
    /// Returns Some((match_start_offset, match_len)) if the pattern matches.
    pub matcher: Box<dyn Fn(&str) -> Option<(usize, usize)>>,
    /// The handler that produces a transaction.
    pub handler: Box<dyn Fn(&EditorState, usize, usize, &str) -> Option<Transaction>>,
}

/// Check all input rules after a character is typed.
/// `text_before` is the text content of the current block up to and including the typed char.
/// `from` and `to` are the positions (in the document) of the matched range.
pub fn check_input_rules(
    rules: &[InputRule],
    state: &EditorState,
    text_before: &str,
    block_start: usize,
) -> Option<Transaction> {
    for rule in rules {
        if let Some((match_offset, match_len)) = (rule.matcher)(text_before) {
            let from = block_start + match_offset;
            let to = block_start + match_offset + match_len;
            let matched_text = &text_before[match_offset..match_offset + match_len];
            if let Some(txn) = (rule.handler)(state, from, to, matched_text) {
                return Some(txn);
            }
        }
    }
    None
}

/// Extract the text content before the cursor in the current block.
/// Returns `(text_before_cursor, block_content_start_pos)` or None if
/// the cursor is not inside a text-containing block.
pub fn get_block_text_before(doc: &Node, cursor_pos: usize) -> Option<(String, usize)> {
    let Node::Element { content, .. } = doc else {
        return None;
    };

    let mut offset = 0;
    for child in &content.children {
        let child_size = child.node_size();

        if let Node::Element { node_type, .. } = child {
            if !node_type.is_leaf() {
                let content_start = offset + 1; // +1 for open boundary
                let content_end = offset + child_size - 1; // -1 for close boundary

                if cursor_pos >= content_start && cursor_pos <= content_end {
                    let block_text = child.text_content();
                    let cursor_offset = cursor_pos - content_start;
                    let text_before: String =
                        block_text.chars().take(cursor_offset).collect();
                    return Some((text_before, content_start));
                }
            }
        }

        offset += child_size;
    }
    None
}

/// Build the default set of MVP input rules.
pub fn default_input_rules() -> Vec<InputRule> {
    vec![
        // Block-level rules (at line start)
        heading_rule("# ", 1),
        heading_rule("## ", 2),
        heading_rule("### ", 3),
        blockquote_rule(),
        bullet_list_rule("* "),
        bullet_list_rule("- "),
        bullet_list_rule("+ "),
        ordered_list_rule(),
        task_list_rule("[ ] "),
        task_list_checked_rule("[x] "),
        hr_rule(),
        // Inline mark rules
        bold_rule(),        // **text**
        bold_underscore_rule(), // __text__
        italic_rule(),      // *text*
        italic_underscore_rule(), // _text_
        code_rule(),        // `text`
    ]
}

// ─── Block Rules ────────────────────────────────────────────────

fn heading_rule(trigger: &'static str, level: u8) -> InputRule {
    InputRule {
        name: match level {
            1 => "heading1",
            2 => "heading2",
            _ => "heading3",
        },
        matcher: Box::new(move |text| {
            if text == trigger {
                Some((0, trigger.len()))
            } else {
                None
            }
        }),
        handler: Box::new(move |state, from, to, _matched| {
            // Delete the trigger text and convert block to heading
            let block_pos = from - 1; // position of the block node in its parent's content
            let mut attrs = HashMap::new();
            attrs.insert("level".to_string(), level.to_string());
            let txn = state
                .transaction()
                .delete(from, to)
                .ok()?
                .set_node_type(block_pos, NodeType::Heading, attrs)
                .ok()?;
            Some(txn)
        }),
    }
}

fn blockquote_rule() -> InputRule {
    InputRule {
        name: "blockquote",
        matcher: Box::new(|text| {
            if text == "> " {
                Some((0, 2))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, to, _| {
            // Delete trigger text, then wrap the block in a blockquote
            let txn = state.transaction().delete(from, to).ok()?;
            let cursor = txn.selection.from();
            let block = find_block_at(&txn.doc, cursor)?;

            let bq = Node::element(NodeType::Blockquote);
            let wrapper = Slice::new(Fragment::from(vec![bq]), 1, 1);
            let txn = txn.step(Step::ReplaceAround {
                from: block.offset,
                to: block.offset + block.node_size,
                gap_from: block.offset,
                gap_to: block.offset + block.node_size,
                insert: wrapper,
                structure: true,
            }).ok()?;
            Some(txn)
        }),
    }
}

fn bullet_list_rule(trigger: &'static str) -> InputRule {
    InputRule {
        name: "bullet_list",
        matcher: Box::new(move |text| {
            if text == trigger {
                Some((0, trigger.len()))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, to, _| {
            wrap_block_after_delete(state, from, to, NodeType::BulletList, NodeType::ListItem)
        }),
    }
}

fn ordered_list_rule() -> InputRule {
    InputRule {
        name: "ordered_list",
        matcher: Box::new(|text| {
            if text == "1. " {
                Some((0, 3))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, to, _| {
            wrap_block_after_delete(state, from, to, NodeType::OrderedList, NodeType::ListItem)
        }),
    }
}

fn task_list_rule(trigger: &'static str) -> InputRule {
    InputRule {
        name: "task_list",
        matcher: Box::new(move |text| {
            if text == trigger {
                Some((0, trigger.len()))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, to, _| {
            wrap_block_after_delete(state, from, to, NodeType::TaskList, NodeType::TaskItem)
        }),
    }
}

fn task_list_checked_rule(trigger: &'static str) -> InputRule {
    InputRule {
        name: "task_list_checked",
        matcher: Box::new(move |text| {
            if text == trigger {
                Some((0, trigger.len()))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, to, _| {
            // Same as task_list but the item starts checked
            let txn = state.transaction().delete(from, to).ok()?;
            let cursor = txn.selection.from();
            let block = find_block_at(&txn.doc, cursor)?;

            let mut attrs = HashMap::new();
            attrs.insert("checked".to_string(), "true".to_string());
            let item = Node::Element {
                node_type: NodeType::TaskItem,
                attrs,
                content: Fragment::empty(),
                marks: vec![],
            };
            let list = Node::element_with_content(NodeType::TaskList, Fragment::from(vec![item]));
            let wrapper = Slice::new(Fragment::from(vec![list]), 2, 2);
            let txn = txn.step(Step::ReplaceAround {
                from: block.offset,
                to: block.offset + block.node_size,
                gap_from: block.offset,
                gap_to: block.offset + block.node_size,
                insert: wrapper,
                structure: true,
            }).ok()?;
            Some(txn)
        }),
    }
}

fn hr_rule() -> InputRule {
    InputRule {
        name: "horizontal_rule",
        matcher: Box::new(|text| {
            if text == "---" || text == "___" {
                Some((0, text.len()))
            } else {
                None
            }
        }),
        handler: Box::new(|state, from, _to, _| {
            // Replace the entire paragraph with HR + a new empty paragraph
            let block = find_block_at(&state.doc, from)?;
            let hr = Node::element(NodeType::HorizontalRule);
            let new_para = Node::element_with_content(NodeType::Paragraph, Fragment::empty());
            let content = Fragment::from(vec![hr, new_para]);
            let slice = Slice::new(content, 0, 0);
            let mut txn = state
                .transaction()
                .replace(block.offset, block.offset + block.node_size, slice)
                .ok()?;
            // Place cursor inside the new empty paragraph (HR size=1, +1 for para open)
            txn.selection = Selection::cursor(block.offset + 2);
            Some(txn)
        }),
    }
}

/// Helper: delete trigger text, then wrap the resulting block in a list.
fn wrap_block_after_delete(
    state: &EditorState,
    from: usize,
    to: usize,
    list_type: NodeType,
    item_type: NodeType,
) -> Option<Transaction> {
    let txn = state.transaction().delete(from, to).ok()?;
    let cursor = txn.selection.from();
    let block = find_block_at(&txn.doc, cursor)?;

    let item = Node::element(item_type);
    let list = Node::element_with_content(list_type, Fragment::from(vec![item]));
    let wrapper = Slice::new(Fragment::from(vec![list]), 2, 2);
    let txn = txn.step(Step::ReplaceAround {
        from: block.offset,
        to: block.offset + block.node_size,
        gap_from: block.offset,
        gap_to: block.offset + block.node_size,
        insert: wrapper,
        structure: true,
    }).ok()?;
    Some(txn)
}

// ─── Inline Mark Rules ──────────────────────────────────────────

fn bold_rule() -> InputRule {
    InputRule {
        name: "bold",
        matcher: Box::new(|text| {
            // Match **text** pattern
            if text.len() >= 5 && text.ends_with("**") {
                let inner = &text[..text.len() - 2];
                if let Some(start) = inner.rfind("**") {
                    let content_start = start + 2;
                    if content_start < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            // Extract the text between ** and **
            if matched.len() < 5 {
                return None;
            }
            let inner = &matched[2..matched.len() - 2];
            inline_mark_replace(state, from, to, inner, MarkType::Bold)
        }),
    }
}

/// Shared handler for inline mark rules: replace matched text with marked text,
/// place cursor after it, and clear stored marks so subsequent typing is plain.
fn inline_mark_replace(
    state: &EditorState,
    from: usize,
    to: usize,
    inner: &str,
    mark_type: MarkType,
) -> Option<Transaction> {
    let node = Node::text_with_marks(inner, vec![Mark::new(mark_type)]);
    let slice = Slice::new(Fragment::from(vec![node]), 0, 0);
    let inner_len = super::model::char_len(inner);
    let mut txn = state.transaction().replace(from, to, slice).ok()?;
    txn.selection = Selection::cursor(from + inner_len);
    txn.stored_marks = Some(vec![]); // clear marks so next typed char is plain
    Some(txn)
}

fn italic_rule() -> InputRule {
    InputRule {
        name: "italic",
        matcher: Box::new(|text| {
            // Match *text* pattern (but not **text**)
            if text.len() >= 3 && text.ends_with('*') && !text.ends_with("**") {
                let inner = &text[..text.len() - 1];
                // Find the opening * from the right, not part of a ** pair
                if let Some(start) = inner.rfind('*') {
                    let is_double =
                        start > 0 && inner.as_bytes().get(start - 1) == Some(&b'*');
                    if !is_double && start + 1 < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            if matched.len() < 3 { return None; }
            inline_mark_replace(state, from, to, &matched[1..matched.len() - 1], MarkType::Italic)
        }),
    }
}

fn bold_underscore_rule() -> InputRule {
    InputRule {
        name: "bold_underscore",
        matcher: Box::new(|text| {
            // Match __text__ pattern
            if text.len() >= 5 && text.ends_with("__") {
                let inner = &text[..text.len() - 2];
                if let Some(start) = inner.rfind("__") {
                    let content_start = start + 2;
                    if content_start < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            if matched.len() < 5 { return None; }
            inline_mark_replace(state, from, to, &matched[2..matched.len() - 2], MarkType::Bold)
        }),
    }
}

fn italic_underscore_rule() -> InputRule {
    InputRule {
        name: "italic_underscore",
        matcher: Box::new(|text| {
            // Match _text_ pattern (but not __text__)
            if text.len() >= 3 && text.ends_with('_') && !text.ends_with("__") {
                let inner = &text[..text.len() - 1];
                if let Some(start) = inner.rfind('_') {
                    let is_double =
                        start > 0 && inner.as_bytes().get(start - 1) == Some(&b'_');
                    if !is_double && start + 1 < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            if matched.len() < 3 { return None; }
            inline_mark_replace(state, from, to, &matched[1..matched.len() - 1], MarkType::Italic)
        }),
    }
}

fn code_rule() -> InputRule {
    InputRule {
        name: "inline_code",
        matcher: Box::new(|text| {
            // Match `text` pattern
            if text.len() >= 3 && text.ends_with('`') {
                let inner = &text[..text.len() - 1];
                if let Some(start) = inner.rfind('`') {
                    let content_start = start + 1;
                    if content_start < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            if matched.len() < 3 { return None; }
            inline_mark_replace(state, from, to, &matched[1..matched.len() - 1], MarkType::Code)
        }),
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node, NodeType};
    use crate::editor::selection::Selection;
    use crate::editor::state::EditorState;

    fn make_state(text: &str) -> EditorState {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]),
            )]),
        );
        let pos = 1 + super::super::model::char_len(text); // end of text
        EditorState {
            selection: Selection::cursor(pos),
            ..EditorState::create_default(doc)
        }
    }

    // ── Block rules ──

    #[test]
    fn heading_1_matches() {
        let rules = default_input_rules();
        let state = make_state("# ");
        let txn = check_input_rules(&rules, &state, "# ", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn heading_2_matches() {
        let rules = default_input_rules();
        let state = make_state("## ");
        let txn = check_input_rules(&rules, &state, "## ", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn heading_3_matches() {
        let rules = default_input_rules();
        let state = make_state("### ");
        let txn = check_input_rules(&rules, &state, "### ", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn heading_1_converts_paragraph_to_heading() {
        let rules = default_input_rules();
        let state = make_state("# ");
        let txn = check_input_rules(&rules, &state, "# ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::Heading));
        assert_eq!(block.attrs().get("level").unwrap(), "1");
        assert_eq!(block.text_content(), ""); // trigger text deleted
    }

    #[test]
    fn heading_2_converts_paragraph_to_heading() {
        let rules = default_input_rules();
        let state = make_state("## ");
        let txn = check_input_rules(&rules, &state, "## ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::Heading));
        assert_eq!(block.attrs().get("level").unwrap(), "2");
    }

    #[test]
    fn heading_preserves_remaining_text() {
        // Simulate: user typed "# Hello" then the rule fires on the "# " prefix
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("# Hello")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(3), // after "# "
            ..EditorState::create_default(doc)
        };
        let txn = check_input_rules(&default_input_rules(), &state, "# ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::Heading));
        assert_eq!(block.text_content(), "Hello");
    }

    #[test]
    fn bullet_list_star_creates_list() {
        let rules = default_input_rules();
        let state = make_state("* ");
        let txn = check_input_rules(&rules, &state, "* ", 1).unwrap();
        let new_state = state.apply(txn);
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::ListItem));
    }

    #[test]
    fn bullet_list_dash_creates_list() {
        let rules = default_input_rules();
        let state = make_state("- ");
        let txn = check_input_rules(&rules, &state, "- ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::BulletList));
    }

    #[test]
    fn ordered_list_creates_list() {
        let rules = default_input_rules();
        let state = make_state("1. ");
        let txn = check_input_rules(&rules, &state, "1. ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::OrderedList));
    }

    #[test]
    fn task_list_creates_list() {
        let rules = default_input_rules();
        let state = make_state("[ ] ");
        let txn = check_input_rules(&rules, &state, "[ ] ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::TaskList));
    }

    #[test]
    fn blockquote_creates_blockquote() {
        let rules = default_input_rules();
        let state = make_state("> ");
        let txn = check_input_rules(&rules, &state, "> ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::Blockquote));
    }

    #[test]
    fn hr_matches_dashes() {
        let rules = default_input_rules();
        let state = make_state("---");
        let txn = check_input_rules(&rules, &state, "---", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn hr_matches_underscores() {
        let rules = default_input_rules();
        let state = make_state("___");
        let txn = check_input_rules(&rules, &state, "___", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn hr_creates_hr_and_new_paragraph() {
        let rules = default_input_rules();
        let state = make_state("---");
        let txn = check_input_rules(&rules, &state, "---", 1).unwrap();
        let new_state = state.apply(txn);
        // First child should be the horizontal rule
        assert_eq!(
            new_state.doc.child(0).unwrap().node_type(),
            Some(NodeType::HorizontalRule)
        );
        // Second child should be an empty paragraph for the cursor
        let para = new_state.doc.child(1).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
        assert_eq!(para.text_content(), "");
        // Cursor should be inside the new paragraph
        assert_eq!(new_state.selection.from(), 2);
    }

    #[test]
    fn no_match_for_plain_text() {
        let rules = default_input_rules();
        let state = make_state("hello");
        let txn = check_input_rules(&rules, &state, "hello", 1);
        assert!(txn.is_none());
    }

    // ── Inline mark rules ──

    #[test]
    fn bold_rule_matches() {
        let rules = default_input_rules();
        let state = make_state("**bold**");
        let txn = check_input_rules(&rules, &state, "**bold**", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn italic_rule_matches() {
        let rules = default_input_rules();
        let state = make_state("*italic*");
        let txn = check_input_rules(&rules, &state, "*italic*", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn code_rule_matches() {
        let rules = default_input_rules();
        let state = make_state("`code`");
        let txn = check_input_rules(&rules, &state, "`code`", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn bold_rule_produces_bold_text() {
        let rules = default_input_rules();
        let state = make_state("**hello**");
        let txn = check_input_rules(&rules, &state, "**hello**", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // Should contain bold "hello"
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "hello");
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    }

    #[test]
    fn code_rule_produces_code_text() {
        let rules = default_input_rules();
        let state = make_state("`fn main()`");
        let txn = check_input_rules(&rules, &state, "`fn main()`", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "fn main()");
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Code));
    }

    // ── Underscore variants ──

    #[test]
    fn bold_underscore_matches() {
        let rules = default_input_rules();
        let state = make_state("__bold__");
        let txn = check_input_rules(&rules, &state, "__bold__", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn italic_underscore_matches() {
        let rules = default_input_rules();
        let state = make_state("_italic_");
        let txn = check_input_rules(&rules, &state, "_italic_", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn bold_underscore_produces_bold_text() {
        let rules = default_input_rules();
        let state = make_state("__hello__");
        let txn = check_input_rules(&rules, &state, "__hello__", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "hello");
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    }

    // ── Inline mark content verification ──

    #[test]
    fn italic_rule_produces_italic_text() {
        let rules = default_input_rules();
        let state = make_state("*hello*");
        let txn = check_input_rules(&rules, &state, "*hello*", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "hello");
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Italic));
    }

    #[test]
    fn italic_underscore_produces_italic_text() {
        let rules = default_input_rules();
        let state = make_state("_hello_");
        let txn = check_input_rules(&rules, &state, "_hello_", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        let first = para.child(0).unwrap();
        assert_eq!(first.text_content(), "hello");
        assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Italic));
    }

    // ── Inline mark edge cases ──

    #[test]
    fn bold_empty_content_no_match() {
        let rules = default_input_rules();
        let state = make_state("****");
        assert!(check_input_rules(&rules, &state, "****", 1).is_none());
    }

    #[test]
    fn italic_empty_content_no_match() {
        let rules = default_input_rules();
        let state = make_state("**");
        assert!(check_input_rules(&rules, &state, "**", 1).is_none());
    }

    #[test]
    fn code_empty_content_no_match() {
        let rules = default_input_rules();
        let state = make_state("``");
        assert!(check_input_rules(&rules, &state, "``", 1).is_none());
    }

    #[test]
    fn single_star_no_match() {
        let rules = default_input_rules();
        let state = make_state("*");
        assert!(check_input_rules(&rules, &state, "*", 1).is_none());
    }

    #[test]
    fn single_backtick_no_match() {
        let rules = default_input_rules();
        let state = make_state("`");
        assert!(check_input_rules(&rules, &state, "`", 1).is_none());
    }

    #[test]
    fn bold_with_preceding_text() {
        let rules = default_input_rules();
        let state = make_state("hello **world**");
        let txn = check_input_rules(&rules, &state, "hello **world**", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        // "hello " should remain as plain text, "world" should be bold
        assert_eq!(para.text_content(), "hello world");
        let mut found_bold = false;
        for i in 0..para.child_count() {
            let child = para.child(i).unwrap();
            if child.marks().iter().any(|m| m.mark_type == MarkType::Bold) {
                assert_eq!(child.text_content(), "world");
                found_bold = true;
            }
        }
        assert!(found_bold, "should have bold 'world'");
    }

    #[test]
    fn italic_with_preceding_text() {
        let rules = default_input_rules();
        let state = make_state("hello *world*");
        let txn = check_input_rules(&rules, &state, "hello *world*", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "hello world");
        let has_italic = (0..para.child_count()).any(|i| {
            let c = para.child(i).unwrap();
            c.text_content() == "world" && c.marks().iter().any(|m| m.mark_type == MarkType::Italic)
        });
        assert!(has_italic);
    }

    #[test]
    fn code_with_preceding_text() {
        let rules = default_input_rules();
        let state = make_state("hello `code`");
        let txn = check_input_rules(&rules, &state, "hello `code`", 1).unwrap();
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "hello code");
        let has_code = (0..para.child_count()).any(|i| {
            let c = para.child(i).unwrap();
            c.text_content() == "code" && c.marks().iter().any(|m| m.mark_type == MarkType::Code)
        });
        assert!(has_code);
    }

    // ── Missing block rule variants ──

    #[test]
    fn bullet_list_plus_creates_list() {
        let rules = default_input_rules();
        let state = make_state("+ ");
        let txn = check_input_rules(&rules, &state, "+ ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::BulletList));
    }

    #[test]
    fn task_list_checked_creates_checked_item() {
        let rules = default_input_rules();
        let state = make_state("[x] ");
        let txn = check_input_rules(&rules, &state, "[x] ", 1).unwrap();
        let new_state = state.apply(txn);
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::TaskList));
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::TaskItem));
        assert_eq!(item.attrs().get("checked").unwrap(), "true");
    }

    // ── Block rules preserve remaining text ──

    #[test]
    fn bullet_list_preserves_remaining_text() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("* Hello")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(3), // after "* "
            ..EditorState::create_default(doc)
        };
        let txn = check_input_rules(&default_input_rules(), &state, "* ", 1).unwrap();
        let new_state = state.apply(txn);
        let list = new_state.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.text_content(), "Hello");
    }

    #[test]
    fn blockquote_preserves_remaining_text() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("> Hello")]),
            )]),
        );
        let state = EditorState {
            selection: Selection::cursor(3), // after "> "
            ..EditorState::create_default(doc)
        };
        let txn = check_input_rules(&default_input_rules(), &state, "> ", 1).unwrap();
        let new_state = state.apply(txn);
        let bq = new_state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        assert_eq!(bq.text_content(), "Hello");
    }

    // ── Block structure depth ──

    #[test]
    fn blockquote_contains_paragraph() {
        let rules = default_input_rules();
        let state = make_state("> ");
        let txn = check_input_rules(&rules, &state, "> ", 1).unwrap();
        let new_state = state.apply(txn);
        let bq = new_state.doc.child(0).unwrap();
        assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
        let inner = bq.child(0).unwrap();
        assert_eq!(inner.node_type(), Some(NodeType::Paragraph));
    }

    #[test]
    fn bullet_list_contains_item_with_paragraph() {
        let rules = default_input_rules();
        let state = make_state("* ");
        let txn = check_input_rules(&rules, &state, "* ", 1).unwrap();
        let new_state = state.apply(txn);
        let list = new_state.doc.child(0).unwrap();
        let item = list.child(0).unwrap();
        assert_eq!(item.node_type(), Some(NodeType::ListItem));
        let para = item.child(0).unwrap();
        assert_eq!(para.node_type(), Some(NodeType::Paragraph));
    }

    // ── Block rules should NOT match with text before trigger ──

    #[test]
    fn heading_trigger_not_at_start_no_match() {
        let rules = default_input_rules();
        let state = make_state("hello # ");
        assert!(check_input_rules(&rules, &state, "hello # ", 1).is_none());
    }

    #[test]
    fn bullet_trigger_not_at_start_no_match() {
        let rules = default_input_rules();
        let state = make_state("hello * ");
        assert!(check_input_rules(&rules, &state, "hello * ", 1).is_none());
    }

    // ── get_block_text_before ──

    #[test]
    fn get_block_text_before_middle() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        // Cursor at position 6 → after "Hello" (5 chars from content start at 1)
        let (text, start) = get_block_text_before(&doc, 6).unwrap();
        assert_eq!(text, "Hello");
        assert_eq!(start, 1);
    }

    #[test]
    fn get_block_text_before_at_start() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let (text, start) = get_block_text_before(&doc, 1).unwrap();
        assert_eq!(text, "");
        assert_eq!(start, 1);
    }

    #[test]
    fn get_block_text_before_at_end() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello")]),
            )]),
        );
        let (text, start) = get_block_text_before(&doc, 6).unwrap();
        assert_eq!(text, "Hello");
        assert_eq!(start, 1);
    }

    #[test]
    fn get_block_text_before_second_paragraph() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("First")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Second")]),
                ),
            ]),
        );
        // First para: pos 0(open) 1-5(text) 6(close) = size 7
        // Second para: pos 7(open) 8-13(text) 14(close)
        let (text, start) = get_block_text_before(&doc, 11).unwrap();
        assert_eq!(text, "Sec");
        assert_eq!(start, 8);
    }

    #[test]
    fn get_block_text_before_outside_block_returns_none() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element(NodeType::HorizontalRule)]),
        );
        // HR is a leaf — cursor at position 0 is at doc level, not inside a text block
        assert!(get_block_text_before(&doc, 0).is_none());
    }

    #[test]
    fn get_block_text_before_empty_paragraph() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::empty(),
            )]),
        );
        let (text, start) = get_block_text_before(&doc, 1).unwrap();
        assert_eq!(text, "");
        assert_eq!(start, 1);
    }

    // ── Inline mark cursor placement (regression: #selection-after-replace) ──

    #[test]
    fn bold_rule_cursor_after_text_not_selecting() {
        let rules = default_input_rules();
        let state = make_state("hello **world**");
        let txn = check_input_rules(&rules, &state, "hello **world**", 1).unwrap();
        let new_state = state.apply(txn);
        // Cursor must be a cursor (empty selection), not a range over "world"
        assert!(new_state.selection.empty(),
            "selection should be a cursor, not a range: from={} to={}",
            new_state.selection.from(), new_state.selection.to());
        // Cursor should be right after "world"
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "hello world");
        // Position: 1(para open) + 6("hello ") + 5("world") = 12
        assert_eq!(new_state.selection.from(), 12);
    }

    #[test]
    fn italic_rule_cursor_after_text_not_selecting() {
        let rules = default_input_rules();
        let state = make_state("*word*");
        let txn = check_input_rules(&rules, &state, "*word*", 1).unwrap();
        let new_state = state.apply(txn);
        assert!(new_state.selection.empty(),
            "selection should be a cursor after italic conversion");
    }

    #[test]
    fn code_rule_cursor_after_text_not_selecting() {
        let rules = default_input_rules();
        let state = make_state("`code`");
        let txn = check_input_rules(&rules, &state, "`code`", 1).unwrap();
        let new_state = state.apply(txn);
        assert!(new_state.selection.empty(),
            "selection should be a cursor after code conversion");
    }

    #[test]
    fn bold_rule_clears_stored_marks() {
        // Regression: typing "asdf **1234**qwer" made "qwer" bold because
        // stored marks inherited from the bold text node to the left.
        let rules = default_input_rules();
        let state = make_state("asdf **1234**");
        let txn = check_input_rules(&rules, &state, "asdf **1234**", 1).unwrap();
        let new_state = state.apply(txn);
        // stored_marks should be empty (no marks) so next typed char is plain
        assert_eq!(new_state.stored_marks, Some(vec![]),
            "stored_marks should be explicitly empty after inline mark rule");
    }

    #[test]
    fn code_rule_clears_stored_marks() {
        let rules = default_input_rules();
        let state = make_state("`code`");
        let txn = check_input_rules(&rules, &state, "`code`", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(new_state.stored_marks, Some(vec![]));
    }

    // ── check_input_rules: first match wins ──

    #[test]
    fn first_matching_rule_wins() {
        // "* " matches bullet_list rule before any other rule
        let rules = default_input_rules();
        let state = make_state("* ");
        let txn = check_input_rules(&rules, &state, "* ", 1).unwrap();
        let new_state = state.apply(txn);
        // Should be a bullet list, not anything else
        assert_eq!(new_state.doc.child(0).unwrap().node_type(), Some(NodeType::BulletList));
    }
}
