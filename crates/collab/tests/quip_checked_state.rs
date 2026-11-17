//! Verbatim Quip HTML from the user's neutral Checked Unchecked test note.
use ogrenotes_collab::{
    export, import_quip::from_quip_html, snapshot, validate::schema_violations,
};
use yrs::{Doc, GetString, ReadTxn, Transact, Xml, XmlElementRef, XmlFragment, XmlOut};

const CHECKLIST: &str = include_str!("fixtures/quip/authentic/checked-unchecked.html");

fn task_items(doc: &Doc) -> Vec<(String, String)> {
    fn text(txn: &impl ReadTxn, element: &XmlElementRef) -> String {
        let mut output = String::new();
        for child in element.children(txn) {
            match child {
                XmlOut::Element(child) => output.push_str(&text(txn, &child)),
                XmlOut::Text(child) => output.push_str(&child.get_string(txn)),
                _ => {}
            }
        }
        output
    }
    assert!(schema_violations(doc).is_empty());
    let txn = doc.transact();
    let fragment = txn.get_xml_fragment("content").unwrap();
    let list = fragment
        .children(&txn)
        .find_map(|node| match node {
            XmlOut::Element(node) if node.tag().as_ref() == "task_list" => Some(node),
            _ => None,
        })
        .expect("the real Quip checklist must remain editable native tasks");
    list.children(&txn)
        .map(|node| {
            let XmlOut::Element(item) = node else {
                panic!("task item")
            };
            assert_eq!(item.tag().as_ref(), "task_item");
            (
                item.get_attribute(&txn, "checked").unwrap().to_string(),
                text(&txn, &item),
            )
        })
        .collect()
}

fn assert_checklist(doc: &Doc) {
    let expected = vec![
        ("false".to_string(), "First Item".to_string()),
        ("true".to_string(), "Second Item".to_string()),
        ("false".to_string(), "Third Item".to_string()),
    ];
    assert_eq!(task_items(doc), expected);
    let html = export::to_html(doc);
    assert_eq!(task_items(&from_quip_html(&html).doc), expected);
}

#[test]
fn authentic_quip_checked_and_unchecked_items_survive_import_and_snapshot() {
    assert!(CHECKLIST.contains("class='checked'"));
    let imported = from_quip_html(CHECKLIST);
    assert_checklist(&imported.doc);
    let restored = snapshot::deserialize(&snapshot::doc_to_bytes(&imported.doc)).unwrap();
    assert_checklist(restored.inner());
}
