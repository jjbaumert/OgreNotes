// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Test harness for `crates/api` integration tests.
//!
//! Each test creates its own `TestApp` with a unique DynamoDB table and S3 bucket,
//! providing full isolation for parallel test execution. Requires Docker services:
//! DynamoDB Local (port 8000), MinIO (port 9000), Redis (port 6379).

use std::sync::LazyLock;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType,
    Projection, ProjectionType, ScalarAttributeType,
};
use axum::body::Body;
use axum::Router;
use http_body_util::BodyExt;
use hyper::{Method, Request};
use tower::ServiceExt;

use fred::prelude::*;
use ogrenotes_api::routes;
use ogrenotes_api::state::AppState;
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::s3::S3Client;
use std::sync::Arc;

// ─── Tracing ────────────────────────────────────────────────────

/// Initialize a tracing subscriber the first time any test asks. Without
/// this, `tracing::error!`/`tracing::warn!` calls in the API and storage
/// crates go nowhere — test failures show only the canned 500 body
/// ("Something went wrong") with no context. Captures all log levels and
/// routes them through cargo's per-test stdout buffer (`with_test_writer`),
/// so passing tests stay quiet but a failing test's panic dumps the
/// preceding error/warn lines alongside the assertion message.
/// Set a deterministic `MFA_ENCRYPTION_KEY` before any MFA test
/// asks for it. `LazyLock` guarantees exactly one set across all
/// test threads — the post-1.78 `unsafe fn env::set_var` is a race
/// hazard only when paired with concurrent `getenv` calls; running
/// set inside a single-init Lazy ensures all gets happen after the
/// set is observable. Value is 32 base64-url-no-pad bytes (all
/// zeros — fine for tests; production reads its key from the
/// deploy-time secret).
pub static MFA_KEY_INIT: LazyLock<()> = LazyLock::new(|| {
    let key_b64 = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 32 zero bytes
    // Safety: single-init via LazyLock; no other thread is reading
    // this env var until the Lazy completes initialization.
    unsafe {
        std::env::set_var("MFA_ENCRYPTION_KEY", key_b64);
    }
});

pub static TRACING_INIT: LazyLock<()> = LazyLock::new(|| {
    use tracing_subscriber::{fmt, EnvFilter};
    let _ = fmt()
        .with_test_writer()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("warn,ogrenotes=debug")),
        )
        .with_target(true)
        .try_init();
});

// ─── Infrastructure availability ────────────────────────────────

pub static INFRA_AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
    // Quick TCP probe for DynamoDB Local, MinIO, and Redis.
    use std::net::TcpStream;
    use std::time::Duration;
    let timeout = Duration::from_millis(200);
    let dynamo = TcpStream::connect_timeout(&"127.0.0.1:8000".parse().unwrap(), timeout).is_ok();
    let minio = TcpStream::connect_timeout(&"127.0.0.1:9000".parse().unwrap(), timeout).is_ok();
    let redis = TcpStream::connect_timeout(&"127.0.0.1:6379".parse().unwrap(), timeout).is_ok();
    if !dynamo || !minio || !redis {
        eprintln!(
            "Integration test infra unavailable (dynamo={dynamo}, minio={minio}, redis={redis}). \
             Run `docker compose up -d` to start services."
        );
    }
    dynamo && minio && redis
});

/// Call at the top of every integration test.
///
/// **Locally** (`CI` env var unset): skips the test if Docker services are
/// not running, so a developer running `cargo test` without `docker compose
/// up -d` doesn't see N hard connection-error failures — they see a
/// stderr line and the suite passes.
///
/// **In CI** (`CI=true` set automatically by GitHub Actions, GitLab CI,
/// etc.): panics if any of DynamoDB/MinIO/Redis is unreachable, turning
/// what would silently pass into a hard failure. This closes a real
/// hole — without the panic, a Redis crash between `docker compose up`
/// and `cargo test` would silently green every test that calls
/// `require_infra!`, since each test exits via `return` rather than a
/// failure.
macro_rules! require_infra {
    () => {
        if !*crate::common::INFRA_AVAILABLE {
            // Loud-by-default failure when infra (DDB/MinIO/Redis) is
            // missing. The prior contract was "silent skip" via
            // `eprintln! + return`, but cargo test captures stderr by
            // default and reports the test as `ok` — which hides "I
            // didn't actually run" behind "I passed". Local devs and CI
            // both got bitten: a body-shape bug in a test (M-E7 item 10
            // `"DocComment"`) shipped to master because the local run
            // skipped silently, and the same pattern hid the M-E6
            // camelCase detail-JSON bug for 4 days.
            //
            // Opt-out via `SKIP_INFRA_TESTS=1` for cases where running
            // without infra is genuinely intended (e.g. spot-checking a
            // pure-Rust unit test that happens to live in the
            // integration suite). CI's docker-compose step makes infra
            // available before tests run; the panic only fires when
            // someone forgot to bring up the stack.
            if std::env::var("SKIP_INFRA_TESTS").is_ok() {
                eprintln!("SKIPPED: SKIP_INFRA_TESTS=1 and infra unavailable");
                return;
            }
            panic!(
                "Integration infra unavailable (DDB Local / MinIO / Redis). \
                 Bring it up with `docker compose up -d` and re-run, or set \
                 SKIP_INFRA_TESTS=1 to explicitly skip. Check the eprintln above \
                 this panic for which service failed the TCP probe."
            );
        }
    };
}
pub(crate) use require_infra;

// ─── Rate-limit window alignment (#6) ───────────────────────────

/// Seconds a burst test must wait so it starts with at least
/// `margin_secs` of headroom in the current fixed rate-limit window.
///
/// The limiter buckets on `epoch_secs / window_secs`, so a burst
/// that straddles a wall-clock window boundary legitimately gets a
/// fresh budget mid-loop and the "(N+1)th request must 429" assert
/// flakes. Returns 0 when `margin_secs` or more remain; otherwise
/// the seconds until the next boundary.
pub fn rate_limit_alignment_wait(now_secs: u64, window_secs: u64, margin_secs: u64) -> u64 {
    let remaining = window_secs - (now_secs % window_secs);
    if remaining >= margin_secs {
        0
    } else {
        remaining
    }
}

/// Sleep (if needed) so the caller's rate-limit burst starts with at
/// least 10s left in the current 60s window. Call immediately before
/// the first rate-limited request of a burst — after `TestApp` setup,
/// which can itself take wall-clock time. 10s covers the longest
/// burst in this suite (11 in-process requests) with a wide slack
/// factor even under CI-runner scheduling jitter; the extra 250ms
/// clears the boundary despite whole-second truncation of `now`.
pub async fn align_rate_limit_window() {
    const WINDOW_SECS: u64 = 60;
    const MARGIN_SECS: u64 = 10;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let wait = rate_limit_alignment_wait(now, WINDOW_SECS, MARGIN_SECS);
    if wait > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(wait) + std::time::Duration::from_millis(250)).await;
    }
}

// ─── SAML test fixtures ─────────────────────────────────────────

/// Self-signed RSA-2048 X509 cert (DER base64) generated once for
/// the SAML test suite via `openssl req -x509 -newkey rsa:2048
/// -days 36500 -nodes`. Expires Apr 2126 — well beyond any
/// reasonable maintenance horizon for these tests. The private
/// key is NOT bundled because the tests don't mint signed
/// assertions (samael's signing path hardcodes SHA-1, which our
/// production code rejects — see runbook/saml-config.md). The cert
/// just needs to be a real-parseable X509 so the gap-001 signing-
/// cert presence check in put_saml_config can succeed.
pub const SAML_TEST_X509_CERT: &str = "MIIDKTCCAhGgAwIBAgIUabhDEoj04Xmiqsb+AGkTtmI22TQwDQYJKoZIhvcNAQELBQAwIzEhMB8GA1UEAwwYb2dyZW5vdGVzLXNhbWwtdGVzdC1jZXJ0MCAXDTI2MDUxNDEwMTk1MFoYDzIxMjYwNDIwMTAxOTUwWjAjMSEwHwYDVQQDDBhvZ3Jlbm90ZXMtc2FtbC10ZXN0LWNlcnQwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQDRex6qcd23Xkh2vFb3Df+ECWxkYHeIQUbhhpF4bJAB85qMRBnNpe7glQzZ6ePZmu5/FFomwliD3h+k8o+Qq2hfsDhPB7BamK9IccgVk8hoipJeAuRJgCrcJ3BHVawA+RqnYg/ST7tBV03a6op8XOlCk3xF+fBjSZlEgvHV+mwrG5YYmCaVJ9pBlTOQ6qeP36nYFm2O3ftEgsTO4qU2Fb6TbN5z9y+jUdvpSygto4l6xaelnK3hu8FEYUfrIz/uKMKINPS6hsRUGu9+2t/P/DQjUSIyv7BBLvHUdF9WQKOfwIOZ9ZiqGEk4G5cd46D+17AA3ClNnzf1dK5yQ1ODjYMxAgMBAAGjUzBRMB0GA1UdDgQWBBQ5a/9TD6rfqntQrSraYUyyd3qCjTAfBgNVHSMEGDAWgBQ5a/9TD6rfqntQrSraYUyyd3qCjTAPBgNVHRMBAf8EBTADAQH/MA0GCSqGSIb3DQEBCwUAA4IBAQAJ8CsNbXd91Sx0k/DLZb3Vok3daYp4DtK+Ow/vc2gEdlSkn4PJekXe3P6uDBVUfTretNpM508NuTF0jFItClGQuFwB+fF/AZofftBd4he9rU7oU6jmZJAALNJMfkOiPee+Fkj2L3WlT53oG3MgwA6GxYAZ0x6wZuLVNis9FXZa281YktGqN64tv7c5FgBBu30t71ZA3GI9wig7ab6i7kBklWfzJ0spE1VOQLgMRbz73V6HH3zFcNOWjsGnBkgE2uVRkXUAx4YSrzLn3RP9qG93ijcGkLb3RTCbq2KAafbjdrPtjFmBqhV3Y9yKMpy3EeqeJo/TMbrekIAAx1G02JNc";

/// IdP metadata template with a real signing cert. Tests use this
/// for `put_saml_config` and downstream ACS scenarios. The HTTP-
/// Redirect SSO endpoint is mandatory for `/auth/saml/login` to
/// produce a redirect; the signing KeyDescriptor is mandatory for
/// `put_saml_config` to accept the upload (gap-001).
pub fn saml_test_idp_metadata() -> String {
    format!(r##"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.test/saml/test">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <KeyInfo xmlns="http://www.w3.org/2000/09/xmldsig#">
        <X509Data><X509Certificate>{SAML_TEST_X509_CERT}</X509Certificate></X509Data>
      </KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.test/sso/redirect"/>
  </IDPSSODescriptor>
</EntityDescriptor>"##)
}

// ─── TestApp ────────────────────────────────────────────────────

pub struct TestApp {
    pub router: Router,
    pub state: AppState,
    pub table_name: String,
    pub bucket: String,
    /// See `default_xff` derivation in `new_with_claude` for context.
    /// Read by `json_request` / `bytes_request` / `raw_request` to
    /// inject `X-Forwarded-For` when the caller didn't supply one.
    pub default_xff: String,
    dynamo_client: aws_sdk_dynamodb::Client,
    s3_client: aws_sdk_s3::Client,
}

impl TestApp {
    /// Create a fully-wired test application with isolated DynamoDB table + S3 bucket.
    pub async fn new() -> Self {
        Self::new_with_claude(None).await
    }

    /// Create a TestApp with a custom (or stubbed) `ClaudeMessages` impl
    /// installed as `state.claude_client`. Used by `test_ask_acl.rs` to
    /// script the agent loop deterministically; everything else passes
    /// `None` (the equivalent of "ANTHROPIC_API_KEY not set" — `/api/v1/ask`
    /// returns 503).
    pub async fn new_with_claude(
        claude: Option<Arc<dyn ogrenotes_api::claude::ClaudeMessages>>,
    ) -> Self {
        Self::new_with_claude_and_admin_emails(claude, Vec::new()).await
    }

    /// Create a TestApp whose `config.admin_emails` allowlist is pre-seeded.
    /// Lets tests exercise the login-time admin-email auto-promotion path
    /// (`auth_policy::apply_admin_email_promotion`), which the default
    /// constructors leave unreachable (empty allowlist).
    #[allow(dead_code)]
    pub async fn new_with_admin_emails(admin_emails: Vec<String>) -> Self {
        Self::new_with_claude_and_admin_emails(None, admin_emails).await
    }

    /// The full builder. The two constructors above are thin wrappers so
    /// existing callers (`new`, `test_ask_acl`) keep their signatures.
    pub async fn new_with_claude_and_admin_emails(
        claude: Option<Arc<dyn ogrenotes_api::claude::ClaudeMessages>>,
        admin_emails: Vec<String>,
    ) -> Self {
        // Force the tracing subscriber on first call so internal_error
        // logs surface in the failing-test stdout dump.
        let _ = *TRACING_INIT;

        let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789".chars().collect();
        let prefix = nanoid::nanoid!(8, &alphabet);
        let table_name = format!("test-{prefix}-ogrenotes");
        let bucket = format!("test-{prefix}-ogrenotes");

        // Build AWS SDK config pointed at local services
        let dynamo_config = aws_sdk_dynamodb::config::Builder::new()
            .endpoint_url("http://127.0.0.1:8000")
            .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                "fakekey", "fakesecret", None, None, "test",
            ))
            .behavior_version_latest()
            .build();
        let dynamo_client = aws_sdk_dynamodb::Client::from_conf(dynamo_config);

        let s3_config = aws_sdk_s3::config::Builder::new()
            .endpoint_url("http://127.0.0.1:9000")
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "minioadmin", "minioadmin", None, None, "test",
            ))
            .force_path_style(true)
            .behavior_version_latest()
            .build();
        let s3_client = aws_sdk_s3::Client::from_conf(s3_config);

        // Create DynamoDB table
        create_table(&dynamo_client, &table_name).await;

        // Create S3 bucket
        let _ = s3_client.create_bucket().bucket(&bucket).send().await;

        // Build AppState
        let dynamo = DynamoClient::new(dynamo_client.clone(), table_name.clone());
        let s3 = S3Client::new(s3_client.clone(), bucket.clone());

        let redis_config = fred::types::RedisConfig::from_url("redis://127.0.0.1:6379")
            .expect("invalid redis URL");
        let redis_client = RedisClient::new(redis_config, None, None, None);
        redis_client.connect();
        redis_client.wait_for_connect().await.expect("Redis connect failed");
        // AppState::new builds the pub/sub and session stores from this
        // shared command client (#97).
        let redis = Arc::new(redis_client);

        let config = AppConfig {
            aws_region: "us-east-1".into(),
            dynamodb_table_prefix: format!("test-{prefix}-"),
            s3_bucket: bucket.clone(),
            redis_url: "redis://127.0.0.1:6379".into(),
            oauth_client_id: "test-client-id".into(),
            oauth_client_secret: "test-client-secret".into(),
            oauth_redirect_uri: "http://localhost:8080/auth/callback".into(),
            jwt_secret: "test-jwt-secret-that-is-at-least-32-bytes-long".into(),
            google_client_id: None,
            google_client_secret: None,
            api_port: 0,
            frontend_origin: "http://localhost:8080".into(),
            search_index_path: String::new(),
            qdrant_url: None,
            embedding_model_id: "amazon.titan-embed-text-v2:0".into(),
            embedding_dimensions: 1024,
            anthropic_api_key: None,
            anthropic_model: "claude-sonnet-4-6".into(),
            admin_emails,
            dev_mode: true,
            embed_youtube_nocookie: true,
            deploy_env: "test".into(),
            email_enabled: false,
            email_from_address: String::new(),
            smtp_host: "localhost".into(),
            smtp_port: 1025,
            smtp_username: None,
            smtp_password: None,
            smtp_starttls: false,
            email_daily_cap: 25,
            email_digest_enabled: false,
            email_digest_hour_utc: 15,
            // Retention worker stays disabled in tests — the unit
            // tests for the date-filter logic don't need the
            // scheduler running, and integration tests that wanted
            // to exercise it would need to inject a fake clock.
            security_audit_retention_enabled: false,
            security_audit_retention_days: 90,
            security_audit_retention_hour_utc: 4,
            // Tight MFA failure cap so the integration test only
            // needs a handful of wrong-code submissions to trip it.
            mfa_challenge_max_failures: 3,
            // Tight SCIM rate-limit so the integration test trips
            // it after a handful of requests rather than 100+.
            scim_request_rate_limit_per_minute: 5,
            // M-E7 rate-limit coverage: tight on tests so loops
            // trip quickly; high on dev_login because every
            // test-side `create_user` call hits /auth/dev-login,
            // so the shared Redis bucket can accumulate fast
            // across parallel integration test binaries.
            rate_limit_comments_per_min: 5,
            rate_limit_content_write_per_min: 5,
            rate_limit_user_search_per_min: 3,
            rate_limit_ws_upgrade_per_min: 5,
            rate_limit_ws_messages_per_min: 6000,
            rate_limit_dev_login_per_min: 1000,
            rate_limit_rum_per_min: 100,
            rate_limit_client_telemetry_per_min: 100,
            rate_limit_import_per_min: 50,
            rate_limit_bulk_export_per_min: 50,
            rate_limit_bulk_op_per_min: 100,
            // Trash worker stays disabled in tests; the unit
            // tests for the cutoff arithmetic don't need the
            // scheduler running, and integration tests that
            // exercise it call sweep() directly with controlled
            // cutoffs.
            trash_cleanup_enabled: false,
            trash_retention_days: 30,
            trash_cleanup_hour_utc: 3,
            trash_cleanup_dry_run: false,
            // Worker subsystem defaults — integration tests that
            // exercise the queue use a per-test stream suffix so
            // concurrent runs don't collide on this shared name.
            job_stream_name: format!("ogrenotes-jobs-test-{prefix}"),
            worker_concurrency: 1,
            // Phase 2a — default to log for tests so any
            // schema-violating fixture doesn't break existing suites.
            // Tests that need `reject` semantics can build their own
            // AppState with the string flipped.
            liveapp_strict_validation: "log".to_string(),
            liveapp_gate_exempt_doc_ids: std::collections::HashSet::new(),
            liveapp_gate_walk_scope: "full".to_string(),
            // Tight caps so integration tests can exercise the limit
            // without provisioning hundreds of fake users / connections.
            max_members_per_doc: 3,
            max_members_per_folder: 3,
            max_ws_connections_per_doc: 3,
            max_ws_connections_per_user_per_doc: 2,
            // Tight rate limits so test loops can hit them in a few
            // requests instead of hundreds per minute.
            //
            // `rate_limit_sharing_per_min` is set HIGHER than
            // `max_members_per_folder` (3) so the existing
            // membership-cap test (test_sharing.rs::
            // folder_member_cap_returns_429_with_retry_after) hits
            // the member cap on its 4th add — not the rate limit,
            // which would shadow the cap-specific 429 message.
            // The new test_rate_limits suite uses update mutations
            // (not bound by member cap) to exercise the rate limit.
            rate_limit_auth_login_per_min: 3,
            rate_limit_auth_refresh_per_min: 3,
            rate_limit_search_per_min: 3,
            rate_limit_sharing_per_min: 10,
            // Low cap so the dedicated admin rate-limit test in
            // test_rate_limits.rs can fire after a short loop;
            // existing admin tests do at most a promote+demote pair
            // per test, well under 10.
            rate_limit_admin_mut_per_min: 10,
            // Low so the rate-limit regression test in
            // test_rate_limits.rs can fire 429 after a short loop.
            // Existing ACS failure-path tests in test_saml_acs.rs
            // each do a single POST, well under 3. Production default
            // is 10/min/IP via env (RATE_LIMIT_SAML_ACS_PER_MIN).
            rate_limit_saml_acs_per_min: 3,
            // Tests run against modest update volumes — 32 MiB
            // matches the production default and keeps the
            // #91-harness test (`test_get_content_survives_many_
            // inline_updates_2mb`, exercises 2 MiB) green.
            max_pending_updates_bytes: 32 * 1024 * 1024,
        };

        let search_index = ogrenotes_search::SearchIndex::open_in_memory()
            .expect("failed to create in-memory search index");

        // Build a per-test JobQueue against the shared Redis. The
        // stream name was already configured per-test in the AppConfig
        // above so concurrent test binaries don't collide.
        let job_queue_config = fred::types::RedisConfig::from_url("redis://127.0.0.1:6379")
            .expect("parse REDIS_URL");
        let job_queue_client = RedisClient::new(job_queue_config, None, None, None);
        job_queue_client.connect();
        job_queue_client
            .wait_for_connect()
            .await
            .expect("job queue Redis connect failed");
        let job_queue = ogrenotes_worker::JobQueue::new(
            Arc::new(job_queue_client),
            config.job_stream_name.clone(),
        )
        .await
        .expect("job queue init");
        let job_producer: Arc<dyn ogrenotes_worker::JobProducer> = Arc::new(job_queue);

        let state = AppState::new(
            config,
            dynamo,
            s3,
            redis,
            search_index,
            None,
            claude,
            Some(job_producer),
        );
        // Apply the same security-headers stack as production so
        // tests can assert against the live policy (#35). dev_mode
        // is true here so Strict-Transport-Security is omitted.
        let router = routes::apply_security_headers(
            routes::api_router().with_state(state.clone()),
            state.config.dev_mode,
            &routes::security_csp(),
        );

        // Per-test default `X-Forwarded-For` so each TestApp has its
        // own per-IP rate-limit bucket in the shared Redis. Without
        // this, every test that hit `/auth/refresh`, `/auth/login/*`,
        // or any other per-IP-limited route shared the `"unknown"`
        // bucket and the 4th cumulative test-process call within a
        // minute 429'd. Tests that need to exercise a *specific* IP
        // (e.g. `test_rate_limits`) still pass an explicit
        // `X-Forwarded-For` via `raw_request` / their own builders,
        // and the helpers below only inject when none is present.
        //
        // We use the full random `prefix` (8-char nanoid) as the
        // identifier rather than hashing it into a documentation-range
        // IPv4 octet. The rate-limit middleware treats this header as
        // an opaque string for bucket keying (`ip_identifier` does no
        // IP parsing — see `crates/api/src/middleware/rate_limit.rs`),
        // so collision-free uniqueness is what matters. An earlier
        // version mapped to `198.51.100.{prefix-hash % 254}`, but
        // `cargo test` runs integration tests in parallel by default,
        // and 30+ concurrent tests against 254 buckets gave an ~85%
        // birthday-collision rate — colliding tests shared the
        // auth_refresh cap and serially-passing test runs would
        // intermittently 429 in CI (e.g. run 25359757901 on da27888,
        // `refresh_token_reuse_revokes_all_sessions` got 429 instead
        // of 401 on its 3rd refresh because a parallel
        // `test_refresh_*` was burning the same bucket). Using the
        // unique prefix removes the collision surface entirely.
        let default_xff = format!("test-{prefix}");

        Self {
            router,
            state,
            table_name,
            bucket,
            default_xff,
            dynamo_client,
            s3_client,
        }
    }

    /// Raw DynamoDB client for tests that need to bypass the repo layer
    /// (e.g. inserting padding items to force scan pagination).
    pub fn dynamo_client(&self) -> &aws_sdk_dynamodb::Client {
        &self.dynamo_client
    }

    /// Rebuild the router with `liveapp_strict_validation` swapped to
    /// `mode`. Phase-2a integration tests use this to exercise the
    /// `reject` branch without spinning up a whole second TestApp.
    /// Config values other than the mode string are preserved by
    /// cloning through the existing `AppConfig`. Router is rebuilt
    /// because it captures `state.clone()` at construction time.
    #[allow(dead_code)]
    pub fn set_liveapp_validation_mode(&mut self, mode: &str) {
        use ogrenotes_common::config::AppConfig;
        let old: &AppConfig = &self.state.config;
        let new_config = AppConfig {
            liveapp_strict_validation: mode.to_string(),
            ..old.clone()
        };
        self.state.config = std::sync::Arc::new(new_config);
        self.router = ogrenotes_api::routes::apply_security_headers(
            ogrenotes_api::routes::api_router().with_state(self.state.clone()),
            self.state.config.dev_mode,
            &ogrenotes_api::routes::security_csp(),
        );
    }

    /// Populate the LiveApp gate exemption list and rebuild the
    /// router so the gate skips the specified docs. Test-only
    /// counterpart to the `LIVEAPP_GATE_EXEMPT_DOC_IDS` env var.
    #[allow(dead_code)]
    pub fn set_liveapp_gate_exempt_doc_ids(&mut self, doc_ids: &[&str]) {
        use ogrenotes_common::config::AppConfig;
        let old: &AppConfig = &self.state.config;
        let new_config = AppConfig {
            liveapp_gate_exempt_doc_ids: doc_ids.iter().map(|s| s.to_string()).collect(),
            ..old.clone()
        };
        self.state.config = std::sync::Arc::new(new_config);
        self.router = ogrenotes_api::routes::apply_security_headers(
            ogrenotes_api::routes::api_router().with_state(self.state.clone()),
            self.state.config.dev_mode,
            &ogrenotes_api::routes::security_csp(),
        );
    }

    /// Raw S3 client for tests that need to stage objects directly (e.g.
    /// seeding snapshot blobs for the snapshot-backfill migration).
    #[allow(dead_code)]
    pub fn s3_client(&self) -> &aws_sdk_s3::Client {
        &self.s3_client
    }

    // ─── Helper: create a user via dev-login ────────────────────

    pub async fn create_user(&self, email: &str) -> (String, String) {
        let body = serde_json::json!({ "email": email });
        let (status, json) = self.json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body)).await;
        assert_eq!(status, 200, "dev-login failed: {json}");
        let user_id = json["userId"].as_str().unwrap().to_string();
        let token = json["accessToken"].as_str().unwrap().to_string();
        (user_id, token)
    }

    /// Create a user with an explicit display name. Without this helper,
    /// `create_user` leaves the name at the dev-login default ("Dev User"),
    /// which makes name-dependent assertions (e.g., `/invite` announcements)
    /// ambiguous when the test creates more than one user.
    #[allow(dead_code)]
    pub async fn create_user_with_name(&self, email: &str, name: &str) -> (String, String) {
        let body = serde_json::json!({ "email": email, "name": name });
        let (status, json) = self.json_request(Method::POST, "/api/v1/auth/dev-login", None, Some(body)).await;
        assert_eq!(status, 200, "dev-login failed: {json}");
        let user_id = json["userId"].as_str().unwrap().to_string();
        let token = json["accessToken"].as_str().unwrap().to_string();
        (user_id, token)
    }

    /// Create a user and return just the token (convenience).
    pub async fn create_user_token(&self, email: &str) -> String {
        self.create_user(email).await.1
    }

    // ─── Helper: create a document ──────────────────────────────

    pub async fn create_doc(&self, token: &str, title: &str, folder_id: Option<&str>) -> String {
        let mut body = serde_json::json!({ "title": title });
        if let Some(fid) = folder_id {
            body["folderId"] = serde_json::Value::String(fid.to_string());
        }
        let (status, json) = self.json_request(Method::POST, "/api/v1/documents", Some(token), Some(body)).await;
        assert_eq!(status, 201, "create_doc failed: {json}");
        json["id"].as_str().unwrap().to_string()
    }

    // ─── Helper: create a folder ────────────────────────────────

    pub async fn create_folder(&self, token: &str, title: &str, parent_id: Option<&str>) -> String {
        let mut body = serde_json::json!({ "title": title });
        if let Some(pid) = parent_id {
            body["parentId"] = serde_json::Value::String(pid.to_string());
        }
        let (status, json) = self.json_request(Method::POST, "/api/v1/folders", Some(token), Some(body)).await;
        assert_eq!(status, 201, "create_folder failed: {json}");
        json["id"].as_str().unwrap().to_string()
    }

    // ─── Generic request helpers ────────────────────────────────

    /// Send a JSON request and return (status_code, response_json).
    pub async fn json_request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (u16, serde_json::Value) {
        let body_str = body.map(|b| serde_json::to_string(&b).unwrap()).unwrap_or_default();
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("X-Forwarded-For", &self.default_xff);

        if !body_str.is_empty() {
            builder = builder.header("Content-Type", "application/json");
        }

        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }

        let req = builder.body(Body::from(body_str)).unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();

        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::String(
                String::from_utf8_lossy(&bytes).to_string(),
            ))
        };

        (status, json)
    }

    /// Send a binary request and return (status_code, response_bytes).
    pub async fn bytes_request(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Vec<u8>,
        content_type: &str,
    ) -> (u16, Vec<u8>) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", content_type)
            .header("X-Forwarded-For", &self.default_xff);

        if let Some(t) = token {
            builder = builder.header("Authorization", format!("Bearer {t}"));
        }

        let req = builder.body(Body::from(body)).unwrap();
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();

        (status, bytes)
    }

    /// Send a raw request (for WS upgrade tests etc.) and return (status, body_bytes).
    pub async fn raw_request(
        &self,
        mut req: Request<Body>,
    ) -> (u16, Vec<u8>) {
        // Default the per-test client IP if the caller didn't set
        // one — tests that *want* a specific IP (rate-limit suite)
        // pass `X-Forwarded-For` themselves and override.
        if !req.headers().contains_key("x-forwarded-for") {
            req.headers_mut().insert(
                "x-forwarded-for",
                self.default_xff.parse().unwrap(),
            );
        }
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, bytes)
    }

    // ─── Cleanup ────────────────────────────────────────────────

    pub async fn cleanup(&self) {
        // Delete DynamoDB table
        let _ = self.dynamo_client
            .delete_table()
            .table_name(&self.table_name)
            .send()
            .await;

        // Delete all objects in S3 bucket, then the bucket
        if let Ok(objects) = self.s3_client
            .list_objects_v2()
            .bucket(&self.bucket)
            .send()
            .await
        {
            for obj in objects.contents() {
                if let Some(key) = obj.key() {
                    let _ = self.s3_client
                        .delete_object()
                        .bucket(&self.bucket)
                        .key(key)
                        .send()
                        .await;
                }
            }
        }
        let _ = self.s3_client
            .delete_bucket()
            .bucket(&self.bucket)
            .send()
            .await;
    }
}

// ─── DynamoDB table creation (mirrors setup_dev.rs) ─────────────

async fn create_table(client: &aws_sdk_dynamodb::Client, table_name: &str) {
    fn attr_def(name: &str, attr_type: ScalarAttributeType) -> AttributeDefinition {
        AttributeDefinition::builder()
            .attribute_name(name)
            .attribute_type(attr_type)
            .build()
            .unwrap()
    }

    let _ = client
        .create_table()
        .table_name(table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .key_schema(KeySchemaElement::builder().attribute_name("PK").key_type(KeyType::Hash).build().unwrap())
        .key_schema(KeySchemaElement::builder().attribute_name("SK").key_type(KeyType::Range).build().unwrap())
        .attribute_definitions(attr_def("PK", ScalarAttributeType::S))
        .attribute_definitions(attr_def("SK", ScalarAttributeType::S))
        .attribute_definitions(attr_def("owner_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("updated_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("parent_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("title", ScalarAttributeType::S))
        .attribute_definitions(attr_def("doc_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("workspace_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("user_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("created_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("external_id_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("is_deleted_gsi", ScalarAttributeType::S))
        .attribute_definitions(attr_def("deleted_at", ScalarAttributeType::N))
        .attribute_definitions(attr_def("actor_id_gsi", ScalarAttributeType::S))
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI1-owner-updated")
                .key_schema(KeySchemaElement::builder().attribute_name("owner_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("updated_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI2-parent-title")
                .key_schema(KeySchemaElement::builder().attribute_name("parent_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("title").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI3-workspace-updated")
                .key_schema(KeySchemaElement::builder().attribute_name("workspace_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("updated_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI4-user-created")
                .key_schema(KeySchemaElement::builder().attribute_name("user_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("created_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI5-docid-updated")
                .key_schema(KeySchemaElement::builder().attribute_name("doc_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("updated_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        // GSI6: sparse external_id index. Hash-only; SCIM filters are
        // equality matches.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI6-external-id")
                .key_schema(KeySchemaElement::builder().attribute_name("external_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        // GSI7: sparse soft-deleted index. Hash on a constant
        // "deleted" partition + range on deleted_at. The M-E7
        // trash-cleanup worker scans this for purge-eligible docs.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI7-deleted-at")
                .key_schema(KeySchemaElement::builder().attribute_name("is_deleted_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("deleted_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        // GSI8: actor_id -> created_at (#49). Actor-centric forensic
        // index over AdminAudit rows.
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("GSI8-actor-created")
                .key_schema(KeySchemaElement::builder().attribute_name("actor_id_gsi").key_type(KeyType::Hash).build().unwrap())
                .key_schema(KeySchemaElement::builder().attribute_name("created_at").key_type(KeyType::Range).build().unwrap())
                .projection(Projection::builder().projection_type(ProjectionType::All).build())
                .build().unwrap(),
        )
        .send()
        .await
        .expect("Failed to create test DynamoDB table");
}
