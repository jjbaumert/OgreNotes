// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Seed the fixed set of sample templates that every user's Template
//! gallery pulls in. Idempotent — the seed is keyed off stable
//! `sample-<sample_id>` doc ids, so re-runs skip existing rows.
//!
//! Usage:
//!
//! ```text
//! DYNAMODB_TABLE_PREFIX=ogrenotes \
//! S3_BUCKET=ogrenotes-dev \
//! AWS_REGION=us-east-1 \
//! cargo run --bin seed_sample_templates
//! ```
//!
//! Pass `--dry-run` to log what would change without writing.
//! Pass `--force` to rewrite the snapshot of every existing sample so a
//! fixture edit propagates (without `--force` a fixture change is a
//! silent no-op — the doc-id idempotency check considers the row
//! "already provisioned" regardless of content).
//!
//! This provisions the well-known SAMPLES_SYSTEM_USER_ID user and
//! SAMPLES_WORKSPACE_ID workspace on first run and then imports the
//! HTML fixtures under `crates/api/src/seed/samples/` as regular
//! `is_template = true` documents in that workspace. `list_templates`
//! queries this workspace unconditionally, so every user sees them.

use ogrenotes_api::seed::run_seed_samples;
use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::security_audit_repo::SecurityAuditRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;
use ogrenotes_storage::s3::S3Client;
use std::env;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let dry_run = env::args().any(|a| a == "--dry-run");
    let force = env::args().any(|a| a == "--force");

    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let bucket = env::var("S3_BUCKET").expect("S3_BUCKET is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Seed sample templates (dry_run={dry_run} force={force})");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");
    println!("  Bucket: {bucket}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let s3 = S3Client::new(aws_sdk_s3::Client::new(&aws_config), bucket);

    let user_repo = UserRepo::new(dynamo.clone());
    let workspace_repo = WorkspaceRepo::new(dynamo.clone());
    let folder_repo = FolderRepo::new(dynamo.clone());
    let security_audit_repo = SecurityAuditRepo::new(dynamo.clone());
    let doc_repo = Arc::new(DocRepo::new(dynamo, s3));

    let stats = run_seed_samples(
        &user_repo,
        &workspace_repo,
        &doc_repo,
        &folder_repo,
        &security_audit_repo,
        dry_run,
        force,
    )
        .await
        .expect("seed_sample_templates");

    println!("---");
    println!(
        "user_created={} workspace_created={} templates_created={} templates_skipped_existing={} templates_refreshed={}",
        stats.user_created,
        stats.workspace_created,
        stats.templates_created,
        stats.templates_skipped_existing,
        stats.templates_refreshed,
    );
    println!("Seed complete.");
}
