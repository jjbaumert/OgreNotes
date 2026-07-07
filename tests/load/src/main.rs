// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P9 piece E — goose-rs load-test driver.
//!
//! Single binary, scenario selected at CLI via goose's standard
//! `--scenarios` flag. v1 ships one scenario (registered as
//! `read-heavy` for display, invoked as `readheavy` because goose
//! requires the flag value to be all-alphanumeric); the
//! other three from `design/performance-budgets.md` (`edit_heavy`,
//! `chat_heavy`, `search_spike`) are listed in the README as TBD —
//! the WS-edit and search-payload work they need is outside the
//! piece-E harness scope.
//!
//! Auth: each goose user runs `dev_login` once at session start and
//! caches the access token in session data. Subsequent transactions
//! pull the token out and attach `Authorization: Bearer <token>`.
//! The dev-login endpoint requires `DEV_MODE=true` on the target;
//! the CI workflow sets that explicitly. Production builds with
//! `DEV_MODE=false` return 404 from that route — running the load
//! test against prod is a deliberate "no".
//!
//! Invocation (against a local stack):
//!
//!     cargo run --release --manifest-path tests/load/Cargo.toml -- \
//!         --host http://127.0.0.1:3000 \
//!         --users 50 --hatch-rate 5 --run-time 60s \
//!         --scenarios readheavy \
//!         --report-file target/goose-report.html
//!
//! Goose tracks per-request p50/p95/p99/max + RPS into the report.
//! The CI workflow asserts the read_heavy p95 for `GET /documents`
//! stays inside design/performance-budgets.md's SLA (300 ms).

use goose::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct DevLoginRequest {
    email: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenResponse {
    access_token: String,
}

/// Per-user session cached after `dev_login` runs at session start.
/// Pulled out of `user.get_session_data` by every measured
/// transaction so the bearer token follows the user.
struct Session {
    token: String,
}

async fn dev_login(user: &mut GooseUser) -> TransactionResult {
    // Globally-unique email per goose user. Including the process
    // PID lets two parallel goose runs against the same backend not
    // collide on the user-row uniqueness check.
    let email = format!(
        "load-{}-{}@ogrenotes.example.com",
        std::process::id(),
        user.weighted_users_index,
    );
    let body = DevLoginRequest {
        email,
        name: "LoadBot".to_string(),
    };
    let request_builder = user
        .get_request_builder(&GooseMethod::Post, "/api/v1/auth/dev-login")?
        .json(&body);
    let goose_request = GooseRequest::builder()
        .set_request_builder(request_builder)
        .name("dev_login")
        .build();
    let goose_response = user.request(goose_request).await?;
    // `goose_response.response` is `Result<reqwest::Response, reqwest::Error>`
    // — Err means the request failed at the transport layer (goose has
    // already counted it as a failure for the report). Bail quietly; the
    // next session-start tick will retry.
    let response = match goose_response.response {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let token: TokenResponse = match response.json().await {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    user.set_session_data(Session {
        token: token.access_token,
    });
    Ok(())
}

/// Build a `reqwest::RequestBuilder` with the per-user bearer
/// token attached. Returns `None` if the session-cache miss means
/// `dev_login` didn't run (would mean the on_start hook is wired
/// wrong — failure mode caught here surfaces it loudly).
fn authed_builder(
    user: &mut GooseUser,
    method: &GooseMethod,
    path: &str,
) -> Option<reqwest::RequestBuilder> {
    let token = user.get_session_data::<Session>()?.token.clone();
    let builder = user.get_request_builder(method, path).ok()?;
    Some(builder.header("authorization", format!("Bearer {token}")))
}

async fn list_documents(user: &mut GooseUser) -> TransactionResult {
    let Some(request_builder) = authed_builder(user, &GooseMethod::Get, "/api/v1/documents") else {
        return Ok(());
    };
    let req = GooseRequest::builder()
        .set_request_builder(request_builder)
        .name("GET /documents")
        .build();
    let _ = user.request(req).await?;
    Ok(())
}

async fn get_me(user: &mut GooseUser) -> TransactionResult {
    let Some(request_builder) = authed_builder(user, &GooseMethod::Get, "/api/v1/users/me") else {
        return Ok(());
    };
    let req = GooseRequest::builder()
        .set_request_builder(request_builder)
        .name("GET /users/me")
        .build();
    let _ = user.request(req).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), GooseError> {
    GooseAttack::initialize()?
        .register_scenario(
            // Goose canonicalizes scenario names by stripping every
            // non-alphanumeric character and lower-casing the result —
            // so `ReadHeavy` and `read-heavy` both register as
            // `readheavy`. The `--scenarios` flag, however, refuses
            // non-alphanumeric values up front with `invalid
            // 'configuration.scenarios' value`. The CLI form must
            // therefore be the bare canonical `readheavy` (see
            // `--scenarios readheavy` in load-tests.yml + the README).
            // We keep the registered display name human-readable; the
            // mismatch with the CLI form is documented inline.
            scenario!("read-heavy")
                .register_transaction(
                    transaction!(dev_login)
                        .set_name("dev_login")
                        .set_on_start(),
                )
                // 80% list, 20% me-fetch per design/performance-budgets.md.
                // Goose weights are relative integers, so 80 : 20 is the
                // direct mapping. The "open document" step is omitted in
                // v1 — it requires either a pre-seeded doc-id pool or a
                // list+pick pattern that JSON-decodes the list, which adds
                // surface area the v1 harness doesn't need to be useful.
                .register_transaction(
                    transaction!(list_documents)
                        .set_name("list_documents")
                        .set_weight(80)?,
                )
                .register_transaction(
                    transaction!(get_me)
                        .set_name("get_me")
                        .set_weight(20)?,
                ),
        )
        .execute()
        .await?;
    Ok(())
}
