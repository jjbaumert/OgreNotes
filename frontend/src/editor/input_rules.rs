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
            let bold_node = Node::text_with_marks(inner, vec![Mark::new(MarkType::Bold)]);
            let content = Fragment::from(vec![bold_node]);
            let slice = Slice::new(content, 0, 0);
            let txn = state.transaction().replace(from, to, slice).ok()?;
            Some(txn)
        }),
    }
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
            if matched.len() < 3 {
                return None;
            }
            let inner = &matched[1..matched.len() - 1];
            let italic_node = Node::text_with_marks(inner, vec![Mark::new(MarkType::Italic)]);
            let content = Fragment::from(vec![italic_node]);
            let slice = Slice::new(content, 0, 0);
            let txn = state.transaction().replace(from, to, slice).ok()?;
            Some(txn)
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
            if matched.len() < 5 {
                return None;
            }
            let inner = &matched[2..matched.len() - 2];
            let bold_node = Node::text_with_marks(inner, vec![Mark::new(MarkType::Bold)]);
            let content = Fragment::from(vec![bold_node]);
            let slice = Slice::new(content, 0, 0);
            let txn = state.transaction().replace(from, to, slice).ok()?;
            Some(txn)
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
            if matched.len() < 3 {
                return None;
            }
            let inner = &matched[1..matched.len() - 1];
            let italic_node = Node::text_with_marks(inner, vec![Mark::new(MarkType::Italic)]);
            let content = Fragment::from(vec![italic_node]);
            let slice = Slice::new(content, 0, 0);
            let txn = state.transaction().replace(from, to, slice).ok()?;
            Some(txn)
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
            if matched.len() < 3 {
                return None;
            }
            let inner = &matched[1..matched.len() - 1];
            let code_node = Node::text_with_marks(inner, vec![Mark::new(MarkType::Code)]);
            let content = Fragment::from(vec![code_node]);
            let slice = Slice::new(content, 0, 0);
            let txn = state.transaction().replace(from, to, slice).ok()?;
            Some(txn)
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
}
