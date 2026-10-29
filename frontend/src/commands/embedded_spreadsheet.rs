// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Targeted mutations for document-embedded spreadsheets.
use crate::editor::{
    model::{Fragment, Node, NodeType},
    state::{EditorState, Transaction},
    transform::Step,
};
use ogrenotes_spreadsheet::embedded::{
    self, MAX_CELL_CHARS, MAX_FORMAT_CHARS, MAX_SHEETS, decode_sheet,
};
use std::collections::HashMap;

pub fn new_sheet(name: &str) -> Node {
    Node::element_with_attrs(
        NodeType::SpreadsheetSheet,
        HashMap::from([
            ("sheetName".into(), name.into()),
            ("gridRows".into(), embedded::DEFAULT_ROWS.to_string()),
            ("gridCols".into(), embedded::DEFAULT_COLS.to_string()),
        ]),
        Fragment::empty(),
    )
}

/// Resolve both identities against the current document before changing a key.
pub fn set_attribute(
    state: &EditorState,
    workbook: &str,
    sheet: &str,
    key: String,
    value: String,
) -> Option<Transaction> {
    let (_, book) = crate::editor::commands::find_element_by_block_id(&state.doc, workbook)?;
    let Node::Element {
        node_type: NodeType::Spreadsheet,
        content,
        ..
    } = book
    else {
        return None;
    };
    let child = content
        .children
        .iter()
        .find(|node| node.block_id() == Some(sheet))?;
    let Node::Element {
        node_type: NodeType::SpreadsheetSheet,
        attrs,
        ..
    } = child
    else {
        return None;
    };
    if attrs.get(&key).map(String::as_str).unwrap_or("") == value {
        return None;
    }
    let mut candidate = attrs.clone();
    if key == "sheetName" {
        if value.trim().is_empty() || content.children.iter().any(|node| matches!(node, Node::Element { attrs, .. } if node.block_id() != Some(sheet) && attrs.get("sheetName").is_some_and(|name| name.eq_ignore_ascii_case(&value)))) { return None; }
    } else if !(key.starts_with("cell:") || key.starts_with("format:")) {
        return None;
    }
    if (key.starts_with("cell:") && value.len() > MAX_CELL_CHARS)
        || (key.starts_with("format:") && value.len() > MAX_FORMAT_CHARS)
    {
        return None;
    }
    if value.is_empty() {
        candidate.remove(&key);
    } else {
        candidate.insert(key.clone(), value.clone());
    }
    let decoded = content
        .children
        .iter()
        .map(|node| {
            let Node::Element {
                node_type: NodeType::SpreadsheetSheet,
                attrs,
                ..
            } = node
            else {
                return None;
            };
            decode_sheet(if node.block_id() == Some(sheet) {
                &candidate
            } else {
                attrs
            })
            .ok()
        })
        .collect::<Option<Vec<_>>>()?;
    embedded::validate_workbook(&decoded).ok()?;
    let (pos, _) = crate::editor::commands::find_element_by_block_id(&state.doc, sheet)?;
    state
        .transaction()
        .step(Step::SetAttr {
            pos,
            attr: key,
            value,
        })
        .ok()
        .map(|txn| txn.set_meta("history", "isolate"))
}

pub fn add_sheet(state: &EditorState, workbook: &str) -> Option<Transaction> {
    let (pos, book) = crate::editor::commands::find_element_by_block_id(&state.doc, workbook)?;
    let Node::Element {
        node_type: NodeType::Spreadsheet,
        content,
        ..
    } = &book
    else {
        return None;
    };
    if content.children.len() >= MAX_SHEETS {
        return None;
    }
    let existing = content
        .children
        .iter()
        .map(|node| {
            let Node::Element {
                node_type: NodeType::SpreadsheetSheet,
                attrs,
                ..
            } = node
            else {
                return None;
            };
            decode_sheet(attrs).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    embedded::validate_workbook(&existing).ok()?;
    let name = (1..=MAX_SHEETS + 1).map(|n| format!("Sheet{n}")).find(|name| !content.children.iter().any(|node| matches!(node, Node::Element { attrs, .. } if attrs.get("sheetName").is_some_and(|existing| existing.eq_ignore_ascii_case(name)))))?;
    state
        .transaction()
        .insert(
            pos + book.node_size() - 1,
            Fragment::from(vec![new_sheet(&name)]),
        )
        .ok()
        .map(|txn| txn.set_meta("history", "isolate"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::plugins::HistoryPlugin;

    fn fixture() -> (EditorState, String, String, String) {
        let first = Node::element_with_attrs(
            NodeType::Spreadsheet,
            HashMap::new(),
            Fragment::from(vec![new_sheet("Sheet1")]),
        );
        let second = Node::element_with_attrs(
            NodeType::Spreadsheet,
            HashMap::new(),
            Fragment::from(vec![new_sheet("Sheet1")]),
        );
        let book = first.block_id().unwrap().to_owned();
        let sheet = first.child(0).unwrap().block_id().unwrap().to_owned();
        let other = second.block_id().unwrap().to_owned();
        (
            EditorState::create_default(Node::element_with_content(
                NodeType::Doc,
                Fragment::from(vec![first, second]),
            )),
            book,
            sheet,
            other,
        )
    }

    #[test]
    fn cell_and_format_changes_are_independent_and_undoable() {
        let (state, book, sheet, other) = fixture();
        let txn = set_attribute(
            &state,
            &book,
            &sheet,
            embedded::cell_key(0, 0),
            "=1+2".into(),
        )
        .unwrap();
        assert!(matches!(txn.steps.as_slice(), [Step::SetAttr { attr, .. }] if attr == "cell:0:0"));
        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        let state = state.apply(txn);
        let txn = set_attribute(
            &state,
            &book,
            &sheet,
            embedded::format_key(0, 0),
            "currency".into(),
        )
        .unwrap();
        history.record(&txn, &state.doc);
        let state = state.apply(txn);
        assert!(
            set_attribute(
                &state,
                &other,
                &sheet,
                embedded::cell_key(0, 0),
                "wrong workbook".into()
            )
            .is_none()
        );
        let state = state.apply(history.undo(&state).unwrap());
        let (_, node) =
            crate::editor::commands::find_element_by_block_id(&state.doc, &sheet).unwrap();
        let Node::Element { attrs, .. } = node else {
            panic!()
        };
        let decoded = decode_sheet(&attrs).unwrap();
        assert_eq!(decoded.cells.get(&(0, 0)).map(String::as_str), Some("=1+2"));
        assert!(decoded.formats.is_empty());
        assert!(set_attribute(&state, &book, &sheet, "cell:999999:0".into(), "x".into()).is_none());
    }

    #[test]
    fn adding_a_sheet_preserves_existing_cells_and_the_other_workbook() {
        let (state, book, sheet, other) = fixture();
        let state = state.apply(
            set_attribute(&state, &book, &sheet, embedded::cell_key(0, 0), "42".into()).unwrap(),
        );
        let other_before = crate::editor::commands::find_element_by_block_id(&state.doc, &other)
            .unwrap()
            .1;
        let txn = add_sheet(&state, &book).unwrap();
        assert!(matches!(txn.steps.as_slice(), [Step::Replace { from, to, .. }] if from == to));
        let state = state.apply(txn);
        let (_, node) =
            crate::editor::commands::find_element_by_block_id(&state.doc, &book).unwrap();
        assert_eq!(node.child_count(), 2);
        assert_eq!(
            crate::editor::commands::find_element_by_block_id(&state.doc, &other)
                .unwrap()
                .1,
            other_before
        );
        let Node::Element { attrs, .. } = node.child(0).unwrap() else {
            panic!()
        };
        assert_eq!(attrs.get("cell:0:0").map(String::as_str), Some("42"));
    }
}
