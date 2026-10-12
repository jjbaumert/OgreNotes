// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Bounded, iterative materialized-tree inspection using the canonical schema.
//! Unlike the importer validator, also examines root text and missing roots.

use ogrenotes_collab::{document::OgreDoc, schema::NodeType};
use serde::{Deserialize, Serialize};
use yrs::{
    Map, ReadTxn, Transact,
    types::xml::{XmlFragment, XmlOut},
};

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Violation {
    pub path: String,
    pub rule: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Check {
    pub status: String,
    pub complete: bool,
    pub unresolved_dependencies: bool,
    pub limit_hit: bool,
    pub nodes_visited: usize,
    pub violations: Vec<Violation>,
    pub error: Option<String>,
}

pub(super) struct Inspector {
    pub remaining_nodes: usize,
    pub max_depth: usize,
}

impl Inspector {
    pub fn check(&mut self, doc: &OgreDoc) -> Check {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.walk(doc)));
        result.unwrap_or_else(|_| Check::error("tree inspection panicked"))
    }

    fn walk(&mut self, doc: &OgreDoc) -> Check {
        let txn = doc.inner().transact();
        let pending = txn.store().pending_update().is_some() || txn.store().pending_ds().is_some();
        let mut check = Check {
            status: "valid".into(),
            complete: true,
            unresolved_dependencies: pending,
            limit_hit: false,
            nodes_visited: 0,
            violations: Vec::new(),
            error: None,
        };
        // Encoded Yjs root types can be undefined until accessed. Do not infer
        // a persisted type mismatch from root_refs()/casting alone.
        // A fragment root has no map entries. Probe before the fragment
        // accessor restores its type hint; root type tags alone are not
        // recoverable reliably from a Yjs full-state encoding.
        if txn.get_map("content").is_some_and(|map| map.len(&txn) > 0) {
            check.add("content".into(), "map_entries_in_content_root".into());
        }
        let Some(root) = txn.get_xml_fragment("content") else {
            check.add("content".into(), "missing_content_root".into());
            return check.finish();
        };
        let mut stack = Vec::new();
        for i in (0..root.len(&txn)).rev() {
            if stack.len() >= self.remaining_nodes {
                return check.limit("node budget exceeded");
            }
            stack.push((
                root.get(&txn, i),
                Some(NodeType::Doc),
                format!("content[{i}]"),
                1,
            ));
        }
        while let Some((child, parent, path, depth)) = stack.pop() {
            if self.remaining_nodes == 0 {
                return check.limit("cumulative node budget exceeded");
            }
            self.remaining_nodes -= 1;
            check.nodes_visited += 1;
            if depth > self.max_depth {
                return check.limit("tree depth budget exceeded");
            }
            match child {
                Some(XmlOut::Element(el)) => {
                    let tag = el.tag();
                    let node = NodeType::from_tag(tag);
                    match node {
                        None => check.add(path.clone(), "unknown_node_tag".into()),
                        Some(nt) => {
                            // Inline atoms belong inside textblocks. The generic
                            // importer validator's inline exemption is broader.
                            if let Some(parent) = parent {
                                let textblock = !parent.is_leaf()
                                    && !parent.is_inline()
                                    && parent.valid_children().is_empty();
                                if !(parent.valid_children().contains(&nt)
                                    || (textblock && nt.is_inline()))
                                {
                                    check.add(
                                        path.clone(),
                                        format!("illegal_child:{nt:?}:parent:{parent:?}"),
                                    );
                                }
                            }
                            if nt.is_leaf() && el.len(&txn) != 0 {
                                check.add(path.clone(), format!("leaf_has_children:{nt:?}"));
                            }
                        }
                    }
                    for i in (0..el.len(&txn)).rev() {
                        if stack.len() >= self.remaining_nodes {
                            return check.limit("node budget exceeded");
                        }
                        // Position-only paths cannot reveal tags or text.
                        stack.push((el.get(&txn, i), node, format!("{path}/{i}"), depth + 1));
                    }
                }
                Some(XmlOut::Text(_)) => {
                    if !parent.is_some_and(|nt| {
                        !nt.is_leaf() && !nt.is_inline() && nt.valid_children().is_empty()
                    }) {
                        check.add(path, "raw_text_outside_textblock".into());
                    }
                }
                Some(_) => check.add(path, "unsupported_xml_child".into()),
                None => check.add(path, "unreadable_xml_child".into()),
            }
            if check.violations.len() >= 100 {
                return check.limit("violation report budget exceeded");
            }
        }
        check.finish()
    }
}

impl Check {
    fn add(&mut self, path: String, rule: String) {
        self.violations.push(Violation { path, rule });
    }
    fn finish(mut self) -> Self {
        self.status = if self.unresolved_dependencies {
            "inconclusive"
        } else if !self.violations.is_empty() {
            "invalid"
        } else {
            "valid"
        }
        .into();
        if self.unresolved_dependencies {
            self.complete = false;
            self.error = Some("unresolved CRDT predecessors or deletes".into());
        }
        self
    }
    fn limit(mut self, error: &str) -> Self {
        self.complete = false;
        self.limit_hit = true;
        self.error = Some(error.into());
        self.status = if self.violations.is_empty() || self.unresolved_dependencies {
            "inconclusive"
        } else {
            "invalid"
        }
        .into();
        self
    }
    pub fn error(error: &str) -> Self {
        Self {
            status: "error".into(),
            complete: false,
            unresolved_dependencies: false,
            limit_hit: false,
            nodes_visited: 0,
            violations: Vec::new(),
            error: Some(error.into()),
        }
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<OgreDoc, String> {
    std::panic::catch_unwind(|| OgreDoc::from_state_bytes(bytes))
        .map_err(|_| "snapshot decoding/application panicked".to_string())?
        .map_err(|_| "snapshot decoding/application failed".into())
}

pub(super) fn apply(doc: &mut OgreDoc, bytes: &[u8]) -> Result<(), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| doc.apply_update(bytes)))
        .map_err(|_| "update decoding/application panicked".to_string())?
        .map_err(|_| "update decoding/application failed".into())
}
