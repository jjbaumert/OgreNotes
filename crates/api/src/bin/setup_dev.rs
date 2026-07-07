// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use ogrenotes_common::config::table_name_for_prefix;
use std::env;

#[tokio::main]
async fn main() {
    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let bucket = env::var("S3_BUCKET").expect("S3_BUCKET is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    // Use the shared helper so this binary and the server can never disagree
    // on the table name. A previous rename left the two out of sync and
    // silently broke CI for four days.
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Setting up dev resources...");
    println!("  Region:     {region}");
    println!("  Table:      {table_name}");
    println!("  Bucket:     {bucket}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = aws_sdk_dynamodb::Client::new(&aws_config);
    let s3 = aws_sdk_s3::Client::new(&aws_config);

    // Create DynamoDB table
    match dynamo
        .create_table()
        .table_name(&table_name)
        .billing_mode(BillingMode::PayPerRequest)
        // Key schema
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("PK")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("SK")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        // Attribute definitions
        .attribute_definitions(attr_def("PK", ScalarAttributeType::S))
        .attribute_definitions(attr_def("SK", ScalarAttributeType::S))
        .attribute_definitions(attr_def("owner_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("updated_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("parent_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("title", ScalarAttributeType::S))
        .attribute_definitions(attr_def("doc_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("workspace_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("user_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("created_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("external_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("is_deleted_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("deleted_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("actor_id_gsi", ScalarAttributeType::S))
        // GSI1: owner -> updated_at (list user's docs by recent)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI1-owner-updated")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("owner_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("updated_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI2: parent -> title (list folder children alphabetically)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI2-parent-title")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("parent_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("title")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI3: workspace_id -> updated_at (list workspace docs by recent)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI3-workspace-updated")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("workspace_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("updated_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI4: user_id -> created_at (list user's workspace memberships, sessions, activity)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI4-user-created")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("user_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("created_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI5: doc_id -> updated_at (per-document threads/activity)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI5-docid-updated")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("doc_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("updated_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI6: external_id (sparse, hash-only) — SCIM `externalId`
        // dedupe and SAML JIT lookup. PK only; SCIM filters are
        // equality matches, no range needed.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI6-external-id")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("external_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI7: sparse soft-deleted index for the M-E7 trash-cleanup
        // worker. Hash on a constant "deleted" partition, range on
        // deleted_at (microseconds since epoch). All soft-deleted
        // docs share one partition — see IS_DELETED_GSI_PARTITION
        // in doc_repo for the scaling notes.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI7-deleted-at")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("is_deleted_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("deleted_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        // GSI8: actor_id -> created_at (#49). Actor-centric forensic
        // view of AdminAudit rows — "every action admin Y took since T"
        // without scanning the target-keyed table. Sparse: only audit
        // rows set `actor_id_gsi`.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI8-actor-created")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("actor_id_gsi")
                        .key_type(KeyType::Hash)
                        .build()
                        .unwrap(),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("created_at")
                        .key_type(KeyType::Range)
                        .build()
                        .unwrap(),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .unwrap(),
        )
        .send()
        .await
    {
        Ok(_) => println!("  DynamoDB table created: {table_name}"),
        Err(e) => {
            let service_err = e.into_service_error();
            if service_err.is_resource_in_use_exception() {
                println!("  DynamoDB table already exists: {table_name}");
            } else {
                eprintln!("  Failed to create DynamoDB table: {service_err}");
                eprintln!("  Debug: {service_err:?}");
                eprintln!();
                eprintln!("  Common causes:");
                eprintln!("    - AWS credentials not configured (run: aws configure --profile {}))",
                    std::env::var("AWS_PROFILE").unwrap_or_else(|_| "default".into()));
                eprintln!("    - Insufficient IAM permissions (need dynamodb:CreateTable)");
                eprintln!("    - Wrong AWS_REGION");
                std::process::exit(1);
            }
        }
    }

    // Create S3 bucket
    match s3.create_bucket().bucket(&bucket).send().await {
        Ok(_) => println!("  S3 bucket created: {bucket}"),
        Err(e) => {
            let service_err = e.into_service_error();
            let err_str = service_err.to_string();
            if err_str.contains("BucketAlreadyOwnedByYou")
                || err_str.contains("BucketAlreadyExists")
            {
                println!("  S3 bucket already exists: {bucket}");
            } else {
                eprintln!("  Failed to create S3 bucket: {err_str}");
                eprintln!("  Debug: {service_err:?}");
                std::process::exit(1);
            }
        }
    }

    // Block public access on the bucket
    match s3
        .put_public_access_block()
        .bucket(&bucket)
        .public_access_block_configuration(
            aws_sdk_s3::types::PublicAccessBlockConfiguration::builder()
                .block_public_acls(true)
                .ignore_public_acls(true)
                .block_public_policy(true)
                .restrict_public_buckets(true)
                .build(),
        )
        .send()
        .await
    {
        Ok(_) => println!("  Public access blocked on: {bucket}"),
        Err(e) => eprintln!("  Warning: failed to set public access block: {e}"),
    }

    println!("Dev setup complete.");
}

fn attr_def(name: &str, attr_type: ScalarAttributeType) -> AttributeDefinition {
    AttributeDefinition::builder()
        .attribute_name(name)
        .attribute_type(attr_type)
        .build()
        .unwrap()
}
