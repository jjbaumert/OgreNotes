// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Browser paste contracts exercised through the persistent Yrs bridge.

use super::*;
use ogrenotes_frontend::editor::{schema::default_schema, yrs_bridge};
use yrs::types::xml::{Xml, XmlFragment};
use yrs::{Doc as YrsDoc, ReadTxn, Transact, Update, updates::decoder::Decode};

const HTML: &str = "<ul><li>before $x$ after<ul><li>nested</li></ul></li></ul>";
const PLAIN: &str = "before $x$ after\nnested";
const UNTOUCHED_ID: &str = "paste-untouched";

fn initial_doc(in_list: bool) -> Node {
    let target = if in_list {
        Node::element_with_content(
            NodeType::BulletList,
            Fragment::from(vec![Node::element_with_content(
                NodeType::ListItem,
                Fragment::from(vec![Node::element(NodeType::Paragraph)]),
            )]),
        )
    } else {
        Node::element(NodeType::Paragraph)
    };
    let untouched = Node::element_with_attrs(
        NodeType::Paragraph,
        [("blockId".into(), UNTOUCHED_ID.into())].into(),
        Fragment::from(vec![Node::text("untouched")]),
    );
    Node::element_with_content(NodeType::Doc, Fragment::from(vec![target, untouched]))
}

fn apply_update(ydoc: &YrsDoc, bytes: &[u8]) {
    ydoc.transact_mut()
        .apply_update(Update::decode_v1(bytes).expect("decode Yrs update"))
        .expect("apply Yrs update");
}

fn peers(initial: &Node) -> (YrsDoc, YrsDoc) {
    let bytes = yrs_bridge::doc_to_ydoc_bytes(initial);
    let local = YrsDoc::with_client_id(101);
    let peer = YrsDoc::with_client_id(202);
    apply_update(&local, &bytes);
    apply_update(&peer, &bytes);
    (local, peer)
}

fn untouched(doc: &Node) -> &Node {
    (0..doc.child_count())
        .map(|index| doc.child(index).unwrap())
        .find(|node| node.attrs().get("blockId").map(String::as_str) == Some(UNTOUCHED_ID))
        .expect("untouched block survives paste")
}

// Check actual shared-type IDs, including the text beneath the block. A stable
// model blockId alone would not detect deletion and reinsertion in Yrs.
fn untouched_yrs_ids(ydoc: &YrsDoc) -> Vec<yrs::BranchID> {
    let txn = ydoc.transact();
    let fragment = txn.get_xml_fragment("content").expect("content fragment");
    for index in 0..fragment.len(&txn) {
        let child = fragment.get(&txn, index).unwrap();
        if let Some(element) = child.clone().into_xml_element()
            && element.get_attribute(&txn, "blockId").as_deref() == Some(UNTOUCHED_ID)
        {
            let mut ids = vec![child.id()];
            for offset in 0..element.len(&txn) {
                ids.push(element.get(&txn, offset).unwrap().id());
            }
            assert!(
                ids.len() > 1,
                "identity check includes the text shared type"
            );
            return ids;
        }
    }
    panic!("untouched block missing from Yrs");
}

fn walk<'a>(node: &'a Node, nodes: &mut Vec<&'a Node>) {
    nodes.push(node);
    for index in 0..node.child_count() {
        walk(node.child(index).unwrap(), nodes);
    }
}

fn assert_rich_paste(doc: &Node, untouched_text: &str) {
    default_schema()
        .validate(doc)
        .expect("persisted paste satisfies schema");
    assert_eq!(untouched(doc).text_content(), untouched_text);
    let mut nodes = Vec::new();
    walk(doc, &mut nodes);
    let equations: Vec<_> = nodes
        .iter()
        .filter(|node| node.node_type() == Some(NodeType::MathInline))
        .collect();
    assert_eq!(equations.len(), 1, "inline equation survives exactly once");
    assert_eq!(
        equations[0].attrs().get("source").map(String::as_str),
        Some("x")
    );
    let paragraph = nodes
        .iter()
        .find(|node| {
            node.node_type() == Some(NodeType::Paragraph) && node.text_content().contains("before ")
        })
        .expect("paragraph around mixed text and equation");
    assert_eq!(paragraph.child_count(), 3);
    assert_eq!(paragraph.child(0).unwrap().text_content(), "before ");
    assert_eq!(
        paragraph.child(1).unwrap().node_type(),
        Some(NodeType::MathInline)
    );
    assert_eq!(paragraph.child(2).unwrap().text_content(), " after");
    let item = nodes
        .iter()
        .find(|node| {
            node.node_type() == Some(NodeType::ListItem)
                && node.child_count() == 2
                && node.child(0) == Some(*paragraph)
        })
        .expect("mixed paragraph and nested list remain in the same item");
    let nested = item.child(1).unwrap();
    assert_eq!(nested.node_type(), Some(NodeType::BulletList));
    assert_eq!(nested.child_count(), 1);
    let nested_item = nested.child(0).unwrap();
    assert_eq!(nested_item.node_type(), Some(NodeType::ListItem));
    assert_eq!(
        nested_item.child(0).unwrap().node_type(),
        Some(NodeType::Paragraph)
    );
    assert_eq!(nested_item.text_content(), "nested");
}

fn assert_reload(ydoc: &YrsDoc, expected: &Node, untouched_text: &str) {
    let bytes = ydoc
        .transact()
        .encode_state_as_update_v1(&yrs::StateVector::default());
    let restored = yrs_bridge::ydoc_bytes_to_doc(&bytes).expect("reload persisted Yrs snapshot");
    assert_eq!(&restored, expected);
    assert_rich_paste(&restored, untouched_text);
}

#[wasm_bindgen_test]
fn rich_paste_yrs_incremental_peer_reload_and_noop_keep_identity() {
    for in_list in [false, true] {
        let initial = initial_doc(in_list);
        let (local, peer) = peers(&initial);
        let baseline = yrs_bridge::read_doc_from_ydoc(&local).unwrap();
        let before_ids = untouched_yrs_ids(&local);
        let peer_vector = peer.transact().state_vector();
        let container = create_container();
        let (view, txns) = create_editor(container.clone(), baseline.clone());
        set_cursor(&view, if in_list { 3 } else { 1 });
        dispatch_paste_html(&view, &txns, PLAIN, HTML);
        let pasted = view.state().doc;
        assert_rich_paste(&pasted, "untouched");

        let synchronized = yrs_bridge::sync_model_to_ydoc_diffed(&local, &pasted, Some(&baseline));
        assert_eq!(untouched_yrs_ids(&local), before_ids);
        let delta = local.transact().encode_state_as_update_v1(&peer_vector);
        apply_update(&peer, &delta);
        let local_model = yrs_bridge::read_doc_from_ydoc(&local).unwrap();
        assert_eq!(local_model, synchronized);
        assert_eq!(yrs_bridge::read_doc_from_ydoc(&peer).unwrap(), local_model);
        assert_eq!(untouched_yrs_ids(&peer), before_ids);
        assert_reload(&peer, &local_model, "untouched");

        let updates = std::rc::Rc::new(std::cell::RefCell::new(0usize));
        let observed = updates.clone();
        let _subscription = local
            .observe_update_v1(move |_, _| {
                *observed.borrow_mut() += 1;
            })
            .unwrap();
        let before_vector = local.transact().state_vector();
        yrs_bridge::sync_model_to_ydoc_diffed(&local, &synchronized, Some(&synchronized));
        assert_eq!(
            *updates.borrow(),
            0,
            "second sync emits no incremental update"
        );
        assert_eq!(local.transact().state_vector(), before_vector);
        assert_eq!(untouched_yrs_ids(&local), before_ids);
        drop(_subscription);

        txns.borrow_mut().clear();
        let mut previous = synchronized;
        for (input, expected) in [("historyUndo", &baseline), ("historyRedo", &pasted)] {
            dispatch_before_input(view.container(), input, None);
            assert_eq!(txns.borrow().len(), 1, "{input} dispatches once");
            let state = apply_all(&view, &txns);
            txns.borrow_mut().clear();
            assert_eq!(
                &state.doc, expected,
                "{input} restores the browser document"
            );
            default_schema()
                .validate(&state.doc)
                .expect("history preserves schema");

            let peer_vector = peer.transact().state_vector();
            let synced = yrs_bridge::sync_model_to_ydoc_diffed(&local, &state.doc, Some(&previous));
            let delta = local.transact().encode_state_as_update_v1(&peer_vector);
            apply_update(&peer, &delta);
            assert_eq!(yrs_bridge::read_doc_from_ydoc(&local).unwrap(), synced);
            assert_eq!(yrs_bridge::read_doc_from_ydoc(&peer).unwrap(), synced);
            assert_eq!(untouched_yrs_ids(&local), before_ids);
            assert_eq!(untouched_yrs_ids(&peer), before_ids);

            let snapshot = peer
                .transact()
                .encode_state_as_update_v1(&yrs::StateVector::default());
            let restored = yrs_bridge::ydoc_bytes_to_doc(&snapshot).unwrap();
            assert_eq!(restored, synced, "{input} persists through peer reload");
            default_schema()
                .validate(&restored)
                .expect("history reload preserves schema");
            assert_eq!(untouched(&restored).text_content(), "untouched");
            if input == "historyRedo" {
                assert_rich_paste(&restored, "untouched");
            }
            previous = synced;
        }
        cleanup(&container);
    }
}

#[wasm_bindgen_test]
fn rich_paste_yrs_preserves_concurrent_peer_text_in_untouched_block() {
    for in_list in [false, true] {
        let initial = initial_doc(in_list);
        let (local, peer) = peers(&initial);
        let baseline = yrs_bridge::read_doc_from_ydoc(&local).unwrap();
        let before_ids = untouched_yrs_ids(&local);
        let initial_vector = local.transact().state_vector();
        let container = create_container();
        let (view, txns) = create_editor(container.clone(), baseline.clone());
        set_cursor(&view, if in_list { 3 } else { 1 });
        dispatch_paste_html(&view, &txns, PLAIN, HTML);
        let pasted = view.state().doc;

        let mut remote = baseline.clone();
        if let Node::Element { content, .. } = &mut remote
            && let Node::Element { content, .. } = &mut content.children[1]
        {
            *content = Fragment::from(vec![Node::text("untouched peer edit")]);
        }
        yrs_bridge::sync_model_to_ydoc_diffed(&peer, &remote, Some(&baseline));
        let remote_delta = peer.transact().encode_state_as_update_v1(&initial_vector);
        apply_update(&local, &remote_delta);
        assert_eq!(
            untouched(&yrs_bridge::read_doc_from_ydoc(&local).unwrap()).text_content(),
            "untouched peer edit"
        );

        // The editor still holds its pre-peer baseline. Its paste must not
        // overwrite the remote text while syncing the unrelated changed list.
        let peer_vector = peer.transact().state_vector();
        yrs_bridge::sync_model_to_ydoc_diffed(&local, &pasted, Some(&baseline));
        let paste_delta = local.transact().encode_state_as_update_v1(&peer_vector);
        apply_update(&peer, &paste_delta);
        let merged = yrs_bridge::read_doc_from_ydoc(&local).unwrap();
        assert_eq!(yrs_bridge::read_doc_from_ydoc(&peer).unwrap(), merged);
        assert_rich_paste(&merged, "untouched peer edit");
        assert_eq!(untouched_yrs_ids(&local), before_ids);
        assert_eq!(untouched_yrs_ids(&peer), before_ids);
        assert_reload(&peer, &merged, "untouched peer edit");
        cleanup(&container);
    }
}
