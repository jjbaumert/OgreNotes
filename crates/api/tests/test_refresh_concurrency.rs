// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Deterministic refresh races over HTTP, with real DynamoDB writes. The
//! session-only proxy holds completed reads so both requests have observed
//! the same token before either is allowed to update it. A separate update
//! gate pins the expiry check to the timestamp submitted by the application.

mod common;

use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use ogrenotes_api::routes;
use ogrenotes_storage::{dynamo::DynamoClient, repo::session_repo::SessionRepo};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc};

struct Server {
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { base, task }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct SessionReadGate {
    capture_update: Option<mpsc::Sender<i64>>,
    update_held: AtomicBool,
    client: reqwest::Client,
    seen: AtomicUsize,
    hold_count: usize,
    captured: mpsc::Sender<()>,
    release: Semaphore,
}

async fn proxy_dynamo(
    State(gate): State<Arc<SessionReadGate>>,
    mut headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, HeaderMap, Body) {
    let target = headers.get("x-amz-target").unwrap().to_str().unwrap();
    let is_get = target.ends_with(".GetItem");
    if target.ends_with(".UpdateItem") {
        if let Some(captured) = &gate.capture_update {
            if !gate.update_held.swap(true, Ordering::SeqCst) {
                let request: Value = serde_json::from_slice(&body).unwrap();
                let verified_at = request["ExpressionAttributeValues"][":now"]["N"]
                    .as_str().unwrap().parse::<i64>().unwrap();
                captured.send(verified_at).await.unwrap();
                gate.release.acquire().await.unwrap().forget();
            }
        }
    }
    headers.remove("host");
    let response = gate
        .client
        .post("http://127.0.0.1:8000/")
        .headers(headers)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.unwrap();
    assert!(status.is_success() || status == StatusCode::BAD_REQUEST);
    if is_get && gate.seen.fetch_add(1, Ordering::SeqCst) < gate.hold_count {
        gate.captured.send(()).await.unwrap();
        gate.release.acquire().await.unwrap().forget();
    }
    (status, headers, Body::from(body))
}

async fn gated_app(
    hold_count: usize,
) -> (
    common::TestApp,
    Server,
    Server,
    Arc<SessionReadGate>,
    mpsc::Receiver<()>,
) {
    gated_app_with_update_capture(hold_count, None).await
}

async fn gated_app_with_update_capture(
    hold_count: usize,
    capture_update: Option<mpsc::Sender<i64>>,
) -> (
    common::TestApp,
    Server,
    Server,
    Arc<SessionReadGate>,
    mpsc::Receiver<()>,
) {
    let mut app = common::TestApp::new().await;
    let (captured, receiver) = mpsc::channel(hold_count.max(1));
    let gate = Arc::new(SessionReadGate {
        capture_update,
        update_held: AtomicBool::new(false),
        client: reqwest::Client::new(),
        seen: AtomicUsize::new(0),
        hold_count,
        captured,
        release: Semaphore::new(0),
    });
    let proxy = Server::start(
        Router::new()
            .route("/", post(proxy_dynamo))
            .with_state(gate.clone()),
    )
    .await;
    let config = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url(&proxy.base)
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "test", "test", None, None, "test",
        ))
        .behavior_version_latest()
        .build();
    app.state.session_repo = Arc::new(SessionRepo::new(DynamoClient::new(
        aws_sdk_dynamodb::Client::from_conf(config),
        app.table_name.clone(),
    )));
    let server = Server::start(routes::stateful_router(app.state.clone())).await;
    (app, server, proxy, gate, receiver)
}

async fn post_json(base: &str, path: &str, body: &Value) -> (StatusCode, Value) {
    let response = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header(
            "X-Forwarded-For",
            format!("refresh-race-{}", nanoid::nanoid!()),
        )
        .json(body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    (status, response.json().await.unwrap())
}

fn refresh_body(login: &Value) -> Value {
    json!({"userId": login["userId"], "sessionId": login["sessionId"], "refreshToken": login["refreshToken"]})
}

async fn wait_for_reads(receiver: &mut mpsc::Receiver<()>, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        for _ in 0..count {
            receiver.recv().await.expect("session read captured");
        }
    })
    .await
    .expect("requests reached the controlled session read boundary");
}

#[tokio::test]
async fn concurrent_http_refresh_consumes_token_once_and_preserves_winner() {
    common::require_infra!();
    let (app, server, _proxy, gate, mut reads) = gated_app(2).await;
    let (status, login) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"race@test.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, other_session) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"race@test.com"}),
    )
    .await;
    let body = refresh_body(&login);
    let tasks: Vec<_> = (0..2)
        .map(|_| {
            let base = server.base.clone();
            let body = body.clone();
            tokio::spawn(async move { post_json(&base, "/api/v1/auth/refresh", &body).await })
        })
        .collect();
    wait_for_reads(&mut reads, 2).await;
    gate.release.add_permits(2);
    let mut responses = Vec::new();
    for task in tasks {
        responses.push(task.await.unwrap());
    }
    assert_eq!(
        responses.iter().filter(|r| r.0 == StatusCode::OK).count(),
        1,
        "exactly one request may consume the old token"
    );
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.0 == StatusCode::UNAUTHORIZED)
            .count(),
        1
    );
    let winner = &responses.iter().find(|r| r.0 == StatusCode::OK).unwrap().1;
    let current = json!({"userId": login["userId"], "sessionId": login["sessionId"], "refreshToken": winner["refreshToken"]});
    assert_eq!(
        post_json(&server.base, "/api/v1/auth/refresh", &current)
            .await
            .0,
        StatusCode::OK,
        "the rejected concurrent request must not revoke the winning successor"
    );
    assert_eq!(
        post_json(
            &server.base,
            "/api/v1/auth/refresh",
            &refresh_body(&other_session)
        )
        .await
        .0,
        StatusCode::OK,
        "a CAS loser must not revoke an independent session"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn http_refresh_racing_logout_cannot_recreate_the_deleted_session() {
    common::require_infra!();
    let (app, server, _proxy, gate, mut reads) = gated_app(1).await;
    let (status, login) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"logout-race@test.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let base = server.base.clone();
    let body = refresh_body(&login);
    let pending =
        tokio::spawn(async move { post_json(&base, "/api/v1/auth/refresh", &body).await });
    wait_for_reads(&mut reads, 1).await;
    let logout = reqwest::Client::new()
        .post(format!("{}/api/v1/auth/logout", server.base))
        .bearer_auth(login["accessToken"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    gate.release.add_permits(1);
    assert_eq!(pending.await.unwrap().0, StatusCode::UNAUTHORIZED);
    assert!(
        app.state
            .session_repo
            .get(
                login["userId"].as_str().unwrap(),
                login["sessionId"].as_str().unwrap()
            )
            .await
            .unwrap()
            .is_none(),
        "a stale update must not recreate even a partial session row"
    );
    app.cleanup().await;
}

#[tokio::test]
async fn sequential_http_reuse_still_revokes_a_second_independent_session() {
    common::require_infra!();
    let (app, server, _proxy, _gate, _reads) = gated_app(0).await;
    let (status, first) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"reuse-two@test.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, second) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"reuse-two@test.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(first["sessionId"], second["sessionId"]);
    let old = refresh_body(&first);
    let (status, rotated) = post_json(&server.base, "/api/v1/auth/refresh", &old).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        post_json(&server.base, "/api/v1/auth/refresh", &old)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let current = json!({"userId": first["userId"], "sessionId": first["sessionId"], "refreshToken": rotated["refreshToken"]});
    assert_eq!(
        post_json(&server.base, "/api/v1/auth/refresh", &current)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_json(&server.base, "/api/v1/auth/refresh", &refresh_body(&second))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.cleanup().await;
}

#[tokio::test]
async fn http_refresh_cannot_extend_a_session_that_expired_after_its_read() {
    common::require_infra!();
    let (app, server, _proxy, gate, mut reads) = gated_app(1).await;
    let (status, login) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"expiry-race@test.com"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let base = server.base.clone();
    let body = refresh_body(&login);
    let pending =
        tokio::spawn(async move { post_json(&base, "/api/v1/auth/refresh", &body).await });
    wait_for_reads(&mut reads, 1).await;
    use aws_sdk_dynamodb::types::AttributeValue;
    app.dynamo_client()
        .update_item()
        .table_name(&app.table_name)
        .key(
            "PK",
            AttributeValue::S(format!("USER#{}", login["userId"].as_str().unwrap())),
        )
        .key(
            "SK",
            AttributeValue::S(format!("SESSION#{}", login["sessionId"].as_str().unwrap())),
        )
        .update_expression("SET expires_at = :expired")
        .expression_attribute_values(":expired", AttributeValue::N("0".into()))
        .send()
        .await
        .unwrap();
    gate.release.add_permits(1);
    assert_eq!(pending.await.unwrap().0, StatusCode::UNAUTHORIZED);
    let session = app
        .state
        .session_repo
        .get(
            login["userId"].as_str().unwrap(),
            login["sessionId"].as_str().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.expires_at, 0);
    app.cleanup().await;
}

/// Expiry is a credential-verification deadline, sampled by the application
/// immediately before submitting the conditional update. DynamoDB evaluates
/// the current row against that timestamp, not against a database wall clock.
#[tokio::test]
async fn http_refresh_authorized_before_expiry_can_finish_after_expiry() {
    common::require_infra!();
    let (captured, mut updates) = mpsc::channel(1);
    let (app, server, _proxy, gate, _reads) =
        gated_app_with_update_capture(0, Some(captured)).await;
    let (status, login) = post_json(
        &server.base,
        "/api/v1/auth/dev-login",
        &json!({"email":"expiry-boundary@test.com"}),
    ).await;
    assert_eq!(status, StatusCode::OK);
    let base = server.base.clone();
    let body = refresh_body(&login);
    let pending = tokio::spawn(async move {
        post_json(&base, "/api/v1/auth/refresh", &body).await
    });
    let verified_at = tokio::time::timeout(Duration::from_secs(10), updates.recv())
        .await.expect("rotation reached UpdateItem before forwarding")
        .expect("verification timestamp captured");

    // Put the old row's expiry strictly between the captured verification
    // time and the delayed write. Choosing the deadline after the handshake
    // avoids a short-TTL race with CI scheduling. This fixture changes only
    // the stored expiry; the held production UpdateItem remains untouched.
    let expires_at = verified_at + 1;
    use aws_sdk_dynamodb::types::AttributeValue;
    app.dynamo_client().update_item()
        .table_name(&app.table_name)
        .key("PK", AttributeValue::S(format!("USER#{}", login["userId"].as_str().unwrap())))
        .key("SK", AttributeValue::S(format!("SESSION#{}", login["sessionId"].as_str().unwrap())))
        .update_expression("SET expires_at = :expiry")
        .expression_attribute_values(":expiry", AttributeValue::N(expires_at.to_string()))
        .send().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while ogrenotes_common::time::now_usec() <= expires_at {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }).await.expect("old session deadline passed before forwarding UpdateItem");
    let session = app.state.session_repo.get(
        login["userId"].as_str().unwrap(), login["sessionId"].as_str().unwrap(),
    ).await.unwrap().unwrap();
    assert_eq!(session.expires_at, expires_at);
    assert!(session.is_expired(), "the stored old session expired while the update was held");

    gate.release.add_permits(1);
    let (status, rotated) = pending.await.unwrap();
    assert_eq!(status, StatusCode::OK, "expiry is checked at verification, not commit");
    let successor = json!({"userId": login["userId"], "sessionId": login["sessionId"],
        "refreshToken": rotated["refreshToken"]});
    assert_eq!(post_json(&server.base, "/api/v1/auth/refresh", &successor).await.0,
        StatusCode::OK, "the authorized successor remains usable");
    app.cleanup().await;
}
