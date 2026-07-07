// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for the Phase 4 M-E1 piece D backfill binary.
//!
//! Exercises `ogrenotes_api::backfill::run_backfill_user_role` against a
//! real DynamoDB table created by the test harness. The plan calls
//! out one specific assertion: an existing admin must keep
//! `is_admin() == true` after the migration. That's the headline test
//! (`admin_keeps_admin_status_post_backfill`); the rest covers the
//! idempotence, error-counting, and non-admin paths the production
//! deploy gate depends on.

mod common;

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use ogrenotes_api::backfill::run_backfill_user_role;
use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::dynamo::DynamoClient;

/// Write a raw PROFILE item that lacks the `role` attribute — the
/// shape every pre-Phase-4 user row had. `is_admin = true` only when
/// `was_admin` is true, mirroring the conditional emit in the old
/// `user_to_item`.
async fn write_pre_migration_user(
    app: &common::TestApp,
    user_id: &str,
    email: &str,
    was_admin: bool,
) {
    let now = now_usec();
    let mut item = HashMap::new();
    item.insert("PK".to_string(), AttributeValue::S(format!("USER#{user_id}")));
    item.insert("SK".to_string(), AttributeValue::S("PROFILE".to_string()));
    item.insert("user_id".to_string(), AttributeValue::S(user_id.to_string()));
    item.insert("name".to_string(), AttributeValue::S("Backfill Subject".to_string()));
    item.insert("email".to_string(), AttributeValue::S(email.to_string()));
    item.insert("home_folder_id".to_string(), AttributeValue::S(new_id()));
    item.insert("private_folder_id".to_string(), AttributeValue::S(new_id()));
    item.insert("trash_folder_id".to_string(), AttributeValue::S(new_id()));
    // Legacy provider field stored as plain lowercase string (matches
    // how user_to_item writes it). `Unknown` is the legacy default
    // and the only thing find_or_create_user expects to upgrade.
    item.insert("provider".to_string(), AttributeValue::S("unknown".to_string()));
    if was_admin {
        item.insert("is_admin".to_string(), AttributeValue::Bool(true));
    }
    item.insert("created_at".to_string(), AttributeValue::N(now.to_string()));
    item.insert("updated_at".to_string(), AttributeValue::N(now.to_string()));
    // Deliberately omit `role` to simulate a pre-migration row.

    app.dynamo_client()
        .put_item()
        .table_name(&app.table_name)
        .set_item(Some(item))
        .send()
        .await
        .expect("put pre-migration user");
}

fn dynamo_for(app: &common::TestApp) -> DynamoClient {
    DynamoClient::new(app.dynamo_client().clone(), app.table_name.clone())
}

#[tokio::test]
async fn admin_keeps_admin_status_post_backfill() {
    // This is the test the M-E1 plan explicitly calls out. After the
    // backfill writes the materialized `role` column, `get_by_id`
    // (which now REQUIRES the `role` attribute) must return a User
    // whose `is_admin()` matches the legacy `is_admin = true` row.
    common::require_infra!();
    let app = common::TestApp::new().await;

    let admin_id = new_id();
    write_pre_migration_user(&app, &admin_id, "admin@test.com", true).await;

    let dynamo = dynamo_for(&app);
    let stats = run_backfill_user_role(&app.state.user_repo, &dynamo, false)
        .await
        .expect("backfill");

    assert!(stats.scanned >= 1, "scanned must include our seeded admin");
    assert!(stats.set_admin >= 1, "must have materialized role=admin for at least one row");

    // The whole point of the migration: the row is now readable
    // through the typed repo path, and `is_admin()` matches the
    // legacy flag.
    let user = app
        .state
        .user_repo
        .get_by_id(&admin_id)
        .await
        .expect("get_by_id after backfill")
        .expect("admin row still present");
    assert!(user.is_admin(), "admin must still be admin after backfill");

    app.cleanup().await;
}

#[tokio::test]
async fn non_admin_gets_user_role() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let user_id = new_id();
    write_pre_migration_user(&app, &user_id, "user@test.com", false).await;

    let dynamo = dynamo_for(&app);
    let stats = run_backfill_user_role(&app.state.user_repo, &dynamo, false)
        .await
        .expect("backfill");
    assert!(stats.set_user >= 1, "non-admin must materialize role=user");

    let user = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect("get_by_id")
        .expect("user row present");
    assert!(!user.is_admin(), "non-admin must remain non-admin after backfill");

    app.cleanup().await;
}

#[tokio::test]
async fn dry_run_classifies_without_writing() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let user_id = new_id();
    write_pre_migration_user(&app, &user_id, "dry@test.com", true).await;

    let dynamo = dynamo_for(&app);
    let stats = run_backfill_user_role(&app.state.user_repo, &dynamo, true)
        .await
        .expect("dry-run");
    // Classification still increments — dry-run reports what WOULD
    // happen, otherwise an operator can't preview the change.
    assert!(stats.set_admin >= 1, "dry-run must count the would-be admin write");

    // Critically: the row was NOT modified. Reading through the
    // typed repo (which now requires `role`) must still fail because
    // dry-run skipped the actual write.
    let err = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect_err("typed read must fail on still-unmigrated row");
    let msg = err.to_string();
    assert!(
        msg.contains("role"),
        "dry-run-skipped row should surface as MissingField(role), got: {msg}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn rerun_is_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let user_id = new_id();
    write_pre_migration_user(&app, &user_id, "rerun@test.com", true).await;

    let dynamo = dynamo_for(&app);

    // First pass migrates.
    let first = run_backfill_user_role(&app.state.user_repo, &dynamo, false)
        .await
        .expect("first pass");
    assert!(first.set_admin + first.set_user >= 1);

    // Second pass on the same table sees the row as already-migrated
    // and emits no writes. The deploy gate re-runs after partial
    // failures — re-runs MUST be free of side effects on completed
    // rows.
    let second = run_backfill_user_role(&app.state.user_repo, &dynamo, false)
        .await
        .expect("second pass");
    assert_eq!(second.set_admin, 0, "rerun must not promote anyone");
    assert_eq!(second.set_user, 0, "rerun must not demote anyone");
    assert!(
        second.already_migrated >= 1,
        "rerun must classify the seeded row as already migrated"
    );

    app.cleanup().await;
}
