// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #142: document templates — Mark as Template / unmark, workspace-scoped
// template gallery, and the copy-document endpoint that backs "New from
// template" plus general document duplication.

mod common;

use hyper::Method;

/// Mark → list → unmark → list. Covers the three core endpoints round-trip
/// against a single doc owned by one user.
#[tokio::test]
async fn test_template_mark_unmark_roundtrip() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-mark@test.com").await;
    // Bare `POST /documents` — no `workspaceId`. Regression guard for the
    // real user flow: `list_templates` unions a GSI1 owner scan with the
    // GSI3 workspace scan, so the caller's own templates surface even
    // when `workspace_id` is null. (`POST /documents` should default the
    // workspace per design/high-level-design.md; that's a separate #142
    // follow-up. The union keeps the gallery correct in the meantime.)
    let doc_id = app.create_doc(&token, "Meeting Notes Template", None).await;

    // Initially not a template — neither the GET response nor the gallery surfaces it.
    let (s, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["isTemplate"].as_bool(), Some(false));

    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));

    // Mark.
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // GET reports the flag flipped. The doc stays editable — an earlier
    // revision auto-locked here (to prevent accidental edits to the template)
    // but real users found the sudden read-only surprising, so the mark
    // no longer touches the lock. Owners can still lock deliberately via
    // the Format menu → Lock Edits.
    let (_, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(body["isTemplate"].as_bool(), Some(true), "isTemplate must flip true");
    assert_eq!(body["locked"].as_bool(), Some(false), "marking template must not auto-lock");

    // Gallery now lists it.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"].as_str(), Some(doc_id.as_str()));
    assert_eq!(list[0]["title"].as_str(), Some("Meeting Notes Template"));

    // Re-mark is idempotent (no-op write, still listed once).
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(1));

    // Unmark.
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": false })),
        )
        .await;
    assert_eq!(s, 204);
    let (_, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(body["isTemplate"].as_bool(), Some(false));

    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));

    app.cleanup().await;
}

/// A template marked by user A in workspace W is visible to user B (a member
/// of W with the doc's link-share set to View). User C in workspace X never
/// sees it — the gallery is scoped to the caller's default workspace.
#[tokio::test]
async fn test_template_visible_to_workspace_peer_only() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_id, alice_token) = app.create_user("template-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("template-bob@test.com").await;
    let (_, charlie_token) = app.create_user("template-charlie@test.com").await;

    // Alice's default workspace, seeded at dev-login.
    let alice_ws = app
        .state
        .user_repo
        .get_by_id(&alice_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("alice has a default workspace");

    // Bob joins Alice's workspace as a Member, AND we point Bob's default
    // workspace at Alice's — required because `GET /documents/templates`
    // queries the caller's *default* workspace, not the union of memberships.
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: alice_ws.clone(),
            user_id: bob_id.clone(),
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();
    app.state
        .user_repo
        .set_default_workspace(&bob_id, &alice_ws)
        .await
        .unwrap();

    // Alice creates a doc *in* W and marks it a template. The workspaceId
    // body field is required: the bare POST /documents creates a doc with
    // no workspace, which would make it invisible to a workspace query.
    let (_, body) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({
                "title": "Sales Battlecard",
                "workspaceId": alice_ws,
            })),
        )
        .await;
    let doc_id = body["id"].as_str().unwrap().to_string();
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&alice_token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // Workspace-link-share so workspace peers actually have View access.
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&alice_token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(s, 204);

    // Bob (workspace peer with View via link) sees it.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&bob_token), None)
        .await;
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1, "workspace peer sees the template");
    assert_eq!(list[0]["id"].as_str(), Some(doc_id.as_str()));

    // Charlie (different workspace, no membership) sees nothing.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&charlie_token), None)
        .await;
    assert_eq!(
        body.as_array().map(|a| a.len()),
        Some(0),
        "cross-workspace user must not see the template",
    );

    app.cleanup().await;
}

/// Copy creates a new doc with the source's content, owned by the caller, in
/// the caller's Private folder by default, never inheriting `is_template`.
#[tokio::test]
async fn test_copy_document_defaults() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-copy-defaults@test.com").await;
    let src_id = app.create_doc(&token, "Source", None).await;

    // Mark source as a template so we cover the "copy of a template" path
    // (the more interesting case — the result must be a normal doc).
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{src_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // Capture source content for the byte-equal check after the copy.
    let (s, src_bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{src_id}/content"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(s, 200);

    // Copy with no overrides.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{src_id}/copy"),
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(s, 201);
    let new_id = body["id"].as_str().unwrap().to_string();
    assert_ne!(new_id, src_id, "copy must mint a fresh id");
    assert_eq!(
        body["title"].as_str(),
        Some("Copy of Source"),
        "default title prefixes 'Copy of '",
    );
    assert_eq!(body["isTemplate"].as_bool(), Some(false), "copy is never a template");
    assert_eq!(body["locked"].as_bool(), Some(false), "copy is not auto-locked");

    // Default destination is the caller's Private folder.
    let user = app.state.user_repo.get_by_id(&_uid).await.unwrap().unwrap();
    assert_eq!(
        body["folderId"].as_str(),
        Some(user.private_folder_id.as_str()),
        "default destination must be the caller's Private folder",
    );

    // Content byte-equal: yrs forwards the snapshot verbatim at v1
    // (mail-merge substitution lands in Phase 2).
    let (s, copy_bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{new_id}/content"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(copy_bytes, src_bytes, "copied snapshot must equal source byte-for-byte");

    app.cleanup().await;
}

/// Copy honors explicit `title` and `folderId` overrides.
#[tokio::test]
async fn test_copy_document_title_and_folder_overrides() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-copy-overrides@test.com").await;
    let src_id = app.create_doc(&token, "Boring Source", None).await;
    let dest = app.create_folder(&token, "Quarterly Plans", None).await;

    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{src_id}/copy"),
            Some(&token),
            Some(serde_json::json!({
                "title": "Q3 Plan",
                "folderId": dest,
            })),
        )
        .await;
    assert_eq!(s, 201);
    assert_eq!(body["title"].as_str(), Some("Q3 Plan"));
    assert_eq!(body["folderId"].as_str(), Some(dest.as_str()));

    app.cleanup().await;
}

/// A user without View on the source cannot copy it (the route is gated on
/// View, so the result is 404 to avoid existence-probe side channels).
#[tokio::test]
async fn test_copy_document_denied_without_view_access() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_aid, alice_token) = app.create_user("template-copy-alice@test.com").await;
    let (_bid, bob_token) = app.create_user("template-copy-bob@test.com").await;

    // Alice's private doc, not shared anywhere.
    let private_id = app.create_doc(&alice_token, "Private Plan", None).await;

    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{private_id}/copy"),
            Some(&bob_token),
            Some(serde_json::json!({})),
        )
        .await;
    // `check_doc_access` returns Forbidden → 403 for a live doc the caller
    // has no membership on. The 404 path is reserved for trashed-for-
    // non-owner (so deletion isn't a side channel for existence probes);
    // live no-access surfaces as 403.
    assert_eq!(s, 403, "bob must not be able to copy a doc he can't even see");

    app.cleanup().await;
}

/// Marking-as-template requires Edit. A bare workspace-link viewer can't
/// promote someone else's doc into the template gallery.
#[tokio::test]
async fn test_mark_template_denied_for_viewer() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_id, alice_token) = app.create_user("template-perm-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("template-perm-bob@test.com").await;

    // Both share Alice's workspace; Bob is a member with default ws set.
    let alice_ws = app
        .state
        .user_repo
        .get_by_id(&alice_id)
        .await
        .unwrap()
        .unwrap()
        .default_workspace_id
        .expect("alice has a default workspace");
    app.state
        .workspace_repo
        .add_member(&ogrenotes_storage::models::workspace::WorkspaceMember {
            workspace_id: alice_ws.clone(),
            user_id: bob_id.clone(),
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();
    app.state
        .user_repo
        .set_default_workspace(&bob_id, &alice_ws)
        .await
        .unwrap();

    // Alice creates the doc in the workspace and shares it View-mode via link.
    let (_, body) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({
                "title": "Read-Only Doc",
                "workspaceId": alice_ws,
            })),
        )
        .await;
    let doc_id = body["id"].as_str().unwrap().to_string();
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&alice_token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(s, 204);

    // Bob (View via link) tries to mark template — gets 403 (he can read but not edit).
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&bob_token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 403, "viewer must not be able to mark someone else's doc as a template");

    app.cleanup().await;
}

// ─── Phase 2: mail merge ─────────────────────────────────────────

/// Build a document whose body contains `[[…]]` placeholders and return
/// the yrs state bytes. Tests upload this via PUT /content so the doc has
/// realistic content the mail-merge scanner can find.
fn build_template_content(markdown: &str) -> Vec<u8> {
    use yrs::{ReadTxn, Transact};
    let doc = ogrenotes_collab::import::from_markdown(markdown);
    doc.transact()
        .encode_state_as_update_v1(&yrs::StateVector::default())
}

/// `GET /documents/templates` returns each template's unique placeholder
/// keys so the picker can decide whether to prompt for values before copy.
#[tokio::test]
async fn test_list_templates_reports_placeholders() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-ph-list@test.com").await;

    // Two templates: one with placeholders (including a duplicate that
    // must dedupe), one without.
    let with_ph = app.create_doc(&token, "Meeting Notes", None).await;
    let plain = app.create_doc(&token, "Plain Doc", None).await;
    let content = build_template_content(
        "Hi [[name]], welcome to [[team.name]]. See you [[name]] soon.",
    );
    let (s, _) = app
        .bytes_request(
            hyper::Method::PUT,
            &format!("/api/v1/documents/{with_ph}/content"),
            Some(&token),
            content,
            "application/octet-stream",
        )
        .await;
    assert_eq!(s, 204);

    for id in [&with_ph, &plain] {
        let (s, _) = app
            .json_request(
                Method::PUT,
                &format!("/api/v1/documents/{id}/template"),
                Some(&token),
                Some(serde_json::json!({ "isTemplate": true })),
            )
            .await;
        assert_eq!(s, 204);
    }

    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 2);

    let with_ph_row = list
        .iter()
        .find(|r| r["id"].as_str() == Some(with_ph.as_str()))
        .expect("templated row");
    let plain_row = list
        .iter()
        .find(|r| r["id"].as_str() == Some(plain.as_str()))
        .expect("plain row");

    // Sorted, deduped placeholder keys — the scanner returns a BTreeSet in
    // sorted order, so `name` (which appears twice) shows up once.
    let placeholders: Vec<&str> = with_ph_row["placeholders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(placeholders, vec!["name", "team.name"]);

    // A template with no placeholders reports an empty array (never null).
    assert_eq!(
        plain_row["placeholders"].as_array().map(|a| a.len()),
        Some(0),
        "template with no placeholders must report an empty array",
    );

    app.cleanup().await;
}

/// `POST /documents/{id}/copy` with `values` substitutes placeholders in
/// the copied snapshot.
#[tokio::test]
async fn test_copy_document_substitutes_values() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-copy-values@test.com").await;
    let src_id = app.create_doc(&token, "Onboarding", None).await;

    let content = build_template_content(
        "Welcome [[name]]! Your team is [[team|Unassigned]]. Reach out at [[email]].",
    );
    let (s, _) = app
        .bytes_request(
            hyper::Method::PUT,
            &format!("/api/v1/documents/{src_id}/content"),
            Some(&token),
            content,
            "application/octet-stream",
        )
        .await;
    assert_eq!(s, 204);

    // Copy with a values dict — `name` present, `team` uses fallback,
    // `email` missing → stays verbatim.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{src_id}/copy"),
            Some(&token),
            Some(serde_json::json!({
                "values": { "name": "Arnie" }
            })),
        )
        .await;
    assert_eq!(s, 201);
    let new_id = body["id"].as_str().unwrap();

    // Read the copy's rendered text and confirm the substitution shape.
    // Markdown export is a stable way to inspect body text without
    // depending on the yrs internal shape.
    let (s, body) = app
        .bytes_request(
            hyper::Method::GET,
            &format!("/api/v1/documents/{new_id}/export/markdown"),
            Some(&token),
            vec![],
            "text/markdown",
        )
        .await;
    assert_eq!(s, 200);
    let text = String::from_utf8(body).expect("utf8");
    assert!(text.contains("Welcome Arnie!"), "value present: got {text:?}");
    assert!(text.contains("team is Unassigned"), "fallback applied: got {text:?}");
    assert!(
        text.contains("[[email]]"),
        "missing key must stay verbatim: got {text:?}",
    );

    app.cleanup().await;
}

/// Regression: `list_templates` and `copy_document` must merge pending
/// UPDATE# rows on top of the S3 snapshot, not read the snapshot alone.
/// Otherwise, the very common "type in a new doc and Mark as Template
/// before any put_content ran" scenario shows an empty placeholder set
/// AND produces a blank copy — because all the text lives in the CRDT
/// op log the scanner would have skipped.
#[tokio::test]
async fn test_pending_updates_are_visible_to_scanner_and_copy() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-pending@test.com").await;
    let doc_id = app.create_doc(&token, "Pending Template", None).await;

    // Simulate live typing: build a diff against the empty snapshot that
    // inserts `Hello [[name]]!` into the initial paragraph, and append it
    // as an UPDATE# row directly. This is the exact shape the WebSocket
    // handler writes when a client sends an incremental update.
    let base = ogrenotes_collab::document::OgreDoc::new();
    let edited = ogrenotes_collab::document::OgreDoc::new();
    {
        use yrs::{types::xml::{XmlFragment, XmlOut, XmlTextPrelim}, Transact, WriteTxn};
        let mut txn = edited.inner().transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        if let Some(XmlOut::Element(para)) = frag.get(&txn, 0) {
            para.insert(&mut txn, 0, XmlTextPrelim::new("Hello [[name]]!"));
        }
    }
    let sv = base.state_vector();
    let diff = edited.encode_diff(&sv).unwrap();

    let update = ogrenotes_storage::models::document::DocUpdate {
        doc_id: doc_id.clone(),
        clock: format!("{}_test", ogrenotes_common::time::now_usec()),
        update_bytes: diff,
        user_id: "test-user".to_string(),
        created_at: ogrenotes_common::time::now_usec(),
        client_version: None,
    };
    app.state.doc_repo.append_update(&update).await.unwrap();

    // Mark as template.
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // Gallery: placeholders must include `name` even though the snapshot
    // is still empty and only the UPDATE# row carries the token.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    let list = body.as_array().unwrap();
    let row = list
        .iter()
        .find(|r| r["id"].as_str() == Some(doc_id.as_str()))
        .expect("template row");
    let placeholders: Vec<&str> = row["placeholders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        placeholders,
        vec!["name"],
        "scanner must see placeholders in pending UPDATE# rows",
    );

    // Copy the doc with a values dict. The copy must reflect BOTH the
    // pending-updates content AND the substitution.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/copy"),
            Some(&token),
            Some(serde_json::json!({ "values": { "name": "Arnie" } })),
        )
        .await;
    assert_eq!(s, 201);
    let new_id = body["id"].as_str().unwrap();

    let (_, body) = app
        .bytes_request(
            hyper::Method::GET,
            &format!("/api/v1/documents/{new_id}/export/markdown"),
            Some(&token),
            vec![],
            "text/markdown",
        )
        .await;
    let text = String::from_utf8(body).expect("utf8");
    assert!(
        text.contains("Hello Arnie!"),
        "copy must include pending-updates content, substituted; got {text:?}",
    );

    app.cleanup().await;
}

// ─── Phase 3: sample templates ───────────────────────────────────

/// The seed binary's `run_seed_samples` provisions system user +
/// samples workspace + every fixture, and `list_templates` returns
/// them tagged `gallery: "sample"` to every caller regardless of
/// workspace.
#[tokio::test]
async fn test_sample_templates_visible_to_every_user_and_tagged() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    // Seed the samples workspace + all fixtures directly against the
    // repos. Idempotent — a second call skips existing rows.
    let stats = ogrenotes_api::seed::run_seed_samples(
        &app.state.user_repo,
        &app.state.workspace_repo,
        &app.state.doc_repo,
        &app.state.folder_repo,
        &app.state.security_audit_repo,
        false, // dry_run
        false, // force
    )
    .await
    .expect("seed_sample_templates");
    // Test may run against a table that already carries the samples from
    // a prior invocation — the assertion must cover both fresh-create and
    // idempotent-skip. What matters is that every fixture is present after
    // the call.
    assert_eq!(
        stats.templates_created + stats.templates_skipped_existing,
        ogrenotes_api::seed::SAMPLE_TEMPLATES.len(),
        "every fixture must be accounted for; got stats: {stats:?}",
    );

    let (_uid, token) = app.create_user("template-samples@test.com").await;
    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    let list = body.as_array().unwrap();

    // Every fixture must be present as a `sample`-tagged row.
    for fixture in ogrenotes_api::seed::SAMPLE_TEMPLATES {
        let expected_id = ogrenotes_api::seed::sample_doc_id(fixture.sample_id);
        let row = list
            .iter()
            .find(|r| r["id"].as_str() == Some(&expected_id))
            .unwrap_or_else(|| panic!("sample {} missing from list", fixture.sample_id));
        assert_eq!(
            row["gallery"]["type"].as_str(),
            Some("sample"),
            "sample fixture {} must be tagged as `sample`, got: {row}",
            fixture.sample_id,
        );
        assert_eq!(row["title"].as_str(), Some(fixture.title));
    }

    app.cleanup().await;
}

/// Any authenticated user can copy a sample template even though they
/// are not a member of the samples workspace. The copy path bypasses
/// the standard ACL for docs that live in the samples workspace and are
/// marked as templates.
#[tokio::test]
async fn test_any_user_can_copy_a_sample_template() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    ogrenotes_api::seed::run_seed_samples(
        &app.state.user_repo,
        &app.state.workspace_repo,
        &app.state.doc_repo,
        &app.state.folder_repo,
        &app.state.security_audit_repo,
        false, // dry_run
        false, // force
    )
    .await
    .expect("seed");

    let (_uid, token) = app.create_user("template-copy-sample@test.com").await;
    let sample_id = ogrenotes_api::seed::sample_doc_id(
        ogrenotes_api::seed::SAMPLE_TEMPLATES[0].sample_id,
    );
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{sample_id}/copy"),
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(s, 201, "copy of a sample must succeed for any user; got body: {body}");
    let new_id = body["id"].as_str().unwrap();
    assert_ne!(new_id, sample_id, "copy must mint a new id");
    assert_eq!(body["isTemplate"].as_bool(), Some(false), "copy is a plain doc");

    app.cleanup().await;
}

/// Regression: A user who plants a doc in the samples workspace by
/// passing `workspaceId=samples-workspace` to `POST /documents` must
/// NOT have that row appear in every user's Samples gallery, and must
/// NOT be able to have it copied by other users. Both branches gate on
/// `owner_id == SAMPLES_SYSTEM_USER_ID` in addition to the workspace
/// membership.
#[tokio::test]
async fn test_planted_sample_workspace_doc_is_ignored() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_a, alice) = app.create_user("template-plant-alice@test.com").await;
    let (_b, bob) = app.create_user("template-plant-bob@test.com").await;

    // Alice plants a doc into the samples workspace and marks it a template.
    let (_, body) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice),
            Some(serde_json::json!({
                "title": "Planted",
                "workspaceId": ogrenotes_api::seed::SAMPLES_WORKSPACE_ID,
            })),
        )
        .await;
    let planted_id = body["id"].as_str().unwrap().to_string();
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{planted_id}/template"),
            Some(&alice),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    // Bob does NOT see the planted doc in his gallery.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&bob), None)
        .await;
    let list = body.as_array().unwrap();
    assert!(
        list.iter().all(|r| r["id"].as_str() != Some(planted_id.as_str())),
        "planted samples-workspace doc must not appear in a non-planter's gallery",
    );

    // Bob CANNOT copy the planted doc — the copy path's sample bypass
    // also gates on owner_id. Standard ACL rejects him too since he isn't
    // a member of the samples workspace.
    let (s, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{planted_id}/copy"),
            Some(&bob),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(
        s, 403,
        "planted samples-workspace doc must not be copyable by non-planter",
    );

    app.cleanup().await;
}

/// A user's own templates are tagged `gallery: "mine"`.
#[tokio::test]
async fn test_list_templates_tags_own_row_mine() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-tag-mine@test.com").await;
    let doc_id = app.create_doc(&token, "Owned", None).await;
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);

    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&token), None)
        .await;
    let row = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"].as_str() == Some(doc_id.as_str()))
        .expect("row");
    assert_eq!(row["gallery"]["type"].as_str(), Some("mine"));

    app.cleanup().await;
}

/// A workspace peer's template surfaces to another workspace member
/// tagged `gallery: "shared"`.
#[tokio::test]
async fn test_list_templates_tags_peer_row_shared() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (alice_id, alice_token) = app.create_user("template-tag-alice@test.com").await;
    let (bob_id, bob_token) = app.create_user("template-tag-bob@test.com").await;

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
            workspace_id: alice_ws.clone(),
            user_id: bob_id.clone(),
            role: ogrenotes_storage::models::WorkspaceRole::Member,
            joined_at: 0,
        })
        .await
        .unwrap();
    app.state
        .user_repo
        .set_default_workspace(&bob_id, &alice_ws)
        .await
        .unwrap();

    let (_, body) = app
        .json_request(
            Method::POST,
            "/api/v1/documents",
            Some(&alice_token),
            Some(serde_json::json!({
                "title": "Shared Template",
                "workspaceId": alice_ws,
            })),
        )
        .await;
    let doc_id = body["id"].as_str().unwrap().to_string();
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/template"),
            Some(&alice_token),
            Some(serde_json::json!({ "isTemplate": true })),
        )
        .await;
    assert_eq!(s, 204);
    let (s, _) = app
        .json_request(
            Method::PATCH,
            &format!("/api/v1/documents/{doc_id}/link-settings"),
            Some(&alice_token),
            Some(serde_json::json!({ "linkSharingMode": "view" })),
        )
        .await;
    assert_eq!(s, 204);

    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/templates", Some(&bob_token), None)
        .await;
    let row = body
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"].as_str() == Some(doc_id.as_str()))
        .expect("bob sees the template");
    assert_eq!(row["gallery"]["type"].as_str(), Some("shared"));

    app.cleanup().await;
}

/// Copy without a `values` field is a byte-passthrough — placeholders
/// stay verbatim, matching Phase 1's behavior.
#[tokio::test]
async fn test_copy_document_without_values_is_passthrough() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("template-copy-noval@test.com").await;
    let src_id = app.create_doc(&token, "Template", None).await;

    let content = build_template_content("Placeholder: [[name]] stays put.");
    let (s, _) = app
        .bytes_request(
            hyper::Method::PUT,
            &format!("/api/v1/documents/{src_id}/content"),
            Some(&token),
            content.clone(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(s, 204);

    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{src_id}/copy"),
            Some(&token),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(s, 201);
    let new_id = body["id"].as_str().unwrap();

    // No values → snapshot is forwarded verbatim; the copy still contains
    // the raw placeholder token.
    let (_, copy_bytes) = app
        .bytes_request(
            hyper::Method::GET,
            &format!("/api/v1/documents/{new_id}/content"),
            Some(&token),
            vec![],
            "application/octet-stream",
        )
        .await;
    assert_eq!(
        copy_bytes, content,
        "no-values copy must be a byte-identical passthrough",
    );

    app.cleanup().await;
}
