//! Browser-level integration tests for the editor.
//!
//! These tests run inside a real browser (headless Chrome/Firefox) via wasm-pack.
//! They test DOM rendering, selection behavior, and event handling that can't be
//! tested with native `cargo test`.
//!
//! Run with: `wasm-pack test --headless --chrome`

use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;
use web_sys::{Document, HtmlElement};

use std::collections::HashMap;

use ogrenotes_frontend::editor::model::{Fragment, Mark, MarkType, Node, NodeType};
use ogrenotes_frontend::editor::selection::Selection;
use ogrenotes_frontend::editor::state::{EditorState, Transaction};
use ogrenotes_frontend::editor::view::EditorView;

wasm_bindgen_test_configure!(run_in_browser);

// ─── Test Helpers ──────────────────────────────────────────────

fn document() -> Document {
    web_sys::window().unwrap().document().unwrap()
}

/// Create a container div attached to the document body for testing.
/// The caller should remove it after the test via `cleanup(&container)`.
fn create_container() -> HtmlElement {
    let doc = document();
    let div = doc.create_element("div").unwrap();
    div.set_id(&format!("test-{}", js_sys::Math::random()));
    doc.body().unwrap().append_child(&div).unwrap();
    div.dyn_into::<HtmlElement>().unwrap()
}

/// Remove a test container from the document.
fn cleanup(container: &HtmlElement) {
    container.remove();
}

/// Create an EditorView with a given document model, returning the view
/// and a Vec to collect dispatched transactions.
fn create_editor(
    container: HtmlElement,
    doc: Node,
) -> (EditorView, std::rc::Rc<std::cell::RefCell<Vec<Transaction>>>) {
    let txns = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let txns_clone = std::rc::Rc::clone(&txns);
    let state = EditorState::create_default(doc);
    let history = std::rc::Rc::new(std::cell::RefCell::new(
        ogrenotes_frontend::editor::plugins::HistoryPlugin::new(),
    ));
    let view = EditorView::new(container, state, move |txn: Transaction| {
        txns_clone.borrow_mut().push(txn);
    }, history);
    (view, txns)
}

/// Get the innerHTML of the editor container.
fn inner_html(view: &EditorView) -> String {
    view.container().inner_html()
}

/// Dispatch a synthetic `beforeinput` event.
fn dispatch_before_input(el: &HtmlElement, input_type: &str, data: Option<&str>) {
    let init = web_sys::InputEventInit::new();
    init.set_input_type(input_type);
    if let Some(d) = data {
        init.set_data(Some(d));
    }
    init.set_bubbles(true);
    init.set_cancelable(true);
    let event = web_sys::InputEvent::new_with_event_init_dict("beforeinput", &init).unwrap();
    el.dispatch_event(&event).unwrap();
}

/// Dispatch a synthetic `keydown` event.
#[allow(dead_code)]
fn dispatch_keydown(el: &HtmlElement, key: &str, ctrl: bool, shift: bool, alt: bool) {
    let init = web_sys::KeyboardEventInit::new();
    init.set_key(key);
    init.set_ctrl_key(ctrl);
    init.set_shift_key(shift);
    init.set_alt_key(alt);
    init.set_bubbles(true);
    init.set_cancelable(true);
    let event = web_sys::KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init).unwrap();
    el.dispatch_event(&event).unwrap();
}

/// Apply all collected transactions to the view state, updating the view after each.
/// Returns the final state.
fn apply_all(view: &EditorView, txns: &std::rc::Rc<std::cell::RefCell<Vec<Transaction>>>) -> EditorState {
    let txns = txns.borrow();
    let mut state = view.state();
    for txn in txns.iter() {
        state = state.apply(txn.clone());
        view.update_state(state.clone());
    }
    state
}

/// Place a browser cursor at a specific model position by setting the
/// view state selection and syncing to DOM.
fn set_cursor(view: &EditorView, pos: usize) {
    let state = view.state();
    let new_state = state.apply(state.transaction().set_selection(Selection::cursor(pos)));
    view.update_state(new_state);
}

/// Set a browser range selection by updating the view state.
fn set_selection(view: &EditorView, anchor: usize, head: usize) {
    let state = view.state();
    let new_state = state.apply(state.transaction().set_selection(Selection::text(anchor, head)));
    view.update_state(new_state);
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

fn strike_doc() -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("normal "),
                Node::text_with_marks("struck", vec![Mark::new(MarkType::Strike)]),
                Node::text(" end"),
            ]),
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

fn heading_doc(level: u8, text: &str) -> Node {
    let mut attrs = HashMap::new();
    attrs.insert("level".to_string(), level.to_string());
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_attrs(
            NodeType::Heading,
            attrs,
            Fragment::from(vec![Node::text(text)]),
        )]),
    )
}

fn blockquote_doc(text: &str) -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Blockquote,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text(text)]),
            )]),
        )]),
    )
}

fn ordered_list_doc(text: &str) -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::OrderedList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text(text)]),
                )]),
            )]),
        )]),
    )
}

fn task_list_doc(text: &str, checked: bool) -> Node {
    let mut attrs = HashMap::new();
    attrs.insert("checked".to_string(), checked.to_string());
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::TaskList,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::TaskItem,
                attrs,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text(text)]),
                )]),
            )]),
        )]),
    )
}

fn code_block_doc(text: &str, lang: &str) -> Node {
    let mut attrs = HashMap::new();
    if !lang.is_empty() {
        attrs.insert("language".to_string(), lang.to_string());
    }
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_attrs(
            NodeType::CodeBlock,
            attrs,
            Fragment::from(vec![Node::text(text)]),
        )]),
    )
}

fn italic_doc() -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("normal "),
                Node::text_with_marks("styled", vec![Mark::new(MarkType::Italic)]),
                Node::text(" end"),
            ]),
        )]),
    )
}

fn hard_break_doc() -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("before"),
                Node::element(NodeType::HardBreak),
                Node::text("after"),
            ]),
        )]),
    )
}

fn three_para_doc() -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Alpha")]),
            ),
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Beta")]),
            ),
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Gamma")]),
            ),
        ]),
    )
}

// ─── Rendering Tests ───────────────────────────────────────────

#[wasm_bindgen_test]
fn renders_paragraph_text() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    let html = inner_html(&view);
    assert!(html.contains("<p>Hello world</p>"), "Expected paragraph, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_bold_text() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), bold_doc());

    let html = inner_html(&view);
    assert!(
        html.contains("<strong>Hello</strong>"),
        "Expected bold text, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_heading() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Heading,
            Fragment::from(vec![Node::text("Title")]),
        )]),
    );
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), doc);

    let html = inner_html(&view);
    assert!(html.contains("<h1>Title</h1>"), "Expected h1, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn empty_paragraph_renders_with_sentinel_br() {
    let doc = Node::empty_doc(); // doc with one empty paragraph
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), doc);

    let html = inner_html(&view);
    assert!(
        html.contains("<br data-sentinel"),
        "Expected sentinel <br> in empty paragraph, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn sentinel_br_removed_when_paragraph_has_content() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    let html = inner_html(&view);
    assert!(
        !html.contains("data-sentinel"),
        "Paragraph with text should not have sentinel br, got: {html}"
    );

    cleanup(&container);
}

// ─── Position Mapping Tests ────────────────────────────────────

#[wasm_bindgen_test]
fn selection_sync_reads_correct_position() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    // Set browser selection to position 3 (inside "Hello world")
    let window = web_sys::window().unwrap();
    let sel = window.get_selection().unwrap().unwrap();
    // Find the text node inside the <p>
    let p = view.container().first_child().unwrap();
    let text_node = p.first_child().unwrap();
    let range = document().create_range().unwrap();
    range.set_start(&text_node, 3).unwrap();
    range.collapse_with_to_start(true);
    sel.remove_all_ranges().unwrap();
    sel.add_range(&range).unwrap();

    // Read selection from view
    let model_sel = view.read_dom_selection();
    assert!(model_sel.is_some(), "Should read DOM selection");
    let model_sel = model_sel.unwrap();
    // Position 3 in "Hello world" = model position 4 (1 for para open + 3 chars)
    assert_eq!(model_sel.from(), 4, "Expected model position 4, got {}", model_sel.from());

    cleanup(&container);
}

#[wasm_bindgen_test]
fn sentinel_br_not_counted_in_positions() {
    // Two paragraphs: first empty (has sentinel br), second has "Text"
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            Node::element(NodeType::Paragraph), // empty
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Text")]),
            ),
        ]),
    );
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), doc);

    // Set browser selection to start of "Text" in the second paragraph
    let window = web_sys::window().unwrap();
    let sel = window.get_selection().unwrap().unwrap();
    let second_p = view.container().child_nodes().item(1).unwrap();
    let text_node = second_p.first_child().unwrap();
    let range = document().create_range().unwrap();
    range.set_start(&text_node, 0).unwrap();
    range.collapse_with_to_start(true);
    sel.remove_all_ranges().unwrap();
    sel.add_range(&range).unwrap();

    let model_sel = view.read_dom_selection().unwrap();
    // Model: doc[ para(empty, size=2), para("Text", size=6) ]
    // Second para content starts at position 3 (para1 size 2, para2 open = 1)
    assert_eq!(
        model_sel.from(),
        3,
        "Sentinel br should not inflate position. Expected 3, got {}",
        model_sel.from()
    );

    cleanup(&container);
}

// ─── Input Handling Tests ──────────────────────────────────────

#[wasm_bindgen_test]
fn insert_text_via_before_input() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Place cursor at position 1 (start of paragraph content)
    let state = view.state();
    let new_state = state.apply(state.transaction().set_selection(Selection::cursor(1)));
    view.update_state(new_state);

    // Dispatch a beforeinput insertText event
    dispatch_before_input(view.container(), "insertText", Some("X"));

    // Check that a transaction was dispatched
    let txns = txns.borrow();
    assert!(!txns.is_empty(), "Expected a transaction to be dispatched");

    // Apply the transaction and check the result
    let state = view.state();
    let new_state = state.apply(txns[0].clone());
    assert_eq!(
        new_state.doc.child(0).unwrap().text_content(),
        "XHello world"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn mark_inheritance_when_typing_in_bold() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    // Place cursor at position 3 (inside bold "Hello")
    let state = view.state();
    let new_state = state.apply(state.transaction().set_selection(Selection::cursor(3)));
    view.update_state(new_state);

    // Type "X" — should inherit bold mark
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let txns = txns.borrow();
    assert!(!txns.is_empty(), "Expected a transaction");
    let state = view.state();
    let new_state = state.apply(txns[0].clone());
    let para = new_state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert!(
        first.marks().iter().any(|m| m.mark_type == MarkType::Bold),
        "Inserted text should inherit bold mark"
    );
    assert!(
        first.text_content().contains('X'),
        "Inserted character should be in the bold text"
    );

    cleanup(&container);
}

// ─── Heading Input Rule Test ───────────────────────────────────

#[wasm_bindgen_test]
fn heading_input_rule_fires_on_hash_space() {
    // Start with a paragraph containing "#"
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("#")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Place cursor at position 2 (after "#")
    let state = view.state();
    let new_state = state.apply(state.transaction().set_selection(Selection::cursor(2)));
    view.update_state(new_state);

    // Type " " — should trigger heading input rule
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let txns = txns.borrow();
    assert!(!txns.is_empty(), "Expected a transaction");

    // Apply all dispatched transactions
    let mut state = view.state();
    for txn in txns.iter() {
        state = state.apply(txn.clone());
    }

    let block = state.doc.child(0).unwrap();
    assert_eq!(
        block.node_type(),
        Some(NodeType::Heading),
        "Paragraph should be converted to heading"
    );
    assert_eq!(block.text_content(), "", "Trigger text '# ' should be removed");

    cleanup(&container);
}

// ─── Enter / Split Block Tests ─────────────────────────────────

#[wasm_bindgen_test]
fn enter_splits_paragraph_into_two() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor after "Hello" (position 6)
    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2, "Should have two paragraphs");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello");
    assert_eq!(state.doc.child(1).unwrap().text_content(), " world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_at_end_creates_empty_paragraph() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor at end of text (position 12)
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello world");
    assert_eq!(state.doc.child(1).unwrap().text_content(), "");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_on_empty_paragraph_creates_new_empty_paragraph() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), Node::empty_doc());

    // Cursor in empty paragraph (position 1)
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2, "Should have two empty paragraphs");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "");
    assert_eq!(state.doc.child(1).unwrap().text_content(), "");

    // Verify the new empty paragraph renders with sentinel br
    let html = inner_html(&view);
    let sentinel_count = html.matches("data-sentinel").count();
    assert_eq!(sentinel_count, 2, "Both empty paragraphs should have sentinel brs");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn typing_after_enter_on_empty_line_works() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), Node::empty_doc());

    // Press Enter on empty paragraph
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertParagraph", None);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();

    // Cursor should be in the second paragraph (position 3)
    assert_eq!(state.selection.from(), 3);

    // Type "Hello" in the new paragraph
    dispatch_before_input(view.container(), "insertText", Some("Hello"));

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(1).unwrap().text_content(), "Hello");

    cleanup(&container);
}

// ─── Backspace / Join Block Tests ──────────────────────────────

#[wasm_bindgen_test]
fn backspace_at_block_start_joins_with_previous() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    // Cursor at start of second paragraph (position 8)
    // para1: offset 0, size 7, content 1..6
    // para2: offset 7, size 7, content_start 8
    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1, "Should join into one paragraph");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "HelloWorld");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_mid_text_deletes_character() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor at position 4 (after "Hel")
    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Helo world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_with_cross_block_selection_merges() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    // Select from position 3 (after "He") to position 10 (after "Wo")
    set_selection(&view, 3, 10);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1, "Should merge into one paragraph");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Herld");

    cleanup(&container);
}

// ─── Stored Marks / Split Block Tests ──────────────────────────

#[wasm_bindgen_test]
fn enter_clears_stored_marks() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Set stored marks to bold, then press Enter
    let state = view.state();
    let state_with_marks = EditorState {
        selection: Selection::cursor(6),
        stored_marks: Some(vec![Mark::new(MarkType::Bold)]),
        ..state
    };
    view.update_state(state_with_marks);

    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert!(
        state.stored_marks.is_none(),
        "stored_marks should be cleared after Enter"
    );

    cleanup(&container);
}

// ─── Mark Inheritance Tests ────────────────────────────────────

#[wasm_bindgen_test]
fn typing_in_strikethrough_inherits_mark() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), strike_doc());

    // "normal " = 7 chars, "struck" = 6 chars, " end" = 4 chars
    // Para content starts at position 1
    // "struck" starts at position 8, ends at 14
    // Place cursor at position 11 (inside "struck", after "str")
    set_cursor(&view, 11);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();

    // Find the text containing "X"
    let mut found_x_with_strike = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            found_x_with_strike = child
                .marks()
                .iter()
                .any(|m| m.mark_type == MarkType::Strike);
            break;
        }
    }
    assert!(
        found_x_with_strike,
        "Text typed inside strikethrough section should inherit the strike mark"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn typing_outside_bold_does_not_inherit() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    // bold "Hello" ends at position 6, plain " world" starts at 6
    // Place cursor at position 8 (inside " world", after " w")
    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();

    let mut found_x_with_bold = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            found_x_with_bold = child
                .marks()
                .iter()
                .any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(
        !found_x_with_bold,
        "Text typed outside bold section should NOT inherit bold mark"
    );

    cleanup(&container);
}

// ─── Selection Direction Tests ─────────────────────────────────

#[wasm_bindgen_test]
fn backward_selection_preserves_direction_in_dom() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    // Set a backward selection: anchor=8, head=3 (selecting "llo w" right-to-left)
    set_selection(&view, 8, 3);

    // Read back the DOM selection
    let window = web_sys::window().unwrap();
    let dom_sel = window.get_selection().unwrap().unwrap();

    // The anchor should be to the right of the focus (backward)
    let anchor_offset = dom_sel.anchor_offset();
    let focus_offset = dom_sel.focus_offset();

    // Read as model positions
    let model_sel = view.read_dom_selection().unwrap();
    assert_eq!(model_sel.anchor(), 8, "Anchor should be at position 8");
    assert_eq!(model_sel.head(), 3, "Head should be at position 3");
    assert_eq!(model_sel.from(), 3, "from() should be the left edge");
    assert_eq!(model_sel.to(), 8, "to() should be the right edge");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn forward_selection_preserved() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    // Forward selection: anchor=3, head=8
    set_selection(&view, 3, 8);

    let model_sel = view.read_dom_selection().unwrap();
    assert_eq!(model_sel.anchor(), 3);
    assert_eq!(model_sel.head(), 8);

    cleanup(&container);
}

// ─── Update without re-render Tests ────────────────────────────

#[wasm_bindgen_test]
fn selection_only_change_does_not_wipe_dom() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), simple_doc());

    let html_before = inner_html(&view);

    // Change only the selection (no doc change)
    set_cursor(&view, 6);

    let html_after = inner_html(&view);
    assert_eq!(
        html_before, html_after,
        "DOM should not be re-rendered for selection-only changes"
    );

    cleanup(&container);
}

// ─── Horizontal Rule Input Rule Tests ─────────────────────────

#[wasm_bindgen_test]
fn hr_input_rule_creates_hr_and_new_paragraph() {
    // Start with a paragraph containing "--"
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("--")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Place cursor after "--" (position 3)
    set_cursor(&view, 3);

    // Type "-" to complete "---" and trigger the HR input rule
    dispatch_before_input(view.container(), "insertText", Some("-"));

    let state = apply_all(&view, &txns);

    // Doc should now have: HR + empty paragraph
    assert_eq!(state.doc.child_count(), 2, "Should have HR and paragraph, got {}", state.doc.child_count());
    assert_eq!(
        state.doc.child(0).unwrap().node_type(),
        Some(NodeType::HorizontalRule),
        "First child should be HR"
    );
    assert_eq!(
        state.doc.child(1).unwrap().node_type(),
        Some(NodeType::Paragraph),
        "Second child should be a paragraph"
    );
    assert_eq!(
        state.doc.child(1).unwrap().text_content(),
        "",
        "New paragraph should be empty"
    );

    // Cursor should be inside the new empty paragraph
    assert_eq!(state.selection.from(), 2, "Cursor should be inside new paragraph");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn hr_renders_hr_element_in_dom() {
    // Start with a paragraph containing "--"
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("--")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "insertText", Some("-"));
    apply_all(&view, &txns);

    let html = inner_html(&view);
    assert!(html.contains("<hr"), "DOM should contain <hr> element, got: {html}");
    assert!(html.contains("<p>"), "DOM should contain a new paragraph after HR, got: {html}");

    cleanup(&container);
}

// ─── List Enter / Split Tests ─────────────────────────────────

fn bullet_list_doc(text: &str) -> Node {
    Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text(text)]),
                )]),
            )]),
        )]),
    )
}

fn two_item_bullet_list_doc() -> Node {
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

#[wasm_bindgen_test]
fn enter_in_list_item_creates_new_list_item() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc("Hello"));

    // Positions: BulletList(0) > ListItem(1) > Para(2) > "Hello"(3..8)
    // Cursor after "He" (position 5)
    set_cursor(&view, 5);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);

    // BulletList should now have 2 items
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should have 2 list items after Enter");

    let item1 = list.child(0).unwrap();
    assert_eq!(item1.node_type(), Some(NodeType::ListItem));
    assert_eq!(item1.text_content(), "He");

    let item2 = list.child(1).unwrap();
    assert_eq!(item2.node_type(), Some(NodeType::ListItem));
    assert_eq!(item2.text_content(), "llo");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_at_end_of_list_item_creates_empty_item() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc("Hello"));

    // Cursor at end of "Hello" (position 8)
    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);

    let list = state.doc.child(0).unwrap();
    assert_eq!(list.child_count(), 2, "Should have 2 list items");
    assert_eq!(list.child(0).unwrap().text_content(), "Hello");
    assert_eq!(list.child(1).unwrap().text_content(), "");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_in_list_renders_two_li_elements() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc("Hello"));

    set_cursor(&view, 5);
    dispatch_before_input(view.container(), "insertParagraph", None);
    apply_all(&view, &txns);

    let html = inner_html(&view);
    let li_count = html.matches("<li>").count();
    assert_eq!(li_count, 2, "Should render 2 <li> elements in DOM, got: {html}");

    cleanup(&container);
}

// ─── List Indent / Dedent Tests ───────────────────────────────

#[wasm_bindgen_test]
fn tab_indents_second_list_item() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // Cursor inside "second" (position 12)
    // BulletList(0) > ListItem1(1) > Para(2) > "first"(3..8)
    //               > ListItem2(10) > Para(11) > "second"(12..18)
    set_cursor(&view, 12);

    // Press Tab
    dispatch_keydown(view.container(), "Tab", false, false, false);

    let state = apply_all(&view, &txns);

    // Should now be: BulletList > ListItem("first", BulletList > ListItem("second"))
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 1, "Outer list should have 1 item after indent");

    let outer_item = list.child(0).unwrap();
    assert_eq!(outer_item.child_count(), 2, "Outer item should have paragraph + sub-list");
    assert_eq!(outer_item.child(0).unwrap().text_content(), "first");

    let sub_list = outer_item.child(1).unwrap();
    assert_eq!(sub_list.node_type(), Some(NodeType::BulletList));
    assert_eq!(sub_list.child_count(), 1);
    assert_eq!(sub_list.child(0).unwrap().text_content(), "second");

    // Cursor must stay inside "second", not jump to "first"
    assert_eq!(
        state.selection.from(),
        12,
        "Cursor should remain inside 'second' after indent, got {}",
        state.selection.from()
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn tab_on_first_item_does_nothing() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // Cursor inside "first" (position 3)
    set_cursor(&view, 3);

    // Press Tab — first item can't be indented
    dispatch_keydown(view.container(), "Tab", false, false, false);

    let collected = txns.borrow();
    assert!(collected.is_empty(), "No transaction should be dispatched for first item Tab");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn tab_indent_renders_nested_ul() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    set_cursor(&view, 12);
    dispatch_keydown(view.container(), "Tab", false, false, false);
    apply_all(&view, &txns);

    let html = inner_html(&view);
    // Should have nested <ul> inside the first <li>
    assert!(
        html.contains("<ul>") && html.matches("<ul>").count() >= 2,
        "Should render nested <ul> elements, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn shift_tab_dedents_nested_item() {
    // Build nested: BulletList > ListItem > [Para("first"), BulletList > ListItem > Para("second")]
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
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), nested_doc);

    // Cursor inside "second" (position 12)
    // BulletList(0) > ListItem(1) > Para(2) > "first"(3..8)
    //                             > InnerBulletList(9) > ListItem(10) > Para(11) > "second"(12..18)
    set_cursor(&view, 12);

    // Press Shift-Tab
    dispatch_keydown(view.container(), "Tab", false, true, false);

    let state = apply_all(&view, &txns);

    // Should now be: BulletList > [ListItem("first"), ListItem("second")]
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should have 2 items after dedent");
    assert_eq!(list.child(0).unwrap().text_content(), "first");
    assert_eq!(list.child(1).unwrap().text_content(), "second");

    // Cursor must stay inside "second", not jump to "first"
    assert_eq!(
        state.selection.from(),
        12,
        "Cursor should remain inside 'second' after dedent, got {}",
        state.selection.from()
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn shift_tab_on_top_level_item_does_nothing() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // Cursor inside "second" at top level
    set_cursor(&view, 12);

    // Press Shift-Tab — can't dedent further
    dispatch_keydown(view.container(), "Tab", false, true, false);

    let collected = txns.borrow();
    assert!(collected.is_empty(), "No transaction should be dispatched for top-level Shift-Tab");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn tab_then_shift_tab_roundtrips() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // Indent second item
    set_cursor(&view, 12);
    dispatch_keydown(view.container(), "Tab", false, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());

    // Verify it's nested
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.child_count(), 1, "Should be nested after Tab");

    // Find cursor position inside "second" in the nested structure
    // After nesting: BulletList(0) > ListItem(1) > Para("first")(2..9) > InnerBulletList(9)
    //   > ListItem(10) > Para("second")(11) > "second"(12..)
    // The cursor should have been mapped. Let's place it explicitly.
    set_cursor(&view, 12);

    // Dedent it back
    dispatch_keydown(view.container(), "Tab", false, true, false);
    let state = apply_all(&view, &txns);

    // Should be back to flat: BulletList > [ListItem("first"), ListItem("second")]
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.child_count(), 2, "Should be flat again after Shift-Tab");
    assert_eq!(list.child(0).unwrap().text_content(), "first");
    assert_eq!(list.child(1).unwrap().text_content(), "second");

    // Cursor must stay inside "second" through the full roundtrip
    assert_eq!(
        state.selection.from(),
        12,
        "Cursor should remain inside 'second' after roundtrip, got {}",
        state.selection.from()
    );

    cleanup(&container);
}

// ─── Toolbar Interaction Tests ─────────────────────────────────

#[wasm_bindgen_test]
fn toggle_mark_on_range_applies_mark() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Select "Hello" (positions 1..6)
    set_selection(&view, 1, 6);

    // Manually apply a bold mark toggle (simulating toolbar command)
    let state = view.state();
    let state = EditorState {
        selection: Selection::text(1, 6),
        ..state
    };
    let mark = Mark::new(MarkType::Strike);
    if let Ok(txn) = state.transaction().add_mark(1, 6, mark) {
        let new_state = state.apply(txn);
        view.update_state(new_state);
    }

    let html = inner_html(&view);
    assert!(
        html.contains("<s>Hello</s>"),
        "Strike should be applied to selected text, got: {html}"
    );

    cleanup(&container);
}

// ─── Tier 1A: Keyboard Shortcut Mark Toggles ─────────────────

#[wasm_bindgen_test]
fn ctrl_b_toggles_bold_on() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "b", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert!(
        first.marks().iter().any(|m| m.mark_type == MarkType::Bold),
        "Should apply bold mark"
    );
    let html = inner_html(&view);
    assert!(html.contains("<strong>Hello</strong>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_b_toggles_bold_off() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "b", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains("Hello") {
            assert!(
                !child.marks().iter().any(|m| m.mark_type == MarkType::Bold),
                "Bold should be removed"
            );
        }
    }
    let html = inner_html(&view);
    assert!(!html.contains("<strong>"), "No <strong> in DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_i_toggles_italic() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "i", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Italic),
        "Should apply italic mark"
    );
    let html = inner_html(&view);
    assert!(html.contains("<em>Hello</em>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_u_applies_underline() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "u", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Underline),
        "Should apply underline mark"
    );
    let html = inner_html(&view);
    assert!(html.contains("<u>Hello</u>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_e_applies_code_mark() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "e", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Code),
        "Should apply code mark"
    );
    let html = inner_html(&view);
    assert!(html.contains("<code>Hello</code>"), "DOM: {html}");

    cleanup(&container);
}

// ─── Tier 1B: Keyboard Shortcut Headings ──────────────────────

#[wasm_bindgen_test]
fn ctrl_alt_1_converts_to_h1() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "1", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "1");
    let html = inner_html(&view);
    assert!(html.contains("<h1>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_alt_2_converts_to_h2() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "2", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "2");
    let html = inner_html(&view);
    assert!(html.contains("<h2>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_alt_3_converts_to_h3() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "3", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "3");
    let html = inner_html(&view);
    assert!(html.contains("<h3>"), "DOM: {html}");

    cleanup(&container);
}

// ─── Tier 1C: Block Input Rules ───────────────────────────────

#[wasm_bindgen_test]
fn heading_2_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("##")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let mut state = view.state();
    for txn in txns.borrow().iter() {
        state = state.apply(txn.clone());
    }
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "2");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn heading_3_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("###")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let mut state = view.state();
    for txn in txns.borrow().iter() {
        state = state.apply(txn.clone());
    }
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "3");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn bullet_list_asterisk_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("*")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 2);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    let html = inner_html(&view);
    assert!(html.contains("<ul>") && html.contains("<li>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ordered_list_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("1.")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::OrderedList));
    let html = inner_html(&view);
    assert!(html.contains("<ol>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn task_list_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("[ ]")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::TaskList));
    let html = inner_html(&view);
    assert!(html.contains("data-type=\"taskList\""), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn blockquote_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text(">")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 2);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Blockquote));
    let html = inner_html(&view);
    assert!(html.contains("<blockquote>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn hr_underscore_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("__")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "insertText", Some("_"));

    let state = apply_all(&view, &txns);
    assert_eq!(
        state.doc.child(0).unwrap().node_type(),
        Some(NodeType::HorizontalRule)
    );
    assert_eq!(
        state.doc.child(1).unwrap().node_type(),
        Some(NodeType::Paragraph)
    );

    cleanup(&container);
}

// ─── Tier 1D: Inline Mark Input Rules ─────────────────────────

#[wasm_bindgen_test]
fn bold_asterisk_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("**hello*")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 9);
    dispatch_before_input(view.container(), "insertText", Some("*"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "hello");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    let html = inner_html(&view);
    assert!(html.contains("<strong>hello</strong>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn italic_asterisk_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("*hello")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "insertText", Some("*"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "hello");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Italic));
    let html = inner_html(&view);
    assert!(html.contains("<em>hello</em>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn code_backtick_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("`code")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertText", Some("`"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "code");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Code));
    let html = inner_html(&view);
    assert!(html.contains("<code>code</code>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn bold_underscore_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("__bold_")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "insertText", Some("_"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "bold");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn italic_underscore_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("_ital")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertText", Some("_"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "ital");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Italic));

    cleanup(&container);
}

// ─── Tier 2A: Rendering Untested Node Types ───────────────────

#[wasm_bindgen_test]
fn renders_blockquote() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), blockquote_doc("Quote text"));

    let html = inner_html(&view);
    assert!(
        html.contains("<blockquote>") && html.contains("Quote text"),
        "Expected blockquote, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_ordered_list() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), ordered_list_doc("Item"));

    let html = inner_html(&view);
    assert!(
        html.contains("<ol>") && html.contains("<li>") && html.contains("Item"),
        "Expected ordered list, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_task_list_unchecked() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), task_list_doc("Task", false));

    let html = inner_html(&view);
    assert!(html.contains("data-type=\"taskList\""), "Expected taskList, got: {html}");
    assert!(html.contains("data-checked=\"false\""), "Expected unchecked, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_task_list_checked() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), task_list_doc("Done", true));

    let html = inner_html(&view);
    assert!(html.contains("data-checked=\"true\""), "Expected checked, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_code_block() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), code_block_doc("fn main()", "rust"));

    let html = inner_html(&view);
    assert!(html.contains("<pre>"), "Expected <pre>, got: {html}");
    assert!(html.contains("<code"), "Expected <code>, got: {html}");
    assert!(html.contains("language-rust"), "Expected language class, got: {html}");
    assert!(html.contains("fn main()"), "Expected code content, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_hard_break() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), hard_break_doc());

    let html = inner_html(&view);
    assert!(html.contains("before"), "Expected 'before', got: {html}");
    assert!(html.contains("after"), "Expected 'after', got: {html}");
    assert!(html.contains("<br>") || html.contains("<br/>"), "Expected <br>, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn renders_italic_text() {
    let container = create_container();
    let (view, _txns) = create_editor(container.clone(), italic_doc());

    let html = inner_html(&view);
    assert!(html.contains("<em>styled</em>"), "Expected italic, got: {html}");

    cleanup(&container);
}

// ─── Tier 2B: Delete Forward ──────────────────────────────────

#[wasm_bindgen_test]
fn delete_forward_mid_text() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Helo world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn delete_forward_with_selection() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), " world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn delete_forward_at_end_is_noop() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor at end of content (position 12)
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let collected = txns.borrow();
    // Either no transaction, or transaction that doesn't change doc
    if !collected.is_empty() {
        let state = view.state();
        let new_state = state.apply(collected[0].clone());
        assert_eq!(new_state.doc.child(0).unwrap().text_content(), "Hello world");
    }

    cleanup(&container);
}

// ─── Tier 2C: Select All ──────────────────────────────────────

#[wasm_bindgen_test]
fn ctrl_a_selects_all() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "a", true, false, false);

    let state = apply_all(&view, &txns);
    let content_size = state.doc.content_size();
    assert_eq!(state.selection.from(), 0);
    assert_eq!(state.selection.to(), content_size);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_a_selects_all_multi_block() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "a", true, false, false);

    let state = apply_all(&view, &txns);
    let content_size = state.doc.content_size();
    assert_eq!(state.selection.from(), 0);
    assert_eq!(state.selection.to(), content_size);

    cleanup(&container);
}

// ─── Tier 2D: Mark Edge Cases ─────────────────────────────────

#[wasm_bindgen_test]
fn ctrl_shift_s_toggles_strike() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "s", true, true, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Strike),
        "Should apply strike mark"
    );
    let html = inner_html(&view);
    assert!(html.contains("<s>Hello</s>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_b_cursor_sets_stored_marks_then_types_bold() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Place cursor (no selection) and toggle bold
    set_cursor(&view, 6);
    dispatch_keydown(view.container(), "b", true, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());

    assert!(
        state.stored_marks.as_ref().map_or(false, |m| m.iter().any(|m| m.mark_type == MarkType::Bold)),
        "Stored marks should contain Bold"
    );

    // Now type a character — it should be bold
    dispatch_before_input(view.container(), "insertText", Some("X"));
    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let mut found_bold_x = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            found_bold_x = child.marks().iter().any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(found_bold_x, "Typed 'X' should have bold mark");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn bold_in_code_block_is_noop() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), code_block_doc("code text", ""));

    set_selection(&view, 1, 5);
    dispatch_keydown(view.container(), "b", true, false, false);

    let collected = txns.borrow();
    assert!(collected.is_empty(), "Bold should not apply inside code block");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn multiple_marks_combine() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "b", true, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "i", true, false, false);
    let state = apply_all(&view, &txns);

    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    let marks = first.marks();
    assert!(marks.iter().any(|m| m.mark_type == MarkType::Bold), "Should have Bold");
    assert!(marks.iter().any(|m| m.mark_type == MarkType::Italic), "Should have Italic");

    cleanup(&container);
}

// ─── Tier 2E: Heading Behavior ────────────────────────────────

#[wasm_bindgen_test]
fn enter_in_heading_creates_paragraph() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), heading_doc(1, "Title"));

    // Cursor at end of "Title" — heading(1+5+1=7), content_start=1, end=6
    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2);
    assert_eq!(state.doc.child(0).unwrap().node_type(), Some(NodeType::Heading));
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Title");
    assert_eq!(state.doc.child(1).unwrap().node_type(), Some(NodeType::Paragraph));
    assert_eq!(state.doc.child(1).unwrap().text_content(), "");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_alt_2_on_h1_changes_level() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), heading_doc(1, "Title"));

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "2", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    assert_eq!(block.attrs().get("level").unwrap(), "2");
    let html = inner_html(&view);
    assert!(html.contains("<h2>"), "DOM: {html}");

    cleanup(&container);
}

// ─── Tier 3A: Blockquote Behavior ─────────────────────────────

#[wasm_bindgen_test]
fn typing_inside_blockquote() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), blockquote_doc("Hello"));

    // Blockquote(0) > Paragraph(1) > content_start=2, "Hello"=2..7
    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let bq = state.doc.child(0).unwrap();
    assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
    assert_eq!(bq.text_content(), "HeXllo");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_in_blockquote_splits_paragraph() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), blockquote_doc("Hello world"));

    // Cursor after "Hello" — Blockquote(0) > Para(1) > content=2, pos 7 = after "Hello"
    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    let bq = state.doc.child(0).unwrap();
    assert_eq!(bq.node_type(), Some(NodeType::Blockquote));
    assert_eq!(bq.child_count(), 2, "Blockquote should have 2 paragraphs");
    assert_eq!(bq.child(0).unwrap().text_content(), "Hello");
    assert_eq!(bq.child(1).unwrap().text_content(), " world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_joins_paragraphs_in_blockquote() {
    // Build blockquote with two paragraphs
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Blockquote,
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
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Cursor at start of second paragraph inside blockquote
    // Blockquote(0) > Para1(1..8) > Para2(8) content_start=9
    set_cursor(&view, 9);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    let bq = state.doc.child(0).unwrap();
    assert_eq!(bq.child_count(), 1, "Should join into one paragraph");
    assert_eq!(bq.child(0).unwrap().text_content(), "HelloWorld");

    cleanup(&container);
}

// ─── Tier 3B: Ordered List Behavior ───────────────────────────

#[wasm_bindgen_test]
fn enter_in_ordered_list_creates_new_item() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), ordered_list_doc("Item text"));

    // OrderedList(0) > ListItem(1) > Para(2) > content=3, "Item text"=3..12
    set_cursor(&view, 7); // after "Item"
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::OrderedList));
    assert_eq!(list.child_count(), 2, "Should have 2 list items");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn tab_indent_in_ordered_list() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::OrderedList,
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
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 12);
    dispatch_keydown(view.container(), "Tab", false, false, false);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.child_count(), 1, "Outer list should have 1 item after indent");
    let item = list.child(0).unwrap();
    assert_eq!(item.child_count(), 2, "Item should have paragraph + sub-list");
    let sub_list = item.child(1).unwrap();
    assert_eq!(sub_list.node_type(), Some(NodeType::OrderedList));

    cleanup(&container);
}

// ─── Tier 3C: Task List Behavior ──────────────────────────────

#[wasm_bindgen_test]
fn task_list_checked_input_rule() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("[x]")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 4);
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::TaskList));
    let html = inner_html(&view);
    assert!(html.contains("data-checked=\"true\""), "Expected checked task, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_in_task_list_creates_new_task_item() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), task_list_doc("Task", false));

    // TaskList(0) > TaskItem(1) > Para(2) > content=3, "Task"=3..7
    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::TaskList));
    assert_eq!(list.child_count(), 2, "Should have 2 task items");
    assert_eq!(list.child(0).unwrap().node_type(), Some(NodeType::TaskItem));
    assert_eq!(list.child(1).unwrap().node_type(), Some(NodeType::TaskItem));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn tab_indent_in_task_list() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::TaskList,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::TaskItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("first")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::TaskItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("second")]),
                    )]),
                ),
            ]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 12);
    dispatch_keydown(view.container(), "Tab", false, false, false);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.child_count(), 1, "Outer task list should have 1 item after indent");
    let item = list.child(0).unwrap();
    let sub_list = item.child(1).unwrap();
    assert_eq!(sub_list.node_type(), Some(NodeType::TaskList));

    cleanup(&container);
}

// ─── Tier 3D: Multi-Block and Edge Cases ──────────────────────

#[wasm_bindgen_test]
fn heading_preserves_marks() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 3);
    dispatch_keydown(view.container(), "1", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Heading));
    let first = block.child(0).unwrap();
    assert!(
        first.marks().iter().any(|m| m.mark_type == MarkType::Bold),
        "Bold mark should be preserved after heading conversion"
    );
    let html = inner_html(&view);
    assert!(html.contains("<h1>") && html.contains("<strong>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_doc_start_is_noop() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn typing_in_empty_doc() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), Node::empty_doc());

    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "X");
    let html = inner_html(&view);
    assert!(html.contains("<p>X</p>"), "DOM: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn input_rule_mid_line_no_fire() {
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("hello #")]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    set_cursor(&view, 8); // after "#"
    dispatch_before_input(view.container(), "insertText", Some(" "));

    let state = apply_all(&view, &txns);
    // Should remain a paragraph, not become a heading
    assert_eq!(
        state.doc.child(0).unwrap().node_type(),
        Some(NodeType::Paragraph),
        "# mid-line should not trigger heading"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn delete_across_three_blocks() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), three_para_doc());

    // Para1(0..7) "Alpha", Para2(7..13) "Beta", Para3(13..20) "Gamma"
    // Select from middle of Alpha to middle of Gamma: pos 3 to pos 16
    set_selection(&view, 3, 16);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1, "Should merge into one paragraph");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Almma");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn nested_list_three_levels() {
    let container = create_container();
    // Start with 3 flat bullet list items
    // Positions: BulletList(0) >
    //   ListItem1(1) > Para(2) > "L1"(3,4) > close(5) > close(6)
    //   ListItem2(7) > Para(8) > "L2"(9,10) > close(11) > close(12)
    //   ListItem3(13) > Para(14) > "L3"(15,16) > close(17) > close(18)
    // BulletList close(19)
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("L1")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("L2")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("L3")]),
                    )]),
                ),
            ]),
        )]),
    );
    let (view, txns) = create_editor(container.clone(), doc);

    // Step 1: Indent L2 under L1 (cursor inside "L2" at pos 9)
    set_cursor(&view, 9);
    dispatch_keydown(view.container(), "Tab", false, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);

    // Step 2: Indent L3 under L1 (makes it sibling of L2 in sub-list)
    // After step 1: ListItem1(1) > [Para("L1")(2..5), BulletList(6) > ListItem(7) > Para("L2")(8..11)]
    // ListItem1 close at 14. ListItem3(15) > Para(16) > "L3"(17,18)
    set_cursor(&view, 17);
    dispatch_keydown(view.container(), "Tab", false, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);

    // Step 3: Indent L3 under L2 (now L3 is in sub-list, nest it deeper)
    // After step 2: sub-list has [ListItem(L2), ListItem(L3)]
    // ListItem1(1) > [Para("L1")(2..5), BulletList(6) > ListItem(7..12)(L2), ListItem(13..18)(L3)]
    // L3 content inside sub-list: pos 15 (inside "L3")
    set_cursor(&view, 15);
    dispatch_keydown(view.container(), "Tab", false, false, false);
    let state = apply_all(&view, &txns);

    let html = inner_html(&view);
    let ul_count = html.matches("<ul>").count();
    assert!(ul_count >= 3, "Should have 3 nested <ul> levels, got {ul_count}: {html}");

    cleanup(&container);
}

// ─── Mark Boundary Tests ──────────────────────────────────────
//
// bold_doc() = Paragraph > [bold("Hello"), plain(" world")]
// Positions: Para open(0), content_start(1), "Hello"(1..6), " world"(6..12), close(12)
// Position 6 is the EXACT boundary: marks_at_pos inherits Bold from the left.
// Position 7 is one char into " world": no marks.

#[wasm_bindgen_test]
fn type_at_exact_mark_boundary_inherits_left() {
    // Cursor at position 6 — exact boundary between bold "Hello" and plain " world"
    // Should inherit Bold from the left side.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    // Find the "X" and verify it has Bold
    let mut found_bold_x = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            found_bold_x = child.marks().iter().any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(found_bold_x, "Typing at exact mark boundary should inherit bold from the left");

    // DOM should show "X" inside the <strong> tag
    let html = inner_html(&view);
    assert!(
        html.contains("<strong>HelloX</strong>"),
        "X should be inside <strong>, got: {html}"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn type_one_past_mark_boundary_no_inherit() {
    // Cursor at position 7 — one char into plain " world", past the bold boundary
    // Should NOT inherit Bold.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let mut x_is_bold = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            x_is_bold = child.marks().iter().any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(!x_is_bold, "Typing past mark boundary should NOT inherit bold");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn type_at_start_of_marked_range() {
    // Cursor at position 1 — start of bold "Hello" in bold_doc
    // Position 1 is inside the bold text, should inherit Bold.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert!(
        first.marks().iter().any(|m| m.mark_type == MarkType::Bold),
        "Typing at start of marked range should inherit bold"
    );
    assert!(first.text_content().starts_with('X'), "X should be at the start");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_mark_boundary_removes_marked_char() {
    // Cursor at position 6 (exact boundary). Backspace should delete the last bold char ('o')
    // and leave "Hell" bold + " world" plain.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    // First child should be bold "Hell"
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "Hell");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    // Rest should be plain " world"
    let second = para.child(1).unwrap();
    assert_eq!(second.text_content(), " world");
    assert!(!second.marks().iter().any(|m| m.mark_type == MarkType::Bold));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_into_marked_range_from_plain() {
    // Cursor at position 7 (first char of plain " world"). Backspace deletes that space char
    // from the plain range, not from the bold range.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    // Bold "Hello" should remain intact
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "Hello");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    // Plain text should be "world" (space removed)
    let second = para.child(1).unwrap();
    assert_eq!(second.text_content(), "world");
    assert!(!second.marks().iter().any(|m| m.mark_type == MarkType::Bold));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn delete_forward_at_mark_boundary() {
    // Cursor at position 6 (exact boundary). Delete forward should remove the space
    // (first char of plain " world"), leaving bold "Hello" + plain "world".
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "Hello");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    let second = para.child(1).unwrap();
    assert_eq!(second.text_content(), "world");
    assert!(!second.marks().iter().any(|m| m.mark_type == MarkType::Bold));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn selection_across_mark_boundary() {
    // Select from position 4 (inside bold "Hello") to position 8 (inside plain " world")
    // This crosses the mark boundary. Applying bold should extend bold to cover the selection.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_selection(&view, 4, 8);
    dispatch_keydown(view.container(), "b", true, false, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    // The selected range "lo w" should now ALL be bold
    // Full text is "Hello world", positions 4-8 = "lo w"
    // Since "Hel" was already bold, and "lo w" is now toggled,
    // the toggle should REMOVE bold from the overlap ("lo") and ADD to " w"?
    // Actually, toggle_mark checks if the entire range has the mark.
    // "lo" is bold, " w" is not. So the range is partially bold.
    // toggle_mark adds bold to the unbolded portion.
    // Result: "Hello w" all bold, "orld" plain
    let mut found_w_bold = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('w') {
            found_w_bold = child.marks().iter().any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(found_w_bold, "Bold should extend across the mark boundary to cover 'w'");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn toggle_bold_off_then_type_at_boundary() {
    // At the mark boundary, toggle bold off (stored marks), then type.
    // The typed char should NOT be bold despite being at the boundary.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bold_doc());

    set_cursor(&view, 6); // exact boundary
    // Toggle bold off — sets stored marks WITHOUT bold
    dispatch_keydown(view.container(), "b", true, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);

    // Now type "X" — should use stored marks (no bold)
    dispatch_before_input(view.container(), "insertText", Some("X"));
    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    let mut x_is_bold = false;
    for i in 0..para.child_count() {
        let child = para.child(i).unwrap();
        if child.text_content().contains('X') {
            x_is_bold = child.marks().iter().any(|m| m.mark_type == MarkType::Bold);
            break;
        }
    }
    assert!(!x_is_bold, "After toggling bold off, typed char should NOT be bold");

    cleanup(&container);
}

// ─── Block Boundary Tests ─────────────────────────────────────
//
// two_para_doc() = [Paragraph("Hello"), Paragraph("World")]
// Para1: offset 0, size 7, content 1..6
// Para2: offset 7, size 7, content 8..13

#[wasm_bindgen_test]
fn delete_forward_at_end_of_block_joins_with_next() {
    // Cursor at position 6 (end of "Hello" content in para1).
    // Delete forward should join para1 and para2 into "HelloWorld".
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1, "Should join into one paragraph");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "HelloWorld");
    // Cursor should stay at position 6 (where it was)
    assert_eq!(state.selection.from(), 6);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_block_start_preserves_cursor_position() {
    // Cursor at position 8 (start of "World" content in para2).
    // After joining, cursor should be where the join happened (after "Hello").
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "HelloWorld");
    // Cursor should be at the join point (after "Hello", position 6)
    assert_eq!(state.selection.from(), 6, "Cursor should be at join point");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn type_at_start_of_second_paragraph() {
    // Cursor at position 8 (start of para2 content "World").
    // Typing should insert at the beginning of "World".
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_cursor(&view, 8);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2, "Should still be two paragraphs");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello");
    assert_eq!(state.doc.child(1).unwrap().text_content(), "XWorld");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn type_at_end_of_first_paragraph() {
    // Cursor at position 6 (end of para1 content "Hello").
    // Typing should append to the end of "Hello".
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertText", Some("X"));

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2, "Should still be two paragraphs");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "HelloX");
    assert_eq!(state.doc.child(1).unwrap().text_content(), "World");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_at_start_of_paragraph_creates_empty_before() {
    // Cursor at position 1 (start of simple_doc "Hello world" content).
    // Enter should create an empty paragraph before "Hello world".
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 2);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "");
    assert_eq!(state.doc.child(1).unwrap().text_content(), "Hello world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn selection_spanning_block_boundary_delete_merges() {
    // Select from position 4 in para1 to position 10 in para2 of two_para_doc
    // Deleting should merge remaining text "Hel" + "rld" into "Helrld"
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_para_doc());

    set_selection(&view, 4, 10);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child_count(), 1, "Should merge into one paragraph");
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Helrld");
    // Cursor should be at the deletion point
    assert_eq!(state.selection.from(), 4);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn delete_forward_at_end_of_last_block_is_noop() {
    // Cursor at the very end of the last paragraph. Delete forward should do nothing.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // "Hello world" = 11 chars, para content 1..12. End of content = 12.
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "deleteContentForward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_start_of_first_block_is_noop() {
    // Cursor at position 1 (start of first paragraph content). Backspace should do nothing
    // since there's no previous block to join with.
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello world");
    assert_eq!(state.doc.child_count(), 1);

    cleanup(&container);
}

// ─── Event Consumption / Keystroke Gap Tests ──────────────────

#[wasm_bindgen_test]
fn shift_enter_inserts_hard_break() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor after "Hello" (position 6)
    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "insertLineBreak", None);

    let state = apply_all(&view, &txns);
    // Paragraph should still be one block, but contain a hard break
    assert_eq!(state.doc.child_count(), 1, "Should remain one paragraph");
    let para = state.doc.child(0).unwrap();
    // Should have: "Hello" + HardBreak + " world"
    assert!(para.child_count() >= 3, "Should have text + br + text, got {} children", para.child_count());
    let html = inner_html(&view);
    assert!(html.contains("<br>") || html.contains("<br/>"), "DOM should contain <br>, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_backspace_deletes_word() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor after "Hello" (position 6), Ctrl+Backspace should delete "Hello"
    set_cursor(&view, 6);
    dispatch_before_input(view.container(), "deleteWordBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), " world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_backspace_deletes_word_mid_text() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor at position 12 (end of " world"), Ctrl+Backspace should delete "world"
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "deleteWordBackward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "Hello ");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_delete_deletes_word_forward() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Cursor at position 1 (start of "Hello world"), Ctrl+Delete should delete "Hello "
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "deleteWordForward", None);

    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().text_content(), "world");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_shift_x_toggles_strike() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "x", true, true, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Strike),
        "Ctrl+Shift+X should toggle strikethrough"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_shift_k_toggles_code() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_selection(&view, 1, 6);
    dispatch_keydown(view.container(), "k", true, true, false);

    let state = apply_all(&view, &txns);
    let para = state.doc.child(0).unwrap();
    assert!(
        para.child(0).unwrap().marks().iter().any(|m| m.mark_type == MarkType::Code),
        "Ctrl+Shift+K should toggle code mark"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_alt_0_converts_heading_to_paragraph() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), heading_doc(1, "Title"));

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "0", true, false, true);

    let state = apply_all(&view, &txns);
    let block = state.doc.child(0).unwrap();
    assert_eq!(block.node_type(), Some(NodeType::Paragraph));
    assert_eq!(block.text_content(), "Title");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_shift_l_toggles_bullet_list() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    set_cursor(&view, 1);
    dispatch_keydown(view.container(), "l", true, true, false);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_on_third_list_item_joins_with_second() {
    // Replicate user's exact scenario: Heading + 3 bullet items
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            {
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), "1".to_string());
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs,
                    Fragment::from(vec![Node::text("Test")]),
                )
            },
            Node::element_with_content(
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
            ),
        ]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Heading("Test"): offset 0, size 6 (1+4+1)
    // BulletList: offset 6
    //   ListItem1(7, size=9) > Para(8) > "asdf1"(9..14)
    //   ListItem2(16, size=9) > Para(17) > "asdf2"(18..23)
    //   ListItem3(25, size=9) > Para(26) > "asdf3"(27..32)
    // Cursor at start of "asdf3" = position 27
    set_cursor(&view, 27);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);

    // Should have: Heading + BulletList with 2 items (asdf2 merged with asdf3)
    assert_eq!(state.doc.child_count(), 2, "Should still have heading + list");
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should have 2 items after join");
    assert_eq!(list.child(0).unwrap().text_content(), "asdf1");
    assert_eq!(list.child(1).unwrap().text_content(), "asdf2asdf3");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn select_across_list_items_and_delete() {
    // Select from middle of first bullet to middle of second and delete.
    // Should merge remaining text into one item.
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            {
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), "1".to_string());
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs,
                    Fragment::from(vec![Node::text("Test")]),
                )
            },
            Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("asdf")]),
                        )]),
                    ),
                ]),
            ),
        ]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Heading(0..6), BulletList(6)
    //   ListItem1(7) > Para(8) > "asdf"(9..12) > close(13) > close(14)  size=8
    //   ListItem2(15) > Para(16) > "asdf"(17..20) > close(21) > close(22) size=8
    //   ListItem3(23) > Para(24) > "asdf"(25..28) > close(29) > close(30) size=8
    // Select from middle of item1 "asdf" to middle of item2 "asdf"
    // pos 11 = after "as" in item1, pos 19 = after "as" in item2
    set_selection(&view, 11, 19);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);

    // Should have heading + list with 2 items
    assert_eq!(state.doc.child_count(), 2);
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should merge items 1+2 into one, keep item3");
    // First item: "as" + "df" = "asdf" (before + after selection)
    assert_eq!(list.child(0).unwrap().text_content(), "asdf");
    // Third item unchanged
    assert_eq!(list.child(1).unwrap().text_content(), "asdf");

    // Cursor at the merge point
    assert_eq!(state.selection.from(), 11);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn select_across_all_list_items_and_delete() {
    // Select from start of first item text to end of last item text and delete.
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("one")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("two")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("three")]),
                    )]),
                ),
            ]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // BulletList(0)
    //   ListItem1(1, size=7) > Para(2) > "one"(3..6)
    //   ListItem2(8, size=7) > Para(9) > "two"(10..13)
    //   ListItem3(15, size=9) > Para(16) > "three"(17..22)
    // Select from start of "one"(3) to end of "three"(22)
    set_selection(&view, 3, 22);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);

    // Should still have a list with one empty item
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 1);
    assert_eq!(list.child(0).unwrap().text_content(), "");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_mid_text_in_list_item_deletes_char() {
    // Exact user scenario: "# Test" then "* asdf1" then "asdf2", then backspace at end
    // This tests that mid-text backspace works inside a list item
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            {
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), "1".to_string());
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs,
                    Fragment::from(vec![Node::text("Test")]),
                )
            },
            Node::element_with_content(
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
                ]),
            ),
        ]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Heading("Test"): offset 0, size 6
    // BulletList(6): ListItem1(7, size=9), ListItem2(16, size=9)
    // ListItem2 > Para(17) > "asdf2"(18..23), close at 23
    // Cursor at end of "asdf2" = position 23
    set_cursor(&view, 23);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    // "asdf2" should become "asdf" (last char deleted)
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.child(1).unwrap().text_content(), "asdf");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_mid_text_in_list_item_multiple_times() {
    // Verify multiple backspaces work in a list item
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc("hello"));

    // BulletList(0) > ListItem(1) > Para(2) > "hello"(3..8)
    // Cursor at end of "hello" = position 8
    set_cursor(&view, 8);

    // First backspace: "hello" -> "hell"
    dispatch_before_input(view.container(), "deleteContentBackward", None);
    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().child(0).unwrap().text_content(), "hell");
    txns.borrow_mut().clear();
    view.update_state(state);

    // Second backspace: "hell" -> "hel"
    set_cursor(&view, 7);
    dispatch_before_input(view.container(), "deleteContentBackward", None);
    let state = apply_all(&view, &txns);
    assert_eq!(state.doc.child(0).unwrap().child(0).unwrap().text_content(), "hel");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_empty_second_bullet_merges_and_cursor_at_end() {
    // User scenario: "# Test", "* asdf1", "asdf2", backspace all of "asdf2",
    // then one more backspace to remove the empty bullet.
    // Cursor should end up at the end of "asdf1".
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            {
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), "1".to_string());
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs,
                    Fragment::from(vec![Node::text("Test")]),
                )
            },
            Node::element_with_content(
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
                        Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                    ),
                ]),
            ),
        ]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Heading(0, size=6), BulletList(6)
    //   ListItem1(7, size=9) > Para(8) > "asdf1"(9..14)
    //   ListItem2(16, size=4) > Para(17, size=2, empty)
    // Cursor in empty paragraph: position 18
    set_cursor(&view, 18);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);

    // Should have: Heading + BulletList with 1 item
    assert_eq!(state.doc.child_count(), 2);
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 1, "Empty item should be merged away");
    assert_eq!(list.child(0).unwrap().text_content(), "asdf1");

    // Cursor should be at end of "asdf1" = position 14
    // Heading(0..6) + BulletList(6) > ListItem(7) > Para(8) > "asdf1"(9..14)
    assert_eq!(
        state.selection.from(), 14,
        "Cursor should be at end of 'asdf1', got {}",
        state.selection.from()
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn full_flow_type_bullets_delete_all_then_backspace_bullet() {
    // Simulate the exact user flow:
    // 1. Start with heading + one bullet item "asdf1"
    // 2. Press Enter to create second item
    // 3. Type "asdf2"
    // 4. Backspace 5 times to delete "asdf2"
    // 5. Backspace once more to remove empty bullet
    // Cursor should be at end of "asdf1"
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![
            {
                let mut attrs = HashMap::new();
                attrs.insert("level".to_string(), "1".to_string());
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs,
                    Fragment::from(vec![Node::text("Test")]),
                )
            },
            Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("asdf1")]),
                    )]),
                )]),
            ),
        ]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // Step 2: Press Enter at end of "asdf1" to create second item
    // Heading(0..6), BulletList(6), ListItem(7), Para(8), "asdf1"(9..14)
    set_cursor(&view, 14);
    dispatch_before_input(view.container(), "insertParagraph", None);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());

    // Verify we now have 2 items and cursor is in the empty second item
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.child_count(), 2, "Should have 2 items after Enter");

    // Step 3: Type "asdf2"
    dispatch_before_input(view.container(), "insertText", Some("asdf2"));
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());

    let list = state.doc.child(1).unwrap();
    assert_eq!(list.child(1).unwrap().text_content(), "asdf2");

    // Step 4: Backspace 5 times to delete "asdf2"
    let mut state = state;
    for _ in 0..5 {
        let cursor_pos = state.selection.from();
        set_cursor(&view, cursor_pos);
        dispatch_before_input(view.container(), "deleteContentBackward", None);
        state = apply_all(&view, &txns);
        txns.borrow_mut().clear();
        view.update_state(state.clone());
    }

    let list = state.doc.child(1).unwrap();
    assert_eq!(list.child_count(), 2, "Should still have 2 items (one empty)");
    assert_eq!(list.child(1).unwrap().text_content(), "");

    // Step 5: One more backspace to remove the empty bullet
    let cursor_pos = state.selection.from();
    set_cursor(&view, cursor_pos);
    dispatch_before_input(view.container(), "deleteContentBackward", None);
    let state = apply_all(&view, &txns);

    // Verify final state: heading + list with 1 item
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.child_count(), 1, "Empty item should be removed");
    assert_eq!(list.child(0).unwrap().text_content(), "asdf1");

    // Cursor should be at end of "asdf1"
    let expected_cursor = {
        // Heading(0..6) + BulletList(6) > ListItem(7) > Para(8) > "asdf1"(9..14)
        14
    };
    assert_eq!(
        state.selection.from(), expected_cursor,
        "Cursor should be at end of 'asdf1' (pos {}), got {}",
        expected_cursor, state.selection.from()
    );

    // Verify typing at this position appends to the text
    txns.borrow_mut().clear();
    dispatch_before_input(view.container(), "insertText", Some("X"));
    let state = apply_all(&view, &txns);
    let list = state.doc.child(1).unwrap();
    assert_eq!(
        list.child(0).unwrap().text_content(), "asdf1X",
        "Typing at cursor should append, not insert mid-text"
    );

    cleanup(&container);
}

// ─── Undo/Redo Tests ──────────────────────────────────────────

#[wasm_bindgen_test]
fn ctrl_z_undoes_text_insertion() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Type "X" at start
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertText", Some("X"));
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());
    assert_eq!(state.doc.child(0).unwrap().text_content(), "XHello world");

    // Ctrl+Z to undo — dispatches undo txn which is collected in txns
    dispatch_keydown(view.container(), "z", true, false, false);
    let state = apply_all(&view, &txns);
    assert_eq!(
        state.doc.child(0).unwrap().text_content(),
        "Hello world",
        "Undo should restore original text"
    );

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_shift_z_redoes_after_undo() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), simple_doc());

    // Type "X"
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertText", Some("X"));
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);
    assert_eq!(view.state().doc.child(0).unwrap().text_content(), "XHello world");

    // Undo
    dispatch_keydown(view.container(), "z", true, false, false);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state);
    assert_eq!(view.state().doc.child(0).unwrap().text_content(), "Hello world");

    // Redo
    dispatch_keydown(view.container(), "z", true, true, false);
    let state = apply_all(&view, &txns);
    assert_eq!(
        state.doc.child(0).unwrap().text_content(),
        "XHello world",
        "Redo should restore the typed text"
    );

    cleanup(&container);
}

// ─── Clipboard / HTML Parsing Tests ───────────────────────────

use ogrenotes_frontend::editor::clipboard;

#[wasm_bindgen_test]
fn parse_html_bold_text() {
    let slice = clipboard::parse_from_html("<p><strong>hello</strong></p>");
    assert!(!slice.content.children.is_empty());
    // Should contain a paragraph with bold "hello"
    let para = &slice.content.children[0];
    assert_eq!(para.node_type(), Some(NodeType::Paragraph));
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "hello");
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
}

#[wasm_bindgen_test]
fn parse_html_strips_scripts() {
    let slice = clipboard::parse_from_html("<p>safe</p><script>alert('xss')</script><p>also safe</p>");
    // Should only get the safe paragraphs, no script content
    let all_text: String = slice.content.children.iter().map(|c| c.text_content()).collect();
    assert!(all_text.contains("safe"), "Should contain safe text");
    assert!(!all_text.contains("alert"), "Should NOT contain script content");
}

#[wasm_bindgen_test]
fn parse_html_unknown_elements() {
    let slice = clipboard::parse_from_html("<custom-element>just text</custom-element>");
    let all_text: String = slice.content.children.iter().map(|c| c.text_content()).collect();
    assert!(all_text.contains("just text"), "Should extract text from unknown elements");
}

#[wasm_bindgen_test]
fn parse_html_nested_marks() {
    let slice = clipboard::parse_from_html("<p><em><strong>both</strong></em></p>");
    let para = &slice.content.children[0];
    let first = para.child(0).unwrap();
    assert_eq!(first.text_content(), "both");
    let marks = first.marks();
    assert!(marks.iter().any(|m| m.mark_type == MarkType::Bold), "Should have bold");
    assert!(marks.iter().any(|m| m.mark_type == MarkType::Italic), "Should have italic");
}

#[wasm_bindgen_test]
fn parse_html_list() {
    let slice = clipboard::parse_from_html("<ul><li>item one</li><li>item two</li></ul>");
    assert_eq!(slice.content.children.len(), 1);
    let list = &slice.content.children[0];
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2);
    assert_eq!(list.child(0).unwrap().text_content(), "item one");
    assert_eq!(list.child(1).unwrap().text_content(), "item two");
}

#[wasm_bindgen_test]
fn parse_html_heading() {
    let slice = clipboard::parse_from_html("<h2>Subtitle</h2>");
    let heading = &slice.content.children[0];
    assert_eq!(heading.node_type(), Some(NodeType::Heading));
    assert_eq!(heading.attrs().get("level").unwrap(), "2");
    assert_eq!(heading.text_content(), "Subtitle");
}

#[wasm_bindgen_test]
fn context_fit_heading_in_list_item() {
    let slice = clipboard::parse_from_html("<h1>Title</h1>");
    let fitted = clipboard::fit_slice_to_context(slice, NodeType::ListItem);
    // Heading should be downgraded to Paragraph inside ListItem
    let node = &fitted.content.children[0];
    assert_eq!(node.node_type(), Some(NodeType::Paragraph));
    assert_eq!(node.text_content(), "Title");
}

#[wasm_bindgen_test]
fn parse_text_multiline_creates_paragraphs() {
    let slice = clipboard::parse_from_text("line one\nline two\nline three");
    assert_eq!(slice.content.children.len(), 3);
    assert_eq!(slice.content.children[0].text_content(), "line one");
    assert_eq!(slice.content.children[1].text_content(), "line two");
    assert_eq!(slice.content.children[2].text_content(), "line three");
}

#[wasm_bindgen_test]
fn serialize_and_parse_roundtrip() {
    // Create a doc with bold text, serialize to HTML, parse back
    let doc = bold_doc();
    let slice = doc.slice(0, doc.content_size());
    let html = clipboard::serialize_to_html(&slice);
    let parsed = clipboard::parse_from_html(&html);

    // Should get back a paragraph with bold "Hello" and plain " world"
    assert!(!parsed.content.children.is_empty());
    let para = &parsed.content.children[0];
    assert_eq!(para.node_type(), Some(NodeType::Paragraph));
    let first = para.child(0).unwrap();
    assert!(first.marks().iter().any(|m| m.mark_type == MarkType::Bold));
    assert_eq!(first.text_content(), "Hello");
}

#[wasm_bindgen_test]
fn parse_html_preserves_nested_list_structure() {
    // Simulate copying indented bullet points:
    // • item1
    //   ○ sub-item
    // • item2
    let html = "<ul><li><p>item1</p><ul><li><p>sub-item</p></li></ul></li><li><p>item2</p></li></ul>";
    let slice = clipboard::parse_from_html(html);

    // Top-level should be a BulletList
    assert_eq!(slice.content.children.len(), 1);
    let list = &slice.content.children[0];
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should have 2 top-level items");

    // First item should have paragraph + nested sub-list
    let item1 = list.child(0).unwrap();
    assert_eq!(item1.child_count(), 2, "First item should have para + sub-list");
    assert_eq!(item1.child(0).unwrap().text_content(), "item1");
    let sub_list = item1.child(1).unwrap();
    assert_eq!(sub_list.node_type(), Some(NodeType::BulletList));
    assert_eq!(sub_list.child(0).unwrap().text_content(), "sub-item");

    // Second item is flat
    let item2 = list.child(1).unwrap();
    assert_eq!(item2.text_content(), "item2");
}

#[wasm_bindgen_test]
fn fit_list_content_in_paragraph_extracts_text() {
    // Pasting a BulletList into a Paragraph context should extract text
    let slice = clipboard::parse_from_html("<ul><li><p>item1</p></li><li><p>item2</p></li></ul>");
    let fitted = clipboard::fit_slice_to_context(slice, NodeType::Paragraph);

    // Block content should be downgraded — no BulletList in the result
    for child in &fitted.content.children {
        assert!(
            !matches!(child.node_type(), Some(NodeType::BulletList)),
            "BulletList should not appear inside Paragraph context"
        );
    }
}

// ─── Backspace in List Item Tests ─────────────────────────────

#[wasm_bindgen_test]
fn backspace_at_start_of_sole_list_item_unwraps_to_paragraph() {
    // Doc > BulletList > ListItem > Paragraph("text")
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc("text"));

    // BulletList(0) > ListItem(1) > Para(2) > content_start=3
    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    // Should unwrap to a plain paragraph, no list
    assert_eq!(state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    assert_eq!(state.doc.child(0).unwrap().text_content(), "text");
    assert_eq!(state.doc.child_count(), 1);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_start_of_first_list_item_keeps_remaining() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // BulletList(0) > ListItem1(1) > Para(2) > "first"(3..8)
    // Cursor at start of "first"
    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    // First item should become a paragraph, remaining stay in the list
    assert_eq!(state.doc.child_count(), 2);
    assert_eq!(state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    assert_eq!(state.doc.child(0).unwrap().text_content(), "first");
    let list = state.doc.child(1).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child(0).unwrap().text_content(), "second");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_at_start_of_second_list_item_joins_with_first() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), two_item_bullet_list_doc());

    // ListItem1(1) size=9, ListItem2(10) > Para(11) > content_start=12
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 1, "Should merge into one item");
    assert_eq!(list.child(0).unwrap().text_content(), "firstsecond");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_on_nested_list_item_dedents() {
    // Build nested: BulletList > ListItem > [Para("first"), BulletList > ListItem > Para("nested")]
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
                                Fragment::from(vec![Node::text("nested")]),
                            )]),
                        )]),
                    ),
                ]),
            )]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), nested_doc);

    // Inner ListItem > Para > content_start at position 12
    // BulletList(0) > ListItem(1) > Para("first")(2..8) > InnerBulletList(9) > ListItem(10) > Para(11) > "nested"(12..)
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    // "nested" should be dedented to the outer list as a sibling of "first"'s item
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 2, "Should have 2 items after dedent");
    assert_eq!(list.child(0).unwrap().text_content(), "first");
    assert_eq!(list.child(1).unwrap().text_content(), "nested");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn backspace_on_empty_sole_list_item_unwraps() {
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), bullet_list_doc(""));

    // BulletList(0) > ListItem(1) > Para(2) > content_start=3 (empty)
    set_cursor(&view, 3);
    dispatch_before_input(view.container(), "deleteContentBackward", None);

    let state = apply_all(&view, &txns);
    // Should become an empty paragraph, no list
    assert_eq!(state.doc.child(0).unwrap().node_type(), Some(NodeType::Paragraph));
    assert_eq!(state.doc.child_count(), 1);

    cleanup(&container);
}

#[wasm_bindgen_test]
fn enter_on_empty_list_item_exits_list() {
    // Create a list with two items, second one empty
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![Node::text("item1")]),
                    )]),
                ),
                Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                ),
            ]),
        )]),
    );
    let container = create_container();
    let (view, txns) = create_editor(container.clone(), doc);

    // BulletList(0) > ListItem1(1, size=9) > ListItem2(10, size=4)
    // ListItem2 > Para(11) > content_start=12 (empty)
    // But wait: "item1" = 5 chars. Para = 7. ListItem1 = 9. So ListItem2 at offset 10.
    // ListItem2 > Para(11) size=2. content_start=12.
    // Actually: ListItem1 = 1 + (1+5+1) + 1 = 9. Offset 1, end 10.
    // ListItem2 offset 10, size = 1 + 2 + 1 = 4. Para at 11, content at 12.
    set_cursor(&view, 12);
    dispatch_before_input(view.container(), "insertParagraph", None);

    let state = apply_all(&view, &txns);

    // The empty item should have exited the list.
    // Result: BulletList with 1 item + a paragraph
    assert_eq!(state.doc.child_count(), 2, "Should have list + paragraph");
    let list = state.doc.child(0).unwrap();
    assert_eq!(list.node_type(), Some(NodeType::BulletList));
    assert_eq!(list.child_count(), 1, "List should have 1 item left");
    assert_eq!(list.child(0).unwrap().text_content(), "item1");
    assert_eq!(state.doc.child(1).unwrap().node_type(), Some(NodeType::Paragraph));

    cleanup(&container);
}

// ── Regression: cursor position after select + backspace + type ──

#[wasm_bindgen_test]
fn cursor_after_select_backspace_then_type() {
    // Reproduce: type "1234", Enter, select "1234", backspace, type "a"
    // Cursor should be AFTER "a", not before it.
    let container = create_container();
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::empty(),
        )]),
    );
    let (view, txns) = create_editor(container.clone(), doc);

    // Step 1: Type "1234"
    set_cursor(&view, 1);
    dispatch_before_input(view.container(), "insertText", Some("1234"));
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());
    assert_eq!(state.doc.child(0).unwrap().text_content(), "1234");

    // Step 2: Press Enter at end of "1234"
    set_cursor(&view, 5);
    dispatch_before_input(view.container(), "insertParagraph", None);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());
    assert_eq!(state.doc.child_count(), 2, "should have two paragraphs after Enter");

    // Step 3: Ctrl+A — select entire document content
    // Use content boundaries (1 to content_size-1) since position 0 and content_size
    // are doc-level boundaries that can't be represented in DOM selection.
    // Real Ctrl+A in the browser selects all text within contenteditable,
    // which maps to the first textblock start through last textblock end.
    let first_start = 1; // first paragraph content_start
    let last_end = state.doc.content_size(); // end of all content
    set_selection(&view, first_start, last_end);

    // Step 4: Backspace to delete everything
    dispatch_before_input(view.container(), "deleteContentBackward", None);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());
    let cursor_after_delete = state.selection.from();
    assert_eq!(state.doc.text_content(), "",
        "doc should be empty after Ctrl+A backspace, got: '{}'", state.doc.text_content());

    // Step 5: Type "123" one character at a time WITHOUT manually setting cursor.
    // Let the DOM selection flow naturally after each render, just like real typing.
    let mut state = state;
    for ch in ["1", "2", "3"] {
        dispatch_before_input(view.container(), "insertText", Some(ch));
        state = apply_all(&view, &txns);
        txns.borrow_mut().clear();
        view.update_state(state.clone());
    }

    // Verify: text is "123" in order, not scrambled
    let para_text = state.doc.child(0).unwrap().text_content();
    assert_eq!(para_text, "123",
        "paragraph should be '123', got: '{para_text}'");

    // Verify: cursor is at position 4 (para_open=1 + "123"=3)
    assert_eq!(state.selection.from(), 4,
        "model cursor should be at position 4 (after '123'), got {}",
        state.selection.from());
    assert!(state.selection.empty(), "should be a cursor, not a range");

    // Verify DOM selection matches the model
    if let Some(dom_sel) = view.read_dom_selection() {
        assert_eq!(dom_sel.from(), 4,
            "DOM cursor should be at position 4, got {}", dom_sel.from());
    }

    cleanup(&container);
}

#[wasm_bindgen_test]
fn cursor_after_select_all_backspace_then_type_simple() {
    // Simpler version: single paragraph "abcde", select all, backspace, type "x"
    let container = create_container();
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("abcde")]),
        )]),
    );
    let (view, txns) = create_editor(container.clone(), doc);

    // Select all text in paragraph (1..6)
    set_selection(&view, 1, 6);

    // Backspace
    dispatch_before_input(view.container(), "deleteContentBackward", None);
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());
    assert_eq!(state.doc.child(0).unwrap().text_content(), "",
        "paragraph should be empty after deleting all text");

    // Type "x"
    let cursor_after_delete = state.selection.from();
    set_cursor(&view, cursor_after_delete);
    dispatch_before_input(view.container(), "insertText", Some("x"));
    let state = apply_all(&view, &txns);
    txns.borrow_mut().clear();
    view.update_state(state.clone());

    // Verify content
    assert_eq!(state.doc.child(0).unwrap().text_content(), "x");

    // Verify cursor is AFTER "x" (position 2)
    assert_eq!(state.selection.from(), 2,
        "cursor should be at position 2 (after 'x'), got {}",
        state.selection.from());

    // Verify DOM selection matches
    if let Some(dom_sel) = view.read_dom_selection() {
        assert_eq!(dom_sel.from(), 2,
            "DOM cursor should be at position 2, got {}", dom_sel.from());
    }

    cleanup(&container);
}

// ── Link click behavior ──

#[wasm_bindgen_test]
fn link_renders_with_href_and_target_blank() {
    // A document with a link mark should render an <a> with href and target="_blank"
    let container = create_container();
    let link = Mark::new(MarkType::Link).with_attr("href", "https://example.com");
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![
                Node::text("before "),
                Node::text_with_marks("link text", vec![link]),
                Node::text(" after"),
            ]),
        )]),
    );
    let (view, _txns) = create_editor(container.clone(), doc);

    let html = inner_html(&view);
    assert!(html.contains("<a"), "should render an <a> element, got: {html}");
    assert!(html.contains("href=\"https://example.com\""),
        "should have href attribute, got: {html}");
    assert!(html.contains("target=\"_blank\""),
        "should have target=_blank, got: {html}");
    assert!(html.contains("link text"),
        "should contain link text, got: {html}");

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_click_on_link_prevents_default() {
    // Ctrl+click on a link should preventDefault (to open in new tab)
    // Regular click should NOT preventDefault (cursor positioning)
    let container = create_container();
    let link = Mark::new(MarkType::Link).with_attr("href", "https://example.com");
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text_with_marks("click me", vec![link])]),
        )]),
    );
    let (view, _txns) = create_editor(container.clone(), doc);

    // Find the <a> element in the rendered DOM
    let anchor = view.container().query_selector("a").unwrap()
        .expect("should have an <a> element");

    // Regular click — should NOT prevent default (cursor positioning)
    {
        let init = web_sys::MouseEventInit::new();
        init.set_bubbles(true);
        init.set_cancelable(true);
        init.set_ctrl_key(false);
        init.set_meta_key(false);
        let event = web_sys::MouseEvent::new_with_mouse_event_init_dict("click", &init).unwrap();
        anchor.dispatch_event(&event).unwrap();
        assert!(!event.default_prevented(),
            "regular click should NOT prevent default");
    }

    // Ctrl+click — should prevent default (open link)
    {
        let init = web_sys::MouseEventInit::new();
        init.set_bubbles(true);
        init.set_cancelable(true);
        init.set_ctrl_key(true);
        let event = web_sys::MouseEvent::new_with_mouse_event_init_dict("click", &init).unwrap();
        anchor.dispatch_event(&event).unwrap();
        assert!(event.default_prevented(),
            "Ctrl+click on link should prevent default (opens in new tab)");
    }

    cleanup(&container);
}

#[wasm_bindgen_test]
fn ctrl_click_on_non_link_does_nothing() {
    // Ctrl+click on regular text (not a link) should not preventDefault
    let container = create_container();
    let doc = Node::element_with_content(
        NodeType::Doc,
        Fragment::from(vec![Node::element_with_content(
            NodeType::Paragraph,
            Fragment::from(vec![Node::text("plain text")]),
        )]),
    );
    let (view, _txns) = create_editor(container.clone(), doc);

    let para = view.container().query_selector("p").unwrap()
        .expect("should have a <p> element");

    let init = web_sys::MouseEventInit::new();
    init.set_bubbles(true);
    init.set_cancelable(true);
    init.set_ctrl_key(true);
    let event = web_sys::MouseEvent::new_with_mouse_event_init_dict("click", &init).unwrap();
    para.dispatch_event(&event).unwrap();
    assert!(!event.default_prevented(),
        "Ctrl+click on non-link should NOT prevent default");

    cleanup(&container);
}
