// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Backfill SNAPSHOT# rows for orphaned S3 snapshot blobs.
//!
//! Before commits 1748412..d8885d9 only the idle-doc compaction task wrote
//! a SNAPSHOT# row alongside its S3 PUT. Initial doc creation, REST content
//! saves, restores, and imports all left the blob in S3 without a matching
//! DynamoDB row, so `GET /documents/:id/versions` underreported the
//! version list. This script walks every `docs/{doc_id}/snapshots/*.bin`
//! object in S3 and writes the missing SNAPSHOT# rows.
//!
//! Idempotent: an object whose row already exists is skipped. Safe to
//! re-run.
//!
//! Synthetic rows are attributed to `user_id = "system"` and use the S3
//! object's `LastModified` as `created_at` (microseconds since epoch).
//!
//! Usage:
//!   DYNAMODB_TABLE_PREFIX=ogrenotes \
//!   S3_BUCKET=ogrenotes-dev \
//!   AWS_REGION=us-east-1 \
//!   cargo run --bin backfill_snapshots
//!
//! Pass `--dry-run` to log what would change without writing.

use ogrenotes_api::backfill::run_backfill_snapshots;
use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::snapshot_repo::SnapshotRepo;
use std::env;

#[tokio::main]
async fn main() {
    let dry_run = env::args().any(|a| a == "--dry-run");

    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let bucket = env::var("S3_BUCKET").expect("S3_BUCKET is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Backfill SNAPSHOT# rows (dry_run={dry_run})");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");
    println!("  Bucket: {bucket}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let s3_client = aws_sdk_s3::Client::new(&aws_config);
    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let snapshot_repo = SnapshotRepo::new(dynamo);

    let stats = run_backfill_snapshots(&s3_client, &bucket, &snapshot_repo, dry_run)
        .await
        .expect("backfill_snapshots");

    println!(
        "Done. seen={} already_present={} inserted={} skipped_bad_key={} errors={}",
        stats.seen, stats.already_present, stats.inserted, stats.skipped_bad_key, stats.errors
    );
}
