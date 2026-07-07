// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E1 piece D: backfill `User.role` for pre-Phase-4 rows.
//!
//! After piece A landed, `user_from_item` requires the `role`
//! attribute on every PROFILE row and fails loud on its absence.
//! That makes the new code a release gate: production must run this
//! binary BEFORE deploying piece-A code, or every login / admin
//! action will surface `RepoError::MissingField("role")` until the
//! migration completes.
//!
//! The actual loop lives in `ogrenotes_api::backfill::run_backfill_user_role`
//! so the integration test (`tests/test_backfill_user_role.rs`) can
//! exercise the same logic against a real DynamoDB table.
//!
//! Usage:
//!   DYNAMODB_TABLE_PREFIX=ogrenotes \
//!   AWS_REGION=us-east-1 \
//!   cargo run --bin backfill_user_role
//!
//! Pass `--dry-run` to log what would change without writing. Safe
//! to re-run; rows that already have `role` are skipped.

use std::env;

use ogrenotes_api::backfill::run_backfill_user_role;
use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::user_repo::UserRepo;

#[tokio::main]
async fn main() {
    let dry_run = env::args().any(|a| a == "--dry-run");

    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Backfill user.role (dry_run={dry_run})");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let user_repo = UserRepo::new(dynamo.clone());

    let stats = run_backfill_user_role(&user_repo, &dynamo, dry_run)
        .await
        .expect("backfill scan failed");

    println!(
        "Users: scanned={}, already_migrated={}, set_admin={}, set_user={}, errors={}",
        stats.scanned,
        stats.already_migrated,
        stats.set_admin,
        stats.set_user,
        stats.errors,
    );
    if stats.errors > 0 {
        // Non-zero exit so CI / deploy scripts can gate on it. The
        // backfill is idempotent so re-running picks up retries.
        std::process::exit(1);
    }
    println!("Backfill complete.");
}
