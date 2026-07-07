// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

/// Regression: an authenticated request must update `User.last_active_at`
/// via the debounced middleware so the email-service "active in-app"
/// suppression has real data to read.
#[tokio::test]
async fn test_authed_request_writes_last_active_at() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("active@test.com").await;

    // Confirm the field starts at zero — it's initialized to 0 on signup
    // and has no other writer prior to this milestone.
    let pre = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect("repo get")
        .expect("user exists");
    assert_eq!(pre.last_active_at, 0, "last_active_at should be 0 pre-request");

    // Any authed request goes through AuthUser::from_request_parts, which
    // calls activity_tracker.mark(...).
    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 200);

    // The tracker writes via `tokio::spawn`; give it a moment to land.
    // A longer sleep here would hide a real regression, so keep it tight.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let post = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect("repo get")
        .expect("user exists");
    assert!(
        post.last_active_at > 0,
        "last_active_at should be updated after an authed request, got {}",
        post.last_active_at,
    );

    app.cleanup().await;
}
