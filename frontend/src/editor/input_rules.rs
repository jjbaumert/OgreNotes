// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
    /// Returns `Some((match_start_offset, match_len))` if the pattern matches.
    ///
    /// Both are **byte** offsets into the matched string — matchers are
    /// written with `str::rfind` / `str::len`, which speak bytes.
    /// [`check_input_rules`] converts them to model positions; matchers
    /// must never do that arithmetic themselves (#152).
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
    // Code blocks hold literal text — markdown trigger characters must
    // not auto-format there. Without this gate the inline mark rules
    // fire on code (`__init__` bolds, backticks make Code marks the
    // schema forbids in code blocks), and a block whose entire text is
    // a block trigger ("# ", "> ", "``` ") would even convert the code
    // block's node type.
    if let Some(block) = find_block_at(&state.doc, state.selection.from()) {
        if block.node_type == NodeType::CodeBlock {
            return None;
        }
    }
    for rule in rules {
        let Some((match_offset, match_len)) = (rule.matcher)(text_before) else {
            continue;
        };
        // Matchers speak bytes; the document model speaks positions.
        // `get` (rather than indexing) also makes a matcher that returned
        // a non-boundary offset decline instead of panicking.
        let Some(matched_text) = text_before.get(match_offset..match_offset + match_len) else {
            continue;
        };
        // A match spanning an inline atom has no plain-text equivalent —
        // firing would replace a hard break or a mention with a literal
        // placeholder character. Decline instead.
        if matched_text.contains(INLINE_ATOM) {
            continue;
        }
        // #152: `block_start + match_offset` added a *byte* offset to a
        // model position. `text_before` carries exactly one char per
        // model position (see `block_text`), so counting chars is the
        // conversion — no per-rule arithmetic required.
        let from = block_start + text_before[..match_offset].chars().count();
        let to = from + matched_text.chars().count();
        if let Some(txn) = (rule.handler)(state, from, to, matched_text) {
            return Some(txn);
        }
    }
    None
}

/// Stand-in for an inline leaf node in the text input rules match against.
///
/// A rule match is turned into a model range by adding a char offset to
/// the block's content start, which is only sound when one char of rule
/// text equals one model position. [`Node::text_content`] breaks that in
/// both directions: a `HardBreak` yields zero chars for its one position,
/// a `Mention` yields its whole display string for its one position.
/// Every inline leaf therefore contributes exactly one of these chars.
///
/// U+FFFC OBJECT REPLACEMENT CHARACTER is inert for every rule's pattern,
/// and [`check_input_rules`] declines any match that spans one, so an
/// atom can never be rewritten into literal text.
///
/// A *literal* U+FFFC in user-typed or pasted text is indistinguishable
/// from an atom placeholder here, so a markdown rule spanning one is
/// likewise declined and silently does not fire on that text. This is
/// deliberate and fail-safe: the position arithmetic counts chars
/// uniformly, so a literal U+FFFC is one char = one model position under
/// either interpretation and can never mis-position an edit — the only
/// consequence is the declined rule, never corruption. Do not "fix" the
/// guard to special-case literal U+FFFC; distinguishing the two buys
/// nothing and reintroduces the byte/position confusion this removed.
const INLINE_ATOM: char = '\u{FFFC}';

/// Render a textblock's content as the text input rules match against.
///
/// Invariant: `char_len(block_text(f)) == f.size()` for every fragment —
/// char offsets into the result *are* model offsets into the block.
fn block_text(content: &Fragment) -> String {
    let mut out = String::new();
    push_block_text(&content.children, &mut out);
    out
}

fn push_block_text(nodes: &[Node], out: &mut String) {
    for child in nodes {
        match child {
            Node::Text { text, .. } => out.push_str(text),
            Node::Element {
                node_type, content, ..
            } => {
                if node_type.is_leaf() {
                    out.push(INLINE_ATOM);
                } else {
                    // Not schema-legal inside a textblock, but the size
                    // invariant must hold regardless: open + content +
                    // close, matching `Node::node_size`.
                    out.push(INLINE_ATOM);
                    push_block_text(&content.children, out);
                    out.push(INLINE_ATOM);
                }
            }
        }
    }
}

/// Extract the text content before the cursor in the current block.
/// Returns `(text_before_cursor, block_content_start_pos)` or None if
/// the cursor is not inside a text-containing block.
///
/// Descends through containers (lists, blockquotes) via `find_block_at`
/// to locate the innermost textblock holding the cursor — the same
/// descent the rule handlers already use. The previous version walked
/// only `doc.content.children`, so a cursor inside a nested textblock
/// never matched and input rules silently never fired in lists or
/// blockquotes (#1).
///
/// The text comes from [`block_text`], not `Node::text_content`: the
/// cursor offset is a *model* offset, so truncating by chars is only
/// exact when each model position contributes one char (#152).
pub fn get_block_text_before(doc: &Node, cursor_pos: usize) -> Option<(String, usize)> {
    let block = find_block_at(doc, cursor_pos)?;
    let cursor_offset = cursor_pos.checked_sub(block.content_start)?;
    let text_before: String = block_text(&block.content).chars().take(cursor_offset).collect();
    Some((text_before, block.content_start))
}

/// Build the default set of MVP input rules.
pub fn default_input_rules() -> Vec<InputRule> {
    vec![
        // Block-level rules (at line start)
        heading_rule("# ", 1),
        heading_rule("## ", 2),
        heading_rule("### ", 3),
        blockquote_rule(),
        code_block_rule(),
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

/// Markdown fence rule: `"``` "` converts the block to a plain code
/// block; `"```lang "` also stores the raw tag in the `language` attr
/// (aliases like `rs` resolve at render time via `Language::from_tag`,
/// exactly as markdown import's fence info does — markdown.rs stores
/// the first fence token verbatim too). Anchored like `heading_rule`:
/// the fence must be the block's entire text before the cursor.
/// Promised by design/rich-text-editor.md's input-rule table.
fn code_block_rule() -> InputRule {
    InputRule {
        name: "code_block",
        matcher: Box::new(|text| {
            let tag = fence_tag(text)?;
            // ASCII-only tags. Real fence infos are ASCII; `+ # . - _`
            // cover c++, c#, objective-c, tf-vars… A backtick in the tag
            // (e.g. "````rust ") also lands here and is rejected.
            // Positions no longer depend on this — `check_input_rules`
            // converts the byte length below to a model length (#152) —
            // but the restriction is kept deliberately.
            if !tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+#.-_".contains(c))
            {
                return None;
            }
            Some((0, text.len()))
        }),
        handler: Box::new(|state, from, to, matched| {
            let block_pos = from - 1;
            let tag = fence_tag(matched)?;
            let mut attrs = HashMap::new();
            if !tag.is_empty() {
                attrs.insert("language".to_string(), tag.to_string());
            }
            let txn = state
                .transaction()
                .delete(from, to)
                .ok()?
                .set_node_type(block_pos, NodeType::CodeBlock, attrs)
                .ok()?;
            Some(txn)
        }),
    }
}

/// Split a fence trigger into its info tag: `"```rust "` → `Some("rust")`,
/// `"``` "` → `Some("")`. Shared by the fence rule's matcher and handler
/// so neither hand-slices byte ranges out of the match (#152).
fn fence_tag(text: &str) -> Option<&str> {
    text.strip_prefix("```")?.strip_suffix(' ')
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

/// The char immediately before byte offset `at`, if any.
///
/// The matchers locate their delimiters with `str::rfind`, which yields a
/// byte offset; `s.as_bytes()[at - 1]` then inspects the *last byte* of
/// the preceding char rather than the char itself (#152). The outcome
/// happened to agree for the ASCII comparisons here — a UTF-8
/// continuation byte is never an ASCII letter — but the shape invited
/// exactly the confusion this file is being cleaned of.
fn char_before(s: &str, at: usize) -> Option<char> {
    s.get(..at)?.chars().next_back()
}

/// Whether `c` counts as a word char for CommonMark's intra-word
/// underscore guard. Deliberately ASCII-only, preserving the behaviour of
/// the byte comparison it replaces: `café_x_` still italicises, which
/// CommonMark would not. That is a separate policy question from #152's
/// offset arithmetic and is left alone here.
fn is_word_char(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
}

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
            // Extract the text between ** and ** by pattern, not by byte
            // index — the delimiters are ASCII but the content is not (#152).
            let inner = strip_delimiters(matched, "**")?;
            inline_mark_replace(state, from, to, inner, MarkType::Bold)
        }),
    }
}

/// Strip a matching pair of delimiters off an inline-mark match:
/// `("**bold**", "**")` → `Some("bold")`. Returns `None` when the
/// delimiters are absent or the content between them is empty, which is
/// the guard the byte-length checks used to provide.
fn strip_delimiters<'a>(matched: &'a str, delim: &str) -> Option<&'a str> {
    let inner = matched.strip_prefix(delim)?.strip_suffix(delim)?;
    (!inner.is_empty()).then_some(inner)
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
                    let is_double = char_before(inner, start) == Some('*');
                    if !is_double && start + 1 < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            let inner = strip_delimiters(matched, "*")?;
            inline_mark_replace(state, from, to, inner, MarkType::Italic)
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
                    // Same intra-word guard as the italic underscore rule: the
                    // opening `__` must not follow a word char, so `snake__case__`
                    // isn't bolded mid-word.
                    let preceded_by_word = is_word_char(char_before(inner, start));
                    if !preceded_by_word && content_start < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            let inner = strip_delimiters(matched, "__")?;
            inline_mark_replace(state, from, to, inner, MarkType::Bold)
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
                    let is_double = char_before(inner, start) == Some('_');
                    // CommonMark: `_` emphasis is not allowed intra-word, so the
                    // opening `_` must be at the block start or follow a non-word
                    // char. Without this, identifiers like `SUSTAINED_TYPE_` and
                    // `snake_case_` get mangled into italics mid-word (the bug the
                    // frontend-doctor sustained-type-reload scenario caught).
                    let preceded_by_word = is_word_char(char_before(inner, start));
                    if !is_double && !preceded_by_word && start + 1 < inner.len() {
                        return Some((start, text.len() - start));
                    }
                }
            }
            None
        }),
        handler: Box::new(|state, from, to, matched| {
            let inner = strip_delimiters(matched, "_")?;
            inline_mark_replace(state, from, to, inner, MarkType::Italic)
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
            let inner = strip_delimiters(matched, "`")?;
            inline_mark_replace(state, from, to, inner, MarkType::Code)
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

    fn para(text: &str) -> Node {
        Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(text)]))
    }

    #[test]
    fn get_block_text_before_descends_into_containers() {
        // #1: input rules must fire inside lists/blockquotes. The old
        // top-level-only walk returned the container's text (so rules
        // never matched); now it descends to the innermost textblock.

        // Doc > BulletList(open@0) > ListItem(open@1) > Paragraph(open@2,
        // content@3) > "# ". Cursor after "# " → 5.
        let doc_list = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![para("# ")]),
                )]),
            )]),
        );
        assert_eq!(get_block_text_before(&doc_list, 5), Some(("# ".to_string(), 3)));

        // Doc > Blockquote(open@0) > Paragraph(content@2) > "## ".
        // Cursor after "## " (3 chars) → 5.
        let doc_bq = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![para("## ")]),
            )]),
        );
        assert_eq!(get_block_text_before(&doc_bq, 5), Some(("## ".to_string(), 2)));

        // Top-level paragraph still works (regression).
        let doc_top =
            Node::element_with_content(NodeType::Doc, Fragment::from(vec![para("- ")]));
        assert_eq!(get_block_text_before(&doc_top, 3), Some(("- ".to_string(), 1)));
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
    fn triple_backtick_converts_paragraph_to_code_block() {
        let rules = default_input_rules();
        let state = make_state("``` ");
        let txn = check_input_rules(&rules, &state, "``` ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert!(block.attrs().get("language").is_none(), "bare fence sets no language");
        assert_eq!(block.text_content(), ""); // trigger text deleted
    }

    #[test]
    fn triple_backtick_with_lang_sets_language_attr() {
        let rules = default_input_rules();
        let state = make_state("```python ");
        let txn = check_input_rules(&rules, &state, "```python ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.node_type(), Some(NodeType::CodeBlock));
        assert_eq!(block.attrs().get("language").unwrap(), "python");
        assert_eq!(block.text_content(), "");
    }

    #[test]
    fn triple_backtick_stores_alias_tag_verbatim() {
        // "rs" resolves via Language::from_tag at render time; the attr
        // stores the raw tag, exactly like markdown import does.
        let rules = default_input_rules();
        let state = make_state("```rs ");
        let txn = check_input_rules(&rules, &state, "```rs ", 1).unwrap();
        let new_state = state.apply(txn);
        let block = new_state.doc.child(0).unwrap();
        assert_eq!(block.attrs().get("language").unwrap(), "rs");
    }

    #[test]
    fn triple_backtick_tag_allows_symbol_language_names() {
        // c++, c#, and dotted/dashed tags are real fence infos.
        let rules = default_input_rules();
        let state = make_state("```c++ ");
        let txn = check_input_rules(&rules, &state, "```c++ ", 1).unwrap();
        let new_state = state.apply(txn);
        assert_eq!(
            new_state.doc.child(0).unwrap().attrs().get("language").unwrap(),
            "c++"
        );
    }

    #[test]
    fn triple_backtick_does_not_fire_mid_text() {
        // The fence must be the entire block text before the cursor,
        // same as the heading rules.
        let rules = default_input_rules();
        let state = make_state("x``` ");
        assert!(check_input_rules(&rules, &state, "x``` ", 1).is_none());
        let state = make_state("x```python ");
        assert!(check_input_rules(&rules, &state, "x```python ", 1).is_none());
    }

    #[test]
    fn triple_backtick_rejects_tags_with_inner_space_or_backtick() {
        let rules = default_input_rules();
        for text in ["``` x ", "````rust ", "```a`b "] {
            let state = make_state(text);
            assert!(
                check_input_rules(&rules, &state, text, 1).is_none(),
                "{text:?} must not create a code block"
            );
        }
    }

    #[test]
    fn triple_backtick_rejects_non_ascii_tag_without_panicking() {
        // Positions are char-based but the matcher API byte-slices;
        // non-ASCII tags are declined outright so the two never diverge.
        let rules = default_input_rules();
        let state = make_state("```pythön ");
        assert!(check_input_rules(&rules, &state, "```pythön ", 1).is_none());
    }

    fn code_block_state(text: &str) -> EditorState {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::CodeBlock,
                Fragment::from(vec![Node::text(text)]),
            )]),
        );
        let pos = 1 + super::super::model::char_len(text);
        EditorState {
            selection: Selection::cursor(pos),
            ..EditorState::create_default(doc)
        }
    }

    #[test]
    fn no_input_rule_fires_inside_a_code_block() {
        // Code blocks hold literal text — markdown trigger characters
        // must not auto-format. `__init__` was bolding (the
        // bold-underscore mark rule), and a block whose whole text is
        // a block trigger would even convert node type.
        let rules = default_input_rules();
        for text in [
            "def __init__",  // bold-underscore
            "x *y*",         // italic
            "a `b`",         // inline code mark
            "# ",            // heading trigger
            "> ",            // blockquote trigger
            "``` ",          // nested fence
            "- ",            // bullet list trigger
        ] {
            let state = code_block_state(text);
            assert!(
                check_input_rules(&rules, &state, text, 1).is_none(),
                "rule fired inside a code block for {text:?}"
            );
        }
    }

    #[test]
    fn bold_underscore_still_fires_in_paragraph() {
        // Guard the gate's scope: normal blocks keep their rules.
        let rules = default_input_rules();
        let state = make_state("def __init__");
        assert!(check_input_rules(&rules, &state, "def __init__", 1).is_some());
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
    fn italic_underscore_matches_after_word_boundary() {
        // Opening `_` after a space is a valid emphasis opener.
        let rules = default_input_rules();
        let state = make_state("foo _italic_");
        let txn = check_input_rules(&rules, &state, "foo _italic_", 1);
        assert!(txn.is_some());
    }

    #[test]
    fn italic_underscore_no_match_intra_word() {
        // Regression for the frontend-doctor sustained-type-reload failure:
        // typing the closing `_` of `SUSTAINED_TYPE_` must NOT italicize
        // `TYPE`, because the opening `_` is intra-word (preceded by 'D').
        // CommonMark forbids intra-word `_` emphasis.
        let rules = default_input_rules();
        let state = make_state("SUSTAINED_TYPE_");
        let txn = check_input_rules(&rules, &state, "SUSTAINED_TYPE_", 1);
        assert!(
            txn.is_none(),
            "intra-word `_TYPE_` must not trigger italic emphasis"
        );
    }

    #[test]
    fn bold_underscore_no_match_intra_word() {
        // `__` emphasis is likewise forbidden intra-word, so `snake__case__`
        // is not bolded.
        let rules = default_input_rules();
        let state = make_state("snake__case__");
        let txn = check_input_rules(&rules, &state, "snake__case__", 1);
        assert!(
            txn.is_none(),
            "intra-word `__case__` must not trigger bold emphasis"
        );
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

    // ── #152: byte offsets vs. model positions ──
    //
    // Rule matches are byte offsets into the block's text; the document
    // model counts *positions* (one per char, one per inline leaf). The
    // two only coincide for pure-ASCII, atom-free blocks. Every test
    // below drives the same pipeline `view.rs` drives — build the text
    // with `get_block_text_before`, then match — so the offset handoff
    // is exercised, not bypassed.

    fn hard_break() -> Node {
        Node::element(NodeType::HardBreak)
    }

    fn mention(display: &str) -> Node {
        let mut attrs = HashMap::new();
        attrs.insert("display".to_string(), display.to_string());
        attrs.insert("user_id".to_string(), "u1".to_string());
        Node::Element {
            node_type: NodeType::Mention,
            attrs,
            content: Fragment::empty(),
            marks: vec![],
        }
    }

    /// A doc holding one paragraph of `children`, cursor at its end.
    fn state_with(children: Vec<Node>) -> EditorState {
        let para = Node::element_with_content(NodeType::Paragraph, Fragment::from(children));
        let content_size = para.content_size();
        let doc = Node::element_with_content(NodeType::Doc, Fragment::from(vec![para]));
        EditorState {
            selection: Selection::cursor(1 + content_size),
            ..EditorState::create_default(doc)
        }
    }

    /// Run the rules exactly as `view.rs` does, deriving the rule text
    /// and the block start from the document instead of hardcoding them.
    fn fire_rules(state: &EditorState) -> Option<Transaction> {
        let rules = default_input_rules();
        let (text_before, block_start) =
            get_block_text_before(&state.doc, state.selection.from())?;
        check_input_rules(&rules, state, &text_before, block_start)
    }

    /// Concatenated text of every run in `para` carrying `mark_type`.
    fn marked_text(para: &Node, mark_type: MarkType) -> String {
        (0..para.child_count())
            .filter_map(|i| {
                let c = para.child(i).unwrap();
                c.marks()
                    .iter()
                    .any(|m| m.mark_type == mark_type)
                    .then(|| c.text_content())
            })
            .collect()
    }

    #[test]
    fn bold_after_accented_char_keeps_the_accent() {
        // #152 repro 1: typing `é**x**` produced `é*x`. `é` is two bytes,
        // so the matcher's byte offset ran one position past the model
        // position of the opening `**`.
        let state = state_with(vec![Node::text("é**x**")]);
        let txn = fire_rules(&state).expect("bold rule should fire after `é`");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "éx");
        assert_eq!(marked_text(para, MarkType::Bold), "x");
        // Cursor sits after the bold `x`: 1 (para open) + 1 (é) + 1 (x).
        assert_eq!(new_state.selection.from(), 3);
    }

    #[test]
    fn bold_after_hard_break_keeps_the_break() {
        // #152 repro 2: a hard break contributes 0 chars of text but 1
        // model position, so the replacement window started one position
        // early and swallowed the break.
        let state = state_with(vec![hard_break(), Node::text("**x**")]);
        let txn = fire_rules(&state).expect("bold rule should fire after a hard break");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(
            para.child(0).unwrap().node_type(),
            Some(NodeType::HardBreak),
            "the hard break must survive the input rule"
        );
        assert_eq!(para.text_content(), "x");
        assert_eq!(marked_text(para, MarkType::Bold), "x");
    }

    #[test]
    fn bold_after_cjk_text_keeps_the_text() {
        // Three bytes per char: the offset drifts twice as fast as `é`.
        let state = state_with(vec![Node::text("日本**x**")]);
        let txn = fire_rules(&state).expect("bold rule should fire after CJK text");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "日本x");
        assert_eq!(marked_text(para, MarkType::Bold), "x");
    }

    #[test]
    fn bold_after_emoji_keeps_the_emoji() {
        // Four bytes, one char, one model position — and outside the BMP,
        // so it is a surrogate pair on the DOM side.
        let state = state_with(vec![Node::text("👍**x**")]);
        let txn = fire_rules(&state).expect("bold rule should fire after an emoji");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "👍x");
        assert_eq!(marked_text(para, MarkType::Bold), "x");
    }

    #[test]
    fn bold_wraps_multibyte_content() {
        // The multibyte char is *inside* the match: the match length is a
        // byte length, so the closing edge overshot the block.
        let state = state_with(vec![Node::text("**é**")]);
        let txn = fire_rules(&state).expect("bold rule should fire around `é`");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "é");
        assert_eq!(marked_text(para, MarkType::Bold), "é");
    }

    #[test]
    fn bold_wraps_multi_codepoint_emoji() {
        // `👍🏽` is emoji + skin-tone modifier: 8 bytes, 2 chars, 2 model
        // positions. Byte length and model length disagree by six.
        let state = state_with(vec![Node::text("**👍🏽**")]);
        let txn = fire_rules(&state).expect("bold rule should fire around a modified emoji");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "👍🏽");
        assert_eq!(marked_text(para, MarkType::Bold), "👍🏽");
    }

    #[test]
    fn italic_after_accented_char_keeps_the_accent() {
        let state = state_with(vec![Node::text("é*x*")]);
        let txn = fire_rules(&state).expect("italic rule should fire after `é`");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "éx");
        assert_eq!(marked_text(para, MarkType::Italic), "x");
    }

    #[test]
    fn inline_code_after_accented_char_keeps_the_accent() {
        let state = state_with(vec![Node::text("é`x`")]);
        let txn = fire_rules(&state).expect("code rule should fire after `é`");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "éx");
        assert_eq!(marked_text(para, MarkType::Code), "x");
    }

    #[test]
    fn italic_underscore_after_a_multibyte_word_keeps_the_word() {
        // The underscore rules also inspect the char *before* the opening
        // delimiter (CommonMark's intra-word guard). With `café ` ahead of
        // it the byte offset of that delimiter runs one past its position.
        let state = state_with(vec![Node::text("café _x_")]);
        let txn = fire_rules(&state).expect("italic rule should fire after `café `");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(para.text_content(), "café x");
        assert_eq!(marked_text(para, MarkType::Italic), "x");
    }

    #[test]
    fn bold_after_mention_atom_keeps_the_mention() {
        // A Mention is one model position but contributes its whole
        // display string to `text_content` — the mismatch runs the other
        // way from a hard break's.
        let state = state_with(vec![mention("@alice"), Node::text("**x**")]);
        let txn = fire_rules(&state).expect("bold rule should fire after a mention");
        let new_state = state.apply(txn);
        let para = new_state.doc.child(0).unwrap();
        assert_eq!(
            para.child(0).unwrap().node_type(),
            Some(NodeType::Mention),
            "the mention must survive the input rule"
        );
        assert_eq!(marked_text(para, MarkType::Bold), "x");
    }

    #[test]
    fn inline_rule_declines_a_match_spanning_an_atom() {
        // `*<br>*` has no plain-text inner content to re-mark; firing
        // would replace the break with a literal character.
        let state = state_with(vec![Node::text("*"), hard_break(), Node::text("*")]);
        assert!(
            fire_rules(&state).is_none(),
            "an emphasis run spanning an inline atom must not fire"
        );
    }

    #[test]
    fn block_rule_does_not_fire_after_an_inline_atom() {
        // `# ` is only a heading trigger at the *start* of the block. A
        // preceding hard break contributes no text, so the trigger looked
        // anchored and the conversion ate the break.
        let state = state_with(vec![hard_break(), Node::text("# ")]);
        assert!(
            fire_rules(&state).is_none(),
            "`# ` after a hard break is not at the block start"
        );
    }

    #[test]
    fn code_block_rule_does_not_fire_after_an_inline_atom() {
        // Same shape as the heading case, on the fence rule.
        let state = state_with(vec![hard_break(), Node::text("``` ")]);
        assert!(
            fire_rules(&state).is_none(),
            "```` ``` ```` after a hard break is not at the block start"
        );
    }

    #[test]
    fn get_block_text_before_counts_an_inline_atom_as_one_char() {
        // Doc > Paragraph(content@1) > [HardBreak@1, "ab"@2..4].
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![hard_break(), Node::text("ab")]),
            )]),
        );
        assert_eq!(
            get_block_text_before(&doc, 4),
            Some(("\u{FFFC}ab".to_string(), 1)),
            "a hard break occupies exactly one char of rule text"
        );
    }

    #[test]
    fn get_block_text_before_collapses_a_mention_to_one_char() {
        // Doc > Paragraph(content@1) > [Mention@1, "ab"@2..4]. The
        // mention's six-char display string is one model position.
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![mention("@alice"), Node::text("ab")]),
            )]),
        );
        assert_eq!(
            get_block_text_before(&doc, 4),
            Some(("\u{FFFC}ab".to_string(), 1)),
            "a mention occupies exactly one char of rule text"
        );
    }

    #[test]
    fn block_text_has_one_char_per_model_position() {
        // The invariant the whole offset conversion rests on. Text runs
        // contribute one char per char, every inline leaf contributes one
        // char, and the non-leaf arm contributes its two boundary
        // positions — matching `Node::node_size` in each case.
        let fragment = Fragment::from(vec![
            Node::text("é日👍👍🏽x"),
            hard_break(),
            mention("@alice"),
            Node::text("tail"),
            // Not schema-legal inside a textblock; pinned so the arm that
            // handles it cannot silently break the invariant.
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("nested"), hard_break()]),
            ),
        ]);
        assert_eq!(
            crate::editor::model::char_len(&block_text(&fragment)),
            fragment.size(),
            "block_text must emit exactly one char per model position"
        );
    }

    #[test]
    fn fence_tag_is_char_safe() {
        // Latent path: the fence matcher rejects non-ASCII tags, so this
        // input never reaches the rule today. Pinned directly because the
        // handler used to reach in at byte offsets 3 and len-1.
        assert_eq!(fence_tag("```pythön "), Some("pythön"));
        assert_eq!(fence_tag("```日本語 "), Some("日本語"));
        assert_eq!(fence_tag("``` "), Some(""));
        assert_eq!(fence_tag("```rust"), None, "no trailing space");
        assert_eq!(fence_tag("``rust "), None, "short fence");
    }

    #[test]
    fn strip_delimiters_is_char_safe() {
        assert_eq!(strip_delimiters("**é**", "**"), Some("é"));
        assert_eq!(strip_delimiters("`日本`", "`"), Some("日本"));
        assert_eq!(strip_delimiters("*👍🏽*", "*"), Some("👍🏽"));
        assert_eq!(strip_delimiters("****", "**"), None, "empty content");
        assert_eq!(strip_delimiters("``", "`"), None, "empty content");
        assert_eq!(strip_delimiters("*x*", "**"), None, "wrong delimiter");
    }

    #[test]
    fn a_match_offset_off_a_char_boundary_is_declined_not_panicked() {
        // Byte 1 of "é" is a continuation byte. A matcher that returns it
        // must make the rule decline, not slice through the char.
        let rules = vec![InputRule {
            name: "rogue",
            matcher: Box::new(|text: &str| Some((1, text.len() - 1))),
            handler: Box::new(|_, _, _, _| unreachable!("must not reach the handler")),
        }];
        let state = state_with(vec![Node::text("éx")]);
        assert!(check_input_rules(&rules, &state, "éx", 1).is_none());
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
