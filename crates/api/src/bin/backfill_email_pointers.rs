// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Backfill email→user pointer items for issue #36.
//!
//! `get_by_email` now resolves an `EMAIL#<lowercased> → user_id` pointer
//! with a single `GetItem` instead of scanning the whole table. Users
//! created before #36 predate those pointers, so this one-time job writes
//! a pointer for every existing PROFILE row. Idempotent (pointers are
//! put-overwrites) — safe to re-run.
//!
//! Correctness does not depend on this job — `get_by_email` falls back to
//! the legacy scan on a pointer miss — but the cost win (no table scan on
//! slash-command handle resolution / the users-by-email endpoint) only
//! lands once existing users are backfilled.
//!
//! Usage:
//!   DYNAMODB_TABLE_PREFIX=ogrenotes \
//!   AWS_REGION=us-east-1 \
//!   cargo run --bin backfill_email_pointers

use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::user_repo::UserRepo;
use std::env;

#[tokio::main]
async fn main() {
    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Backfill email→user pointers (#36)");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let user_repo = UserRepo::new(dynamo);

    let (scanned, written) = user_repo
        .backfill_email_pointers()
        .await
        .expect("backfill_email_pointers");

    println!("Profiles scanned:  {scanned}");
    println!("Pointers written:  {written}");
    println!("Backfill complete.");
}
