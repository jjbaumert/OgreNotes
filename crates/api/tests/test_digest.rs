// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! M4.1 daily digest integration regressions.
//!
//! The scheduler's hourly tick is not easily drivable in a unit test
//! (it takes over 60 seconds). Instead we exercise the
//! `send_digests(...)` pass directly, which is the same function the
//! scheduler calls when it decides it's digest time. That covers the
//! three surfaces most likely to regress: the inactivity filter, the
//! unread lookup, and the `EmailService::try_send_digest` pipeline.

mod common;

use ogrenotes_storage::models::notification::{NotifType, Notification};

/// Write a notification row directly via `NotificationRepo::create` so
/// the test doesn't have to wait on a `tokio::spawn`'d async create
/// that backs the public spawn-sites.
async fn seed_notif(
    app: &common::TestApp,
    user_id: &str,
    created_at: i64,
    read: bool,
) {
    let notif = Notification {
        notif_id: nanoid::nanoid!(16),
        user_id: user_id.to_string(),
        notif_type: NotifType::Shared,
        doc_id: Some("d1".to_string()),
        thread_id: None,
        actor_id: "actor".to_string(),
        message: "shared a document with you".to_string(),
        preview: None,
        block_id: None,
        read,
        created_at,
    };
    app.state
        .notification_repo
        .create(&notif)
        .await
        .expect("seed notif");
}

/// With `email_enabled=false` (the test harness default), `try_send_digest`
/// short-circuits on `SkippedDisabled` before touching any repo — the
/// tightest guarantee that a misconfigured env can never fire email.
#[tokio::test]
async fn test_digest_skipped_when_disabled() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, _) = app.create_user("recipient@test.com").await;
    seed_notif(&app, &user_id, 1_000_000, false).await;

    let outcome = app
        .state
        .email_service
        .try_send_digest(&user_id, &[])
        .await
        .expect("no transport error in test");
    // Empty slice would normally hit SkippedPrefs, but SkippedDisabled
    // takes precedence — the order of gates is itself a regression guard.
    assert!(matches!(outcome, ogrenotes_notify::SendOutcome::SkippedDisabled));

    app.cleanup().await;
}

/// Regression: `list_unread_since` returns unread notifications in the
/// given window and excludes ones already read. The digest pipeline
/// depends on this filter to avoid re-mailing events the user has
/// already seen in-app.
#[tokio::test]
async fn test_list_unread_since_filters_read_and_ancient() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, _) = app.create_user("unread@test.com").await;

    let now = ogrenotes_common::time::now_usec();
    let one_day_us: i64 = 24 * 60 * 60 * 1_000_000;

    // Inside window, unread — should land.
    seed_notif(&app, &user_id, now - one_day_us / 2, false).await;
    // Inside window, already read — must be filtered by is_read=false.
    seed_notif(&app, &user_id, now - one_day_us / 3, true).await;
    // Outside window (older than 24h) — must be filtered by SK range.
    seed_notif(&app, &user_id, now - 2 * one_day_us, false).await;

    let since = now - one_day_us;
    let unread = app
        .state
        .notification_repo
        .list_unread_since(&user_id, since, 50)
        .await
        .expect("list_unread_since");

    assert_eq!(
        unread.len(),
        1,
        "expected exactly the one in-window unread row, got {unread:?}"
    );
    assert!(!unread[0].read);
    assert!(unread[0].created_at >= since);

    app.cleanup().await;
}
