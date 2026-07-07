// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for admin-curated Company template galleries (#142 Phase 4).
//! Row layout: PK `WORKSPACE#<workspace_id>`, SK `TEMPLATE_GALLERY#<gallery_id>`.
//! `doc_ids` is stored inline as a JSON string attribute; membership
//! changes rewrite the whole row.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::template_gallery::{
    TemplateGallery, MAX_GALLERY_DOC_IDS, MAX_GALLERY_NAME_LEN,
};

pub struct TemplateGalleryRepo {
    db: DynamoClient,
}

impl TemplateGalleryRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Upsert a gallery row. Duplicate doc ids in the input are
    /// deduped in-place before writing; order is preserved by
    /// first-occurrence. Enforces the same name / doc-count caps
    /// the API-layer validator does so any writer that bypasses the
    /// admin route (migration binary, bulk-import job, direct
    /// maintenance script) cannot land a row that would exceed
    /// DynamoDB's 400 KB item limit or produce pathologically wide
    /// picker section headers.
    pub async fn put(&self, gallery: &TemplateGallery) -> Result<(), RepoError> {
        if gallery.name.chars().count() > MAX_GALLERY_NAME_LEN {
            return Err(RepoError::InvalidArgument(format!(
                "gallery name is too long (max {MAX_GALLERY_NAME_LEN} characters)",
            )));
        }
        if gallery.doc_ids.len() > MAX_GALLERY_DOC_IDS {
            return Err(RepoError::InvalidArgument(format!(
                "gallery cannot hold more than {MAX_GALLERY_DOC_IDS} templates",
            )));
        }
        // Dedup while preserving first-occurrence order. Cheap even
        // at the MAX_GALLERY_DOC_IDS cap and keeps the on-disk row
        // in canonical shape (no duplicate entries picker-side).
        let mut seen = std::collections::HashSet::new();
        let doc_ids: Vec<String> = gallery
            .doc_ids
            .iter()
            .filter(|id| seen.insert(id.as_str().to_string()))
            .cloned()
            .collect();
        let doc_ids_json = serde_json::to_string(&doc_ids)
            .map_err(|e| RepoError::InvalidArgument(format!("encode doc_ids: {e}")))?;

        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(gallery.pk()));
        item.insert("SK".to_string(), AttributeValue::S(gallery.sk()));
        item.insert(
            "workspace_id".to_string(),
            AttributeValue::S(gallery.workspace_id.clone()),
        );
        item.insert(
            "gallery_id".to_string(),
            AttributeValue::S(gallery.gallery_id.clone()),
        );
        item.insert("name".to_string(), AttributeValue::S(gallery.name.clone()));
        item.insert("doc_ids".to_string(), AttributeValue::S(doc_ids_json));
        item.insert(
            "created_by".to_string(),
            AttributeValue::S(gallery.created_by.clone()),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(gallery.created_at.to_string()),
        );
        item.insert(
            "updated_at".to_string(),
            AttributeValue::N(gallery.updated_at.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    pub async fn get(
        &self,
        workspace_id: &str,
        gallery_id: &str,
    ) -> Result<Option<TemplateGallery>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = format!("{}{gallery_id}", TemplateGallery::SK_PREFIX);
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        match item {
            Some(it) => Ok(Some(from_item(&it)?)),
            None => Ok(None),
        }
    }

    /// All galleries for a workspace, in DDB SK order. Used by the
    /// admin CRUD list endpoint and by `list_templates` to fold
    /// company galleries into the picker.
    pub async fn list_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<TemplateGallery>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let items = self
            .db
            .query(&pk, Some(TemplateGallery::SK_PREFIX))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        items.iter().map(from_item).collect()
    }

    pub async fn delete(
        &self,
        workspace_id: &str,
        gallery_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = format!("{}{gallery_id}", TemplateGallery::SK_PREFIX);
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn from_item(item: &HashMap<String, AttributeValue>) -> Result<TemplateGallery, RepoError> {
    let doc_ids_json = get_s(item, "doc_ids")?;
    let doc_ids: Vec<String> = serde_json::from_str(&doc_ids_json)
        .map_err(|e| RepoError::Dynamo(format!("decode doc_ids: {e}")))?;
    Ok(TemplateGallery {
        workspace_id: get_s(item, "workspace_id")?,
        gallery_id: get_s(item, "gallery_id")?,
        name: get_s(item, "name")?,
        doc_ids,
        created_by: get_s(item, "created_by")?,
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
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
            created_at: 1_700_000_000_000_000,
            updated_at: 1_700_000_000_000_000,
        }
    }

    fn item_for(g: &TemplateGallery) -> HashMap<String, AttributeValue> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(g.pk()));
        item.insert("SK".to_string(), AttributeValue::S(g.sk()));
        item.insert("workspace_id".to_string(), AttributeValue::S(g.workspace_id.clone()));
        item.insert("gallery_id".to_string(), AttributeValue::S(g.gallery_id.clone()));
        item.insert("name".to_string(), AttributeValue::S(g.name.clone()));
        item.insert(
            "doc_ids".to_string(),
            AttributeValue::S(serde_json::to_string(&g.doc_ids).unwrap()),
        );
        item.insert("created_by".to_string(), AttributeValue::S(g.created_by.clone()));
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(g.created_at.to_string()),
        );
        item.insert(
            "updated_at".to_string(),
            AttributeValue::N(g.updated_at.to_string()),
        );
        item
    }

    #[test]
    fn from_item_round_trips_put_shape() {
        let g = fixture();
        let back = from_item(&item_for(&g)).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn doc_ids_json_is_parsed_back_to_vec() {
        // Regression guard: if anyone converts `doc_ids` to a DDB List
        // instead of a JSON string, this round trip breaks — the whole
        // gallery-membership contract sits on that encoding.
        let mut item = item_for(&fixture());
        item.insert(
            "doc_ids".to_string(),
            AttributeValue::S(r#"["a","b","c"]"#.to_string()),
        );
        let back = from_item(&item).unwrap();
        assert_eq!(back.doc_ids, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }
}
