// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

/// Regression: `EmailCapRepo::increment_if_under_cap` atomically rejects
/// the next increment once today's counter reaches the configured cap.
/// This is the data-layer guarantee the `SkippedCap` outcome depends on.
#[tokio::test]
async fn test_email_cap_blocks_over_limit() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Build a cap repo with a tight cap so the test stays fast.
    let cap_repo = ogrenotes_notify::EmailCapRepo::new(
        ogrenotes_storage::dynamo::DynamoClient::new(
            app.dynamo_client().clone(),
            app.table_name.clone(),
        ),
        3,
    );

    let user_id = "cap-test-user";

    for i in 0..3 {
        let ok = cap_repo
            .increment_if_under_cap(user_id)
            .await
            .expect("dynamo ok");
        assert!(ok, "increment #{i} should succeed (cap=3)");
    }

    let blocked = cap_repo
        .increment_if_under_cap(user_id)
        .await
        .expect("dynamo ok");
    assert!(!blocked, "4th increment should be rejected by the cap");

    // Cap is per (user, date). A different user must not be blocked.
    let other = cap_repo
        .increment_if_under_cap("different-user")
        .await
        .expect("dynamo ok");
    assert!(other, "cap is per-user; different user should not be blocked");

    app.cleanup().await;
}
