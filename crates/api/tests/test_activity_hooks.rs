// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Regressions for the activity-hook spawn sites wired in M6.
//!
//! Activity rows are written `tokio::spawn`'d fire-and-forget from the
//! event handlers, so these tests exercise the full HTTP surface and
//! assert the rows show up in `GET /documents/:id/activity`. A short
//! `tokio::time::sleep` after each mutation gives the spawn time to land
//! — longer than necessary would hide a real regression, so keep it tight.

mod common;

use hyper::Method;

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

async fn list_activity(app: &common::TestApp, token: &str, doc_id: &str) -> serde_json::Value {
    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/activity"),
            Some(token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    json
}

fn event_types(activity_json: &serde_json::Value) -> Vec<String> {
    activity_json["activities"]
        .as_array()
        .expect("activities array")
        .iter()
        .filter_map(|e| e["eventType"].as_str().map(String::from))
        .collect()
}

/// Posting a reply to an existing comment thread must record a `comment`
/// activity event — the `create_thread` hook alone misses every
/// subsequent message, which is most of what happens on an active doc.
#[tokio::test]
async fn test_reply_records_comment_activity() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("replier@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let body = serde_json::json!({ "threadType": "document" });
    let (_, tj) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/threads"),
            Some(&token),
            Some(body),
        )
        .await;
    let thread_id = tj["threadId"].as_str().unwrap().to_string();

    settle().await;
    let before = event_types(&list_activity(&app, &token, &doc_id).await);
    let comment_count_before = before.iter().filter(|t| *t == "comment").count();

    let body = serde_json::json!({ "content": "reply body" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/threads/{thread_id}/messages"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 201);

    settle().await;
    let after = event_types(&list_activity(&app, &token, &doc_id).await);
    let comment_count_after = after.iter().filter(|t| *t == "comment").count();
    assert!(
        comment_count_after > comment_count_before,
        "reply should append a comment activity row; before={before:?} after={after:?}",
    );

    app.cleanup().await;
}
