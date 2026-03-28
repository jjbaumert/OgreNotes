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

    /// Add a member to a folder.
    async fn add_member(&self, member: &FolderMember) -> Result<(), RepoError> {
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
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
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

