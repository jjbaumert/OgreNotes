// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Structural validity of a materialized document against the canonical
//! schema. Generalizes the Quip importer's private `assert_valid_tree`:
//! every importer and transform must leave a tree this function accepts,
//! because downstream an invalid tree is document corruption — content
//! stranded outside any textblock is unreachable by `find_block_at` and
//! surfaces as undeletable fragments (the orphan-container bug class).

use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut};
use yrs::{Doc, ReadTxn, Transact};

use crate::schema::NodeType;

/// Every structural rule the document tree violates, as human-readable
/// strings with a path. Empty means valid.
///
/// Rules:
/// 1. Every element tag names a `NodeType`.
/// 2. A child element is legal under its parent per `valid_children`,
///    with the inline exemption `insert_block` also makes (`valid_children`
///    is a block-containment predicate and never lists inline leaves).
/// 3. A leaf node has no children.
/// 4. Raw text may appear only inside a textblock (a non-leaf node whose
///    `valid_children` is empty: Paragraph, Heading, CodeBlock).
pub fn schema_violations(doc: &Doc) -> Vec<String> {
    let txn = doc.transact();
    let mut out = Vec::new();
    let Some(frag) = txn.get_xml_fragment("content") else {
        return out;
    };
    for i in 0..frag.len(&txn) {
        if let Some(XmlOut::Element(el)) = frag.get(&txn, i) {
            walk(&txn, &el, NodeType::Doc, &format!("content[{i}]"), &mut out);
        }
    }
    out
}

fn is_textblock(nt: NodeType) -> bool {
    !nt.is_leaf() && nt.valid_children().is_empty() && !nt.is_inline()
}

fn walk<T: ReadTxn>(
    txn: &T,
    el: &XmlElementRef,
    parent: NodeType,
    path: &str,
    out: &mut Vec<String>,
) {
    let tag = el.tag().to_string();
    let Some(nt) = NodeType::from_tag(&tag) else {
        out.push(format!("{path}: unknown tag <{tag}>"));
        return;
    };
    if !(nt.is_inline() || parent.valid_children().contains(&nt)) {
        out.push(format!("{path}: {nt:?} is not a legal child of {parent:?}"));
    }
    let len = el.len(txn);
    if nt.is_leaf() && len > 0 {
        out.push(format!("{path}: leaf {nt:?} has {len} children"));
    }
    for i in 0..len {
        let child_path = format!("{path}/{tag}[{i}]");
        match el.get(txn, i) {
            Some(XmlOut::Element(child)) => walk(txn, &child, nt, &child_path, out),
            Some(XmlOut::Text(_)) => {
                if !is_textblock(nt) {
                    out.push(format!("{child_path}: raw text inside {nt:?}"));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
    use yrs::WriteTxn;

    fn doc_with(build: impl FnOnce(&mut yrs::TransactionMut, &yrs::XmlFragmentRef)) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            build(&mut txn, &frag);
        }
        doc
    }

    #[test]
    fn paragraph_with_text_is_valid() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty("paragraph"));
            p.insert(txn, 0, XmlTextPrelim::new("hi"));
        });
        assert!(schema_violations(&doc).is_empty());
    }

    #[test]
    fn bare_text_inside_a_list_is_reported() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty("bullet_list"));
            ul.insert(txn, 0, XmlTextPrelim::new("orphan"));
        });
        let v = schema_violations(&doc);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("raw text inside BulletList"), "{v:?}");
    }

    #[test]
    fn paragraph_directly_under_a_list_is_reported() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty("bullet_list"));
            ul.insert(txn, 0, XmlElementPrelim::empty("paragraph"));
        });
        let v = schema_violations(&doc);
        assert!(
            v.iter().any(|s| s.contains("Paragraph is not a legal child of BulletList")),
            "{v:?}"
        );
    }

    #[test]
    fn unknown_tag_and_leaf_with_children_are_reported() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty("marquee"));
            let hr = frag.insert(txn, 1, XmlElementPrelim::empty("horizontal_rule"));
            hr.insert(txn, 0, XmlTextPrelim::new("x"));
        });
        let v = schema_violations(&doc);
        assert!(v.iter().any(|s| s.contains("unknown tag <marquee>")), "{v:?}");
        assert!(
            v.iter().any(|s| s.contains("leaf HorizontalRule has 1 children")),
            "{v:?}"
        );
    }
}
