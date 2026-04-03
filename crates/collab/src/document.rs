use yrs::{
    Doc, ReadTxn, Transact, WriteTxn,
    Update,
    updates::decoder::Decode,
    updates::encoder::Encode,
    types::xml::{XmlElementPrelim, XmlFragment},
};

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
    pub fn apply_update(&mut self, update_bytes: &[u8]) -> Result<(), DocError> {
        let mut txn = self.doc.transact_mut();
        let update =
            Update::decode_v1(update_bytes).map_err(|e| DocError::ApplyUpdate(e.to_string()))?;
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
}
