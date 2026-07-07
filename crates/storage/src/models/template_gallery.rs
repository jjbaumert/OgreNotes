// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #142 Phase 4 — admin-curated Company template galleries.
//!
//! One row per gallery:
//!
//!   PK = `WORKSPACE#<workspace_id>`
//!   SK = `TEMPLATE_GALLERY#<gallery_id>`
//!
//! `doc_ids` is stored inline (JSON string) rather than as separate
//! membership rows. Trade-off: rewrites the whole row on every
//! add/remove, but list-membership is a single GetItem and gallery
//! rendering doesn't fan out. DynamoDB's 400 KB item cap comfortably
//! holds thousands of ~12-char doc ids, well past any realistic
//! curated set.
//!
//! A doc referenced by a gallery still lives in whatever workspace it
//! was created in — the gallery only tracks membership + display
//! grouping. The list-templates handler fetches metadata for each
//! referenced doc and drops rows the caller can't view.

use serde::{Deserialize, Serialize};

/// Max distinct doc ids in a single gallery. Bounded so a runaway
/// admin can't grow a row past the DDB item limit, and so the list
/// handler's per-gallery metadata fan-out is predictable.
pub const MAX_GALLERY_DOC_IDS: usize = 500;

/// Max chars in a gallery's display name. Prevents pathological
/// section headers in the picker UI.
pub const MAX_GALLERY_NAME_LEN: usize = 80;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TemplateGallery {
    pub workspace_id: String,
    pub gallery_id: String,
    /// Display name shown as the picker section header (e.g.
    /// "Engineering templates"). Capped at [`MAX_GALLERY_NAME_LEN`].
    pub name: String,
    /// Membership: doc ids grouped into this gallery. Order is
    /// preserved (admins may want to hand-curate presentation
    /// order). Deduplication is enforced by the repo write path.
    pub doc_ids: Vec<String>,
    /// The user who created the gallery. Retained for audit; the
    /// separate SecurityAudit row is the authoritative trail.
    pub created_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TemplateGallery {
    pub fn pk(&self) -> String {
        format!("WORKSPACE#{}", self.workspace_id)
    }

    pub fn sk(&self) -> String {
        format!("TEMPLATE_GALLERY#{}", self.gallery_id)
    }

    /// SK prefix for a scan of all galleries in a workspace.
    pub const SK_PREFIX: &'static str = "TEMPLATE_GALLERY#";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> TemplateGallery {
        TemplateGallery {
            workspace_id: "ws-1".to_string(),
            gallery_id: "g-1".to_string(),
            name: "Engineering".to_string(),
            doc_ids: vec!["doc-a".to_string(), "doc-b".to_string()],
            created_by: "admin-1".to_string(),
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn pk_sk_format() {
        let g = fixture();
        assert_eq!(g.pk(), "WORKSPACE#ws-1");
        assert_eq!(g.sk(), "TEMPLATE_GALLERY#g-1");
    }

    #[test]
    fn json_roundtrip() {
        let g = fixture();
        let s = serde_json::to_string(&g).unwrap();
        let back: TemplateGallery = serde_json::from_str(&s).unwrap();
        assert_eq!(back, g);
    }
}
