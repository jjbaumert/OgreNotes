// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! One-shot handoff of the deck the editor is showing to present mode (#209).
//!
//! Present mode used to fetch the deck over REST on mount. Clicking
//! **Present** right after an edit navigates in-app while the editor is still
//! persisting that edit over its WebSocket, so the REST read could observe a
//! partially persisted document (an extra empty slide, about half the time).
//!
//! The editor's in-memory document is exactly what the author was looking
//! at, so the Present button stashes it here and present mode takes it on
//! mount. It is keyed by document id and consumed on first read: a direct
//! load or a refresh of the present URL finds nothing and falls back to
//! REST, and a stale stash for another document is never used.

use std::cell::RefCell;

use crate::editor::model::Node;

thread_local! {
    static STASH: RefCell<Option<(String, Node)>> = const { RefCell::new(None) };
}

/// Leave `doc` for present mode to pick up for `doc_id`. Replaces any
/// earlier stash.
pub fn stash(doc_id: &str, doc: Node) {
    STASH.with(|s| *s.borrow_mut() = Some((doc_id.to_string(), doc)));
}

/// Take the stashed document for `doc_id`, if the stash is for it. The stash
/// is cleared either way, so it can only ever be used once and only right
/// after the navigation that left it.
pub fn take(doc_id: &str) -> Option<Node> {
    STASH
        .with(|s| s.borrow_mut().take())
        .and_then(|(id, doc)| (id == doc_id).then_some(doc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, NodeType};

    fn doc() -> Node {
        Node::element_with_content(NodeType::Doc, Fragment::empty())
    }

    #[test]
    fn take_returns_the_stash_for_its_document_once() {
        stash("d1", doc());
        assert!(take("d1").is_some());
        assert!(take("d1").is_none(), "a handoff is consumed on first read");
    }

    #[test]
    fn take_ignores_and_clears_a_stash_for_another_document() {
        stash("d1", doc());
        assert!(take("d2").is_none(), "never present another document's deck");
        assert!(take("d1").is_none(), "the mismatched stash is cleared, not kept");
    }
}
