// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};

use super::{AccessLevel, DocType, LinkSharingMode, ViewOptions};

/// Document metadata stored in DynamoDB.
/// PK: DOC#<doc_id>, SK: METADATA
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentMeta {
    pub doc_id: String,
    pub title: String,
    pub owner_id: String,
    /// The folder this document belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// #149: additional folders this document is also in (multi-folder
    /// membership). `folder_id` stays the *primary*; the full membership set
    /// is `{folder_id} ∪ additional_folder_ids`. Stored sparsely (omitted when
    /// empty); legacy rows decode to empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_folder_ids: Vec<String>,
    /// The workspace this document belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub doc_type: DocType,
    pub snapshot_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_s3_key: Option<String>,
    pub is_deleted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
    /// Link sharing mode (None = disabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_sharing_mode: Option<LinkSharingMode>,
    /// View-mode sub-options for a `View`-mode link (comments, history,
    /// conversation, request-access). Ignored when the mode is `Edit`.
    /// Defaults to all-false; enforced at the feature endpoints (Phase 2).
    #[serde(default)]
    pub link_view_options: ViewOptions,
    /// #140: when true, all edit paths (REST `put_content`, WS `Update` /
    /// `SyncStep2`) are blocked for every user — a doc-wide freeze toggled by
    /// the owner. Distinct from sharing: it is not a per-user grant. Defaults
    /// to false; stored sparsely (the DynamoDB attribute is present only when
    /// true), so legacy rows decode to unlocked.
    #[serde(default)]
    pub locked: bool,
    /// #142: when true, this doc is a template — surfaced in the Template
    /// gallery and copyable via `POST /documents/:id/copy`. Stored sparsely
    /// alongside `locked` (attribute present only when true) so legacy rows
    /// decode to non-template.
    #[serde(default)]
    pub is_template: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl DocumentMeta {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk() -> &'static str {
        "METADATA"
    }

    /// S3 key for the current snapshot.
    pub fn snapshot_key(&self) -> String {
        format!("docs/{}/snapshots/{}.bin", self.doc_id, self.snapshot_version)
    }
}

/// Document membership for direct sharing.
/// PK: DOC#<doc_id>, SK: MEMBER#<user_id>
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocMember {
    pub doc_id: String,
    pub user_id: String,
    pub access_level: AccessLevel,
    pub added_at: i64,
}

impl DocMember {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("MEMBER#{}", self.user_id)
    }
}

/// Document open receipt — tracks the first time a user opens a document.
/// PK: DOC#<doc_id>, SK: OPEN#<user_id>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocOpen {
    pub doc_id: String,
    pub user_id: String,
    pub first_opened_at: i64,
}

impl DocOpen {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("OPEN#{}", self.user_id)
    }
}

/// A user's favorite (starred) document. A per-user marker that does NOT
/// move the doc — it stays in its folder. (#144)
/// PK: USER#<user_id>, SK: FAV#<doc_id>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Favorite {
    pub user_id: String,
    pub doc_id: String,
    pub added_at: i64,
}

impl Favorite {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("FAV#{}", self.doc_id)
    }
}

/// A user's named collection — a sub-group within Favorites. Per-user, like
/// [`Favorite`]; orthogonal to the star (membership doesn't set the star). (#144)
/// PK: USER#<user_id>, SK: COLLECTION#<collection_id>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collection {
    pub user_id: String,
    pub collection_id: String,
    pub name: String,
    pub created_at: i64,
}

impl Collection {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("COLLECTION#{}", self.collection_id)
    }

    /// SK prefix for querying all of a user's collections.
    pub const SK_PREFIX: &'static str = "COLLECTION#";
}

/// Membership of a document in a [`Collection`]. (#144)
/// PK: USER#<user_id>, SK: COLLITEM#<collection_id>#<doc_id>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionItem {
    pub user_id: String,
    pub collection_id: String,
    pub doc_id: String,
    pub added_at: i64,
}

impl CollectionItem {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("COLLITEM#{}#{}", self.collection_id, self.doc_id)
    }

    /// SK prefix for querying every membership row of one collection.
    pub fn sk_prefix(collection_id: &str) -> String {
        format!("COLLITEM#{collection_id}#")
    }

    /// SK prefix for querying every membership row across all collections (then
    /// filter by the `doc_id` attribute to find which collections hold a doc).
    pub const SK_PREFIX_ALL: &'static str = "COLLITEM#";
}

/// CRDT update log entry.
/// PK: DOC#<doc_id>, SK: UPDATE#<clock>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocUpdate {
    pub doc_id: String,
    pub clock: String,
    #[serde(with = "serde_bytes")]
    pub update_bytes: Vec<u8>,
    pub user_id: String,
    pub created_at: i64,
    /// Client version that produced this update (None for pre-versioning updates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
}

impl DocUpdate {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.doc_id)
    }

    pub fn sk(&self) -> String {
        format!("UPDATE#{}", self.clock)
    }
}

/// Type of relationship between two documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationType {
    Implements,
    DerivedFrom,
    DependsOn,
    References,
    Supersedes,
}

impl RelationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RelationType::Implements => "implements",
            RelationType::DerivedFrom => "derived-from",
            RelationType::DependsOn => "depends-on",
            RelationType::References => "references",
            RelationType::Supersedes => "supersedes",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "implements" => Some(RelationType::Implements),
            "derived-from" => Some(RelationType::DerivedFrom),
            "depends-on" => Some(RelationType::DependsOn),
            "references" => Some(RelationType::References),
            "supersedes" => Some(RelationType::Supersedes),
            _ => None,
        }
    }
}

/// Directed relationship between two documents.
/// Forward entry: PK: DOC#<source>, SK: REL#<type>#<target>
/// Reverse entry: PK: DOC#<target>, SK: RREL#<type>#<source>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRelationship {
    pub source_doc_id: String,
    pub target_doc_id: String,
    pub relation_type: RelationType,
    pub created_by: String,
    pub created_at: i64,
}

impl DocRelationship {
    pub fn pk(&self) -> String {
        format!("DOC#{}", self.source_doc_id)
    }

    pub fn sk(&self) -> String {
        format!("REL#{}#{}", self.relation_type.as_str(), self.target_doc_id)
    }

    pub fn reverse_pk(&self) -> String {
        format!("DOC#{}", self.target_doc_id)
    }

    pub fn reverse_sk(&self) -> String {
        format!("RREL#{}#{}", self.relation_type.as_str(), self.source_doc_id)
    }
}

mod serde_bytes {
    use serde::{self, Deserialize, Deserializer, Serializer};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    pub fn serialize<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_doc() -> DocumentMeta {
        let now = now_usec();
        DocumentMeta {
            doc_id: new_id(),
            title: "Test Document".to_string(),
            owner_id: new_id(),
            folder_id: None,
            additional_folder_ids: Vec::new(),
            workspace_id: None,
            doc_type: DocType::Document,
            snapshot_version: 1,
            snapshot_s3_key: Some("docs/abc/snapshots/1.bin".to_string()),
            is_deleted: false,
            deleted_at: None,
            link_sharing_mode: None,
            link_view_options: ViewOptions::default(),
            locked: false,
            is_template: false,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn document_pk_format() {
        let doc = sample_doc();
        assert_eq!(doc.pk(), format!("DOC#{}", doc.doc_id));
    }

    #[test]
    fn document_sk_format() {
        assert_eq!(DocumentMeta::sk(), "METADATA");
    }

    #[test]
    fn document_snapshot_key() {
        let mut doc = sample_doc();
        doc.doc_id = "abc123".to_string();
        doc.snapshot_version = 5;
        assert_eq!(doc.snapshot_key(), "docs/abc123/snapshots/5.bin");
    }

    #[test]
    fn document_json_roundtrip() {
        let doc = sample_doc();
        let json = serde_json::to_string(&doc).unwrap();
        let back: DocumentMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn document_soft_delete_fields() {
        let mut doc = sample_doc();
        doc.is_deleted = true;
        doc.deleted_at = Some(now_usec());
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"is_deleted\":true"));
        assert!(json.contains("deleted_at"));
    }

    #[test]
    fn relation_type_as_str_from_str_round_trips_every_variant() {
        // from_str is the storage decode path (doc_rel_from_item);
        // as_str is the encode path. They must stay inverses or
        // relationship rows silently vanish from listings.
        for rt in [
            RelationType::Implements,
            RelationType::DerivedFrom,
            RelationType::DependsOn,
            RelationType::References,
            RelationType::Supersedes,
        ] {
            assert_eq!(RelationType::from_str(rt.as_str()), Some(rt.clone()));
        }
        assert_eq!(RelationType::from_str("blames"), None);
        assert_eq!(RelationType::from_str(""), None);
    }

    #[test]
    fn relation_type_as_str_agrees_with_serde_kebab_case() {
        // The enum carries both a hand-written as_str() and a
        // kebab-case serde rename. If they drift, the same value
        // serializes to two different wire strings depending on code
        // path (same invariant class as models/mod.rs's
        // as_str_agrees_with_serde).
        for rt in [
            RelationType::Implements,
            RelationType::DerivedFrom,
            RelationType::DependsOn,
            RelationType::References,
            RelationType::Supersedes,
        ] {
            let serde_token = serde_json::to_string(&rt)
                .unwrap()
                .trim_matches('"')
                .to_string();
            assert_eq!(rt.as_str(), serde_token, "drift on {rt:?}");
        }
    }

    #[test]
    fn doc_relationship_forward_and_reverse_keys() {
        let rel = DocRelationship {
            source_doc_id: "src".to_string(),
            target_doc_id: "dst".to_string(),
            relation_type: RelationType::DependsOn,
            created_by: "u1".to_string(),
            created_at: 0,
        };
        assert_eq!(rel.pk(), "DOC#src");
        assert_eq!(rel.sk(), "REL#depends-on#dst");
        assert_eq!(rel.reverse_pk(), "DOC#dst");
        assert_eq!(rel.reverse_sk(), "RREL#depends-on#src");
    }

    #[test]
    fn doc_update_pk_sk_format() {
        let upd = DocUpdate {
            doc_id: "doc1".to_string(),
            clock: "000042-client7".to_string(),
            update_bytes: vec![],
            user_id: "u1".to_string(),
            created_at: 0,
            client_version: None,
        };
        assert_eq!(upd.pk(), "DOC#doc1");
        assert_eq!(upd.sk(), "UPDATE#000042-client7");
    }

    #[test]
    fn doc_open_favorite_key_formats() {
        let open = DocOpen {
            doc_id: "doc1".to_string(),
            user_id: "u1".to_string(),
            first_opened_at: 0,
        };
        assert_eq!(open.pk(), "DOC#doc1");
        assert_eq!(open.sk(), "OPEN#u1");

        // #144: favorites hang off the USER# partition, not DOC#.
        let fav = Favorite {
            user_id: "u1".to_string(),
            doc_id: "doc1".to_string(),
            added_at: 0,
        };
        assert_eq!(fav.pk(), "USER#u1");
        assert_eq!(fav.sk(), "FAV#doc1");
    }

    #[test]
    fn collection_key_formats_and_prefix_consistency() {
        // #144: the repo queries collections by Collection::SK_PREFIX
        // and membership rows by CollectionItem::sk_prefix(id) /
        // SK_PREFIX_ALL. Each sk() must start with its query prefix or
        // the corresponding listing returns nothing.
        let coll = Collection {
            user_id: "u1".to_string(),
            collection_id: "c1".to_string(),
            name: "Reading list".to_string(),
            created_at: 0,
        };
        assert_eq!(coll.pk(), "USER#u1");
        assert_eq!(coll.sk(), "COLLECTION#c1");
        assert!(coll.sk().starts_with(Collection::SK_PREFIX));

        let item = CollectionItem {
            user_id: "u1".to_string(),
            collection_id: "c1".to_string(),
            doc_id: "doc1".to_string(),
            added_at: 0,
        };
        assert_eq!(item.pk(), "USER#u1");
        assert_eq!(item.sk(), "COLLITEM#c1#doc1");
        assert!(item.sk().starts_with(&CollectionItem::sk_prefix("c1")));
        assert!(item.sk().starts_with(CollectionItem::SK_PREFIX_ALL));
        assert_eq!(CollectionItem::sk_prefix("c1"), "COLLITEM#c1#");
    }

    #[test]
    fn collection_item_prefix_cannot_match_sibling_collection() {
        // The per-collection prefix ends with '#'. Without it, listing
        // collection "c1" would also match rows of a collection named
        // "c10" — a cross-collection leak.
        let in_c10 = CollectionItem {
            user_id: "u1".to_string(),
            collection_id: "c10".to_string(),
            doc_id: "doc1".to_string(),
            added_at: 0,
        };
        assert!(!in_c10.sk().starts_with(&CollectionItem::sk_prefix("c1")));
    }

    #[test]
    fn doc_update_bytes_json_round_trip_is_base64() {
        // DocUpdate.update_bytes serializes via the local serde_bytes
        // shim as a base64 string (CRDT payloads are binary). Pin
        // both the encoding and the round trip.
        let upd = DocUpdate {
            doc_id: "d".to_string(),
            clock: "c".to_string(),
            update_bytes: vec![0, 1, 2, 255, 254, 253],
            user_id: "u".to_string(),
            created_at: 0,
            client_version: Some("1.2.3".to_string()),
        };
        let json = serde_json::to_string(&upd).unwrap();
        assert!(
            json.contains("\"update_bytes\":\"AAEC//79\""),
            "expected standard-base64 payload, got {json}"
        );
        let back: DocUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(back.update_bytes, upd.update_bytes);
        assert_eq!(back.client_version, upd.client_version);
    }

    #[test]
    fn doc_update_bytes_rejects_invalid_base64() {
        let json = r#"{"doc_id":"d","clock":"c","update_bytes":"@@not-base64@@","user_id":"u","created_at":0}"#;
        assert!(serde_json::from_str::<DocUpdate>(json).is_err());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any byte payload survives the base64 serde shim. The CRDT
        /// op-log depends on this codec being lossless for arbitrary
        /// binary — a single corrupted byte breaks Y.Doc replay.
        #[test]
        fn doc_update_bytes_round_trip_any_payload(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let upd = DocUpdate {
                doc_id: "d".to_string(),
                clock: "c".to_string(),
                update_bytes: bytes.clone(),
                user_id: "u".to_string(),
                created_at: 0,
                client_version: None,
            };
            let json = serde_json::to_string(&upd).unwrap();
            let back: DocUpdate = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back.update_bytes, bytes);
        }
    }
}
