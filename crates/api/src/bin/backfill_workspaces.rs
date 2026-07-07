// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Backfill workspace assignments for pre-M1 data.
//!
//! Scans users without a `default_workspace_id` and creates one for each.
//! Then scans documents without a `workspace_id` and assigns each to its
//! owner's default workspace. Safe to re-run; no-ops on already-assigned rows.
//!
//! Usage:
//!   DYNAMODB_TABLE_PREFIX=ogrenotes \
//!   S3_BUCKET=ogrenotes-dev \
//!   AWS_REGION=us-east-1 \
//!   cargo run --bin backfill_workspaces
//!
//! Pass `--dry-run` to log what would change without writing.
//!
//! **Run against a quiesced instance.** Pass 1 builds an in-memory map of
//! user_id → default_workspace_id, and pass 2 uses that map to assign
//! workspaces to docs. A signup that completes between the two passes will
//! have its `default_workspace_id` written by `find_or_create_user` but
//! won't be in this map, so any docs it creates during pass 2 are logged as
//! orphaned and skipped. Subsequent re-runs will pick them up, but for a
//! single-pass run you want no live writers.

use ogrenotes_api::backfill::run_backfill_workspaces;
use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;
use ogrenotes_storage::s3::S3Client;
use std::env;

#[tokio::main]
async fn main() {
    let dry_run = env::args().any(|a| a == "--dry-run");

    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let bucket = env::var("S3_BUCKET").expect("S3_BUCKET is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Backfill workspace_id (dry_run={dry_run})");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let s3 = S3Client::new(aws_sdk_s3::Client::new(&aws_config), bucket);

    let user_repo = UserRepo::new(dynamo.clone());
    let workspace_repo = WorkspaceRepo::new(dynamo.clone());
    let doc_repo = DocRepo::new(dynamo, s3);

    let stats = run_backfill_workspaces(&user_repo, &workspace_repo, &doc_repo, dry_run)
        .await
        .expect("backfill_workspaces");

    println!(
        "Users: seen={}, workspaces_created={}",
        stats.users_seen, stats.workspaces_created
    );
    println!(
        "Docs: seen={}, assigned={}, skipped_orphan={}",
        stats.docs_seen, stats.docs_assigned, stats.docs_skipped_no_owner_ws
    );
    println!("Backfill complete.");
}
