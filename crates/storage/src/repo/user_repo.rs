use aws_sdk_dynamodb::types::AttributeValue;
use std::collections::HashMap;

use crate::dynamo::DynamoClient;
use crate::models::user::User;
use crate::repo::{RepoError, get_s, get_n};

/// Repository for user operations.
pub struct UserRepo {
    db: DynamoClient,
}

impl UserRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Create a new user record.
    pub async fn create(&self, user: &User) -> Result<(), RepoError> {
        let mut item = user_to_item(user);
        item.insert("PK".to_string(), AttributeValue::S(user.pk()));
        item.insert("SK".to_string(), AttributeValue::S(User::sk().to_string()));

        self.db
            .put_item_conditional(item, "attribute_not_exists(PK)")
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// Get a user by ID.
    pub async fn get_by_id(&self, user_id: &str) -> Result<Option<User>, RepoError> {
        let pk = format!("USER#{user_id}");
        let item = self
            .db
            .get_item(&pk, User::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;

        match item {
            Some(item) => Ok(Some(user_from_item(&item)?)),
            None => Ok(None),
        }
    }

    /// Get a user by email (scans USER# items -- replace with GSI for production).
    pub async fn get_by_email(&self, email: &str) -> Result<Option<User>, RepoError> {
        // For MVP: scan all USER#/PROFILE items and filter by email.
        // Phase 2: add a GSI on email for efficient lookup.
        let result = self
            .db
            .inner()
            .scan()
            .table_name(self.db.table_name())
            .filter_expression("SK = :sk AND email = :email")
            .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()))
            .expression_attribute_values(":email", AttributeValue::S(email.to_string()))
            .limit(1)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        match result.items {
            Some(items) if !items.is_empty() => Ok(Some(user_from_item(&items[0])?)),
            _ => Ok(None),
        }
    }

    /// Update a user's profile fields.
    pub async fn update(
        &self,
        user_id: &str,
        name: Option<&str>,
        avatar_url: Option<Option<&str>>,
        updated_at: i64,
    ) -> Result<(), RepoError> {
        let pk = format!("USER#{user_id}");
        let mut expr_parts = vec!["#updated_at = :updated_at".to_string()];
        let mut remove_parts = Vec::new();
        let mut values = HashMap::new();
        let mut names = HashMap::new();

        names.insert("#updated_at".to_string(), "updated_at".to_string());
        values.insert(
            ":updated_at".to_string(),
            AttributeValue::N(updated_at.to_string()),
        );

        if let Some(n) = name {
            expr_parts.push("#name = :name".to_string());
            names.insert("#name".to_string(), "name".to_string());
            values.insert(":name".to_string(), AttributeValue::S(n.to_string()));
        }

        match avatar_url {
            Some(Some(url)) => {
                expr_parts.push("avatar_url = :avatar_url".to_string());
                values.insert(
                    ":avatar_url".to_string(),
                    AttributeValue::S(url.to_string()),
                );
            }
            Some(None) => {
                remove_parts.push("avatar_url".to_string());
            }
            None => {} // Don't touch the field
        }

        let mut update_expr = format!("SET {}", expr_parts.join(", "));
        if !remove_parts.is_empty() {
            update_expr.push_str(&format!(" REMOVE {}", remove_parts.join(", ")));
        }

        self.db
            .update_item(&pk, User::sk(), &update_expr, values, Some(names))
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn user_to_item(user: &User) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    item.insert("user_id".to_string(), AttributeValue::S(user.user_id.clone()));
    item.insert("name".to_string(), AttributeValue::S(user.name.clone()));
    item.insert("email".to_string(), AttributeValue::S(user.email.clone()));
    if let Some(ref url) = user.avatar_url {
        item.insert("avatar_url".to_string(), AttributeValue::S(url.clone()));
    }
    item.insert("home_folder_id".to_string(), AttributeValue::S(user.home_folder_id.clone()));
    item.insert("private_folder_id".to_string(), AttributeValue::S(user.private_folder_id.clone()));
    item.insert("trash_folder_id".to_string(), AttributeValue::S(user.trash_folder_id.clone()));
    item.insert("created_at".to_string(), AttributeValue::N(user.created_at.to_string()));
    item.insert("updated_at".to_string(), AttributeValue::N(user.updated_at.to_string()));
    item
}

fn user_from_item(item: &HashMap<String, AttributeValue>) -> Result<User, RepoError> {
    Ok(User {
        user_id: get_s(item, "user_id")?,
        name: get_s(item, "name")?,
        email: get_s(item, "email")?,
        avatar_url: item.get("avatar_url").and_then(|v| v.as_s().ok()).cloned(),
        home_folder_id: get_s(item, "home_folder_id")?,
        private_folder_id: get_s(item, "private_folder_id")?,
        trash_folder_id: get_s(item, "trash_folder_id")?,
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}
