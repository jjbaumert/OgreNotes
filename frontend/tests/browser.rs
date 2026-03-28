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
    let view = EditorView::new(container, state, move |txn: Transaction| {
        txns_clone.borrow_mut().push(txn);
    });
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
