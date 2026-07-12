// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration test for the chat-membership-edges backfill migration
//! (`ThreadRepo::backfill_chat_edges`, issue #34).
//!
//! `list_user_chats` now reads reverse `PK=USER#<uid>, SK=CHAT#<thread_id>`
//! edges instead of scanning the whole table for `member_ids`. A chat
//! created before the cutover (or written by any path that bypasses
//! `create_thread`/`add_chat_member`) has a METADATA row with `member_ids`
//! but no edge rows, so it would silently vanish from every member's chat
//! list. This drives the repair end to end: seed a "legacy" chat with no
//! edges, confirm it's invisible, run the backfill, confirm it reappears
//! for every member, then re-run the backfill to pin idempotency.

mod common;

use aws_sdk_dynamodb::types::AttributeValue;
use hyper::Method;

#[tokio::test]
async fn backfill_chat_edges_repairs_legacy_chat_and_is_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (bob_id, token_b) = app.create_user("bob@test.com").await;

    let thread_id = "legacy-chat-backfill";
    let now = ogrenotes_common::time::now_usec();

    // Seed a "legacy" chat: write the THREAD#<id>/METADATA row with
    // member_ids directly via the raw DynamoClient, bypassing
    // `create_thread` (which now writes edges as it goes). This
    // reproduces every chat that existed before the #34 cutover.
    app.dynamo_client()
        .put_item()
        .table_name(&app.table_name)
        .item("PK", AttributeValue::S(format!("THREAD#{thread_id}")))
        .item("SK", AttributeValue::S("METADATA".to_string()))
        .item("doc_id", AttributeValue::S(String::new()))
        .item("thread_type", AttributeValue::S("chat".to_string()))
        .item("status", AttributeValue::S("open".to_string()))
        .item("created_by", AttributeValue::S(alice_id.clone()))
        .item("title", AttributeValue::S("Legacy Room".to_string()))
        .item(
            "member_ids",
            AttributeValue::Ss(vec![alice_id.clone(), bob_id.clone()]),
        )
        .item("created_at", AttributeValue::N(now.to_string()))
        .item("updated_at", AttributeValue::N(now.to_string()))
        .send()
        .await
        .expect("seed legacy chat METADATA row");

    // Precondition: no edge rows exist yet, so the legacy chat is
    // invisible to both members via the normal API surface.
    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    assert!(
        !json["chats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == thread_id),
        "precondition: legacy chat must not be visible to alice before backfill: {json}"
    );

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/chats", Some(&token_b), None)
        .await;
    assert_eq!(status, 200);
    assert!(
        !json["chats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == thread_id),
        "precondition: legacy chat must not be visible to bob before backfill: {json}"
    );

    // Run the backfill: it scans every METADATA row with member_ids and
    // writes the missing edges.
    let first = app
        .state
        .thread_repo
        .backfill_chat_edges()
        .await
        .expect("backfill run 1");
    assert_eq!(first.0, 1, "must scan the one legacy chat's METADATA row: {first:?}");
    assert_eq!(first.1, 2, "must write one edge per member (alice, bob): {first:?}");

    // The chat now appears for both members.
    for (label, token) in [("alice", &token_a), ("bob", &token_b)] {
        let (status, json) = app
            .json_request(Method::GET, "/api/v1/chats", Some(token), None)
            .await;
        assert_eq!(status, 200);
        assert!(
            json["chats"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == thread_id),
            "legacy chat must be visible to {label} after backfill: {json}"
        );
    }

    // Re-run: idempotent — no error, and the visible result is unchanged
    // (edges are put-overwrites, so the second pass rewrites the same rows).
    let second = app
        .state
        .thread_repo
        .backfill_chat_edges()
        .await
        .expect("backfill run 2 must not error");
    assert_eq!(second.0, 1, "re-run must scan the same one legacy chat: {second:?}");
    assert_eq!(second.1, 2, "re-run rewrites the same two edges: {second:?}");

    for (label, token) in [("alice", &token_a), ("bob", &token_b)] {
        let (status, json) = app
            .json_request(Method::GET, "/api/v1/chats", Some(token), None)
            .await;
        assert_eq!(status, 200);
        assert!(
            json["chats"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == thread_id),
            "legacy chat must still be visible to {label} after the second backfill run: {json}"
        );
    }

    app.cleanup().await;
}
