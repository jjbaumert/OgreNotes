// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use ogrenotes_common::metrics::{counter, MetricKey};
use std::collections::HashMap;

/// Wrapper around the DynamoDB client with table name management.
#[derive(Clone)]
pub struct DynamoClient {
    client: Client,
    table_name: String,
}

impl DynamoClient {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }

    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Get a single item by PK and SK.
    pub async fn get_item(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<Option<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("PK", AttributeValue::S(pk.to_string()))
            .key("SK", AttributeValue::S(sk.to_string()))
            .send()
            .await
            .map_err(|e| e.into_service_error())?;

        Ok(result.item)
    }

    /// Put a single item.
    pub async fn put_item(
        &self,
        item: HashMap<String, AttributeValue>,
    ) -> Result<(), aws_sdk_dynamodb::Error> {
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await
            .map_err(|e| e.into_service_error())?;

        Ok(())
    }

    /// Put an item with a condition expression (e.g., for conditional creates).
    pub async fn put_item_conditional(
        &self,
        item: HashMap<String, AttributeValue>,
        condition: &str,
    ) -> Result<(), aws_sdk_dynamodb::Error> {
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .condition_expression(condition)
            .send()
            .await
            .map_err(|e| e.into_service_error())?;

        Ok(())
    }

    /// Delete a single item by PK and SK.
    pub async fn delete_item(
        &self,
        pk: &str,
        sk: &str,
    ) -> Result<(), aws_sdk_dynamodb::Error> {
        self.client
            .delete_item()
            .table_name(&self.table_name)
            .key("PK", AttributeValue::S(pk.to_string()))
            .key("SK", AttributeValue::S(sk.to_string()))
            .send()
            .await
            .map_err(|e| e.into_service_error())?;

        Ok(())
    }

    /// Query items by partition key and optional sort-key prefix.
    ///
    /// **Paginates internally.** DynamoDB caps each `Query` response
    /// at 1 MB and signals continuation via `LastEvaluatedKey`; a
    /// single-page query silently truncates large result sets. The
    /// historical version of this method made a single call and
    /// returned the first page — visible to callers as "everything
    /// past the first 1 MB silently disappears." Discovered when a
    /// 3.3 MB doc-updates query returned only ~16 of 55 rows, dropping
    /// the user's most recent edits during refresh.
    ///
    /// Now loops on `last_evaluated_key` until the server reports
    /// no continuation. Emits `dynamo.query_pages_total` once per
    /// continuation so operators can see when a query is large
    /// enough to paginate; a healthy steady state is one page per
    /// query.
    pub async fn query(
        &self,
        pk: &str,
        sk_prefix: Option<&str>,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
        let mut all_items: Vec<HashMap<String, AttributeValue>> = Vec::new();
        let mut last_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let mut builder = self
                .client
                .query()
                .table_name(&self.table_name)
                .key_condition_expression(if sk_prefix.is_some() {
                    "PK = :pk AND begins_with(SK, :sk)"
                } else {
                    "PK = :pk"
                })
                .expression_attribute_values(":pk", AttributeValue::S(pk.to_string()));

            if let Some(prefix) = sk_prefix {
                builder = builder.expression_attribute_values(
                    ":sk",
                    AttributeValue::S(prefix.to_string()),
                );
            }

            if let Some(start) = last_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }

            let result = builder.send().await.map_err(|e| e.into_service_error())?;

            if let Some(items) = result.items {
                all_items.extend(items);
            }

            match result.last_evaluated_key {
                Some(key) => {
                    counter::inc(MetricKey::new("dynamo.query_pages_total", &[]));
                    last_key = Some(key);
                }
                None => break,
            }
        }

        Ok(all_items)
    }

    /// Query items using a GSI. **Paginates internally** (see
    /// `query` doc-comment for the rationale and the bug it closed).
    ///
    /// `limit` is treated as a cap on the total returned items
    /// across all pages, not a per-page limit — that matches the
    /// caller's intent ("give me at most N matches") even when
    /// the first page returns less than N due to the 1 MB size cap.
    pub async fn query_index(
        &self,
        index_name: &str,
        pk_name: &str,
        pk_value: &str,
        sk_name: Option<&str>,
        sk_value: Option<&str>,
        scan_forward: bool,
        limit: Option<i32>,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
        let key_condition = if sk_name.is_some() {
            format!("{pk_name} = :pk AND {sk} = :sk", sk = sk_name.unwrap())
        } else {
            format!("{pk_name} = :pk")
        };

        let mut all_items: Vec<HashMap<String, AttributeValue>> = Vec::new();
        let mut last_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let mut builder = self
                .client
                .query()
                .table_name(&self.table_name)
                .index_name(index_name)
                .key_condition_expression(&key_condition)
                .expression_attribute_values(":pk", AttributeValue::S(pk_value.to_string()))
                .scan_index_forward(scan_forward);

            if let Some(val) = sk_value {
                builder = builder.expression_attribute_values(
                    ":sk",
                    AttributeValue::S(val.to_string()),
                );
            }

            // Page-size limit: ask for at most the remaining count
            // toward the caller's overall cap, so a 50-item cap
            // never fetches a fresh full 1 MB page on the last call.
            if let Some(cap) = limit {
                let remaining = cap.saturating_sub(all_items.len() as i32);
                if remaining <= 0 {
                    break;
                }
                builder = builder.limit(remaining);
            }

            if let Some(start) = last_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }

            let result = builder.send().await.map_err(|e| e.into_service_error())?;

            if let Some(items) = result.items {
                all_items.extend(items);
            }

            if let Some(cap) = limit {
                if all_items.len() as i32 >= cap {
                    all_items.truncate(cap as usize);
                    break;
                }
            }

            match result.last_evaluated_key {
                Some(key) => {
                    counter::inc(MetricKey::new("dynamo.query_pages_total", &[]));
                    last_key = Some(key);
                }
                None => break,
            }
        }

        Ok(all_items)
    }

    /// Update an item with an update expression.
    pub async fn update_item(
        &self,
        pk: &str,
        sk: &str,
        update_expression: &str,
        expression_values: HashMap<String, AttributeValue>,
        expression_names: Option<HashMap<String, String>>,
    ) -> Result<(), aws_sdk_dynamodb::Error> {
        let mut builder = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("PK", AttributeValue::S(pk.to_string()))
            .key("SK", AttributeValue::S(sk.to_string()))
            .update_expression(update_expression);

        for (k, v) in expression_values {
            builder = builder.expression_attribute_values(k, v);
        }

        if let Some(names) = expression_names {
            for (k, v) in names {
                builder = builder.expression_attribute_names(k, v);
            }
        }

        builder.send().await.map_err(|e| e.into_service_error())?;

        Ok(())
    }

    /// Update an item guarded by a condition expression.
    ///
    /// Returns `Ok(true)` when the update applied, `Ok(false)` when the
    /// condition was not met (`ConditionalCheckFailedException` — an
    /// expected outcome for callers that race or cap, not an error), and
    /// `Err` for any other DynamoDB failure. Mirrors `update_item` but adds
    /// the condition so L3 callers (e.g. the email-cap counter) don't have
    /// to reach for the raw SDK to express a guarded `ADD`.
    pub async fn update_item_conditional(
        &self,
        pk: &str,
        sk: &str,
        update_expression: &str,
        condition_expression: &str,
        expression_values: HashMap<String, AttributeValue>,
        expression_names: Option<HashMap<String, String>>,
    ) -> Result<bool, aws_sdk_dynamodb::Error> {
        let mut builder = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("PK", AttributeValue::S(pk.to_string()))
            .key("SK", AttributeValue::S(sk.to_string()))
            .update_expression(update_expression)
            .condition_expression(condition_expression);

        for (k, v) in expression_values {
            builder = builder.expression_attribute_values(k, v);
        }

        if let Some(names) = expression_names {
            for (k, v) in names {
                builder = builder.expression_attribute_names(k, v);
            }
        }

        match builder.send().await {
            Ok(_) => Ok(true),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_conditional_check_failed_exception() {
                    Ok(false)
                } else {
                    Err(svc.into())
                }
            }
        }
    }

    /// Execute a transactional write of multiple items (all-or-nothing).
    pub async fn transact_write(
        &self,
        items: Vec<aws_sdk_dynamodb::types::TransactWriteItem>,
    ) -> Result<(), aws_sdk_dynamodb::Error> {
        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await
            .map_err(|e| e.into_service_error())?;

        Ok(())
    }

    /// Scan the table with a filter expression on a single attribute.
    /// Used as a fallback when a GSI doesn't exist.
    ///
    /// Hard limit (#39): `max_items` caps the total number of items
    /// returned across paginated requests. The function walks
    /// continuation tokens internally up to `max_items` and then
    /// stops, returning `(items, truncated)`. `truncated == true`
    /// means more matching rows exist beyond the cap; the caller is
    /// responsible for deciding whether to surface that as an error,
    /// log it, or accept it.
    ///
    /// Pre-merge this function called `.send()` exactly once and
    /// returned whatever DDB fit in a single 1 MB Query response.
    /// Past that bound the result was silently truncated. Worse, a
    /// hypothetical caller that walked tokens manually could OOM on
    /// a million-row scan. The explicit cap fixes both.
    ///
    /// Scans remain the most expensive DDB read pattern. Every
    /// existing call site should migrate to a GSI query once the
    /// GSI exists; this signature is a transitional bound, not a
    /// blessed pattern.
    pub async fn scan_with_filter(
        &self,
        attr_name: &str,
        attr_value: &str,
        max_items: usize,
    ) -> Result<(Vec<HashMap<String, AttributeValue>>, bool), aws_sdk_dynamodb::Error> {
        let mut items: Vec<HashMap<String, AttributeValue>> = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut builder = self
                .client
                .scan()
                .table_name(&self.table_name)
                .filter_expression("#attr = :val")
                .expression_attribute_names("#attr", attr_name)
                .expression_attribute_values(":val", AttributeValue::S(attr_value.to_string()));
            if let Some(key) = start_key.clone() {
                builder = builder.set_exclusive_start_key(Some(key));
            }
            let result = builder.send().await.map_err(|e| e.into_service_error())?;
            if let Some(page) = result.items {
                let remaining = max_items.saturating_sub(items.len());
                if page.len() > remaining {
                    items.extend(page.into_iter().take(remaining));
                    // Either we filled the cap exactly or more rows
                    // were on this very page than would fit.
                    return Ok((items, true));
                }
                items.extend(page);
            }
            match result.last_evaluated_key {
                Some(key) if items.len() < max_items => start_key = Some(key),
                Some(_) => {
                    // More pages exist but we've already hit the cap.
                    return Ok((items, true));
                }
                None => return Ok((items, false)),
            }
        }
    }
}
