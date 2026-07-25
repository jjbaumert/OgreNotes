// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for `POST /api/v1/imports/quip/connect` (Quip import
//! Phase 0 Task 7). Points `state.quip_client` at a `wiremock` server
//! (via `TestApp::new_with_quip_base`) instead of real
//! `platform.quip.com`, so the endpoint's token-validate -> create-import
//! -> stash-token -> fetch-roots flow can be exercised end to end without
//! any outbound network call.

mod common;

use aws_sdk_dynamodb::types::AttributeValue;
use hyper::Method;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount the two Quip endpoints `connect` calls: `GET /1/users/current`
/// and `GET /1/folders/`. `bearer` is the exact token the mock expects in
/// the `Authorization` header — a mismatch 404s (wiremock's default for
/// an unmatched request), which would surface as a `QuipError::Api`/`Http`
/// rather than the happy path, catching an accidental token mix-up.
async fn mount_happy_quip(server: &MockServer, bearer: &str) {
    Mock::given(method("GET"))
        .and(path("/1/users/current"))
        .and(header("authorization", format!("Bearer {bearer}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "quip-u1",
            "name": "Ada Lovelace",
            "emails": ["ada@example.com"],
            "private_folder_id": "pf1",
            "shared_folder_ids": ["sf1"]
        })))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(header("authorization", format!("Bearer {bearer}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pf1": {
                "folder": {"id": "pf1", "title": "Private"},
                "children": []
            },
            "sf1": {
                "folder": {"id": "sf1", "title": "Shared"},
                "children": []
            }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn connect_with_valid_token_creates_import_stores_token_and_returns_roots() {
    common::require_infra!();

    let quip_server = MockServer::start().await;
    mount_happy_quip(&quip_server, "good-token").await;

    let app = common::TestApp::new_with_quip_base(quip_server.uri()).await;
    let (_user_id, ogre_token) = app.create_user("importer@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/imports/quip/connect",
            Some(&ogre_token),
            Some(serde_json::json!({ "token": "good-token" })),
        )
        .await;

    assert_eq!(status, 201, "connect failed: {json}");
    let import_id = json["importId"].as_str().expect("importId present").to_string();
    assert!(!import_id.is_empty());

    assert_eq!(json["quipProfile"]["id"], "quip-u1");
    assert_eq!(json["quipProfile"]["name"], "Ada Lovelace");

    let roots = json["rootFolders"].as_array().expect("rootFolders array");
    let mut root_ids: Vec<&str> = roots.iter().map(|f| f["id"].as_str().unwrap()).collect();
    root_ids.sort();
    assert_eq!(root_ids, vec!["pf1", "sf1"]);
    let titles: Vec<&str> = roots.iter().map(|f| f["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"Private"));
    assert!(titles.contains(&"Shared"));

    // The durable manifest row must never carry the token — read it raw
    // (bypassing `ImportRepo`, which structurally can't write one) to pin
    // the wire-level guarantee.
    let item = app
        .dynamo_client()
        .get_item()
        .table_name(&app.table_name)
        .key("PK", AttributeValue::S(format!("IMPORT#{import_id}")))
        .key("SK", AttributeValue::S("META".to_string()))
        .send()
        .await
        .expect("get_item")
        .item
        .expect("IMPORT#/META row exists");
    assert!(!item.contains_key("token"), "IMPORT row must not carry a token attr: {item:?}");
    assert!(!item.contains_key("secret"), "IMPORT row must not carry a secret attr: {item:?}");
    assert_eq!(item.get("status").and_then(|v| v.as_s().ok()), Some(&"scoping".to_string()));
    assert_eq!(
        item.get("quip_user_id").and_then(|v| v.as_s().ok()),
        Some(&"quip-u1".to_string())
    );

    // The token itself lives in the TokenStore, retrievable by import id.
    let stored = app
        .state
        .quip_token_store
        .get(&import_id)
        .await
        .expect("token store get")
        .expect("token was stored");
    assert_eq!(stored.expose(), "good-token");
}

#[tokio::test]
async fn connect_with_quip_401_returns_400_and_creates_nothing() {
    common::require_infra!();

    let quip_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/users/current"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&quip_server)
        .await;

    let app = common::TestApp::new_with_quip_base(quip_server.uri()).await;
    let (_user_id, ogre_token) = app.create_user("rejected@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/imports/quip/connect",
            Some(&ogre_token),
            Some(serde_json::json!({ "token": "bad-token" })),
        )
        .await;

    // NOT 401 -- that status is reserved for OUR auth on this endpoint.
    // A bad Quip token is a bad request.
    assert_eq!(status, 400, "expected 400, got {status}: {json}");
    let message = json["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("invalid Quip token"),
        "message should say the token is invalid: {json}"
    );
    // Never echoes the token or raw Quip status/body.
    assert!(!message.contains("bad-token"));

    // No import record was created and nothing was stashed in the token
    // store -- the whole request short-circuits before either write.
    let scan = app
        .dynamo_client()
        .scan()
        .table_name(&app.table_name)
        .send()
        .await
        .expect("scan");
    let has_import_row = scan.items().iter().any(|item| {
        item.get("PK")
            .and_then(|v| v.as_s().ok())
            .is_some_and(|pk| pk.starts_with("IMPORT#"))
    });
    assert!(!has_import_row, "no IMPORT row should exist after a rejected token");
}

/// Task 7 adversarial-review fix: if `folders()` fails *after* the token
/// has already been stashed in the `TokenStore`, the handler must roll
/// the stash back — a live Quip token must never be left stranded with
/// no forward path (a retry mints a fresh `import_id` and abandons the
/// old one). Mounts a valid `/1/users/current` (so `token_store.put`
/// happens) but a failing `/1/folders/` (so the handler's post-put step
/// errors), then asserts: the response is 503, the manifest row (found
/// via a raw scan for the `IMPORT#` PK the handler must have created) is
/// `status = failed` rather than a dangling `scoping`, and the token
/// store has no token for that import id.
#[tokio::test]
async fn connect_folders_failure_strands_no_token() {
    common::require_infra!();

    let quip_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/1/users/current"))
        .and(header("authorization", "Bearer good-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "quip-u1",
            "name": "Ada Lovelace",
            "emails": ["ada@example.com"],
            "private_folder_id": "pf1",
            "shared_folder_ids": ["sf1"]
        })))
        .mount(&quip_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/1/folders/"))
        .and(header("authorization", "Bearer good-token"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&quip_server)
        .await;

    let app = common::TestApp::new_with_quip_base(quip_server.uri()).await;
    let (_user_id, ogre_token) = app.create_user("stranded@test.com").await;

    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/imports/quip/connect",
            Some(&ogre_token),
            Some(serde_json::json!({ "token": "good-token" })),
        )
        .await;

    assert_eq!(status, 503, "expected 503 from the folders() failure: {json}");

    // The error response carries no importId, so recover it from a raw
    // scan for the IMPORT# row the handler must have created before the
    // folders() call — and assert it was rolled back to Failed rather
    // than left as a dangling Scoping row.
    let scan = app
        .dynamo_client()
        .scan()
        .table_name(&app.table_name)
        .send()
        .await
        .expect("scan");
    let import_items: Vec<_> = scan
        .items()
        .iter()
        .filter(|item| {
            item.get("PK")
                .and_then(|v| v.as_s().ok())
                .is_some_and(|pk| pk.starts_with("IMPORT#"))
        })
        .collect();
    assert_eq!(import_items.len(), 1, "expected exactly one IMPORT row: {import_items:?}");
    let import_item = import_items[0];
    assert_eq!(
        import_item.get("status").and_then(|v| v.as_s().ok()),
        Some(&"failed".to_string()),
        "manifest row must be rolled back to failed, not left scoping: {import_item:?}"
    );
    let import_id = import_item
        .get("PK")
        .and_then(|v| v.as_s().ok())
        .expect("PK present")
        .strip_prefix("IMPORT#")
        .expect("PK has IMPORT# prefix")
        .to_string();

    // The stranded token must be gone.
    let stored = app
        .state
        .quip_token_store
        .get(&import_id)
        .await
        .expect("token store get");
    assert!(
        stored.is_none(),
        "a folders() failure after the token was stashed must delete it, found: {stored:?}"
    );
}

#[tokio::test]
async fn connect_without_ogrenotes_session_returns_401() {
    common::require_infra!();

    // No mock mounted at all -- an unauthenticated request must be
    // rejected by OUR auth extractor before it ever reaches the handler
    // body (and therefore never calls out to Quip).
    let quip_server = MockServer::start().await;
    let app = common::TestApp::new_with_quip_base(quip_server.uri()).await;

    let (status, _json) = app
        .json_request(
            Method::POST,
            "/api/v1/imports/quip/connect",
            None,
            Some(serde_json::json!({ "token": "irrelevant" })),
        )
        .await;

    assert_eq!(status, 401);
}
