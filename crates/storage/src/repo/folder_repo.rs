// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::folder::{Folder, FolderChild, FolderMember};
use crate::models::{AccessLevel, ChildType, FolderType};
use crate::repo::{RepoError, get_s, get_n};

/// Repository for folder operations.
pub struct FolderRepo {
    db: DynamoClient,
}

impl FolderRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new folder with the owner as a member.
    pub async fn create(&self, folder: &Folder) -> Result<(), RepoError> {
        // Write folder metadata
        let mut item = folder_to_item(folder);
        item.insert("PK".to_string(), AttributeValue::S(folder.pk()));
        item.insert(
            "SK".to_string(),
            AttributeValue::S(Folder::sk().to_string()),
        );
        // GSI2: parent_id -> title
        if let Some(ref parent_id) = folder.parent_id {
            item.insert(
                "parent_id_gsi".to_string(),
                AttributeValue::S(parent_id.clone()),
            );
        }

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Add owner as member with OWN access
        let member = FolderMember {
            folder_id: folder.folder_id.clone(),
            user_id: folder.owner_id.clone(),
            access_level: AccessLevel::Own,
            added_at: folder.created_at,
        };
        self.add_member(&member).await
    }

    /// Get folder metadata by ID.
    pub async fn get(&self, folder_id: &str) -> Result<Option<Folder>, RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let item = self
            .db
            .get_item(&pk, Folder::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(folder_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Update folder metadata.
    pub async fn update(
        &self,
        folder_id: &str,
        title: Option<&str>,
        color: Option<u8>,
        parent_id: Option<&str>,
        inherit_mode: Option<&crate::models::InheritMode>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let mut expr_parts = vec!["updated_at = :updated_at".to_string()];
        let mut values = HashMap::new();

        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );

        if let Some(t) = title {
            expr_parts.push("title = :title".to_string());
            values.insert(":title".to_string(), AttributeValue::S(t.to_string()));
        }

        if let Some(c) = color {
            let clamped = Folder::clamp_color(c);
            expr_parts.push("color = :color".to_string());
            values.insert(
                ":color".to_string(),
                AttributeValue::N(clamped.to_string()),
            );
        }

        if let Some(pid) = parent_id {
            expr_parts.push("parent_id = :parent_id".to_string());
            expr_parts.push("parent_id_gsi = :parent_id_gsi".to_string());
            values.insert(
                ":parent_id".to_string(),
                AttributeValue::S(pid.to_string()),
            );
            values.insert(
                ":parent_id_gsi".to_string(),
                AttributeValue::S(pid.to_string()),
            );
        }

        if let Some(mode) = inherit_mode {
            expr_parts.push("inherit_mode = :inherit_mode".to_string());
            values.insert(
                ":inherit_mode".to_string(),
                AttributeValue::S(
                    serde_json::to_string(mode)
                        .unwrap()
                        .trim_matches('"')
                        .to_string(),
                ),
            );
        }

        let update_expr = format!("SET {}", expr_parts.join(", "));

        self.db
            .update_item(&pk, Folder::sk(), &update_expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Delete a folder (metadata only; children should be moved first).
    pub async fn delete(&self, folder_id: &str) -> Result<(), RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        self.db
            .delete_item(&pk, Folder::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Add a child (document or subfolder) to a folder.
    pub async fn add_child(&self, child: &FolderChild) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(child.pk()));
        item.insert("SK".to_string(), AttributeValue::S(child.sk()));
        item.insert(
            "child_type".to_string(),
            AttributeValue::S(
                serde_json::to_string(&child.child_type)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        item.insert(
            "added_at".to_string(),
            AttributeValue::N(child.added_at.to_string()),
        );
        // Include child_id as a regular attribute for queries
        item.insert(
            "child_id".to_string(),
            AttributeValue::S(child.child_id.clone()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Remove a child from a folder.
    pub async fn remove_child(
        &self,
        folder_id: &str,
        child_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let sk = format!("CHILD#{child_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List children of a folder.
    pub async fn list_children(
        &self,
        folder_id: &str,
    ) -> Result<Vec<FolderChild>, RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let items = self
            .db
            .query(&pk, Some("CHILD#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items.iter().map(|item| folder_child_from_item(item, folder_id)).collect()
    }

    /// Add a member to a folder (or update if already exists).
    pub async fn add_member(&self, member: &FolderMember) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(member.pk()));
        item.insert("SK".to_string(), AttributeValue::S(member.sk()));
        item.insert(
            "access_level".to_string(),
            AttributeValue::S(
                serde_json::to_string(&member.access_level)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        item.insert(
            "added_at".to_string(),
            AttributeValue::N(member.added_at.to_string()),
        );
        item.insert(
            "user_id".to_string(),
            AttributeValue::S(member.user_id.clone()),
        );

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a specific member's access level for a folder.
    pub async fn get_member(
        &self,
        folder_id: &str,
        user_id: &str,
    ) -> Result<Option<FolderMember>, RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let sk = format!("MEMBER#{user_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(folder_member_from_item(&item, folder_id)?)),
            None => Ok(None),
        }
    }

    /// List all members of a folder.
    pub async fn list_members(
        &self,
        folder_id: &str,
    ) -> Result<Vec<FolderMember>, RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let items = self
            .db
            .query(&pk, Some("MEMBER#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| folder_member_from_item(item, folder_id))
            .collect()
    }

    /// Remove a member from a folder.
    pub async fn remove_member(
        &self,
        folder_id: &str,
        user_id: &str,
    ) -> Result<(), RepoError> {
        let pk = format!("FOLDER#{folder_id}");
        let sk = format!("MEMBER#{user_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn folder_to_item(folder: &Folder) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert(
        "folder_id".to_string(),
        AttributeValue::S(folder.folder_id.clone()),
    );
    item.insert("title".to_string(), AttributeValue::S(folder.title.clone()));
    item.insert(
        "color".to_string(),
        AttributeValue::N(folder.color.to_string()),
    );
    if let Some(ref pid) = folder.parent_id {
        item.insert("parent_id".to_string(), AttributeValue::S(pid.clone()));
    }
    item.insert(
        "owner_id".to_string(),
        AttributeValue::S(folder.owner_id.clone()),
    );
    item.insert(
        "folder_type".to_string(),
        AttributeValue::S(
            serde_json::to_string(&folder.folder_type)
                .unwrap()
                .trim_matches('"')
                .to_string(),
        ),
    );
    if folder.inherit_mode != crate::models::InheritMode::Inherit {
        item.insert(
            "inherit_mode".to_string(),
            AttributeValue::S(
                serde_json::to_string(&folder.inherit_mode)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
    }
    item.insert(
        "created_at".to_string(),
        AttributeValue::N(folder.created_at.to_string()),
    );
    item.insert(
        "updated_at".to_string(),
        AttributeValue::N(folder.updated_at.to_string()),
    );
    item
}

fn folder_from_item(item: &HashMap<String, AttributeValue>) -> Result<Folder, RepoError> {
    let folder_type_str = get_s(item, "folder_type")?;
    let folder_type: FolderType = serde_json::from_str(&format!("\"{folder_type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("folder_type: {e}")))?;

    Ok(Folder {
        folder_id: get_s(item, "folder_id")?,
        title: get_s(item, "title")?,
        color: item
            .get("color")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u8>().ok())
            .unwrap_or(0),
        parent_id: item.get("parent_id").and_then(|v| v.as_s().ok()).cloned(),
        owner_id: get_s(item, "owner_id")?,
        folder_type,
        inherit_mode: item.get("inherit_mode").and_then(|v| v.as_s().ok())
            .and_then(|s| serde_json::from_str(&format!("\"{s}\"")).ok())
            .unwrap_or_default(),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

fn folder_member_from_item(
    item: &HashMap<String, AttributeValue>,
    folder_id: &str,
) -> Result<FolderMember, RepoError> {
    let access_str = get_s(item, "access_level")?;
    let access_level: AccessLevel = serde_json::from_str(&format!("\"{access_str}\""))
        .map_err(|e| RepoError::MissingField(format!("access_level: {e}")))?;

    Ok(FolderMember {
        folder_id: folder_id.to_string(),
        user_id: get_s(item, "user_id")?,
        access_level,
        added_at: get_n(item, "added_at")?,
    })
}

fn folder_child_from_item(
    item: &HashMap<String, AttributeValue>,
    folder_id: &str,
) -> Result<FolderChild, RepoError> {
    let child_type_str = get_s(item, "child_type")?;
    let child_type: ChildType = serde_json::from_str(&format!("\"{child_type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("child_type: {e}")))?;

    Ok(FolderChild {
        folder_id: folder_id.to_string(),
        child_id: get_s(item, "child_id")?,
        child_type,
        added_at: get_n(item, "added_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InheritMode;

    fn folder_fixture() -> Folder {
        Folder {
            folder_id: "f1".to_string(),
            title: "My Folder".to_string(),
            color: 4,
            parent_id: Some("f0".to_string()),
            owner_id: "u1".to_string(),
            folder_type: FolderType::User,
            inherit_mode: InheritMode::Inherit,
            created_at: 100,
            updated_at: 200,
        }
    }

    #[test]
    fn folder_round_trips_through_item() {
        let folder = folder_fixture();
        let back = folder_from_item(&folder_to_item(&folder)).expect("from_item");
        assert_eq!(back, folder);
    }

    #[test]
    fn folder_default_inherit_mode_is_sparse_and_decodes_as_inherit() {
        // Sparse invariant: Inherit (the default) writes no attribute
        // — the shape every pre-inherit-mode row has — and absence
        // decodes back to Inherit.
        let item = folder_to_item(&folder_fixture());
        assert!(
            !item.contains_key("inherit_mode"),
            "Inherit (default) must not write an attribute"
        );
        let back = folder_from_item(&item).expect("from_item");
        assert_eq!(back.inherit_mode, InheritMode::Inherit);
    }

    #[test]
    fn folder_restricted_inherit_mode_round_trips() {
        // Restricted is the permission-tightening state; losing it on
        // read would silently re-open a folder to parent members.
        let mut folder = folder_fixture();
        folder.inherit_mode = InheritMode::Restricted;
        let item = folder_to_item(&folder);
        assert!(item.contains_key("inherit_mode"));
        let back = folder_from_item(&item).expect("from_item");
        assert_eq!(back.inherit_mode, InheritMode::Restricted);
    }

    #[test]
    fn folder_without_parent_decodes_none() {
        let mut folder = folder_fixture();
        folder.parent_id = None;
        let item = folder_to_item(&folder);
        assert!(!item.contains_key("parent_id"));
        let back = folder_from_item(&item).expect("from_item");
        assert_eq!(back.parent_id, None);
    }

    #[test]
    fn folder_system_type_round_trips_and_unknown_type_errors() {
        let mut folder = folder_fixture();
        folder.folder_type = FolderType::System;
        let back = folder_from_item(&folder_to_item(&folder)).expect("from_item");
        assert_eq!(back.folder_type, FolderType::System);

        let mut item = folder_to_item(&folder_fixture());
        item.insert("folder_type".to_string(), AttributeValue::S("shared".to_string()));
        match folder_from_item(&item) {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("folder_type"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn folder_missing_or_garbage_color_decodes_as_zero() {
        // Color is cosmetic; a legacy row without it (or with a
        // non-u8 value) degrades to 0 rather than failing decode.
        let mut item = folder_to_item(&folder_fixture());
        item.remove("color");
        assert_eq!(folder_from_item(&item).expect("from_item").color, 0);

        let mut item = folder_to_item(&folder_fixture());
        item.insert("color".to_string(), AttributeValue::N("999".to_string()));
        assert_eq!(folder_from_item(&item).expect("from_item").color, 0);
    }

    /// Mimic `add_member`'s column construction (no live table).
    fn member_item(level: &AccessLevel) -> HashMap<String, AttributeValue> {
        let member = FolderMember {
            folder_id: "f1".to_string(),
            user_id: "u2".to_string(),
            access_level: level.clone(),
            added_at: 42,
        };
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(member.pk()));
        item.insert("SK".to_string(), AttributeValue::S(member.sk()));
        item.insert(
            "access_level".to_string(),
            AttributeValue::S(
                serde_json::to_string(&member.access_level)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        item.insert("added_at".to_string(), AttributeValue::N(member.added_at.to_string()));
        item.insert("user_id".to_string(), AttributeValue::S(member.user_id.clone()));
        item
    }

    #[test]
    fn folder_member_access_level_round_trips_for_every_level() {
        // Access levels are stored as the UPPERCASE serde tags
        // ("OWN"/"EDIT"/"COMMENT"/"VIEW"). Losing or misreading one
        // is a permission bug, so all four are pinned.
        for level in [
            AccessLevel::Own,
            AccessLevel::Edit,
            AccessLevel::Comment,
            AccessLevel::View,
        ] {
            let back = folder_member_from_item(&member_item(&level), "f1")
                .unwrap_or_else(|e| panic!("roundtrip failed for {level:?}: {e}"));
            assert_eq!(back.access_level, level);
            assert_eq!(back.folder_id, "f1");
            assert_eq!(back.user_id, "u2");
        }
    }

    #[test]
    fn folder_member_unknown_access_level_errors() {
        let mut item = member_item(&AccessLevel::View);
        item.insert("access_level".to_string(), AttributeValue::S("SUDO".to_string()));
        match folder_member_from_item(&item, "f1") {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("access_level"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn folder_child_round_trips_both_child_types() {
        // Mimic `add_child`'s column construction.
        for ct in [ChildType::Doc, ChildType::Folder] {
            let child = FolderChild {
                folder_id: "f1".to_string(),
                child_id: "c1".to_string(),
                child_type: ct.clone(),
                added_at: 42,
            };
            let mut item = HashMap::new();
            item.insert("PK".to_string(), AttributeValue::S(child.pk()));
            item.insert("SK".to_string(), AttributeValue::S(child.sk()));
            item.insert(
                "child_type".to_string(),
                AttributeValue::S(
                    serde_json::to_string(&child.child_type)
                        .unwrap()
                        .trim_matches('"')
                        .to_string(),
                ),
            );
            item.insert("added_at".to_string(), AttributeValue::N(child.added_at.to_string()));
            item.insert("child_id".to_string(), AttributeValue::S(child.child_id.clone()));
            let back = folder_child_from_item(&item, "f1")
                .unwrap_or_else(|e| panic!("roundtrip failed for {ct:?}: {e}"));
            assert_eq!(back, child);
        }
    }
}

