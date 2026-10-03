// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Editor-model edits exchanged as real Yrs updates. The same cases also run in
//! Chromium, rendering the merged result through EditorView on both replicas.
use ogrenotes_frontend::editor::{
    model::{Fragment, Mark, MarkType, Node, NodeType},
    yrs_bridge::{doc_to_ydoc_bytes, read_doc_from_ydoc, sync_model_to_ydoc_diffed},
};
use yrs::{Doc, ReadTxn, StateVector, Transact, Update, updates::decoder::Decode};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::*;
#[cfg(target_arch = "wasm32")]
wasm_bindgen_test_configure!(run_in_browser);

fn paragraph(id: &str, text: &str) -> Node {
    Node::element_with_attrs(NodeType::Paragraph,
        [("blockId".into(), id.into())].into(), Fragment::from(vec![Node::text(text)]))
}
fn document(blocks: Vec<Node>) -> Node {
    Node::element_with_content(NodeType::Doc, Fragment::from(blocks))
}
fn pair(initial: &Node) -> (Doc, Doc) {
    let bytes = doc_to_ydoc_bytes(initial);
    let a = Doc::new();
    let b = Doc::new();
    for doc in [&a, &b] {
        doc.transact_mut().apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
    }
    (a, b)
}
fn send(from: &Doc, to: &Doc) {
    let update = from.transact().encode_state_as_update_v1(&to.transact().state_vector());
    to.transact_mut().apply_update(Update::decode_v1(&update).unwrap()).unwrap();
}
fn assert_merged(a: &Doc, b: &Doc, expected: &Node) {
    send(a, b);
    send(b, a);
    for doc in [a, b] {
        let actual = read_doc_from_ydoc(doc).unwrap();
        #[cfg(target_arch = "wasm32")]
        assert_rendered(&actual, expected);
        assert_eq!(&actual, expected);
        // Persistence roundtrip is part of the user contract, not just in-memory convergence.
        let bytes = doc.transact().encode_state_as_update_v1(&StateVector::default());
        assert_eq!(ogrenotes_frontend::editor::yrs_bridge::ydoc_bytes_to_doc(&bytes).unwrap(), actual);
    }
}
#[cfg(target_arch = "wasm32")]
fn assert_rendered(actual: &Node, expected: &Node) {
    use ogrenotes_frontend::editor::{state::EditorState, view::EditorView, plugins::HistoryPlugin};
    use std::{rc::Rc, cell::RefCell};
    use wasm_bindgen::JsCast;
    let document = web_sys::window().unwrap().document().unwrap();
    let container: web_sys::HtmlElement = document.create_element("div").unwrap().unchecked_into();
    document.body().unwrap().append_child(&container).unwrap();
    let view = EditorView::new(container.clone(), EditorState::create_default(actual.clone()),
        |_| {}, Rc::new(RefCell::new(HistoryPlugin::new())), |_| {});
    assert_eq!(container.text_content().unwrap(), expected.text_content());
    for index in 0..expected.child_count() {
        let child = expected.child(index).unwrap();
        let rendered = container.child_nodes().item(index as u32).unwrap();
        assert_eq!(rendered.text_content().unwrap(), child.text_content());
    }
    view.destroy();
    container.remove();
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn remote_prepend_survives_stale_local_edit() {
    let initial = document(vec![paragraph("b1", "Hello")]);
    let (a, b) = pair(&initial);
    let remote = document(vec![paragraph("peer", "Peer"), paragraph("b1", "Hello")]);
    sync_model_to_ydoc_diffed(&b, &remote, Some(&initial));
    send(&b, &a);
    let local = document(vec![paragraph("b1", "Hello!")]);
    sync_model_to_ydoc_diffed(&a, &local, Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("peer", "Peer"), paragraph("b1", "Hello!")]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn prepend_preserves_order_for_both_client_id_orderings() {
    for (source_id, peer_id) in [(1, 2), (2, 1)] {
        let initial = document(vec![paragraph("b1", "First"), paragraph("b2", "Second")]);
        let a = Doc::with_client_id(source_id);
        let b = Doc::with_client_id(peer_id);
        sync_model_to_ydoc_diffed(&a, &initial, None);
        send(&a, &b);
        let prepended = document(vec![paragraph("peer", "Peer"), paragraph("b1", "First!"), paragraph("b2", "Second")]);
        sync_model_to_ydoc_diffed(&b, &prepended, Some(&initial));
        assert_eq!(read_doc_from_ydoc(&b).unwrap(), prepended);
        assert_merged(&a, &b, &prepended);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn remote_disjoint_edits_survive_stale_local_insert() {
    let initial = document(vec![paragraph("b1", "abcdef")]);
    let (a, b) = pair(&initial);
    sync_model_to_ydoc_diffed(&b, &document(vec![paragraph("b1", "aBcdeF")]), Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a, &document(vec![paragraph("b1", "abcXdef")]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("b1", "aBcXdeF")]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn concurrent_marks_preserve_one_copy_of_text() {
    let initial = document(vec![paragraph("b1", "Hello")]);
    let (a, b) = pair(&initial);
    let marked = |marks| document(vec![Node::element_with_attrs(NodeType::Paragraph,
        [("blockId".into(), "b1".into())].into(), Fragment::from(vec![Node::text_with_marks("Hello", marks)]))]);
    sync_model_to_ydoc_diffed(&a, &marked(vec![Mark::new(MarkType::Bold)]), Some(&initial));
    sync_model_to_ydoc_diffed(&b, &marked(vec![Mark::new(MarkType::Italic)]), Some(&initial));
    assert_merged(&a, &b, &marked(vec![Mark::new(MarkType::Bold), Mark::new(MarkType::Italic)]));
}

fn stale_text_case(old: &str, remote: &str, local: &str, expected: &str) {
    let initial = document(vec![paragraph("b1", old)]);
    let (a, b) = pair(&initial);
    sync_model_to_ydoc_diffed(&b, &document(vec![paragraph("b1", remote)]), Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a, &document(vec![paragraph("b1", local)]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("b1", expected)]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn stale_deletions_union_without_erasing_peer_insertions() {
    stale_text_case("abcdef", "abef", "abcf", "abf");
    stale_text_case("abcdef", "abcPEERdef", "af", "aPEERf");
    stale_text_case("abcdef", "aBcdeF", "abXcdeYf", "aBXcdeYF");
    stale_text_case("مرحبا 😀 שלום", "يا مرحبا 😀 שלום!", "مرحبا 🌍 שלום", "يا مرحبا 🌍 שלום!");
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn remote_block_delete_is_not_resurrected_by_stale_local_edit() {
    let initial = document(vec![paragraph("b1", "First"), paragraph("b2", "Second")]);
    let (a, b) = pair(&initial);
    sync_model_to_ydoc_diffed(&b, &document(vec![paragraph("b2", "Second")]), Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a, &document(vec![paragraph("b1", "First!"), paragraph("b2", "Second!")]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("b2", "Second!")]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn stale_reorder_preserves_peer_block_and_live_text() {
    let initial = document(vec![paragraph("b1", "First"), paragraph("b2", "Second")]);
    let (a, b) = pair(&initial);
    sync_model_to_ydoc_diffed(&b, &document(vec![paragraph("peer", "Peer"), paragraph("b1", "First!"), paragraph("b2", "Second")]), Some(&initial));
    send(&b, &a);
    assert_eq!(read_doc_from_ydoc(&a).unwrap(), document(vec![paragraph("peer", "Peer"), paragraph("b1", "First!"), paragraph("b2", "Second")]));
    sync_model_to_ydoc_diffed(&a, &document(vec![paragraph("b2", "Second"), paragraph("b1", "First")]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("peer", "Peer"), paragraph("b2", "Second"), paragraph("b1", "First!")]));
}

fn rich_document(chunks: Vec<Node>) -> Node {
    document(vec![Node::element_with_attrs(NodeType::Paragraph,
        [("blockId".into(), "b1".into())].into(), Fragment::from(chunks))])
}
fn marked(text: &str, marks: &[MarkType]) -> Node {
    Node::text_with_marks(text, marks.iter().copied().map(Mark::new).collect())
}
#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn partial_formatting_merges_across_existing_text_items() {
    let initial = rich_document(vec![Node::text("Hello "), marked("world", &[MarkType::Bold])]);
    let (a, b) = pair(&initial);
    let local_a = rich_document(vec![Node::text("Hel"), marked("lo ", &[MarkType::Italic]), marked("wor", &[MarkType::Bold, MarkType::Italic]), marked("ld", &[MarkType::Bold])]);
    let local_b = rich_document(vec![Node::text("Hello "), marked("world", &[MarkType::Underline])]);
    sync_model_to_ydoc_diffed(&a, &local_a, Some(&initial));
    sync_model_to_ydoc_diffed(&b, &local_b, Some(&initial));
    assert_merged(&a, &b, &rich_document(vec![Node::text("Hel"), marked("lo ", &[MarkType::Italic]), marked("wor", &[MarkType::Italic, MarkType::Underline]), marked("ld", &[MarkType::Underline])]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn stale_formatting_preserves_remote_marks_and_text() {
    let initial = document(vec![paragraph("b1", "Hello")]);
    let (a, b) = pair(&initial);
    sync_model_to_ydoc_diffed(&b, &rich_document(vec![marked("Hello!", &[MarkType::Italic])]), Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a, &rich_document(vec![marked("Hello", &[MarkType::Bold])]), Some(&initial));
    assert_merged(&a, &b, &rich_document(vec![marked("Hello", &[MarkType::Bold, MarkType::Italic]), marked("!", &[MarkType::Italic])]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn unicode_rebase_respects_utf16_yrs_documents() {
    let initial = document(vec![paragraph("b1", "A😀B")]);
    let bytes = doc_to_ydoc_bytes(&initial);
    let make_doc = || {
        let doc = Doc::with_options(yrs::Options { offset_kind: yrs::OffsetKind::Utf16, ..Default::default() });
        doc.transact_mut().apply_update(Update::decode_v1(&bytes).unwrap()).unwrap();
        doc
    };
    let (a, b) = (make_doc(), make_doc());
    sync_model_to_ydoc_diffed(&b, &document(vec![paragraph("b1", "ZA😀B!")]), Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a, &document(vec![paragraph("b1", "A🌍B")]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("b1", "ZA🌍B!")]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn long_text_rebase_keeps_disjoint_edits_and_large_replacement() {
    let middle = "ab😀 ".repeat(5_000);
    stale_text_case(&format!("x{middle}y"), &format!("X{middle}Y"),
        &format!("x{middle}!y"), &format!("X{middle}!Y"));
    stale_text_case(&"a".repeat(20_000), &"b".repeat(20_000),
        &format!("{}!", "a".repeat(20_000)), &format!("{}!", "b".repeat(20_000)));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn inline_atoms_keep_identity_during_concurrent_formatting_and_typing() {
    use yrs::{XmlFragment, XmlOut};
    for atom in [Node::element(NodeType::HardBreak), Node::element_with_attrs(
        NodeType::Mention, [("blockId".into(), "mention".into()), ("user_id".into(), "u1".into()),
            ("display".into(), "Alice".into())].into(), Fragment::empty())]
    {
        let make = |before: &str, after: &str, marks: &[MarkType]| rich_document(vec![
            marked(before, marks), atom.clone(), marked(after, marks)]);
        let initial = make("Hello", "world", &[]);
        let (a, b) = pair(&initial);
        let identities = |doc: &Doc| {
            let txn = doc.transact();
            let root = txn.get_xml_fragment("content").unwrap();
            let Some(XmlOut::Element(paragraph)) = root.get(&txn, 0) else { panic!() };
            paragraph.children(&txn).map(|child| child.id()).collect::<Vec<_>>()
        };
        let before = identities(&a);
        sync_model_to_ydoc_diffed(&a, &make("Hello", "world", &[MarkType::Bold]), Some(&initial));
        sync_model_to_ydoc_diffed(&b, &make("Hello", "world", &[MarkType::Italic]), Some(&initial));
        let merged = make("Hello", "world", &[MarkType::Bold, MarkType::Italic]);
        assert_merged(&a, &b, &merged);
        assert_eq!(identities(&a), before, "formatting recreated a text run or atom");
        // Edits on either side of the atom must stay on their original side.
        sync_model_to_ydoc_diffed(&a, &make("Hello!", "world", &[MarkType::Bold, MarkType::Italic]), Some(&merged));
        sync_model_to_ydoc_diffed(&b, &make("Hello", "!world", &[MarkType::Bold, MarkType::Italic]), Some(&merged));
        assert_merged(&a, &b, &make("Hello!", "!world", &[MarkType::Bold, MarkType::Italic]));
        assert_eq!(identities(&a), before);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn delivered_remote_reorder_survives_stale_local_typing() {
    let initial = document(vec![paragraph("b1", "First"), paragraph("b2", "Second")]);
    let (a, b) = pair(&initial);
    let reordered = document(vec![paragraph("b2", "Second"), paragraph("b1", "First")]);
    sync_model_to_ydoc_diffed(&b, &reordered, Some(&initial));
    send(&b, &a);
    sync_model_to_ydoc_diffed(&a,
        &document(vec![paragraph("b1", "First!"), paragraph("b2", "Second")]), Some(&initial));
    assert_merged(&a, &b, &document(vec![paragraph("b2", "Second"), paragraph("b1", "First!")]));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn stale_typing_preserves_peer_root_attribute_changes() {
    let root = |attrs: &[(&str, &str)], text| Node::element_with_attrs(
        NodeType::Doc,
        attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        Fragment::from(vec![paragraph("b1", text)]),
    );
    let initial = root(&[("theme", "light"), ("obsolete", "old")], "Hello");
    let (a, b) = pair(&initial);
    let remote = root(&[("theme", "dark"), ("peer", "added")], "Hello");
    sync_model_to_ydoc_diffed(&b, &remote, Some(&initial));
    send(&b, &a);
    let local = root(&[("theme", "light"), ("obsolete", "old")], "Hello!");
    sync_model_to_ydoc_diffed(&a, &local, Some(&initial));
    assert_merged(&a, &b, &root(&[("theme", "dark"), ("peer", "added")], "Hello!"));
}
