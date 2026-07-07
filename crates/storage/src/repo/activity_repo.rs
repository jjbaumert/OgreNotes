// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for document activity feed events.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::activity::{Activity, ActivityEventType};

pub struct ActivityRepo {
    db: DynamoClient,
}

impl ActivityRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Record an activity event for a document.
    pub async fn create(&self, activity: &Activity) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(activity.pk()));
        item.insert("SK".to_string(), AttributeValue::S(activity.sk()));
        item.insert("activity_id".to_string(), AttributeValue::S(activity.activity_id.clone()));
        item.insert("doc_id".to_string(), AttributeValue::S(activity.doc_id.clone()));
        item.insert(
            "event_type".to_string(),
            AttributeValue::S(
                serde_json::to_string(&activity.event_type)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        );
        item.insert("actor_id".to_string(), AttributeValue::S(activity.actor_id.clone()));
        item.insert("detail".to_string(), AttributeValue::S(activity.detail.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(activity.created_at.to_string()));

        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    /// List activity events for a document, newest first.
    pub async fn list(&self, doc_id: &str, limit: usize) -> Result<Vec<Activity>, RepoError> {
        let pk = format!("DOC#{doc_id}");

        let result = self
            .db
            .inner()
            .query()
            .table_name(self.db.table_name())
            .key_condition_expression("PK = :pk AND begins_with(SK, :prefix)")
            .expression_attribute_values(":pk", AttributeValue::S(pk))
            .expression_attribute_values(":prefix", AttributeValue::S("ACTIVITY#".to_string()))
            .scan_index_forward(false)
            .limit(limit as i32)
            .send()
            .await
            .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

        let items = result.items.unwrap_or_default();
        items
            .iter()
            .map(|item| activity_from_item(item))
            .collect()
    }
}

fn activity_from_item(item: &HashMap<String, AttributeValue>) -> Result<Activity, RepoError> {
    let type_str = get_s(item, "event_type")?;
    let event_type: ActivityEventType = serde_json::from_str(&format!("\"{type_str}\""))
        .map_err(|e| RepoError::MissingField(format!("event_type: {e}")))?;

    Ok(Activity {
        activity_id: get_s(item, "activity_id")?,
        doc_id: get_s(item, "doc_id")?,
        event_type,
        actor_id: get_s(item, "actor_id")?,
        detail: item.get("detail").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default(),
        created_at: get_n(item, "created_at")?,
    })
}
