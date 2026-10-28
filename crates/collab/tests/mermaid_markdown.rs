//! Imported diagrams remain editable leaf nodes through real Yrs snapshots.
use ogrenotes_collab::{export, import::from_markdown, snapshot, validate::schema_violations};
use yrs::{ReadTxn, Transact, Xml, XmlFragment, XmlOut};

#[test]
fn markdown_mermaid_survives_snapshot_export_and_reimport_as_a_leaf() {
    let source = "flowchart LR\nA[Imported] --> B[Editable]\n";
    let imported = from_markdown(&format!("Before.\n\n```mermaid\n{source}```\n\nAfter."));
    let document = snapshot::deserialize(&snapshot::doc_to_bytes(&imported)).unwrap();
    let restored = document.inner();
    assert!(schema_violations(restored).is_empty());
    let txn = restored.transact();
    let fragment = txn.get_xml_fragment("content").unwrap();
    assert_eq!(fragment.len(&txn), 3);
    let XmlOut::Element(node) = fragment.get(&txn, 1).unwrap() else {
        panic!("diagram element")
    };
    assert_eq!(node.tag().as_ref(), "mermaid");
    assert_eq!(node.len(&txn), 0, "a Mermaid node has no text children");
    assert_eq!(node.get_attribute(&txn, "source").as_deref(), Some(source));
    let block_id = node.get_attribute(&txn, "blockId").unwrap().to_string();
    assert_eq!(
        block_id.len(),
        10,
        "imported blocks must be addressable for editing"
    );
    drop(txn);
    let html = export::to_html(restored);
    assert!(html.contains("class=\"mermaid-block\"><svg"));
    assert!(html.contains("@media print"));
    assert!(html.contains("max-width:100%;height:auto"));
    let roundtrip = from_markdown(&export::to_markdown(restored));
    assert!(schema_violations(&roundtrip).is_empty());
    let txn = roundtrip.transact();
    let fragment = txn.get_xml_fragment("content").unwrap();
    let XmlOut::Element(node) = fragment.get(&txn, 1).unwrap() else {
        panic!("diagram");
    };
    assert_eq!(node.get_attribute(&txn, "source").as_deref(), Some(source));
    drop(txn);
    assert!(export::to_html(&roundtrip).contains("class=\"mermaid-block\"><svg"));
}

#[test]
fn empty_oversized_and_nested_fences_remain_safe_code_blocks() {
    for markdown in [
        "```mermaid\n```".to_string(),
        format!(
            "```mermaid\n{}\n```",
            "x".repeat(ogrenotes_mermaid::MAX_SOURCE_LEN + 1)
        ),
        "> ```mermaid\n> flowchart TD\n> A --> B\n> ```".to_string(),
        "- ```mermaid\n  flowchart TD\n  A --> B\n  ```".to_string(),
    ] {
        let doc = from_markdown(&markdown);
        assert!(schema_violations(&doc).is_empty(), "{markdown}");
        let html = export::to_html(&doc);
        assert!(!html.contains("class=\"mermaid-block\""));
        assert!(html.contains("<pre"));
    }
}

#[test]
fn source_cap_roundtrips_stay_live_and_byte_identical() {
    let prefix = "flowchart TD\nA --> B\n%%";
    for (character, final_newline) in [("x", false), ("é", true)] {
        let mut source = format!(
            "{prefix}{}",
            character.repeat(
                ogrenotes_mermaid::MAX_SOURCE_LEN
                    - prefix.chars().count()
                    - usize::from(final_newline)
            )
        );
        if final_newline {
            source.push('\n');
        }
        assert_eq!(source.chars().count(), ogrenotes_mermaid::MAX_SOURCE_LEN);
        let separator = if final_newline { "" } else { "\n" };
        let mut doc = from_markdown(&format!("```mermaid\n{source}{separator}```"));
        for _ in 0..3 {
            assert!(schema_violations(&doc).is_empty());
            let txn = doc.transact();
            let fragment = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(node) = fragment.get(&txn, 0).unwrap() else {
                panic!("diagram");
            };
            assert_eq!(node.tag().as_ref(), "mermaid");
            assert_eq!(node.len(&txn), 0);
            assert_eq!(
                node.get_attribute(&txn, "source").as_deref(),
                Some(source.as_str())
            );
            drop(txn);
            doc = from_markdown(&export::to_markdown(&doc));
        }
    }
}

#[test]
fn native_sources_use_stable_standard_markdown_fence_line_endings() {
    for source in [
        "flowchart TD\nA --> B",
        "flowchart TD\nA --> B\n",
        "flowchart TD\nA --> B\n\n",
    ] {
        let doc = from_markdown("```mermaid\nflowchart TD\nA --> B\n```");
        let mut txn = doc.transact_mut();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let XmlOut::Element(node) = fragment.get(&txn, 0).unwrap() else {
            panic!("diagram");
        };
        node.insert_attribute(&mut txn, "source", source);
        drop(txn);
        let expected = if source.ends_with('\n') {
            source.to_string()
        } else {
            format!("{source}\n")
        };
        let mut markdown = export::to_markdown(&doc);
        for _ in 0..3 {
            let imported = from_markdown(&markdown);
            let txn = imported.transact();
            let fragment = txn.get_xml_fragment("content").unwrap();
            let XmlOut::Element(node) = fragment.get(&txn, 0).unwrap() else {
                panic!("diagram");
            };
            assert_eq!(node.tag().as_ref(), "mermaid");
            assert_eq!(
                node.get_attribute(&txn, "source").as_deref(),
                Some(expected.as_str())
            );
            drop(txn);
            markdown = export::to_markdown(&imported);
        }
    }
}
