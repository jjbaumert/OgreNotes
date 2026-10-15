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
//!         --report-file target/goose-report.html \
//!         --no-reset-metrics
//!
//! Goose tracks per-request p50/p95/p99/max + RPS into the report.
//! The driver checks the home-folder listing p95 against the documented
//! 300 ms read budget and rejects empty or error-heavy runs.

use goose::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static MISSING_SESSIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_EXACT_LATENCY_MS: usize = 1_000;
static DOCUMENT_LATENCIES: OnceLock<Vec<AtomicUsize>> = OnceLock::new();
static ME_LATENCIES: OnceLock<Vec<AtomicUsize>> = OnceLock::new();

fn record_latency(histogram: &OnceLock<Vec<AtomicUsize>>, milliseconds: u64) {
    let counts = histogram.get_or_init(|| {
        (0..=MAX_EXACT_LATENCY_MS + 1)
            .map(|_| AtomicUsize::new(0))
            .collect()
    });
    let bucket = usize::try_from(milliseconds)
        .unwrap_or(MAX_EXACT_LATENCY_MS + 1)
        .min(MAX_EXACT_LATENCY_MS + 1);
    counts[bucket].fetch_add(1, Ordering::Relaxed);
}

fn exact_p95(histogram: &OnceLock<Vec<AtomicUsize>>) -> Option<u64> {
    let counts = histogram.get()?;
    let total: usize = counts
        .iter()
        .map(|count| count.load(Ordering::Relaxed))
        .sum();
    if total == 0 {
        return None;
    }
    let rank = (total * 95).div_ceil(100);
    let mut seen = 0;
    for (millisecond, count) in counts.iter().enumerate() {
        seen += count.load(Ordering::Relaxed);
        if seen >= rank {
            return Some(millisecond as u64);
        }
    }
    None
}

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    home_folder_id: String,
}

/// Per-user session cached after `dev_login` runs at session start.
/// Pulled out of `user.get_session_data` by every measured
/// transaction so the bearer token follows the user.
struct Session {
    token: String,
    home_folder_id: String,
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
    // — Err means Goose already recorded a transport failure. Return an
    // error as well, so on_start cannot appear to have succeeded.
    let mut request = goose_response.request;
    let response = match goose_response.response {
        Ok(response) => response,
        Err(_) => return user.set_failure("dev login transport failure", &mut request, None, None),
    };
    if !response.status().is_success() {
        return user.set_failure("dev login rejected", &mut request, None, None);
    }
    let token: TokenResponse = match response.json().await {
        Ok(token) => token,
        Err(_) => {
            return user.set_failure("dev login invalid token response", &mut request, None, None);
        }
    };
    if token.access_token.trim().is_empty() {
        return user.set_failure("dev login empty access token", &mut request, None, None);
    }
    // The document list is the caller's home folder, not GET /documents
    // (which is POST-only). Resolve its id from the same profile response
    // the frontend uses, and fail setup if it is missing.
    let profile_builder = user
        .get_request_builder(&GooseMethod::Get, "/api/v1/users/me")?
        .header("authorization", format!("Bearer {}", token.access_token));
    let profile_request = GooseRequest::builder()
        .set_request_builder(profile_builder)
        .name("resolve_home_folder")
        .build();
    let profile_response = user.request(profile_request).await?;
    let mut request = profile_response.request;
    let response = match profile_response.response {
        Ok(response) => response,
        Err(_) => {
            return user.set_failure("profile lookup transport failure", &mut request, None, None);
        }
    };
    if !response.status().is_success() {
        return user.set_failure("profile lookup rejected", &mut request, None, None);
    }
    let profile: MeResponse = match response.json().await {
        Ok(profile) => profile,
        Err(_) => {
            return user.set_failure("profile lookup invalid response", &mut request, None, None);
        }
    };
    if profile.home_folder_id.trim().is_empty() {
        return user.set_failure("profile missing home folder", &mut request, None, None);
    }
    user.set_session_data(Session {
        token: token.access_token,
        home_folder_id: profile.home_folder_id,
    });
    Ok(())
}

/// Build a `reqwest::RequestBuilder` with the per-user bearer
/// token attached. A missing session is counted and rejected by the
/// post-run gate; no unauthenticated request is sent.
fn authed_builder(
    user: &mut GooseUser,
    method: &GooseMethod,
    path: impl FnOnce(&Session) -> String,
) -> Result<Option<reqwest::RequestBuilder>, Box<TransactionError>> {
    let Some(session) = user.get_session_data::<Session>() else {
        MISSING_SESSIONS.fetch_add(1, Ordering::Relaxed);
        return Ok(None);
    };
    let token = session.token.clone();
    let path = path(session);
    let builder = user.get_request_builder(method, &path)?;
    Ok(Some(
        builder.header("authorization", format!("Bearer {token}")),
    ))
}

async fn list_documents(user: &mut GooseUser) -> TransactionResult {
    let Some(request_builder) = authed_builder(user, &GooseMethod::Get, |session| {
        format!("/api/v1/folders/{}", session.home_folder_id)
    })?
    else {
        // An on_start login failure leaves this user without a session.
        // Yield so the no-op transaction loop cannot starve Goose's
        // run-time timer; the post-run gate reports the missing session.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        return Ok(());
    };
    let req = GooseRequest::builder()
        .set_request_builder(request_builder)
        .name("GET /home-folder")
        .build();
    let response = user.request(req).await?;
    if response.response.is_ok() {
        record_latency(&DOCUMENT_LATENCIES, response.request.response_time);
    }
    Ok(())
}

async fn get_me(user: &mut GooseUser) -> TransactionResult {
    let Some(request_builder) =
        authed_builder(user, &GooseMethod::Get, |_| "/api/v1/users/me".to_string())?
    else {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        return Ok(());
    };
    let req = GooseRequest::builder()
        .set_request_builder(request_builder)
        .name("GET /users/me")
        .build();
    let response = user.request(req).await?;
    if response.response.is_ok() {
        record_latency(&ME_LATENCIES, response.request.response_time);
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let metrics = GooseAttack::initialize()?
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
                .register_transaction(transaction!(dev_login).set_name("dev_login").set_on_start())
                // This smoke workload is 80% list and 20% me-fetch.
                // It does not yet implement the seeded list+open/edit
                // workload in design/performance-budgets.md.
                .register_transaction(
                    transaction!(list_documents)
                        .set_name("list_documents")
                        .set_weight(80)?,
                )
                .register_transaction(transaction!(get_me).set_name("get_me").set_weight(20)?),
        )
        .execute()
        .await?;
    let summary = assess(&metrics)?;
    let serialized = serde_json::to_string_pretty(&summary)?;
    println!("{serialized}");
    if let Ok(path) = std::env::var("LOAD_SUMMARY_FILE") {
        std::fs::write(path, &serialized)?;
    }
    if summary.failures.is_empty() {
        Ok(())
    } else {
        Err(summary.failures.join("; ").into())
    }
}

// Deserialize the public GooseMetrics wire shape into only the fields
// needed for gates. This keeps report formatting independent of the
// validation logic and makes the acceptance calculation testable.
#[derive(Deserialize)]
struct RequestMetrics {
    path: String,
    success_count: usize,
    fail_count: usize,
}

#[derive(Serialize)]
struct LoadSummary {
    successful_logins: usize,
    successful_authenticated_requests: usize,
    failed_requests: usize,
    error_rate: f64,
    documents_p95_ms: Option<u64>,
    me_p95_ms: Option<u64>,
    minimum_authenticated_requests: usize,
    minimum_successful_logins: usize,
    missing_session_attempts: usize,
    failures: Vec<String>,
}

fn assess(metrics: &goose::metrics::GooseMetrics) -> Result<LoadSummary, Box<dyn Error>> {
    let value = serde_json::to_value(metrics)?;
    let requests: HashMap<String, RequestMetrics> = serde_json::from_value(
        value
            .get("requests")
            .cloned()
            .ok_or("Goose metrics omitted requests")?,
    )?;
    let find = |names: &[&str]| {
        requests
            .values()
            .find(|request| names.contains(&request.path.as_str()))
    };
    let login = find(&["dev_login", "/api/v1/auth/dev-login"]);
    let documents = find(&["GET /home-folder"]);
    let me = find(&["GET /users/me", "/api/v1/users/me"]);
    let successful_logins = login.map_or(0, |request| request.success_count);
    let successful_authenticated_requests = [documents, me]
        .into_iter()
        .flatten()
        .map(|request| request.success_count)
        .sum();
    let (successes, failed_requests) = requests.values().fold((0, 0), |counts, request| {
        (
            counts.0 + request.success_count,
            counts.1 + request.fail_count,
        )
    });
    let error_rate = if successes + failed_requests == 0 {
        1.0
    } else {
        failed_requests as f64 / (successes + failed_requests) as f64
    };
    // Goose rounds its aggregate latency buckets near 300/500 ms. Keep a
    // bounded exact histogram from completed HTTP responses so a 304 ms
    // response cannot pass a 300 ms p95 gate as a rounded 300 ms sample.
    let documents_p95_ms = exact_p95(&DOCUMENT_LATENCIES);
    let me_p95_ms = exact_p95(&ME_LATENCIES);
    let minimum_authenticated_requests = std::env::var("LOAD_MIN_AUTHENTICATED_REQUESTS")
        .ok()
        .map_or(Ok(100), |value| value.parse::<usize>())?;
    let minimum_successful_logins = std::env::var("LOAD_MIN_SUCCESSFUL_LOGINS")
        .ok()
        .map_or(Ok(1), |value| value.parse::<usize>())?;
    let missing_session_attempts = MISSING_SESSIONS.load(Ordering::Relaxed);
    let mut failures = Vec::new();
    if missing_session_attempts > 0 {
        failures.push(format!(
            "{missing_session_attempts} authenticated transactions skipped because login produced no session"
        ));
    }
    if successful_logins < minimum_successful_logins
        || login.is_some_and(|request| request.fail_count > 0)
    {
        failures.push(format!(
            "login failed or too few usable sessions: {successful_logins} successful logins (minimum {minimum_successful_logins})"
        ));
    }
    if successful_authenticated_requests < minimum_authenticated_requests
        || documents.is_none_or(|request| request.success_count == 0)
        || me.is_none_or(|request| request.success_count == 0)
    {
        failures.push(format!(
            "authenticated traffic too low: {successful_authenticated_requests} successful requests (minimum {minimum_authenticated_requests}, with both routes exercised)"
        ));
    }
    if error_rate > 0.01 {
        failures.push(format!(
            "request error rate {:.2}% exceeds 1%",
            error_rate * 100.0
        ));
    }
    for (route, latency, budget) in [
        ("GET /home-folder", documents_p95_ms, 300),
        ("GET /users/me", me_p95_ms, 500),
    ] {
        if latency.is_none_or(|p95| p95 > budget) {
            failures.push(format!(
                "{route} p95 {latency:?} ms exceeds {budget} ms or has no samples"
            ));
        }
    }
    Ok(LoadSummary {
        successful_logins,
        successful_authenticated_requests,
        failed_requests,
        error_rate,
        documents_p95_ms,
        me_p95_ms,
        minimum_authenticated_requests,
        minimum_successful_logins,
        missing_session_attempts,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p95_keeps_the_exact_value_at_the_sla_boundary() {
        let histogram = OnceLock::new();
        for _ in 0..19 {
            record_latency(&histogram, 250);
        }
        record_latency(&histogram, 304);
        assert_eq!(exact_p95(&histogram), Some(250));
        record_latency(&histogram, 304);
        assert_eq!(exact_p95(&histogram), Some(304));
    }
}
