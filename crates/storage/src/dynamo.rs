use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
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

    /// Query items by partition key and optional sort key prefix.
    pub async fn query(
        &self,
        pk: &str,
        sk_prefix: Option<&str>,
    ) -> Result<Vec<HashMap<String, AttributeValue>>, aws_sdk_dynamodb::Error> {
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
            builder =
                builder.expression_attribute_values(":sk", AttributeValue::S(prefix.to_string()));
        }

        let result = builder.send().await.map_err(|e| e.into_service_error())?;

        Ok(result.items.unwrap_or_default())
    }

    /// Query items using a GSI.
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

        let mut builder = self
            .client
            .query()
            .table_name(&self.table_name)
            .index_name(index_name)
            .key_condition_expression(&key_condition)
            .expression_attribute_values(":pk", AttributeValue::S(pk_value.to_string()))
            .scan_index_forward(scan_forward);

        if let Some(val) = sk_value {
            builder =
                builder.expression_attribute_values(":sk", AttributeValue::S(val.to_string()));
        }

        if let Some(lim) = limit {
            builder = builder.limit(lim);
        }

        let result = builder.send().await.map_err(|e| e.into_service_error())?;

        Ok(result.items.unwrap_or_default())
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
}
