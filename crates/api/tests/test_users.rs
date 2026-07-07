// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;

use ogrenotes_storage::models::security_audit::SecurityAuditAction;

/// Poll the SecurityAudit table for a matching row for `user_id`.
/// `put_profile` emits `ProfileUpdated` via `record_security_event`
/// (tokio::spawn), so it can land after the response.
async fn wait_for_user_audit(
    app: &common::TestApp,
    user_id: &str,
    matcher: impl Fn(&SecurityAuditAction) -> bool,
) {
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(user_id, 20)
            .await
            .unwrap();
        if rows.iter().any(|r| matcher(&r.action)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("expected SecurityAudit row for user {user_id} within 200ms");
}

#[tokio::test]
async fn test_get_me() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (user_id, token) = app.create_user("me@test.com").await;

    let (status, json) = app.json_request(Method::GET, "/api/v1/users/me", Some(&token), None).await;
    assert_eq!(status, 200);
    assert_eq!(json["userId"].as_str().unwrap(), user_id);
    assert_eq!(json["email"], "me@test.com");
    assert!(json["name"].is_string());
    assert!(json["homeFolderId"].is_string());

    app.cleanup().await;
}

#[tokio::test]
async fn test_get_me_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app.json_request(Method::GET, "/api/v1/users/me", None, None).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_by_email_exact() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("alice@test.com").await;

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=alice@test.com",
        Some(&token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["email"], "alice@test.com");

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_by_email_not_found() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("searcher@test.com").await;

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=nonexistent@x.com",
        Some(&token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert!(users.is_empty());

    app.cleanup().await;
}

// ─── Plan B: /users/search workspace-scope filter ─────────────

/// Two users in disjoint default workspaces cannot enumerate each
/// other. Prior to the filter, any authenticated user could scan
/// the entire PROFILE partition; post-filter, the substring match
/// still hits the target's row but the workspace check drops it.
#[tokio::test]
async fn test_search_filters_out_cross_workspace_users() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Alice and Bob both get default workspaces on first login;
    // they don't share any workspace.
    let (_alice_id, _alice_token) = app.create_user("alice@scope.test").await;
    let bob_token = app.create_user_token("bob@scope.test").await;

    // Bob searches for Alice by email — must return empty.
    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=alice@scope.test",
        Some(&bob_token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert!(
        users.is_empty(),
        "cross-workspace email lookup must not leak users; got {json}"
    );

    // And via substring search.
    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?q=alice",
        Some(&bob_token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert!(
        users.iter().all(|u| u["email"] != "alice@scope.test"),
        "cross-workspace substring search leaked alice; got {json}"
    );

    app.cleanup().await;
}

/// After Bob joins Alice's workspace, he sees her in search results.
#[tokio::test]
async fn test_search_finds_users_after_shared_workspace() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (alice_id, _alice_token) = app.create_user("alice2@scope.test").await;
    let (bob_id, bob_token) = app.create_user("bob2@scope.test").await;

    let alice_ws = app
        .state
        .user_repo
        .get_by_id(&alice_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("alice has default workspace");
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: alice_ws,
            user_id: bob_id,
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=alice2@scope.test",
        Some(&bob_token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["email"], "alice2@scope.test");

    app.cleanup().await;
}

/// Admins bypass the workspace-scope filter — the admin console
/// needs an unscoped view for cross-workspace user management.
#[tokio::test]
async fn test_admin_search_bypasses_workspace_filter() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let _target = app.create_user("scoped-target@test.com").await;
    // Promote a user to admin via the repo, then re-login so the
    // fresh JWT carries the is_admin claim. Matches the pattern
    // in test_admin.rs — the AuthUser extractor reads is_admin
    // from the JWT, not the DB.
    let (admin_id, _) = app.create_user("scope-admin@test.com").await;
    app.state.user_repo.set_admin(&admin_id, true).await.unwrap();
    let (_, admin_token) = app.create_user("scope-admin@test.com").await;

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=scoped-target@test.com",
        Some(&admin_token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert_eq!(users.len(), 1, "admin lookup must ignore workspace scope; got {json}");

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_by_query() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?q=alice",
        Some(&token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert!(!users.is_empty());
    // At least one result should match
    assert!(users.iter().any(|u| u["email"].as_str().unwrap().contains("alice")));

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_no_params() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("noparam@test.com").await;

    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search",
        Some(&token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert!(users.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn test_search_unauthenticated() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=anyone@test.com",
        None,
        None,
    ).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

/// Regression: when the USER#/PROFILE items span more than one DynamoDB scan
/// page (i.e. >1 MB of data), `get_by_email` must paginate via
/// `LastEvaluatedKey` to find the match. An earlier version returned only the
/// first-page results, which silently caused `find_or_create_user` to insert
/// duplicate USER# rows and broke `/users/search` for sharing dialogs.
///
/// Limitation: DynamoDB Local does not always enforce the 1 MB per-response
/// cap that real DynamoDB does — some versions return the full dataset in one
/// scan call, in which case this test will pass with or without the fix. The
/// test still exercises the pagination code path, and against real DynamoDB
/// (or a Local version that honors the cap) it reliably fails on the
/// un-paginated implementation. Treat as a soft regression gate locally and a
/// hard one in any CI environment that runs against real DynamoDB.
#[tokio::test]
async fn test_search_by_email_across_scan_pagination_boundary() {
    use aws_sdk_dynamodb::types::AttributeValue;

    common::require_infra!();
    let app = common::TestApp::new().await;

    // Real target user — created through the normal login path.
    let (target_user_id, _token) = app.create_user("target@test.com").await;

    // Pad the table with PROFILE-shaped rows large enough that the scan
    // cannot return everything in a single call. DynamoDB caps each scan
    // response at ~1 MB; ~1.5 MB of filler guarantees at least one
    // continuation. Rows are valid enough to deserialize via user_from_item
    // so the repo doesn't error partway through.
    let padding = "x".repeat(100_000);
    for i in 0..15 {
        let mut item = std::collections::HashMap::new();
        item.insert("PK".into(), AttributeValue::S(format!("USER#filler_{i}")));
        item.insert("SK".into(), AttributeValue::S("PROFILE".into()));
        item.insert("user_id".into(), AttributeValue::S(format!("filler_{i}")));
        item.insert("name".into(), AttributeValue::S(padding.clone()));
        item.insert("email".into(), AttributeValue::S(format!("filler_{i}@noise.test")));
        item.insert("home_folder_id".into(), AttributeValue::S(format!("fh_{i}")));
        item.insert("private_folder_id".into(), AttributeValue::S(format!("fp_{i}")));
        item.insert("trash_folder_id".into(), AttributeValue::S(format!("ft_{i}")));
        item.insert("created_at".into(), AttributeValue::N("0".into()));
        item.insert("updated_at".into(), AttributeValue::N("0".into()));
        app.dynamo_client()
            .put_item()
            .table_name(&app.table_name)
            .set_item(Some(item))
            .send()
            .await
            .expect("filler put_item failed");
    }

    let (searcher_id, searcher_token) = app.create_user("searcher@test.com").await;
    // The new `/users/search` workspace-scope filter (Plan B) requires
    // caller and target to share a workspace; add the searcher to the
    // target's default workspace so the pagination assertion isolates
    // that concern and doesn't accidentally test the scope filter.
    let target_ws = app
        .state
        .user_repo
        .get_by_id(&target_user_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("target has a default workspace");
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: target_ws,
            user_id: searcher_id,
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();
    let (status, json) = app.json_request(
        Method::GET,
        "/api/v1/users/search?email=target@test.com",
        Some(&searcher_token),
        None,
    ).await;
    assert_eq!(status, 200);
    let users = json["users"].as_array().unwrap();
    assert_eq!(
        users.len(),
        1,
        "target email must be findable across scan pagination; got {json}"
    );
    assert_eq!(users[0]["email"], "target@test.com");
    assert_eq!(users[0]["userId"], target_user_id);

    app.cleanup().await;
}

// ─── Phase 5 M-P1 piece B: UI preferences ───────────────────────

/// Fresh users have no `uiPrefs` field — the response omits it
/// rather than emitting an empty object, so the frontend's
/// "use defaults" branch fires unambiguously.
#[tokio::test]
async fn me_omits_ui_prefs_for_fresh_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("prefs-fresh@test.com").await;

    let (status, json) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert!(
        json.get("uiPrefs").is_none(),
        "fresh user must not carry uiPrefs; got {json}"
    );

    app.cleanup().await;
}

/// PUT /users/me/prefs persists a partial body and the merged
/// result surfaces on the next GET /users/me. Pre-fix race: a
/// later PUT with only `dyslexicFont` should NOT clobber the
/// previously-set `theme`.
#[tokio::test]
async fn ui_prefs_round_trip_and_merge() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("prefs-merge@test.com").await;

    // First PUT — set theme = dark.
    let (status, body) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "theme": "dark" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["theme"], "dark", "PUT echoes merged result");

    // GET confirms server-side storage.
    let (_, me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(me["uiPrefs"]["theme"], "dark");

    // Second PUT — only locale. theme must NOT clobber back to default.
    let (status, body) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "locale": "ar" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["theme"], "dark", "earlier theme survives partial PUT");
    assert_eq!(body["locale"], "ar");

    let (_, me) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&token), None)
        .await;
    assert_eq!(me["uiPrefs"]["theme"], "dark");
    assert_eq!(me["uiPrefs"]["locale"], "ar");

    app.cleanup().await;
}

/// Defends the M-P1 piece B follow-up fix: bool fields in UiPrefs
/// (`dyslexicFont`, `reduceMotion`) were previously plain `bool`,
/// which the JSON layer defaulted to `false` on any partial PUT
/// that didn't echo them — silently clobbering a user's a11y prefs
/// every time they changed an unrelated setting. Switched to
/// `Option<bool>` with "leave unchanged on absent" semantics.
#[tokio::test]
async fn ui_prefs_partial_put_does_not_clobber_a11y_bools() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("prefs-bool-clobber@test.com").await;

    // Step 1: turn on dyslexicFont AND set theme.
    let (status, body) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "theme": "dark", "dyslexicFont": true })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["dyslexicFont"], true);
    assert_eq!(body["theme"], "dark");

    // Step 2: PUT only `locale`. dyslexicFont MUST stay true.
    let (status, body) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "locale": "ar" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["dyslexicFont"], true,
        "a11y bool must survive a partial PUT (pre-fix bug clobbered to false)",
    );
    assert_eq!(body["locale"], "ar");
    assert_eq!(body["theme"], "dark");

    // Step 3: explicit `dyslexicFont: false` MUST flip it.
    let (status, body) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            Some(&token),
            Some(serde_json::json!({ "dyslexicFont": false })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        body["dyslexicFont"], false,
        "explicit false must replace — only absent/null should preserve",
    );
    // theme + locale still intact.
    assert_eq!(body["theme"], "dark");
    assert_eq!(body["locale"], "ar");

    app.cleanup().await;
}

/// PUT /users/me/prefs requires authentication.
#[tokio::test]
async fn ui_prefs_put_requires_auth() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, _) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/prefs",
            None,
            Some(serde_json::json!({ "theme": "dark" })),
        )
        .await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

// ─── PUT /users/me (profile) ────────────────────────────────────

/// `PUT /users/me` changing the display name returns the updated profile
/// and emits exactly one `ProfileUpdated { name_changed: true,
/// avatar_changed: false }` SecurityAudit row. The handler had only the
/// pure `validate_profile_patch` unit test — the HTTP path and the
/// identity-audit emission (CLAUDE.md requires it on identity writes)
/// were untested.
#[tokio::test]
async fn put_profile_changes_name_and_writes_profile_updated_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("profile-edit@test.com").await;

    let (status, json) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me",
            Some(&token),
            Some(serde_json::json!({ "name": "Renamed Rachel" })),
        )
        .await;
    assert_eq!(status, 200, "put_profile failed: {json}");
    assert_eq!(json["name"].as_str().unwrap(), "Renamed Rachel", "response reflects the new name");

    wait_for_user_audit(&app, &user_id, |a| {
        matches!(
            a,
            SecurityAuditAction::ProfileUpdated { name_changed: true, avatar_changed: false }
        )
    })
    .await;

    app.cleanup().await;
}

/// A no-op profile PUT (same name) writes no audit row — the handler
/// compares against current values and only audits a real change.
#[tokio::test]
async fn put_profile_noop_writes_no_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user_with_name("profile-noop@test.com", "Same Name").await;

    let (status, _) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me",
            Some(&token),
            Some(serde_json::json!({ "name": "Same Name" })),
        )
        .await;
    assert_eq!(status, 200);

    // Give any (erroneous) spawned writer a chance to land, then assert none did.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let rows = app.state.security_audit_repo.list_for_user(&user_id, 20).await.unwrap();
    assert!(
        !rows.iter().any(|r| matches!(r.action, SecurityAuditAction::ProfileUpdated { .. })),
        "an unchanged profile PUT must not emit a ProfileUpdated row"
    );

    app.cleanup().await;
}

// ─── PUT /users/me/status ───────────────────────────────────────

/// `PUT /users/me/status` sets a status, which surfaces on `/users/me`,
/// and clearing it (blank text) removes it. The handler had only the pure
/// `build_status` unit test — no HTTP coverage.
#[tokio::test]
async fn put_status_sets_then_clears() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_user_id, token) = app.create_user("status@test.com").await;

    // Set.
    let (status, json) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/status",
            Some(&token),
            Some(serde_json::json!({ "text": "In a meeting", "emoji": "📅" })),
        )
        .await;
    assert_eq!(status, 200, "set status failed: {json}");
    assert_eq!(json["status"]["text"].as_str().unwrap(), "In a meeting");
    assert_eq!(json["status"]["emoji"].as_str().unwrap(), "📅");

    // GET /me reflects it.
    let (_, me) = app.json_request(Method::GET, "/api/v1/users/me", Some(&token), None).await;
    assert_eq!(me["status"]["text"].as_str().unwrap(), "In a meeting");

    // Clear (blank text ⇒ None).
    let (status, json) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/status",
            Some(&token),
            Some(serde_json::json!({ "text": "" })),
        )
        .await;
    assert_eq!(status, 200);
    assert!(json.get("status").map(|s| s.is_null()).unwrap_or(true), "cleared status is absent");

    let (_, me) = app.json_request(Method::GET, "/api/v1/users/me", Some(&token), None).await;
    assert!(me.get("status").map(|s| s.is_null()).unwrap_or(true), "GET /me shows no status after clear");

    app.cleanup().await;
}

// ─── PUT /users/me/notification-prefs ───────────────────────────

/// `PUT /users/me/notification-prefs` changes the email-notification
/// preference and the new value surfaces on `/users/me`. New users default
/// to "mentionsonly" (`NotifEmailPref::default()`); setting "all" must
/// persist. The endpoint had no test.
#[tokio::test]
async fn put_notification_prefs_updates_email_pref() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_user_id, token) = app.create_user("notifpref@test.com").await;

    // Default is "mentionsonly".
    let (_, me) = app.json_request(Method::GET, "/api/v1/users/me", Some(&token), None).await;
    assert_eq!(me["emailNotifications"].as_str().unwrap(), "mentionsonly");

    let (status, json) = app
        .json_request(
            Method::PUT,
            "/api/v1/users/me/notification-prefs",
            Some(&token),
            Some(serde_json::json!({ "emailNotifications": "all" })),
        )
        .await;
    assert_eq!(status, 200, "set notification-prefs failed: {json}");
    assert_eq!(json["emailNotifications"].as_str().unwrap(), "all");

    let (_, me) = app.json_request(Method::GET, "/api/v1/users/me", Some(&token), None).await;
    assert_eq!(me["emailNotifications"].as_str().unwrap(), "all", "GET /me reflects the new pref");

    app.cleanup().await;
}
