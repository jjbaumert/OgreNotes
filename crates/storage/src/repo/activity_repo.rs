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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(event_type: ActivityEventType) -> Activity {
        Activity {
            activity_id: "act1".to_string(),
            doc_id: "doc1".to_string(),
            event_type,
            actor_id: "u1".to_string(),
            detail: "{}".to_string(),
            created_at: 1_700_000_000_000_000,
        }
    }

    /// Mimic `create`'s column construction (no live table): the
    /// event type is stored as the bare serde tag string.
    fn item_for(activity: &Activity) -> HashMap<String, AttributeValue> {
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
        item
    }

    #[test]
    fn round_trip_covers_every_event_type() {
        // The write path serializes via serde and the read path
        // deserializes via serde, so a drift can only come from a
        // variant added to the enum but not re-parseable (e.g. an
        // alias-only rename). Pin all current variants.
        let cases = [
            ActivityEventType::Edit,
            ActivityEventType::Comment,
            ActivityEventType::Share,
            ActivityEventType::Open,
            ActivityEventType::Restore,
            ActivityEventType::ResolveComment,
            ActivityEventType::Delete,
            ActivityEventType::Move,
        ];
        for et in cases {
            let original = fixture(et.clone());
            let back = activity_from_item(&item_for(&original))
                .unwrap_or_else(|e| panic!("roundtrip failed for {et:?}: {e}"));
            assert_eq!(back.event_type, et, "roundtrip mismatch on {et:?}");
            assert_eq!(back.activity_id, original.activity_id);
            assert_eq!(back.doc_id, original.doc_id);
        }
    }

    #[test]
    fn legacy_lowercase_resolve_comment_still_parses() {
        // The serde alias on ResolveComment keeps pre-camelCase rows
        // readable.
        let mut item = item_for(&fixture(ActivityEventType::ResolveComment));
        item.insert(
            "event_type".to_string(),
            AttributeValue::S("resolveComment".to_string()),
        );
        assert_eq!(
            activity_from_item(&item).unwrap().event_type,
            ActivityEventType::ResolveComment
        );
    }

    #[test]
    fn unknown_event_type_errors_naming_the_field() {
        let mut item = item_for(&fixture(ActivityEventType::Edit));
        item.insert("event_type".to_string(), AttributeValue::S("explode".to_string()));
        match activity_from_item(&item) {
            Err(RepoError::MissingField(msg)) => {
                assert!(msg.contains("event_type"), "must name the field: {msg}")
            }
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn missing_detail_defaults_to_empty_string() {
        let mut item = item_for(&fixture(ActivityEventType::Open));
        item.remove("detail");
        let back = activity_from_item(&item).expect("missing detail is tolerated");
        assert_eq!(back.detail, "");
    }
}
