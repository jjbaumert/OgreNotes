use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use std::env;

#[tokio::main]
async fn main() {
    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let bucket = env::var("S3_BUCKET").expect("S3_BUCKET is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = format!("{table_prefix}ogrenotes");

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
