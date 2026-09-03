// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Native-only structural property: no sequence of editor commands can
//! leave the document outside the schema. `Schema::validate` recurses
//! fully and would catch every orphan class, but its only callers were
//! its own unit tests; this drives it after every `apply`.

use proptest::prelude::*;

use super::commands::{delete_selection, shift_tab_command, tab_command, toggle_list};
use super::model::{Fragment, Node, NodeType};
use super::schema::default_schema;
use super::selection::Selection;
use super::state::{EditorState, Transaction};

fn p(text: &str) -> Node {
    Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(text)]))
}
fn li(text: &str) -> Node {
    Node::element_with_content(NodeType::ListItem, Fragment::from(vec![p(text)]))
}

/// Seed corpus: one doc per container family the past orphans came from.
fn seeds() -> Vec<Node> {
    let d = |children: Vec<Node>| Node::element_with_content(NodeType::Doc, Fragment::from(children));
    vec![
        d(vec![p("alpha"), p("beta")]),
        d(vec![
            p("intro"),
            Node::element_with_content(NodeType::BulletList, Fragment::from(vec![li("one"), li("two")])),
            p("outro"),
        ]),
        d(vec![
            Node::element_with_content(NodeType::Blockquote, Fragment::from(vec![p("quoted")])),
            Node::element_with_content(NodeType::CodeBlock, Fragment::from(vec![Node::text("let x = 1;")])),
        ]),
        d(vec![
            Node::element_with_attrs(
                NodeType::Heading,
                [("level".to_string(), "2".to_string())].into_iter().collect(),
                Fragment::from(vec![Node::text("Title")]),
            ),
            Node::element(NodeType::HorizontalRule),
            p("after rule"),
        ]),
    ]
}

#[derive(Debug, Clone)]
enum Op {
    Insert(String),
    Split,
    JoinBackward,
    JoinForward,
    DeleteSelection,
    Tab,
    ShiftTab,
    ToggleBullet,
    MoveCursor(usize),
    Select(usize, usize),
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        "[a-z ]{1,4}".prop_map(Op::Insert),
        Just(Op::Split),
        Just(Op::JoinBackward),
        Just(Op::JoinForward),
        Just(Op::DeleteSelection),
        Just(Op::Tab),
        Just(Op::ShiftTab),
        Just(Op::ToggleBullet),
        (0usize..64).prop_map(Op::MoveCursor),
        (0usize..64, 0usize..64).prop_map(|(a, b)| Op::Select(a.min(b), a.max(b))),
    ]
}

fn run_command(
    state: &EditorState,
    f: impl Fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool,
) -> EditorState {
    let captured = std::cell::RefCell::new(None);
    let dispatch = |txn: Transaction| {
        *captured.borrow_mut() = Some(txn);
    };
    f(state, Some(&dispatch));
    match captured.into_inner() {
        Some(txn) => state.apply(txn),
        None => state.clone(),
    }
}

fn snap_cursor(doc: &Node, pos: usize) -> Selection {
    let pos = pos.min(doc.content_size());
    Selection::find_from(doc, pos, 1)
        .or_else(|| Selection::find_from(doc, pos, -1))
        .unwrap_or_else(|| Selection::cursor(1))
}

fn step(state: EditorState, op: &Op) -> EditorState {
    match op {
        Op::Insert(s) => match state.transaction().insert_text(s) {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::Split => match state.transaction().split_block() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::JoinBackward => match state.transaction().join_backward() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::JoinForward => match state.transaction().join_forward() {
            Ok(txn) => state.apply(txn),
            Err(_) => state,
        },
        Op::DeleteSelection => run_command(&state, delete_selection),
        Op::Tab => run_command(&state, tab_command),
        Op::ShiftTab => run_command(&state, shift_tab_command),
        Op::ToggleBullet => run_command(&state, |s, d| {
            toggle_list(NodeType::BulletList, NodeType::ListItem, s, d)
        }),
        Op::MoveCursor(pos) => EditorState {
            selection: snap_cursor(&state.doc, *pos),
            ..state
        },
        Op::Select(a, b) => {
            let from = snap_cursor(&state.doc, *a).from();
            let to = snap_cursor(&state.doc, *b).from().max(from);
            EditorState {
                selection: Selection::text(from, to),
                ..state
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// After every applied transaction the document validates against the
    /// schema — no bare text or stray block inside a container, no leaf
    /// with children, no illegal parent/child pair.
    #[test]
    fn every_command_leaves_a_schema_valid_doc(
        seed in 0usize..4,
        ops in proptest::collection::vec(arb_op(), 1..20),
    ) {
        let schema = default_schema();
        let mut state = EditorState::create_default(seeds()[seed].clone());
        prop_assert!(schema.validate(&state.doc).is_ok(), "seed {seed} must be valid");
        for (i, op) in ops.iter().enumerate() {
            let before = state.doc.clone();
            state = step(state, op);
            if let Err(e) = schema.validate(&state.doc) {
                prop_assert!(
                    false,
                    "op #{i} {op:?} broke the schema: {e}\nbefore: {before:?}\nafter:  {:?}\nops: {ops:?}",
                    state.doc
                );
            }
        }
    }
}
