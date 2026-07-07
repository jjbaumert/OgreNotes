// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration test for the workspace backfill migration
//! (`ogrenotes_api::backfill::run_backfill_workspaces`).
//!
//! This is a DESTRUCTIVE two-table migration (creates Workspace rows + writes
//! default_workspace_id + assigns docs), so the headline property is
//! idempotency: a second run must create and assign nothing. The logic lives
//! in the library (not the binary's `main`) so this can drive it against a
//! real DynamoDB table, the same pattern as `test_backfill_user_role`.

mod common;

use aws_sdk_dynamodb::types::AttributeValue;

use ogrenotes_api::backfill::run_backfill_workspaces;

/// Strip the workspace attributes a modern signup writes, returning the row
/// to its pre-migration shape (user without default_workspace_id, doc without
/// workspace_id).
async fn make_pre_migration(app: &common::TestApp, user_id: &str, doc_id: &str) {
    app.dynamo_client()
        .update_item()
        .table_name(&app.table_name)
        .key("PK", AttributeValue::S(format!("USER#{user_id}")))
        .key("SK", AttributeValue::S("PROFILE".to_string()))
        .update_expression("REMOVE default_workspace_id")
        .send()
        .await
        .expect("strip default_workspace_id");
    app.dynamo_client()
        .update_item()
        .table_name(&app.table_name)
        .key("PK", AttributeValue::S(format!("DOC#{doc_id}")))
        .key("SK", AttributeValue::S("METADATA".to_string()))
        .update_expression("REMOVE workspace_id, workspace_id_gsi")
        .send()
        .await
        .expect("strip workspace_id");
}

#[tokio::test]
async fn backfill_creates_workspace_assigns_doc_and_is_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("ws-backfill@test.com").await;
    let doc_id = app.create_doc(&token, "Pre-migration Doc", None).await;

    // Roll the rows back to the pre-workspace schema.
    make_pre_migration(&app, &user_id, &doc_id).await;
    assert!(
        app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap()
            .default_workspace_id.is_none(),
        "precondition: user has no default workspace"
    );
    assert!(
        app.state.doc_repo.get(&doc_id).await.unwrap().unwrap().workspace_id.is_none(),
        "precondition: doc has no workspace"
    );

    // First run: creates a workspace for the user and assigns it to the doc.
    let first = run_backfill_workspaces(
        &app.state.user_repo,
        &app.state.workspace_repo,
        &app.state.doc_repo,
        false,
    )
    .await
    .expect("backfill run 1");
    assert!(first.workspaces_created >= 1, "must create the missing workspace: {first:?}");
    assert!(first.docs_assigned >= 1, "must assign the doc to its owner's workspace: {first:?}");

    // The rows now carry their workspace.
    let user = app.state.user_repo.get_by_id(&user_id).await.unwrap().unwrap();
    let ws = user.default_workspace_id.clone().expect("user now has a default workspace");
    let doc = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap();
    assert_eq!(
        doc.workspace_id.as_deref(),
        Some(ws.as_str()),
        "doc must be assigned to the owner's default workspace"
    );

    // Second run: nothing left to migrate — the idempotency contract.
    let second = run_backfill_workspaces(
        &app.state.user_repo,
        &app.state.workspace_repo,
        &app.state.doc_repo,
        false,
    )
    .await
    .expect("backfill run 2");
    assert_eq!(second.workspaces_created, 0, "re-run must create no workspaces: {second:?}");
    assert_eq!(second.docs_assigned, 0, "re-run must assign no docs: {second:?}");

    app.cleanup().await;
}
