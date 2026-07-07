// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Repository for per-workspace SAML IdP configuration (Phase 4
//! M-E4). One row per workspace; get / put (upsert) / delete.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use super::{get_n, get_s, RepoError};
use crate::dynamo::DynamoClient;
use crate::models::workspace_saml_config::WorkspaceSamlConfig;

pub struct WorkspaceSamlConfigRepo {
    db: DynamoClient,
}

impl WorkspaceSamlConfigRepo {
    pub fn new(db: DynamoClient) -> Self {
        Self { db }
    }

    /// Upsert the SAML config for a workspace. Replaces any prior
    /// row — there's exactly one IdP per workspace in v1.
    pub async fn put(&self, config: &WorkspaceSamlConfig) -> Result<(), RepoError> {
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(config.pk()));
        item.insert(
            "SK".to_string(),
            AttributeValue::S(WorkspaceSamlConfig::sk().to_string()),
        );
        item.insert(
            "workspace_id".to_string(),
            AttributeValue::S(config.workspace_id.clone()),
        );
        item.insert(
            "idp_entity_id".to_string(),
            AttributeValue::S(config.idp_entity_id.clone()),
        );
        item.insert(
            "idp_metadata_xml".to_string(),
            AttributeValue::S(config.idp_metadata_xml.clone()),
        );
        item.insert(
            "attribute_email".to_string(),
            AttributeValue::S(config.attribute_email.clone()),
        );
        item.insert(
            "attribute_name".to_string(),
            AttributeValue::S(config.attribute_name.clone()),
        );
        item.insert(
            "created_at".to_string(),
            AttributeValue::N(config.created_at.to_string()),
        );
        item.insert(
            "updated_at".to_string(),
            AttributeValue::N(config.updated_at.to_string()),
        );
        self.db
            .put_item(item)
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }

    pub async fn get(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceSamlConfig>, RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        let item = self
            .db
            .get_item(&pk, WorkspaceSamlConfig::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))?;
        match item {
            Some(it) => Ok(Some(from_item(&it)?)),
            None => Ok(None),
        }
    }

    pub async fn delete(&self, workspace_id: &str) -> Result<(), RepoError> {
        let pk = format!("WORKSPACE#{workspace_id}");
        self.db
            .delete_item(&pk, WorkspaceSamlConfig::sk())
            .await
            .map_err(|e| RepoError::Dynamo(e.to_string()))
    }
}

fn from_item(item: &HashMap<String, AttributeValue>) -> Result<WorkspaceSamlConfig, RepoError> {
    Ok(WorkspaceSamlConfig {
        workspace_id: get_s(item, "workspace_id")?,
        idp_entity_id: get_s(item, "idp_entity_id")?,
        idp_metadata_xml: get_s(item, "idp_metadata_xml")?,
        attribute_email: get_s(item, "attribute_email")?,
        attribute_name: get_s(item, "attribute_name")?,
        created_at: get_n(item, "created_at")?,
        updated_at: get_n(item, "updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure-serialization round trip; live-DDB exercise lands in the
    /// integration test that drives the admin route.
    #[test]
    fn from_item_round_trips_put_shape() {
        let config = WorkspaceSamlConfig {
            workspace_id: "ws-1".to_string(),
            idp_entity_id: "https://idp.example.com/metadata".to_string(),
            idp_metadata_xml: "<EntityDescriptor/>".to_string(),
            attribute_email: "email".to_string(),
            attribute_name: "name".to_string(),
            created_at: 1_700_000_000_000_000,
            updated_at: 1_700_000_000_000_000,
        };
        let mut item = HashMap::new();
        item.insert("PK".to_string(), AttributeValue::S(config.pk()));
        item.insert(
            "SK".to_string(),
            AttributeValue::S(WorkspaceSamlConfig::sk().to_string()),
        );
        item.insert("workspace_id".to_string(), AttributeValue::S(config.workspace_id.clone()));
        item.insert("idp_entity_id".to_string(), AttributeValue::S(config.idp_entity_id.clone()));
        item.insert("idp_metadata_xml".to_string(), AttributeValue::S(config.idp_metadata_xml.clone()));
        item.insert("attribute_email".to_string(), AttributeValue::S(config.attribute_email.clone()));
        item.insert("attribute_name".to_string(), AttributeValue::S(config.attribute_name.clone()));
        item.insert("created_at".to_string(), AttributeValue::N(config.created_at.to_string()));
        item.insert("updated_at".to_string(), AttributeValue::N(config.updated_at.to_string()));
        let back = from_item(&item).unwrap();
        assert_eq!(back, config);
    }
}
