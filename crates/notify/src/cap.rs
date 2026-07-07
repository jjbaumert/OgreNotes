// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Per-user per-day email cap. Prevents runaway sends if a single user is
//! the target of a noisy burst (e.g. hundreds of comments in an hour).
//!
//! Storage: `PK=USER#<user_id>`, `SK=EMAIL_CAP#<yyyy-mm-dd>` (UTC date).
//! The row carries a single `count` attribute that's atomically
//! incremented via `UpdateItem ADD count :one` guarded by a
//! `count < :cap` condition. Exceeding the cap fails the conditional
//! write and returns `Ok(false)` without mutating state.
//!
//! A new date starts a fresh row, so yesterday's hits don't consume
//! today's quota.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::Utc;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::RepoError;

pub struct EmailCapRepo {
    db: DynamoClient,
    cap: u32,
}

impl EmailCapRepo {
    pub fn new(db: DynamoClient, cap: u32) -> Self {
        Self { db, cap }
    }

    /// Atomically try to reserve one slot in today's quota.
    /// Returns `Ok(true)` on success (counter incremented) or `Ok(false)`
    /// if the cap is already full. Any other DynamoDB error propagates.
    pub async fn increment_if_under_cap(&self, user_id: &str) -> Result<bool, RepoError> {
        let pk = format!("USER#{user_id}");
        let sk = format!("EMAIL_CAP#{}", Utc::now().format("%Y-%m-%d"));

        let values = HashMap::from([
            (":one".to_string(), AttributeValue::N("1".to_string())),
            (":cap".to_string(), AttributeValue::N(self.cap.to_string())),
        ]);
        let names = HashMap::from([("#c".to_string(), "count".to_string())]);

        self.db
            .update_item_conditional(
                &pk,
                &sk,
                "ADD #c :one",
                "attribute_not_exists(#c) OR #c < :cap",
                values,
                Some(names),
            )
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}
