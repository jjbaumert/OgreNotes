// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #144: per-user document favorites (star) — add / remove / list, and the
// `isFavorite` flag on the document GET.

mod common;

use hyper::Method;

#[tokio::test]
async fn test_favorite_lifecycle() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("fav-user@test.com").await;
    let doc_id = app.create_doc(&token, "Star me", None).await;

    // Initially not favorited.
    let (s, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["isFavorite"].as_bool(), Some(false));

    // Favorites list is empty.
    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/favorites", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));

    // Star it.
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/favorite"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // GET now reports isFavorite=true.
    let (s, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body["isFavorite"].as_bool(), Some(true), "doc must report favorited");

    // ...and it appears in the favorites list with its title.
    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/favorites", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1, "favorites list must contain the starred doc");
    assert_eq!(list[0]["id"].as_str(), Some(doc_id.as_str()));
    assert_eq!(list[0]["title"].as_str(), Some("Star me"));

    // Star again — idempotent (still one entry).
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/favorite"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/favorites", Some(&token), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(1), "favoriting is idempotent");

    // Unstar.
    let (s, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/favorite"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // Back to not-favorited + empty list.
    let (_, body) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(body["isFavorite"].as_bool(), Some(false));
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/favorites", Some(&token), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));

    app.cleanup().await;
}

/// A favorite is per-user: one user's star isn't visible to another.
#[tokio::test]
async fn test_favorites_are_per_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_aid, alice) = app.create_user("fav-alice@test.com").await;
    let (_bid, bob) = app.create_user("fav-bob@test.com").await;
    let doc_id = app.create_doc(&alice, "Alice doc", None).await;

    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/favorite"),
            Some(&alice),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // Bob's favorites are unaffected.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/favorites", Some(&bob), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(0), "favorites are per-user");

    app.cleanup().await;
}

/// #144: collections (named groups within Favorites) — create, membership
/// toggle, listing with inlined docs, and delete.
#[tokio::test]
async fn test_collection_lifecycle() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("coll-user@test.com").await;
    let doc_id = app.create_doc(&token, "Grouped doc", None).await;

    // No collections yet; per-doc membership list is empty.
    let (s, body) = app
        .json_request(Method::GET, "/api/v1/documents/collections", Some(&token), None)
        .await;
    assert_eq!(s, 200);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));
    let (s, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/collections"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(body.as_array().map(|a| a.len()), Some(0));

    // "New Collection…" — creates a collection containing this doc.
    let (s, body) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/collections"),
            Some(&token),
            Some(serde_json::json!({ "name": "Reading" })),
        )
        .await;
    assert_eq!(s, 200);
    let cid = body["id"].as_str().expect("collection id").to_string();
    assert_eq!(body["name"].as_str(), Some("Reading"));

    // The doc's membership list now shows the collection as containing it.
    let (_, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/collections"),
            Some(&token),
            None,
        )
        .await;
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"].as_str(), Some(cid.as_str()));
    assert_eq!(list[0]["contains"].as_bool(), Some(true));

    // The sidebar listing inlines the doc.
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/collections", Some(&token), None)
        .await;
    let colls = body.as_array().expect("array");
    assert_eq!(colls.len(), 1);
    assert_eq!(colls[0]["name"].as_str(), Some("Reading"));
    assert_eq!(colls[0]["items"].as_array().map(|a| a.len()), Some(1));
    assert_eq!(colls[0]["items"][0]["id"].as_str(), Some(doc_id.as_str()));

    // Remove the doc from the collection.
    let (s, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/{doc_id}/collections/{cid}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (_, body) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/collections"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body[0]["contains"].as_bool(), Some(false), "doc removed from collection");

    // Re-add via PUT to the existing collection.
    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/collections/{cid}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);

    // Delete the whole collection.
    let (s, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/documents/collections/{cid}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 204);
    let (_, body) = app
        .json_request(Method::GET, "/api/v1/documents/collections", Some(&token), None)
        .await;
    assert_eq!(body.as_array().map(|a| a.len()), Some(0), "collection deleted");

    app.cleanup().await;
}

/// Adding a doc to a collection that isn't the caller's 404s.
#[tokio::test]
async fn test_add_to_unknown_collection_404s() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_uid, token) = app.create_user("coll-404@test.com").await;
    let doc_id = app.create_doc(&token, "Doc", None).await;

    let (s, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/collections/nonexistent"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(s, 404);

    app.cleanup().await;
}
