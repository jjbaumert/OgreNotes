// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::workspace::{Workspace, WorkspaceMember};
use crate::models::WorkspaceRole;
use crate::repo::{get_n, get_s, RepoError};

/// Repository for workspace operations.
pub struct WorkspaceRepo {
    db: DynamoClient,
}

impl WorkspaceRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new workspace and auto-add the owner as a member with Role::Owner.
    pub async fn create(&self, workspace: &Workspace) -> Result<(), RepoError> {
        let mut item = workspace_to_item(workspace);
        item.insert("PK".to_string(), AttributeValue::S(workspace.pk()));
        item.insert("SK".to_string(), AttributeValue::S(Workspace::sk().to_string()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        // Auto-add owner as member with Owner role
        let member = WorkspaceMember {
            workspace_id: workspace.workspace_id.clone(),
            user_id: workspace.owner_id.clone(),
            role: WorkspaceRole::Owner,
            joined_at: workspace.created_at,
        };
        self.add_member(&member).await
    }

    /// Get workspace metadata by ID.
    pub async fn get(&self, workspace_id: &str) -> Result<Option<Workspace>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let item = self
            .db
            .get_item(&pk, Workspace::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(workspace_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Update workspace metadata.
    pub async fn update(
        &self,
        workspace_id: &str,
        name: Option<&str>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let mut expr_parts = vec!["updated_at = :updated_at".to_string()];
        let mut values = HashMap::new();

        values.insert(":updated_at".to_string(), AttributeValue::N(updated_at.to_string()));

        if let Some(n) = name {
            expr_parts.push("#n = :name".to_string());
            values.insert(":name".to_string(), AttributeValue::S(n.to_string()));
        }

        let update_expr = format!("SET {}", expr_parts.join(", "));

        // Use raw update since "name" is a DynamoDB reserved word
        self.db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(Workspace::sk().to_string()))
            .update_expression(&update_expr)
            .set_expression_attribute_values(Some(values))
            .expression_attribute_names("#n", "name")
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        Ok(())
    }

    /// Phase 4 M-E3: flip the `mfa_required` flag. Writes
    /// `Bool(true)` when enabling and REMOVEs the attribute when
    /// disabling — keeps the sparse "default false" shape that
    /// pre-M-E3 rows already have.
    pub async fn set_mfa_required(
        &self,
        workspace_id: &str,
        required: bool,
    ) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let mut values = HashMap::new();
        let expr = if required {
            values.insert(":val".to_string(), AttributeValue::Bool(true));
            "SET mfa_required = :val"
        } else {
            "REMOVE mfa_required"
        };
        self.db
            .update_item(&pk, Workspace::sk(), expr, values, None)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Delete the METADATA row for a workspace. Member rows are a
    /// separate concern; callers that want a full teardown must
    /// also walk and remove WorkspaceMember rows themselves.
    pub async fn delete(&self, workspace_id: &str) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        self.db
            .delete_item(&pk, Workspace::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Add a member to a workspace (or update if already exists).
    pub async fn add_member(&self, member: &WorkspaceMember) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(member.pk()));
        item.insert("SK".to_string(), AttributeValue::S(member.sk()));
        item.insert(
            "role".to_string(),
            AttributeValue::S(
                serde_json::to_string(&member.role)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        item.insert("joined_at".to_string(), AttributeValue::N(member.joined_at.to_string()));
        item.insert("user_id".to_string(), AttributeValue::S(member.user_id.clone()));
        // GSI4: list a user's workspace memberships by join time.
        item.insert("user_id_gsi".to_string(), AttributeValue::S(member.user_id.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(member.joined_at.to_string()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a specific member's role in a workspace.
    pub async fn get_member(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<Option<WorkspaceMember>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = format!("MEMBER#{user_id}");
        let item = self
            .db
            .get_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(member_from_item(&item, workspace_id)?)),
            None => Ok(None),
        }
    }

    /// List the workspace IDs a user is a member of.
    ///
    /// Uses GSI4-user-created — the sparse index every WorkspaceMember
    /// row writes into via `user_id_gsi` on `add_member`. Filters
    /// down to MEMBER# rows in case a future record type ever shares
    /// GSI4 partitions.
    ///
    /// Used by `/users/search` to workspace-scope directory
    /// enumeration: intersect the caller's workspaces with each
    /// candidate hit's workspaces, drop non-overlapping.
    pub async fn list_workspaces_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<String>, RepoError> {
        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .index_name("GSI4-user-created")
            .key_condition_expression("user_id_gsi = :uid")
            .expression_attribute_values(":uid", AttributeValue::S(user_id.to_string()))
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        let mut out = Vec::new();
        for item in items {
            let sk = item
                .get("SK")
                .and_then(|v| v.as_s().ok())
                .cloned()
                .unwrap_or_default();
            if let Some(rest) = sk.strip_prefix("MEMBER#") {
                let _ = rest;
                let pk = item
                    .get("PK")
                    .and_then(|v| v.as_s().ok())
                    .cloned()
                    .unwrap_or_default();
                if let Some(ws_id) = pk.strip_prefix("WORKSPACE#") {
                    out.push(ws_id.to_string());
                }
            }
        }
        Ok(out)
    }

    /// List all members of a workspace.
    pub async fn list_members(&self, workspace_id: &str) -> Result<Vec<WorkspaceMember>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let items = self
            .db
            .query(&pk, Some("MEMBER#"))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        items
            .iter()
            .map(|item| member_from_item(item, workspace_id))
            .collect()
    }

    /// Remove a member from a workspace.
    pub async fn remove_member(&self, workspace_id: &str, user_id: &str) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let sk = format!("MEMBER#{user_id}");
        self.db
            .delete_item(&pk, &sk)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn workspace_to_item(ws: &Workspace) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("workspace_id".to_string(), AttributeValue::S(ws.workspace_id.clone()));
    item.insert("name".to_string(), AttributeValue::S(ws.name.clone()));
    item.insert("owner_id".to_string(), AttributeValue::S(ws.owner_id.clone()));
    // `mfa_required` written conditionally — pre-M-E3 rows didn't
    // have the attribute, and an explicit `false` would needlessly
    // bloat every row. `workspace_from_item` defaults absent values
    // to `false`, matching the model's `#[serde(default)]`.
    if ws.mfa_required {
        item.insert("mfa_required".to_string(), AttributeValue::Bool(true));
    }
    item.insert("created_at".to_string(), AttributeValue::N(ws.created_at.to_string()));
    item.insert("updated_at".to_string(), AttributeValue::N(ws.updated_at.to_string()));
    item
}

fn workspace_from_item(item: &HashMap<String, AttributeValue>) -> Result<Workspace, RepoError> {
    Ok(Workspace {
        workspace_id: get_s(item, "workspace_id")?,
        name: get_s(item, "name")?,
        owner_id: get_s(item, "owner_id")?,
        mfa_required: item
            .get("mfa_required")
            .and_then(|v| v.as_bool().ok())
            .copied()
            .unwrap_or(false),
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

fn member_from_item(
    item: &HashMap<String, AttributeValue>,
    workspace_id: &str,
) -> Result<WorkspaceMember, RepoError> {
    let role_str = get_s(item, "role")?;
    let role: WorkspaceRole = serde_json::from_str(&format!("\"{role_str}\""))
        .map_err(|e| RepoError::MissingField(format!("role: {e}")))?;

    Ok(WorkspaceMember {
        workspace_id: workspace_id.to_string(),
        user_id: get_s(item, "user_id")?,
        role,
        joined_at: get_n(item, "joined_at")?,
    })
}
