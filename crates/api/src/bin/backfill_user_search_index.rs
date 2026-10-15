// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Build the durable workspace-scoped user substring index (#35).
//!
//! Deploy the writer changes first, run this command once against the
//! deployment's DynamoDB table, then switch API traffic to indexed search.
//! It is idempotent and can be rerun to repair interrupted writes. A missing
//! posting is never repaired by a request-time table scan.

use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::user_repo::UserRepo;
use std::env;

#[tokio::main]
async fn main() {
    let prefix = env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table = table_name_for_prefix(&prefix);
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;
    let repo = UserRepo::new(DynamoClient::new(
        aws_sdk_dynamodb::Client::new(&config),
        table,
    ));
    let count = repo
        .backfill_user_search_index()
        .await
        .expect("backfill_user_search_index");
    println!("Indexed {count} user profiles");
}
