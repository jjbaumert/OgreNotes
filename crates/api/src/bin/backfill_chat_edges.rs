// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Backfill chat-membership edges for issue #34.
//!
//! `list_user_chats` now reads reverse edge rows
//! (`PK=USER#<uid>, SK=CHAT#<thread_id>`) instead of scanning the whole
//! table for `contains(member_ids, uid)`. Existing chat/DM threads
//! predate those edges, so this one-time job emits an edge for every
//! current member. Idempotent (edges are put-overwrites) — safe to
//! re-run.
//!
//! **Run this at the #34 cutover, before/with the deploy that ships the
//! new `list_user_chats`**, or existing chats briefly vanish from users'
//! lists until an edge is written by a later add/remove.
//!
//! Usage:
//!   DYNAMODB_TABLE_PREFIX=ogrenotes \
//!   AWS_REGION=us-east-1 \
//!   cargo run --bin backfill_chat_edges

use ogrenotes_common::config::table_name_for_prefix;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::thread_repo::ThreadRepo;
use std::env;

#[tokio::main]
async fn main() {
    let table_prefix =
        env::var("DYNAMODB_TABLE_PREFIX").expect("DYNAMODB_TABLE_PREFIX is required");
    let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let table_name = table_name_for_prefix(&table_prefix);

    println!("Backfill chat-membership edges (#34)");
    println!("  Region: {region}");
    println!("  Table:  {table_name}");

    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(region))
        .load()
        .await;

    let dynamo = DynamoClient::new(aws_sdk_dynamodb::Client::new(&aws_config), table_name);
    let thread_repo = ThreadRepo::new(dynamo);

    let (scanned, written) = thread_repo
        .backfill_chat_edges()
        .await
        .expect("backfill_chat_edges");

    println!("Threads with members scanned: {scanned}");
    println!("Membership edges written:     {written}");
    println!("Backfill complete.");
}
