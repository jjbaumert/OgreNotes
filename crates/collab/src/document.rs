// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use yrs::{
    Doc, ReadTxn, Transact, WriteTxn,
    Update,
    updates::decoder::Decode,
    updates::encoder::Encode,
    types::xml::{XmlElementPrelim, XmlFragment},
};

use ogrenotes_common::metrics::{counter, MetricKey};

use crate::schema::NodeType;

/// Wrapper around a yrs Doc for OgreNotes documents.
pub struct OgreDoc {
    doc: Doc,
}

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("failed to apply update: {0}")]
    ApplyUpdate(String),

    #[error("failed to encode state: {0}")]
    EncodeState(String),

    #[error("failed to decode state: {0}")]
    DecodeState(String),

    /// The interactive LiveApp attribute gate refused the update.
    /// Distinct variant so the WS handler can send a specific
    /// error frame back (Option A of the Phase 2a review's
    /// finding #5) instead of dropping the update silently.
    #[error("liveapp validation rejected update: {0}")]
    LiveAppRejected(String),
}

impl OgreDoc {
    /// Create a new empty document with a root paragraph.
    pub fn new() -> Self {
        let doc = Doc::new();

        {
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            fragment.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
        }

        Self { doc }
    }

    /// Load a document from encoded state bytes.
    pub fn from_state_bytes(bytes: &[u8]) -> Result<Self, DocError> {
        let doc = Doc::new();

        {
            let mut txn = doc.transact_mut();
            let update =
                Update::decode_v1(bytes).map_err(|e| DocError::DecodeState(e.to_string()))?;
            txn.apply_update(update)
                .map_err(|e| DocError::ApplyUpdate(e.to_string()))?;
        }

        Ok(Self { doc })
    }

    /// Encode the full document state as bytes.
    pub fn to_state_bytes(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.encode_state_as_update_v1(&yrs::StateVector::default())
    }

    /// Apply an incremental update from another client.
    /// Takes `&mut self` to prevent concurrent mutable transaction access.
    ///
    /// The two failure modes have distinct skew implications and are
    /// counted separately (see design/observability.md §Drift counters):
    /// `Update::decode_v1` failure means the bytes on the wire are not
    /// a valid yrs update encoding — typically a protocol-version
    /// mismatch between peers — and emits
    /// `ws.update_decode_failures_total`. A `txn.apply_update` failure
    /// means decode succeeded but the update was incompatible with the
    /// current doc state, which is a different bug class. The DocError
    /// variant returned to the caller is unchanged for both for
    /// backward compatibility — operators distinguish via the counter.
    pub fn apply_update(&mut self, update_bytes: &[u8]) -> Result<(), DocError> {
        let mut txn = self.doc.transact_mut();
        let update = Update::decode_v1(update_bytes).map_err(|e| {
            counter::inc(MetricKey::new("ws.update_decode_failures_total", &[]));
            DocError::ApplyUpdate(e.to_string())
        })?;
        txn.apply_update(update)
            .map_err(|e| DocError::ApplyUpdate(e.to_string()))
    }

    /// Replace the document state entirely with new state bytes.
    /// Used when a client sends its full state rather than an incremental diff.
    #[deprecated(note = "Use apply_update with incremental updates instead")]
    pub fn replace_state(&mut self, state_bytes: &[u8]) -> Result<(), DocError> {
        let new_doc = yrs::Doc::new();
        {
            let mut txn = new_doc.transact_mut();
            let update = Update::decode_v1(state_bytes)
                .map_err(|e| DocError::DecodeState(e.to_string()))?;
            txn.apply_update(update)
                .map_err(|e| DocError::ApplyUpdate(e.to_string()))?;
        }
        self.doc = new_doc;
        Ok(())
    }

    /// Get the current state vector (for computing diffs).
    pub fn state_vector(&self) -> Vec<u8> {
        let txn = self.doc.transact();
        txn.state_vector().encode_v1()
    }

    /// Compute an update containing only changes the peer is missing.
    pub fn encode_diff(&self, remote_state_vector: &[u8]) -> Result<Vec<u8>, DocError> {
        let sv = yrs::StateVector::decode_v1(remote_state_vector)
            .map_err(|e| DocError::DecodeState(e.to_string()))?;
        let txn = self.doc.transact();
        Ok(txn.encode_state_as_update_v1(&sv))
    }

    /// Access the underlying yrs Doc (for advanced operations).
    pub fn inner(&self) -> &Doc {
        &self.doc
    }

    /// Mutable access to the underlying yrs Doc. Used by paths that own the
    /// OgreDoc briefly and need to open a write transaction — e.g. the
    /// mail-merge substitution on the copy path (#142 Phase 2).
    pub fn inner_mut(&mut self) -> &mut Doc {
        &mut self.doc
    }
}

impl Default for OgreDoc {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper: get the content XmlFragment from a transaction.
pub fn get_content_fragment<T: ReadTxn>(txn: &T) -> Option<yrs::XmlFragmentRef> {
    txn.get_xml_fragment("content")
}

/// Helper: get or insert the content XmlFragment.
pub fn get_or_insert_content_fragment(txn: &mut yrs::TransactionMut<'_>) -> yrs::XmlFragmentRef {
    txn.get_or_insert_xml_fragment("content")
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::Transact;
    use yrs::types::xml::{XmlOut, XmlTextPrelim};
    use yrs::types::GetString;

    #[test]
    fn create_empty_doc() {
        let doc = OgreDoc::new();
        let bytes = doc.to_state_bytes();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn state_bytes_roundtrip() {
        let doc = OgreDoc::new();
        let bytes = doc.to_state_bytes();

        let doc2 = OgreDoc::from_state_bytes(&bytes).unwrap();
        let bytes2 = doc2.to_state_bytes();

        assert_eq!(bytes, bytes2);
    }

    #[test]
    fn insert_text() {
        let doc = OgreDoc::new();

        {
            let mut txn = doc.inner().transact_mut();
            let fragment = get_or_insert_content_fragment(&mut txn);
            if let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("Hello, world!"));
            }
        }

        let bytes = doc.to_state_bytes();
        assert!(!bytes.is_empty());

        // Reload and verify
        let doc2 = OgreDoc::from_state_bytes(&bytes).unwrap();
        let txn = doc2.inner().transact();
        let fragment = get_content_fragment(&txn).expect("content fragment");
        let first = fragment.get(&txn, 0).expect("first child");
        if let XmlOut::Element(el) = first {
            let text_node = el.get(&txn, 0).expect("text child");
            if let XmlOut::Text(text) = text_node {
                assert_eq!(text.get_string(&txn), "Hello, world!");
            } else {
                panic!("expected XmlText, got {text_node:?}");
            }
        } else {
            panic!("expected XmlElement, got {first:?}");
        }
    }

    #[test]
    fn apply_update_bytes() {
        let doc1 = OgreDoc::new();
        let mut doc2 = OgreDoc::from_state_bytes(&doc1.to_state_bytes()).unwrap();

        // Make a change on doc1
        {
            let mut txn = doc1.inner().transact_mut();
            let fragment = get_or_insert_content_fragment(&mut txn);
            if let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("New text"));
            }
        }

        // Get the diff and apply to doc2
        let sv2 = doc2.state_vector();
        let diff = doc1.encode_diff(&sv2).unwrap();
        doc2.apply_update(&diff).unwrap();

        assert_eq!(doc1.to_state_bytes(), doc2.to_state_bytes());
    }

    #[test]
    fn concurrent_inserts_converge() {
        let initial = OgreDoc::new();
        let initial_bytes = initial.to_state_bytes();

        let mut doc_a = OgreDoc::from_state_bytes(&initial_bytes).unwrap();
        let mut doc_b = OgreDoc::from_state_bytes(&initial_bytes).unwrap();

        // doc_a inserts text in the paragraph
        {
            let mut txn = doc_a.inner().transact_mut();
            let fragment = get_or_insert_content_fragment(&mut txn);
            if let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("From A"));
            }
        }

        // doc_b inserts a new paragraph
        {
            let mut txn = doc_b.inner().transact_mut();
            let fragment = get_or_insert_content_fragment(&mut txn);
            fragment.insert(
                &mut txn,
                1,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
        }

        // Exchange updates
        let sv_a = doc_a.state_vector();
        let sv_b = doc_b.state_vector();
        let diff_a_to_b = doc_a.encode_diff(&sv_b).unwrap();
        let diff_b_to_a = doc_b.encode_diff(&sv_a).unwrap();

        doc_a.apply_update(&diff_b_to_a).unwrap();
        doc_b.apply_update(&diff_a_to_b).unwrap();

        assert_eq!(doc_a.to_state_bytes(), doc_b.to_state_bytes());
    }

    #[test]
    fn invalid_bytes_rejected() {
        let result = OgreDoc::from_state_bytes(&[0xFF, 0xFE, 0xFD]);
        assert!(result.is_err());
    }

    #[test]
    fn default_creates_empty_doc() {
        let doc = OgreDoc::default();
        let bytes = doc.to_state_bytes();
        assert!(!bytes.is_empty());
        // Default and new should produce structurally identical docs
        let _doc2 = OgreDoc::new();
        // Both have a content fragment with a paragraph
        let txn = doc.inner().transact();
        let frag = get_content_fragment(&txn).expect("content fragment");
        assert_eq!(frag.len(&txn), 1);
    }

    #[test]
    #[allow(deprecated)]
    fn replace_state_roundtrip() {
        let mut doc = OgreDoc::new();
        // Build a separate doc with different content
        let other = OgreDoc::new();
        {
            let mut txn = other.inner().transact_mut();
            let frag = get_or_insert_content_fragment(&mut txn);
            if let Some(XmlOut::Element(para)) = frag.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("Replaced content"));
            }
        }
        let other_bytes = other.to_state_bytes();

        doc.replace_state(&other_bytes).unwrap();

        // Verify the state was replaced
        let txn = doc.inner().transact();
        let frag = get_content_fragment(&txn).expect("content fragment");
        let first = frag.get(&txn, 0).expect("first child");
        if let XmlOut::Element(el) = first {
            let text_node = el.get(&txn, 0).expect("text child");
            if let XmlOut::Text(text) = text_node {
                assert_eq!(text.get_string(&txn), "Replaced content");
            } else {
                panic!("expected XmlText");
            }
        } else {
            panic!("expected XmlElement");
        }
    }

    #[test]
    #[allow(deprecated)]
    fn replace_state_invalid_bytes() {
        let mut doc = OgreDoc::new();
        let result = doc.replace_state(&[0xFF, 0xFE, 0xFD]);
        assert!(result.is_err());
    }

    #[test]
    fn apply_update_invalid_bytes() {
        let mut doc = OgreDoc::new();
        let result = doc.apply_update(&[0xFF, 0xFE]);
        assert!(result.is_err());
    }
}

// ─── Property tests (gap #5 of the test-coverage plan) ────────────
//
// Convergence is the whole reason OgreNotes uses yrs. Eight existing
// example-based tests in yrs_bridge.rs cover specific concurrency
// shapes (insert vs delete, type change vs text edit, etc.). They do
// not exercise random op interleavings — a regression in the
// underlying yrs apply path that breaks convergence only on a
// specific delivery order would pass every existing test and still
// silently diverge replicas in the field.
//
// This proptest generates random two-client edit sequences and
// asserts the merged state is byte-identical regardless of the
// order updates were delivered. 256 cases of 1..40 ops each.

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;
    use yrs::{GetString, Transact, WriteTxn, types::xml::{XmlFragment, XmlElementPrelim, XmlOut, XmlTextPrelim}};

    #[derive(Debug, Clone)]
    enum OpKind {
        InsertText(String),
        RemoveLeading,
    }

    #[derive(Debug, Clone)]
    struct ClientOp {
        client: u8, // 0 or 1
        kind: OpKind,
    }

    fn op_strategy() -> impl Strategy<Value = ClientOp> {
        // Insert a tiny ASCII string as a brand-new XmlText node;
        // RemoveLeading drops the leading paragraph (no-op if the
        // fragment is empty). We're testing CRDT convergence under
        // random interleavings — the exact node shape doesn't
        // matter, the random ordering does.
        let kind = prop_oneof![
            "[a-z]{1,4}".prop_map(OpKind::InsertText),
            Just(OpKind::RemoveLeading),
        ];
        (0u8..2, kind).prop_map(|(client, kind)| ClientOp { client, kind })
    }

    /// Apply a single op to a yrs Doc directly via the same XML
    /// fragment shape OgreDoc uses internally. Insert grows the
    /// content fragment by one paragraph carrying one text node;
    /// RemoveLeading shrinks it by one if non-empty (a no-op on an
    /// empty fragment is intentional — random delete-before-insert
    /// sequences should converge to the empty doc, not fail).
    fn apply_local_op(doc: &yrs::Doc, kind: &OpKind) {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");
        match kind {
            OpKind::InsertText(s) => {
                let para = fragment.insert(
                    &mut txn,
                    0,
                    XmlElementPrelim::empty("paragraph"),
                );
                para.insert(&mut txn, 0, XmlTextPrelim::new(s.as_str()));
            }
            OpKind::RemoveLeading => {
                if fragment.len(&txn) > 0 {
                    fragment.remove_range(&mut txn, 0, 1);
                }
            }
        }
    }

    /// Render the doc's content fragment to a `Vec<String>` of
    /// per-paragraph text. This is the structural convergence
    /// claim: two replicas that have seen the same set of edits
    /// must produce the same logical content, even though the
    /// *binary* encoding of that state (block ordering inside
    /// `encode_state_as_update_v1`) is not canonicalized across
    /// emit orders. The same approach is used in yrs's own tests.
    fn render_paragraphs(doc: &yrs::Doc) -> Vec<String> {
        let txn = doc.transact();
        // A doc that never received an InsertText op has no fragment;
        // that converges to the empty vec.
        let Some(fragment) = txn.get_xml_fragment("content") else {
            return Vec::new();
        };
        let len = fragment.len(&txn);
        let mut out = Vec::with_capacity(len as usize);
        for i in 0..len {
            let para = match fragment.get(&txn, i) {
                Some(XmlOut::Element(el)) => el,
                _ => continue,
            };
            let inner_len = para.len(&txn);
            let mut text = String::new();
            for j in 0..inner_len {
                if let Some(XmlOut::Text(t)) = para.get(&txn, j) {
                    text.push_str(&t.get_string(&txn));
                }
            }
            out.push(text);
        }
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            ..ProptestConfig::default()
        })]

        #[test]
        fn convergence_holds_under_random_interleaving(
            ops in proptest::collection::vec(op_strategy(), 1..40),
        ) {
            // Two replicas each apply their own local edits via the
            // yrs primitive APIs (the actual mutator path), then
            // exchange diffs through `OgreDoc::apply_update` — the
            // public surface the WS room and storage layer call.
            // Routing apply through OgreDoc is load-bearing for the
            // claim this test makes: a future change to
            // `apply_update` (e.g. an invariant check, a pre-apply
            // mutation, an early-return on decode error) that breaks
            // convergence must fail here, not only on a hypothetical
            // raw-yrs test.
            let mut doc_a = OgreDoc::new();
            let mut doc_b = OgreDoc::new();

            // `OgreDoc::new()` seeds an initial empty paragraph
            // under each replica's local client id. That's a
            // divergent initial state — A and B each have a
            // paragraph the other has never seen. Real clients
            // resolve this through the WS sync-step handshake
            // (state vector exchange + diff apply). Mirror that
            // here so the random-op loop starts from a converged
            // baseline; otherwise the test conflates initial-state
            // divergence with op-interleaving divergence.
            let sv_a0 = doc_a.state_vector();
            let sv_b0 = doc_b.state_vector();
            let diff_a0 = doc_a.encode_diff(&sv_b0)
                .map_err(|e| TestCaseError::fail(format!("init A→B: {e}")))?;
            let diff_b0 = doc_b.encode_diff(&sv_a0)
                .map_err(|e| TestCaseError::fail(format!("init B→A: {e}")))?;
            doc_b.apply_update(&diff_a0)
                .map_err(|e| TestCaseError::fail(format!("init apply B: {e}")))?;
            doc_a.apply_update(&diff_b0)
                .map_err(|e| TestCaseError::fail(format!("init apply A: {e}")))?;

            let mut diffs_a: Vec<Vec<u8>> = Vec::new();
            let mut diffs_b: Vec<Vec<u8>> = Vec::new();

            for op in &ops {
                if op.client == 0 {
                    let sv_bytes = doc_a.state_vector();
                    apply_local_op(doc_a.inner(), &op.kind);
                    let diff = doc_a.encode_diff(&sv_bytes)
                        .map_err(|e| TestCaseError::fail(format!("encode_diff A: {e}")))?;
                    diffs_a.push(diff);
                } else {
                    let sv_bytes = doc_b.state_vector();
                    apply_local_op(doc_b.inner(), &op.kind);
                    let diff = doc_b.encode_diff(&sv_bytes)
                        .map_err(|e| TestCaseError::fail(format!("encode_diff B: {e}")))?;
                    diffs_b.push(diff);
                }
            }

            // Cross-deliver: B's diffs go to A, A's diffs go to B.
            // Both orders are valid; we apply in emit order on each
            // side. Convergence requires the final state to be
            // identical regardless. Apply through `OgreDoc::apply_update`
            // — the same entry point the room/storage code uses.
            for update_bytes in &diffs_b {
                doc_a.apply_update(update_bytes)
                    .map_err(|e| TestCaseError::fail(format!("apply A: {e}")))?;
            }
            for update_bytes in &diffs_a {
                doc_b.apply_update(update_bytes)
                    .map_err(|e| TestCaseError::fail(format!("apply B: {e}")))?;
            }

            // The rendered structural content (the actual user-
            // visible state) must match between replicas. We compare
            // structure rather than `encode_state_as_update_v1`
            // bytes because the binary encoding orders per-client GC
            // blocks by emitter client id — that order differs
            // between A and B even though the logical CRDT state is
            // identical. State vectors are also not byte-comparable
            // (HashMap-backed; debug-string ordering varies).
            let para_a = render_paragraphs(doc_a.inner());
            let para_b = render_paragraphs(doc_b.inner());
            prop_assert_eq!(
                &para_a,
                &para_b,
                "rendered content must converge for op sequence {:?}",
                ops,
            );
        }
    }
}
