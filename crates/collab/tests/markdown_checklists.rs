//! The user-supplied Markdown sample is not a Quip HTML export.
use ogrenotes_collab::{export, import::from_markdown, snapshot, validate::schema_violations};
use yrs::types::xml::{XmlElementPrelim, XmlElementRef, XmlTextPrelim};
use yrs::{Doc, GetString, ReadTxn, Transact, WriteTxn, Xml, XmlFragment, XmlOut};

const SAMPLE: &str = include_str!("fixtures/markdown/checked-unchecked.md");

fn task_items(doc: &Doc) -> Vec<(bool, String)> {
    fn walk(txn: &impl ReadTxn, element: &XmlElementRef, out: &mut Vec<(bool, String)>) {
        if element.tag().as_ref() == "task_item" {
            let checked = element.get_attribute(txn, "checked").unwrap();
            let mut text = String::new();
            for child in element.children(txn) {
                if let XmlOut::Element(paragraph) = child
                    && paragraph.tag().as_ref() == "paragraph"
                {
                    for span in paragraph.children(txn) {
                        if let XmlOut::Text(span) = span {
                            text.push_str(&span.get_string(txn));
                        }
                    }
                }
            }
            out.push((checked == "true", text));
        }
        for child in element.children(txn) {
            if let XmlOut::Element(child) = child {
                walk(txn, &child, out);
            }
        }
    }
    let txn = doc.transact();
    let fragment = txn.get_xml_fragment("content").unwrap();
    let mut out = Vec::new();
    for child in fragment.children(&txn) {
        if let XmlOut::Element(child) = child {
            walk(&txn, &child, &mut out);
        }
    }
    out
}

fn expected_items() -> Vec<(bool, String)> {
    [
        (false, "First Item"),
        (true, "Second Item"),
        (false, "Third Item"),
    ]
    .into_iter()
    .map(|(checked, text)| (checked, text.to_string()))
    .collect()
}

#[test]
fn supplied_markdown_keeps_task_state_and_text_across_snapshot_and_roundtrips() {
    let mut doc = from_markdown(SAMPLE);
    for _ in 0..3 {
        let restored = snapshot::deserialize(&snapshot::doc_to_bytes(&doc)).unwrap();
        let native = restored.inner();
        assert!(schema_violations(native).is_empty());
        assert_eq!(task_items(native), expected_items());
        let html = export::to_html(native);
        assert_eq!(html.matches("data-checked=\"true\"").count(), 1);
        assert_eq!(html.matches("data-checked=\"false\"").count(), 2);
        assert_eq!(
            task_items(&ogrenotes_collab::import_quip::from_quip_html(&html).doc),
            expected_items(),
            "HTML must retain each label paired with its own checked state"
        );
        let markdown = export::to_markdown(native);
        for text in [
            "# Checked Unchecked",
            "This is markdown:",
            "The second item is checked",
            "* [ ] First Item",
            "* [x] Second Item",
            "* [ ] Third Item",
        ] {
            assert!(markdown.contains(text), "{markdown}");
        }
        doc = from_markdown(&markdown);
    }
}

#[test]
fn native_task_state_exports_to_markdown_and_reimports() {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        let list = fragment.insert(&mut txn, 0, XmlElementPrelim::empty("task_list"));
        for (checked, text) in expected_items() {
            let position = list.len(&txn);
            let item = list.insert(&mut txn, position, XmlElementPrelim::empty("task_item"));
            item.insert_attribute(&mut txn, "checked", checked.to_string());
            let paragraph = item.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
            paragraph.insert(&mut txn, 0, XmlTextPrelim::new(text));
        }
    }
    let markdown = export::to_markdown(&doc);
    assert!(markdown.contains("* [x] Second Item"), "{markdown}");
    let restored = from_markdown(&markdown);
    assert!(schema_violations(&restored).is_empty());
    assert_eq!(task_items(&restored), expected_items());
}

#[test]
fn nested_checklists_do_not_convert_outer_bullets_or_lose_text() {
    let mut doc = from_markdown("- outer\n  - [x] inner\n- sibling\n\nAfter.");
    for _ in 0..3 {
        assert!(schema_violations(&doc).is_empty());
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(list) = fragment.get(&txn, 0).unwrap() else {
            panic!("list")
        };
        assert_eq!(list.tag().as_ref(), "bullet_list");
        drop(txn);
        assert_eq!(task_items(&doc), vec![(true, "inner".to_string())]);
        let markdown = export::to_markdown(&doc);
        for text in ["outer", "sibling", "After.", "* [x] inner"] {
            assert!(markdown.contains(text), "{markdown}");
        }
        doc = from_markdown(&markdown);
    }
}

#[test]
fn mixed_and_loose_task_items_preserve_each_state() {
    for source in [
        "- first\n- [X] second\n- [ ] third",
        "- first\n\n- [X] second\n\n- [ ] third",
        "1. [ ] first\n2. [x] second\n3. third",
    ] {
        let doc = from_markdown(source);
        assert!(schema_violations(&doc).is_empty(), "{source}");
        assert_eq!(
            task_items(&doc),
            [(false, "first"), (true, "second"), (false, "third")]
                .into_iter()
                .map(|(c, t)| (c, t.to_string()))
                .collect::<Vec<_>>(),
            "{source}"
        );
        assert_eq!(
            task_items(&from_markdown(&export::to_markdown(&doc))),
            task_items(&doc)
        );
    }
}

#[test]
fn plain_lists_and_literal_markers_keep_their_text() {
    for source in [
        "- one\n- two",
        "1. one\n2. two",
        "- \\[x] literal\n- `[x]` code",
        "- \\[ ] unchecked literal\n- \\[X] uppercase literal",
    ] {
        let mut doc = from_markdown(source);
        let text = export::to_plain_text(&doc);
        let expected_kind = if source.starts_with("1.") {
            "ordered_list"
        } else {
            "bullet_list"
        };
        for _ in 0..3 {
            assert!(schema_violations(&doc).is_empty(), "{source}");
            assert!(task_items(&doc).is_empty(), "{source}");
            assert_eq!(export::to_plain_text(&doc), text);
            let txn = doc.transact();
            let fragment = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(list) = fragment.get(&txn, 0).unwrap() else {
                panic!("list")
            };
            assert_eq!(list.tag().as_ref(), expected_kind);
            drop(txn);
            doc = from_markdown(&export::to_markdown(&doc));
        }
    }
}

#[test]
fn continuation_paragraphs_and_ordered_parent_nesting_survive_roundtrips() {
    for source in [
        "- [x] first\n\n  second paragraph\n\n- [ ] third",
        "1. outer\n   - [x] inner\n2. sibling",
        "- [x] outer\n  - [ ] inner\n- [ ] sibling",
        "- [x] \n  - [ ] nested\n- [ ] sibling",
        "- [x] a\n\n* bullet",
        "- bullet\n\n* [x] task",
        "> - [x] first\n>\n>   continuation\n> - [ ] second",
        "- [x] first\n\n  > quoted paragraph\n\n- [ ] second",
    ] {
        let mut doc = from_markdown(source);
        let original = {
            let txn = doc.transact();
            txn.get_xml_fragment("content").unwrap().get_string(&txn)
        };
        assert!(original.contains("task_item"), "{original}");
        for _ in 0..3 {
            assert!(schema_violations(&doc).is_empty());
            let txn = doc.transact();
            assert_eq!(
                txn.get_xml_fragment("content").unwrap().get_string(&txn),
                original,
                "{source}"
            );
            drop(txn);
            doc = from_markdown(&export::to_markdown(&doc));
        }
    }
}
