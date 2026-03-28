use crate::document::{DocError, OgreDoc};

/// Serialize a document to snapshot bytes.
pub fn serialize(doc: &OgreDoc) -> Vec<u8> {
    doc.to_state_bytes()
}

/// Deserialize snapshot bytes into a document.
pub fn deserialize(bytes: &[u8]) -> Result<OgreDoc, DocError> {
    OgreDoc::from_state_bytes(bytes)
}

/// Apply a list of pending update blobs on top of a snapshot.
pub fn apply_pending_updates(doc: &mut OgreDoc, updates: &[Vec<u8>]) -> Result<(), DocError> {
    for update in updates {
        doc.apply_update(update)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{get_or_insert_content_fragment, OgreDoc};
    use yrs::types::xml::{XmlFragment as _, XmlOut, XmlTextPrelim};
    use yrs::Transact;

    #[test]
    fn snapshot_empty_doc() {
        let doc = OgreDoc::new();
        let bytes = serialize(&doc);
        let restored = deserialize(&bytes).unwrap();
        assert_eq!(serialize(&restored), bytes);
    }

    #[test]
    fn snapshot_roundtrip_is_deterministic() {
        let doc = OgreDoc::new();
        let bytes1 = serialize(&doc);
        let bytes2 = serialize(&doc);
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn snapshot_with_pending_updates() {
        // Create a doc, serialize as snapshot, then make a change and capture the update
        let doc = OgreDoc::new();

        // Take a snapshot of the initial state
        let base_bytes = serialize(&doc);

        // Make a change on the same doc
        {
            let mut txn = doc.inner().transact_mut();
            let fragment = get_or_insert_content_fragment(&mut txn);
            if let Some(XmlOut::Element(para)) = fragment.get(&txn, 0) {
                para.insert(&mut txn, 0, XmlTextPrelim::new("test"));
            }
        }

        // Compute the diff from the base state to the current state
        let mut base_restored = deserialize(&base_bytes).unwrap();
        let base_sv = base_restored.state_vector();
        let update = doc.encode_diff(&base_sv).unwrap();

        // Apply the update on top of the base snapshot
        apply_pending_updates(&mut base_restored, &[update]).unwrap();

        // The restored doc should now have the same state as the original
        // (same client ID since the diff was computed from the same doc)
        assert_eq!(serialize(&base_restored), serialize(&doc));
    }

    #[test]
    fn invalid_snapshot_rejected() {
        let result = deserialize(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_snapshot_roundtrip(_seed in 0u64..1000) {
            let doc = OgreDoc::new();
            let bytes = serialize(&doc);
            let restored = deserialize(&bytes).unwrap();
            prop_assert_eq!(serialize(&restored), bytes);
        }
    }
}
