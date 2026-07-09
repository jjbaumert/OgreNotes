// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::env;
use std::fmt;

/// Application configuration loaded from environment variables.
#[derive(Clone)]
pub struct AppConfig {
    // AWS
    pub aws_region: String,
    pub dynamodb_table_prefix: String,
    pub s3_bucket: String,

    // Redis
    pub redis_url: String,

    // Auth — GitHub OAuth
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
    pub oauth_redirect_uri: String,
    pub jwt_secret: String,

    // Auth — Google OAuth (optional)
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,

    // Server
    pub api_port: u16,
    pub frontend_origin: String,

    // Search
    pub search_index_path: String,

    // Embeddings / Vector search (optional — disabled when qdrant_url is None)
    pub qdrant_url: Option<String>,
    pub embedding_model_id: String,
    pub embedding_dimensions: u32,

    // AI / Agentic (optional — ask endpoint returns 503 when not set)
    pub anthropic_api_key: Option<String>,
    pub anthropic_model: String,

    // Admin
    /// Emails that are automatically granted admin on login (comma-separated).
    pub admin_emails: Vec<String>,

    /// Enable dev-only features (dev-login endpoint). MUST be false in production.
    pub dev_mode: bool,

    /// Rewrite YouTube embeds to `youtube-nocookie.com` (privacy-enhanced
    /// mode — no cookies set until the viewer actually plays). Default on;
    /// set `EMBED_YOUTUBE_NOCOOKIE=false` to use the standard `youtube.com`
    /// host instead.
    pub embed_youtube_nocookie: bool,

    /// Deployment environment name (e.g. "dev", "staging", "prod"). Used as
    /// the `Environment` dimension on all emitted CloudWatch metrics.
    pub deploy_env: String,

    // Email (M4) — disabled by default; flip EMAIL_ENABLED=true to send.
    /// Master switch. When false the `EmailService` short-circuits to
    /// `SkippedDisabled` before touching any user data.
    pub email_enabled: bool,
    /// `From:` header on every outgoing email. Required whenever
    /// `email_enabled` is true; ignored otherwise.
    pub email_from_address: String,
    /// SMTP host — `localhost` for MailHog, `email-smtp.<region>.amazonaws.com` for SES.
    pub smtp_host: String,
    pub smtp_port: u16,
    /// SMTP username. `None` for MailHog (no auth); required for SES.
    pub smtp_username: Option<String>,
    /// SMTP password. Redacted in `Debug`.
    pub smtp_password: Option<String>,
    /// Upgrade the connection with STARTTLS. `false` for MailHog, `true` for SES.
    pub smtp_starttls: bool,
    /// Per-user per-day send cap. Default 25 per the design doc.
    pub email_daily_cap: u32,

    // Email digest (M4.1)
    /// Enable the daily digest scheduler. When false, the hourly tick
    /// short-circuits before scanning users.
    pub email_digest_enabled: bool,
    /// UTC hour (0-23) at which to fire digest emails. Single global
    /// value for MVP; per-user timezones are a follow-up.
    pub email_digest_hour_utc: u8,

    // SecurityAudit retention (Phase 4 M-E6 piece D)
    /// Enable the daily security-audit retention worker. When false,
    /// the hourly tick short-circuits before walking users. AdminAudit
    /// is unaffected — it's retained permanently per the M-E6 spec.
    pub security_audit_retention_enabled: bool,
    /// Age threshold for SecurityAudit row deletion, in days. A row
    /// with `created_at < now - retention_days*86400e6` is eligible.
    /// 90 days mirrors common forensic-compliance windows; tune via
    /// SECURITY_AUDIT_RETENTION_DAYS without a code change.
    pub security_audit_retention_days: u32,
    /// UTC hour (0-23) at which the retention pass runs. Defaults to
    /// 04 to land between the digest (15) and the (future) trash
    /// worker (03), keeping nightly batches sequential rather than
    /// stacked.
    pub security_audit_retention_hour_utc: u8,

    // Resource caps (#34) — defend against DoS via member sprawl or
    // WebSocket connection flooding. All four are deliberately small
    // for the test stack and tunable without a code change.
    /// Maximum members on a single document (excluding the owner).
    /// `add_doc_member` rejects with 429 once this many already exist.
    pub max_members_per_doc: usize,
    /// Maximum members on a single folder (excluding the owner).
    /// `add_member` (folder route) rejects with 429 once this many already exist.
    pub max_members_per_folder: usize,
    /// Maximum concurrent WebSocket connections to one document, summed
    /// across all users. The `ws_upgrade` handler rejects with 429 once
    /// the room hits this many clients.
    pub max_ws_connections_per_doc: usize,
    /// Maximum concurrent WebSocket connections from a single user to a
    /// single document. Defends against a compromised user's tabs
    /// flooding the room.
    pub max_ws_connections_per_user_per_doc: usize,

    // Rate limits (#36) — Redis-backed fixed-window counters.
    /// Per-IP cap on /auth/login {OAuth init} per minute.
    pub rate_limit_auth_login_per_min: u64,
    /// Per-IP cap on /auth/refresh per minute.
    pub rate_limit_auth_refresh_per_min: u64,
    /// Per-user cap on /search per minute.
    pub rate_limit_search_per_min: u64,
    /// Per-user cap on /sharing mutations (add/remove/update member)
    /// per minute. Read endpoints (list_members) are not rate-limited.
    pub rate_limit_sharing_per_min: u64,
    /// Per-user cap on /admin mutation routes (disable/enable/promote/
    /// demote/ask-enabled) per minute. Phase 4 M-E2 addition — the
    /// admin surface previously had no rate limit at all, which is the
    /// outstanding sliver of #36. Read-only admin routes (list, get,
    /// metrics, audit) are not throttled.
    pub rate_limit_admin_mut_per_min: u64,

    /// Per-IP cap on POST /auth/saml/acs per minute. The ACS endpoint
    /// is unauthenticated by necessity (the IdP POSTs the assertion)
    /// and triggers a DDB read + XML parse + Redis SET NX EX write +
    /// DDB user lookup per call — an attractive DoS target without a
    /// cap. Default 10/min/IP mirrors `/auth/dev-login` and matches
    /// the cadence a legitimate IdP retries assertions at (#83).
    pub rate_limit_saml_acs_per_min: u64,

    /// Per-request cap on the total `update_bytes` `get_pending_updates`
    /// will materialize for a single document (#91). Crossing the cap
    /// surfaces as `RepoError::TooLarge` → `503 Service Unavailable`,
    /// failing one over-budget doc instead of OOM-ing the whole task.
    /// Default 32 MiB — comfortably above the 2 MiB that the gap-#3
    /// regression test exercises, and well under the Fargate task
    /// memory ceiling that one 49 MiB doc OOM-killed in the
    /// triggering incident. Tunable via env
    /// `MAX_PENDING_UPDATES_BYTES`; ops can raise it on a memory-
    /// boosted deploy.
    pub max_pending_updates_bytes: usize,

    // M-E8 gap-002 — MFA challenge / recovery failure cap.
    /// Maximum wrong-code submissions per MFA handle before the
    /// handle is invalidated and further attempts 429. Defends
    /// against the leaked-handle-then-guess attack the pre-merge
    /// security audit called out. Default 5; tunable via
    /// MFA_CHALLENGE_MAX_FAILURES without a code change. Applies
    /// to both `/auth/mfa/challenge` and `/auth/mfa/recovery` and
    /// shares one counter per handle so the attacker can't cycle
    /// between the two endpoints to double their budget.
    pub mfa_challenge_max_failures: u32,

    // M-E8 gap-005 — SCIM endpoint rate limit.
    /// Per-workspace cap on SCIM requests per minute. The check runs
    /// pre-bcrypt inside `verify_scim_request`, so a flood of
    /// well-formed-but-wrong bearer tokens against a known
    /// workspace_id can't drive unbounded bcrypt CPU work against
    /// the API. Failed auth counts against the budget (the cap is
    /// the cap; an attacker can't get bcrypt-for-free by intentionally
    /// failing). Default 100/min — generous for any real IdP
    /// (Okta's typical reconcile rate is ~1 req/sec at most).
    pub scim_request_rate_limit_per_minute: u64,

    // M-E7 rate-limit coverage gaps (item 10).
    /// Per-user cap on comment writes (`create_thread`, `add_message`)
    /// per minute. Defends against compromised-token / abusive
    /// auto-script comment floods.
    pub rate_limit_comments_per_min: u64,
    /// Per-user cap on REST content saves (`PUT /documents/:id/content`)
    /// per minute. Real autosave is debounced; legitimate traffic is
    /// well under this. The WS edit path has its own size+cadence
    /// limits and isn't governed here.
    pub rate_limit_content_write_per_min: u64,
    /// Per-user cap on `/users/search` requests per minute. The
    /// picker debounces client-side at 250 ms but a scripted
    /// caller walks past that; this bounds the per-hit
    /// workspace-scope GSI4 fanout and prevents fast directory
    /// enumeration within a workspace. gap-002 from the
    /// post-hardening security audit.
    pub rate_limit_user_search_per_min: u64,
    /// Per-user cap on WS upgrade attempts per minute. Connection
    /// open is rare in steady state; this bounds reconnect-storm
    /// damage from a compromised token.
    pub rate_limit_ws_upgrade_per_min: u64,
    /// Per-IP cap on `/auth/dev-login` requests per minute. Replaces
    /// the M-E2-era bespoke DashMap limiter so dev-login uses the
    /// same Redis-backed module as every other gate. Tagged with the
    /// `cfg(feature="dev-login")` flag the endpoint itself lives
    /// behind.
    pub rate_limit_dev_login_per_min: u64,

    // M-P9 piece C — frontend RUM ingest rate limit.
    /// Per-user cap on `POST /api/v1/metrics/rum` requests per
    /// minute. Frontend sampling caps each session at one beacon per
    /// vital, so legitimate traffic is well below this; the cap
    /// defends the histogram recorder from a compromised-token
    /// poisoning flood. Default 60/min mirrors search.
    pub rate_limit_rum_per_min: u64,

    /// Observability Phase 1 — `POST /api/v1/client-telemetry` rate
    /// limit. The frontend batches counter deltas every ~10 s, so a
    /// healthy client sends at most ~6 req/min; this cap defends
    /// the EMF projection from a compromised-token flood. Same
    /// posture as `rate_limit_rum_per_min` — both ingest client-
    /// originated metric data into the same recorder.
    pub rate_limit_client_telemetry_per_min: u64,

    // M-P5 piece A — document import rate limit.
    /// Per-user cap on `POST /api/v1/documents/import` per minute.
    /// Each import builds and persists a fresh doc; a 1 MB body
    /// + Markdown/HTML parse is the cost ceiling. 10/min is
    /// generous for hand-driven imports and tight enough to
    /// blunt drive-by abuse of an authenticated endpoint.
    pub rate_limit_import_per_min: u64,

    // M-P5 piece C — bulk export rate limit.
    /// Per-user cap on `POST /api/v1/documents/bulk/export` per
    /// minute. Each call serves up to 100 docs in-memory; a tight
    /// cap defends against build-the-archive-then-throw-away
    /// abuse. 5/min covers occasional power-user exports without
    /// enabling repeated 100-doc bursts.
    pub rate_limit_bulk_export_per_min: u64,

    // M-P7 — bulk operations rate limit.
    /// Per-user cap on `POST /api/v1/documents/bulk/{delete,
    /// restore, move, share}` per minute. Each call walks up to
    /// 100 docs with one DDB write per affected doc, so 20/min
    /// caps the worst case at 2 000 writes/min from any single
    /// user without blocking legitimate batch UX.
    pub rate_limit_bulk_op_per_min: u64,

    // M-E7 trash-cleanup worker (item 9).
    /// Enable the daily trash-purge scheduler. When false, the
    /// hourly tick short-circuits before querying the deleted_at
    /// GSI. Opt-in by default (same posture as the digest +
    /// audit-retention schedulers) — prod stacks turn it on
    /// explicitly via SECURITY_AUDIT_RETENTION_ENABLED's sibling.
    pub trash_cleanup_enabled: bool,
    /// Age threshold for hard-purging a soft-deleted document, in
    /// days. A doc with `deleted_at < now - retention_days*86400e6`
    /// is eligible. 30 days mirrors common product behavior; tune
    /// via TRASH_RETENTION_DAYS without a code change.
    pub trash_retention_days: u32,
    /// UTC hour (0-23) at which the trash sweep runs. Defaults to
    /// 03 to land before the audit-retention worker (04) and the
    /// digest scheduler (15) — sequencing nightly batches keeps
    /// peak DDB write rate predictable.
    pub trash_cleanup_hour_utc: u8,
    /// When true, the worker logs which docs WOULD be purged but
    /// skips the destructive operations (S3 delete, search-index
    /// delete, hard_delete, audit write). First-rollout safety
    /// valve so an operator can dry-run a config change before
    /// committing to deletions.
    pub trash_cleanup_dry_run: bool,

    // ─── LiveApp CRDT-write pre-apply gate ────────────────────────
    /// Rollout state for the LiveApp attribute pre-apply validator.
    /// Values: "off" | "log" | "reject". Default "reject" as of
    /// Phase 2b — hard-refuse interactive writes whose LiveApp
    /// attrs fail `LiveAppBlock::validate_attrs` (structural
    /// error) or diverge from the canonical form (silent-clamp).
    /// The offending client receives a `MessageType::Error` frame
    /// with `liveapp-rejected:<diagnostic>` — see #163 for the
    /// state-recovery follow-up.
    /// Operators can set LIVEAPP_STRICT_VALIDATION=log or off via
    /// env to soften the gate without redeploying if a legitimate
    /// pattern needs a validator fix.
    /// Not applied on `room::apply_update` raw path (compaction /
    /// snapshot restore replay pre-fix bytes).
    pub liveapp_strict_validation: String,

    /// Doc IDs exempted from the LiveApp pre-apply gate. When a
    /// doc's id is in this set, the gate is skipped entirely
    /// (equivalent to `LIVEAPP_STRICT_VALIDATION=off` scoped to
    /// that doc).
    ///
    /// Rationale (gap-001 from the post-hardening audit): the gate
    /// walks the whole doc on every write and rejects on the first
    /// LiveApp violation anywhere. A doc that accumulated an
    /// invalid attribute — via the raw compaction/snapshot-restore
    /// path, or from a pre-hardening window — permanently blocks
    /// all further interactive writes for every collaborator until
    /// someone crafts a REST full-state replacement that satisfies
    /// the validator. This env var is the operator's release-valve:
    /// set `LIVEAPP_GATE_EXEMPT_DOC_IDS=a,b,c` to unblock those
    /// docs while the underlying invalid attr is repaired or the
    /// changed-refs walk lands.
    ///
    /// Set at process start; changes require a task restart to
    /// take effect. Emissions of `liveapp.gate_exempted_total`
    /// (tagged with `doc_id`) let operators verify the exemption
    /// is firing and which doc benefited.
    pub liveapp_gate_exempt_doc_ids: std::collections::HashSet<String>,

    /// Scope of the pre-apply gate walk. Values:
    /// `"full"` | `"changed"` | `"canary"`. Default `"canary"`
    /// during the Phase 3 rollout window.
    ///
    /// - `full` — walk the whole doc on every write. Pre-Phase-3
    ///   behavior; keeps the "one bad attr blocks everything"
    ///   symptom.
    /// - `changed` — walk only elements the transaction touched
    ///   (via yrs observe_deep). The gap-001 fix — cheap and
    ///   scoped, so an unrelated write on a doc with a legacy
    ///   invalid card still passes.
    /// - `canary` — run BOTH walks and emit
    ///   `liveapp.gate_walk_canary_mismatch_total` on
    ///   disagreement. Return Full's answer (safe default).
    ///   Use during rollout to prove equivalence before flipping
    ///   default to `changed`.
    pub liveapp_gate_walk_scope: String,

    // ─── Async worker (Phase 6 M-6.4) ─────────────────────────────
    /// Redis stream key the worker subsystem reads from. Producers
    /// write here on POST /jobs; consumers XREADGROUP from here in
    /// worker mode. One stream per environment is plenty; multiple
    /// streams would only matter if we needed per-job-kind priority
    /// (not a v1 concern).
    pub job_stream_name: String,
    /// Number of consumer tasks the worker-mode entrypoint spawns
    /// per ECS task. Each consumer joins the same consumer group;
    /// XREADGROUP distributes work across them. Scaling out is
    /// orthogonal — add more ECS tasks. Default 4 fits the small
    /// task we ship with (~1 GB / 0.5 vCPU); tune via env on bigger
    /// instances.
    pub worker_concurrency: u32,
}

/// Manual Debug implementation that redacts secrets.
impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("aws_region", &self.aws_region)
            .field("dynamodb_table_prefix", &self.dynamodb_table_prefix)
            .field("s3_bucket", &self.s3_bucket)
            .field("redis_url", &"[redacted]")
            .field("oauth_client_id", &self.oauth_client_id)
            .field("oauth_client_secret", &"[redacted]")
            .field("oauth_redirect_uri", &self.oauth_redirect_uri)
            .field("jwt_secret", &"[redacted]")
            .field("google_client_id", &self.google_client_id)
            .field("google_client_secret", &"[redacted]")
            .field("api_port", &self.api_port)
            .field("frontend_origin", &self.frontend_origin)
            .field("search_index_path", &self.search_index_path)
            .field("qdrant_url", &self.qdrant_url)
            .field("embedding_model_id", &self.embedding_model_id)
            .field("embedding_dimensions", &self.embedding_dimensions)
            .field("anthropic_api_key", &"[redacted]")
            .field("anthropic_model", &self.anthropic_model)
            .field("admin_emails", &self.admin_emails)
            .field("email_enabled", &self.email_enabled)
            .field("email_from_address", &self.email_from_address)
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field("smtp_password", &"[redacted]")
            .field("smtp_starttls", &self.smtp_starttls)
            .field("email_daily_cap", &self.email_daily_cap)
            .field("security_audit_retention_enabled", &self.security_audit_retention_enabled)
            .field("security_audit_retention_days", &self.security_audit_retention_days)
            .field("security_audit_retention_hour_utc", &self.security_audit_retention_hour_utc)
            .field("trash_cleanup_enabled", &self.trash_cleanup_enabled)
            .field("trash_retention_days", &self.trash_retention_days)
            .field("trash_cleanup_hour_utc", &self.trash_cleanup_hour_utc)
            .field("trash_cleanup_dry_run", &self.trash_cleanup_dry_run)
            .field("job_stream_name", &self.job_stream_name)
            .field("worker_concurrency", &self.worker_concurrency)
            .field("email_digest_enabled", &self.email_digest_enabled)
            .field("email_digest_hour_utc", &self.email_digest_hour_utc)
            .finish()
    }
}

impl AppConfig {
    /// Load configuration from environment variables.
    /// Panics if required variables are missing or invalid.
    pub fn from_env() -> Self {
        let api_port_str = env_or("API_PORT", "3000");
        let api_port: u16 = api_port_str
            .parse()
            .unwrap_or_else(|_| panic!("API_PORT must be a valid port number (0-65535), got: {api_port_str}"));

        let jwt_secret = env_required("JWT_SECRET");
        validate_jwt_secret(&jwt_secret);

        let dev_mode = env_or("DEV_MODE", "false") == "true";
        let embed_youtube_nocookie = env_or("EMBED_YOUTUBE_NOCOOKIE", "true") == "true";
        let frontend_origin = env_or("FRONTEND_ORIGIN", "http://localhost:8080");
        validate_frontend_origin(&frontend_origin, dev_mode);

        let email_enabled = env_or("EMAIL_ENABLED", "false") == "true";
        let smtp_username = env::var("SMTP_USERNAME").ok();
        let smtp_password = env::var("SMTP_PASSWORD").ok();
        validate_smtp_credentials(email_enabled, &smtp_username, &smtp_password);

        Self {
            aws_region: env_or("AWS_REGION", "us-east-1"),
            dynamodb_table_prefix: env_required("DYNAMODB_TABLE_PREFIX"),
            s3_bucket: env_required("S3_BUCKET"),
            redis_url: env_or("REDIS_URL", "redis://localhost:6379"),
            oauth_client_id: env_required("OAUTH_CLIENT_ID"),
            oauth_client_secret: env_required("OAUTH_CLIENT_SECRET"),
            oauth_redirect_uri: env_required("OAUTH_REDIRECT_URI"),
            jwt_secret,
            google_client_id: env::var("GOOGLE_CLIENT_ID").ok(),
            google_client_secret: env::var("GOOGLE_CLIENT_SECRET").ok(),
            api_port,
            frontend_origin,
            search_index_path: env_or("SEARCH_INDEX_PATH", "/tmp/ogrenotes-search-index"),
            // Phase 6 M-6.1 piece A: treat an empty QDRANT_URL the same as
            // an absent one. The deploy scripts set QDRANT_URL="" on legacy
            // stacks where the Cloud Map namespace doesn't exist yet, and
            // we want config.qdrant_url to read as None in that case so the
            // embedding-pipeline init in main.rs short-circuits cleanly.
            qdrant_url: env::var("QDRANT_URL")
                .ok()
                .filter(|s| !s.is_empty()),
            embedding_model_id: env_or("EMBEDDING_MODEL_ID", "amazon.titan-embed-text-v2:0"),
            embedding_dimensions: env_or("EMBEDDING_DIMENSIONS", "1024")
                .parse()
                .expect("EMBEDDING_DIMENSIONS must be a valid u32"),
            // Same empty-vs-absent treatment for the Anthropic key; the
            // deploy script may set it to "" on a stack that hasn't
            // wired the SSM secret yet (piece C).
            anthropic_api_key: env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|s| !s.is_empty()),
            anthropic_model: env_or("ANTHROPIC_MODEL", "claude-sonnet-4-6"),
            admin_emails: env::var("ADMIN_EMAILS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            dev_mode,
            embed_youtube_nocookie,
            deploy_env: env_or("DEPLOY_ENV", "dev"),
            email_enabled,
            email_from_address: env_or("EMAIL_FROM_ADDRESS", ""),
            smtp_host: env_or("SMTP_HOST", "localhost"),
            smtp_port: env_or("SMTP_PORT", "1025")
                .parse()
                .unwrap_or_else(|_| panic!("SMTP_PORT must be a valid port number")),
            smtp_username,
            smtp_password,
            smtp_starttls: env_or("SMTP_STARTTLS", "false") == "true",
            email_daily_cap: env_or("EMAIL_DAILY_CAP", "25")
                .parse()
                .unwrap_or_else(|_| panic!("EMAIL_DAILY_CAP must be a valid u32")),
            email_digest_enabled: env_or("EMAIL_DIGEST_ENABLED", "false") == "true",
            email_digest_hour_utc: env_or("EMAIL_DIGEST_HOUR_UTC", "15")
                .parse()
                .ok()
                .filter(|h: &u8| *h < 24)
                .unwrap_or_else(|| panic!("EMAIL_DIGEST_HOUR_UTC must be 0-23")),
            security_audit_retention_enabled: env_or("SECURITY_AUDIT_RETENTION_ENABLED", "false")
                == "true",
            security_audit_retention_days: env_or("SECURITY_AUDIT_RETENTION_DAYS", "90")
                .parse()
                .ok()
                .filter(|d: &u32| *d > 0)
                .unwrap_or_else(|| panic!("SECURITY_AUDIT_RETENTION_DAYS must be a positive integer")),
            security_audit_retention_hour_utc: env_or("SECURITY_AUDIT_RETENTION_HOUR_UTC", "4")
                .parse()
                .ok()
                .filter(|h: &u8| *h < 24)
                .unwrap_or_else(|| panic!("SECURITY_AUDIT_RETENTION_HOUR_UTC must be 0-23")),
            // Resource caps (#34). All four overridable via env without a
            // code change; the defaults are calibrated for the 1-task
            // ECS test stack on a 256-CPU/512-MB Fargate task.
            max_members_per_doc: env_or("MAX_MEMBERS_PER_DOC", "200")
                .parse()
                .unwrap_or_else(|_| panic!("MAX_MEMBERS_PER_DOC must be a positive integer")),
            max_members_per_folder: env_or("MAX_MEMBERS_PER_FOLDER", "200")
                .parse()
                .unwrap_or_else(|_| panic!("MAX_MEMBERS_PER_FOLDER must be a positive integer")),
            max_ws_connections_per_doc: env_or("MAX_WS_CONNECTIONS_PER_DOC", "100")
                .parse()
                .unwrap_or_else(|_| panic!("MAX_WS_CONNECTIONS_PER_DOC must be a positive integer")),
            max_ws_connections_per_user_per_doc: env_or(
                "MAX_WS_CONNECTIONS_PER_USER_PER_DOC", "5",
            )
            .parse()
            .unwrap_or_else(|_| panic!("MAX_WS_CONNECTIONS_PER_USER_PER_DOC must be a positive integer")),
            rate_limit_auth_login_per_min: env_or("RATE_LIMIT_AUTH_LOGIN_PER_MIN", "20")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_AUTH_LOGIN_PER_MIN must be a positive integer")),
            // 60/min: the SPA refreshes the access token on every page load
            // (it's in-memory), so a human reloading repeatedly was tripping
            // the old 10/min cap → 429 → bounced to /login. 60 is comfortably
            // above human refresh rates (and tolerates a few users behind one
            // NAT'd IP) while still capping automated replay; refresh-token
            // rotation reuse-detection remains the primary anti-replay defense.
            rate_limit_auth_refresh_per_min: env_or("RATE_LIMIT_AUTH_REFRESH_PER_MIN", "60")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_AUTH_REFRESH_PER_MIN must be a positive integer")),
            rate_limit_search_per_min: env_or("RATE_LIMIT_SEARCH_PER_MIN", "60")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_SEARCH_PER_MIN must be a positive integer")),
            rate_limit_sharing_per_min: env_or("RATE_LIMIT_SHARING_PER_MIN", "30")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_SHARING_PER_MIN must be a positive integer")),
            rate_limit_admin_mut_per_min: env_or("RATE_LIMIT_ADMIN_MUT_PER_MIN", "30")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_ADMIN_MUT_PER_MIN must be a positive integer")),
            rate_limit_saml_acs_per_min: env_or("RATE_LIMIT_SAML_ACS_PER_MIN", "10")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_SAML_ACS_PER_MIN must be a positive integer")),
            max_pending_updates_bytes: env_or(
                "MAX_PENDING_UPDATES_BYTES",
                "33554432", // 32 MiB
            )
            .parse()
            .unwrap_or_else(|_| panic!("MAX_PENDING_UPDATES_BYTES must be a positive integer")),
            mfa_challenge_max_failures: env_or("MFA_CHALLENGE_MAX_FAILURES", "5")
                .parse()
                .ok()
                .filter(|n: &u32| *n > 0)
                .unwrap_or_else(|| panic!("MFA_CHALLENGE_MAX_FAILURES must be a positive integer")),
            scim_request_rate_limit_per_minute: env_or("SCIM_REQUEST_RATE_LIMIT_PER_MIN", "100")
                .parse()
                .unwrap_or_else(|_| panic!("SCIM_REQUEST_RATE_LIMIT_PER_MIN must be a positive integer")),
            rate_limit_comments_per_min: env_or("RATE_LIMIT_COMMENTS_PER_MIN", "30")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_COMMENTS_PER_MIN must be a positive integer")),
            rate_limit_content_write_per_min: env_or("RATE_LIMIT_CONTENT_WRITE_PER_MIN", "60")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_CONTENT_WRITE_PER_MIN must be a positive integer")),
            rate_limit_user_search_per_min: env_or("RATE_LIMIT_USER_SEARCH_PER_MIN", "60")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_USER_SEARCH_PER_MIN must be a positive integer")),
            rate_limit_ws_upgrade_per_min: env_or("RATE_LIMIT_WS_UPGRADE_PER_MIN", "30")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_WS_UPGRADE_PER_MIN must be a positive integer")),
            rate_limit_client_telemetry_per_min: env_or("RATE_LIMIT_CLIENT_TELEMETRY_PER_MIN", "12")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_CLIENT_TELEMETRY_PER_MIN must be a positive integer")),
            rate_limit_rum_per_min: env_or("RATE_LIMIT_RUM_PER_MIN", "60")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_RUM_PER_MIN must be a positive integer")),
            rate_limit_import_per_min: env_or("RATE_LIMIT_IMPORT_PER_MIN", "10")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_IMPORT_PER_MIN must be a positive integer")),
            rate_limit_bulk_export_per_min: env_or("RATE_LIMIT_BULK_EXPORT_PER_MIN", "5")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_BULK_EXPORT_PER_MIN must be a positive integer")),
            rate_limit_bulk_op_per_min: env_or("RATE_LIMIT_BULK_OP_PER_MIN", "20")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_BULK_OP_PER_MIN must be a positive integer")),
            rate_limit_dev_login_per_min: env_or("RATE_LIMIT_DEV_LOGIN_PER_MIN", "100")
                .parse()
                .unwrap_or_else(|_| panic!("RATE_LIMIT_DEV_LOGIN_PER_MIN must be a positive integer")),
            trash_cleanup_enabled: env_or("TRASH_CLEANUP_ENABLED", "false") == "true",
            trash_retention_days: env_or("TRASH_RETENTION_DAYS", "30")
                .parse()
                .ok()
                .filter(|d: &u32| *d > 0)
                .unwrap_or_else(|| panic!("TRASH_RETENTION_DAYS must be a positive integer")),
            trash_cleanup_hour_utc: env_or("TRASH_CLEANUP_HOUR_UTC", "3")
                .parse()
                .ok()
                .filter(|h: &u8| *h < 24)
                .unwrap_or_else(|| panic!("TRASH_CLEANUP_HOUR_UTC must be 0-23")),
            trash_cleanup_dry_run: env_or("TRASH_CLEANUP_DRY_RUN", "false") == "true",
            job_stream_name: env_or("JOB_STREAM_NAME", "ogrenotes-jobs"),
            worker_concurrency: env_or("WORKER_CONCURRENCY", "4")
                .parse()
                .ok()
                .filter(|n: &u32| *n > 0)
                .unwrap_or_else(|| panic!("WORKER_CONCURRENCY must be a positive integer")),

            // Phase 2b — default flipped from "log" to "reject" after
            // the Phase 2a rollout window on test1-. Operators can still
            // set LIVEAPP_STRICT_VALIDATION=log (or off) via env if the
            // metric shows a legitimate write pattern we need to unblock
            // while shipping a validator fix.
            liveapp_strict_validation: env_or("LIVEAPP_STRICT_VALIDATION", "reject"),
            // gap-001 exemption list. Comma-separated doc IDs, empty
            // by default. Whitespace-trimmed and empty-filtered so
            // "a,,b, c" → {"a", "b", "c"}.
            liveapp_gate_exempt_doc_ids: env::var("LIVEAPP_GATE_EXEMPT_DOC_IDS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            // Phase 3 rollout: start in `canary` so the deployed
            // stack automatically runs BOTH walks and emits the
            // canary-mismatch metric. Watch
            // `liveapp.gate_walk_canary_mismatch_total` for a
            // safe-observation window before flipping the default
            // to `changed`. Operators can force `full` via env if
            // canary itself reveals a subtle bug.
            liveapp_gate_walk_scope: env_or("LIVEAPP_GATE_WALK_SCOPE", "canary"),
        }
    }

    /// Table name with prefix applied.
    pub fn table_name(&self) -> String {
        table_name_for_prefix(&self.dynamodb_table_prefix)
    }
}

/// Canonical DynamoDB table name for a given prefix. Shared with the
/// `setup_dev` binary so a rename only needs to change one place.
pub fn table_name_for_prefix(prefix: &str) -> String {
    format!("{prefix}ogrenote")
}

fn env_required(key: &str) -> String {
    env::var(key).unwrap_or_else(|_| panic!("{key} environment variable is required"))
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Minimum JWT_SECRET length for HS256 (NIST 256-bit recommendation).
/// jsonwebtoken re-checks this when encoding, but failing at config load
/// catches it at startup instead of on the first login.
const MIN_JWT_SECRET_LEN: usize = 32;

fn validate_jwt_secret(secret: &str) {
    if secret.len() < MIN_JWT_SECRET_LEN {
        panic!(
            "JWT_SECRET must be at least {MIN_JWT_SECRET_LEN} bytes for HS256; got {} bytes",
            secret.len()
        );
    }
}

/// Reject half-configured SMTP auth. `SMTP_USERNAME` without `SMTP_PASSWORD`
/// (or vice versa) would silently fall through to an unauthenticated
/// connection — SES then rejects every email with a 535 at send time,
/// which is a pain to diagnose in production. Validated at startup so the
/// crash is loud and immediate. `email_enabled=false` skips the check so
/// local MailHog dev (no auth) still works.
fn validate_smtp_credentials(
    email_enabled: bool,
    username: &Option<String>,
    password: &Option<String>,
) {
    if !email_enabled {
        return;
    }
    match (username.as_deref(), password.as_deref()) {
        (None, None) => {} // unauthenticated SMTP is valid (e.g. MailHog)
        (Some(_), Some(_)) => {} // both set: normal auth
        (Some(_), None) => panic!(
            "SMTP_USERNAME is set but SMTP_PASSWORD is not; SMTP will reject sends. \
             Set both or neither."
        ),
        (None, Some(_)) => panic!(
            "SMTP_PASSWORD is set but SMTP_USERNAME is not; SMTP will reject sends. \
             Set both or neither."
        ),
    }
}

fn validate_frontend_origin(origin: &str, dev_mode: bool) {
    // Minimal URL-shape check: scheme :// host, optional :port, no path.
    // Avoids pulling the `url` crate for this one call site.
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or_else(|| {
            panic!("FRONTEND_ORIGIN must start with http:// or https://, got: {origin}")
        });
    if rest.is_empty() || rest.contains('/') || rest.contains(' ') {
        panic!(
            "FRONTEND_ORIGIN must be host[:port] with no trailing slash or path, got: {origin}"
        );
    }
    if !dev_mode && !origin.starts_with("https://") {
        panic!(
            "FRONTEND_ORIGIN must use https:// when DEV_MODE is not true; got: {origin}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        // Can't call from_env without setting vars, but we can construct directly
        let config = AppConfig {
            aws_region: "us-east-1".into(),
            dynamodb_table_prefix: "test-".into(),
            s3_bucket: "test-bucket".into(),
            redis_url: "redis://secret@host:6379".into(),
            oauth_client_id: "client-id".into(),
            oauth_client_secret: "super-secret-value".into(),
            oauth_redirect_uri: "http://localhost/callback".into(),
            jwt_secret: "my-jwt-secret-key".into(),
            google_client_id: None,
            google_client_secret: None,
            api_port: 3000,
            frontend_origin: "http://localhost:8080".into(),
            search_index_path: "/tmp/ogrenotes-search-index".into(),
            qdrant_url: None,
            embedding_model_id: "amazon.titan-embed-text-v2:0".into(),
            embedding_dimensions: 1024,
            anthropic_api_key: None,
            anthropic_model: "claude-sonnet-4-6".into(),
            admin_emails: Vec::new(),
            dev_mode: false,
            embed_youtube_nocookie: true,
            deploy_env: "test".into(),
            email_enabled: false,
            email_from_address: String::new(),
            smtp_host: "localhost".into(),
            smtp_port: 1025,
            smtp_username: None,
            smtp_password: Some("smtp-password-secret".into()),
            smtp_starttls: false,
            email_daily_cap: 25,
            email_digest_enabled: false,
            email_digest_hour_utc: 15,
            security_audit_retention_enabled: false,
            security_audit_retention_days: 90,
            security_audit_retention_hour_utc: 4,
            max_members_per_doc: 200,
            max_members_per_folder: 200,
            max_ws_connections_per_doc: 100,
            max_ws_connections_per_user_per_doc: 5,
            rate_limit_auth_login_per_min: 20,
            rate_limit_auth_refresh_per_min: 10,
            rate_limit_search_per_min: 60,
            rate_limit_sharing_per_min: 30,
            rate_limit_admin_mut_per_min: 30,
            rate_limit_saml_acs_per_min: 10,
            max_pending_updates_bytes: 32 * 1024 * 1024,
            mfa_challenge_max_failures: 5,
            scim_request_rate_limit_per_minute: 100,
            rate_limit_comments_per_min: 30,
            rate_limit_content_write_per_min: 60,
            rate_limit_user_search_per_min: 60,
            rate_limit_ws_upgrade_per_min: 30,
            rate_limit_client_telemetry_per_min: 12,
            rate_limit_rum_per_min: 60,
            rate_limit_import_per_min: 10,
            rate_limit_bulk_export_per_min: 5,
            rate_limit_bulk_op_per_min: 20,
            rate_limit_dev_login_per_min: 100,
            trash_cleanup_enabled: false,
            trash_retention_days: 30,
            trash_cleanup_hour_utc: 3,
            trash_cleanup_dry_run: false,
            job_stream_name: "ogrenotes-jobs".into(),
            worker_concurrency: 4,
            liveapp_strict_validation: "log".into(),
            liveapp_gate_exempt_doc_ids: std::collections::HashSet::new(),
            liveapp_gate_walk_scope: "full".into(),
        };
        let debug_output = format!("{config:?}");
        assert!(!debug_output.contains("smtp-password-secret"));
        assert!(!debug_output.contains("super-secret-value"));
        assert!(!debug_output.contains("my-jwt-secret-key"));
        assert!(!debug_output.contains("secret@host"));
        assert!(debug_output.contains("[redacted]"));
        // Non-secret fields should still be visible
        assert!(debug_output.contains("us-east-1"));
        assert!(debug_output.contains("test-bucket"));
    }

    #[test]
    fn jwt_secret_32_bytes_ok() {
        // Exactly 32 bytes — the minimum — must pass.
        validate_jwt_secret(&"a".repeat(32));
    }

    #[test]
    #[should_panic(expected = "JWT_SECRET must be at least 32 bytes")]
    fn jwt_secret_too_short_panics() {
        validate_jwt_secret("short-secret");
    }

    #[test]
    fn frontend_origin_accepts_https() {
        validate_frontend_origin("https://app.example.com", false);
        validate_frontend_origin("https://app.example.com:8443", false);
    }

    #[test]
    fn frontend_origin_accepts_http_in_dev() {
        validate_frontend_origin("http://localhost:8080", true);
    }

    #[test]
    #[should_panic(expected = "must use https://")]
    fn frontend_origin_http_outside_dev_panics() {
        validate_frontend_origin("http://example.com", false);
    }

    #[test]
    #[should_panic(expected = "must start with http://")]
    fn frontend_origin_bad_scheme_panics() {
        validate_frontend_origin("file:///etc/passwd", true);
    }

    #[test]
    #[should_panic(expected = "no trailing slash or path")]
    fn frontend_origin_trailing_slash_panics() {
        validate_frontend_origin("https://example.com/", false);
    }

    #[test]
    #[should_panic(expected = "no trailing slash or path")]
    fn frontend_origin_with_path_panics() {
        validate_frontend_origin("https://example.com/app", false);
    }

    #[test]
    fn smtp_both_unset_is_ok_when_enabled() {
        // MailHog-style: no auth.
        validate_smtp_credentials(true, &None, &None);
    }

    #[test]
    fn smtp_both_set_is_ok_when_enabled() {
        validate_smtp_credentials(true, &Some("u".into()), &Some("p".into()));
    }

    #[test]
    fn smtp_validation_skipped_when_disabled() {
        // Half-configured SMTP is fine if email is off — a future deploy
        // can fill in the missing half before flipping EMAIL_ENABLED.
        validate_smtp_credentials(false, &Some("u".into()), &None);
        validate_smtp_credentials(false, &None, &Some("p".into()));
    }

    #[test]
    #[should_panic(expected = "SMTP_USERNAME is set but SMTP_PASSWORD")]
    fn smtp_username_without_password_panics() {
        validate_smtp_credentials(true, &Some("u".into()), &None);
    }

    #[test]
    #[should_panic(expected = "SMTP_PASSWORD is set but SMTP_USERNAME")]
    fn smtp_password_without_username_panics() {
        validate_smtp_credentials(true, &None, &Some("p".into()));
    }

    #[test]
    fn table_name_for_prefix_appends_canonical_suffix() {
        // DynamoDB key contract shared with the setup_dev binary: the
        // canonical table name is exactly `<prefix>ogrenote` (no separator
        // added, no pluralization). Changing this orphans every deployed
        // table.
        assert_eq!(table_name_for_prefix("test1-"), "test1-ogrenote");
        assert_eq!(table_name_for_prefix(""), "ogrenote");
    }

    #[test]
    #[should_panic(expected = "no trailing slash or path")]
    fn frontend_origin_empty_host_panics() {
        // "https://" alone parses the scheme but leaves an empty host —
        // must be rejected, not accepted as a degenerate origin.
        validate_frontend_origin("https://", false);
    }

    #[test]
    #[should_panic(expected = "no trailing slash or path")]
    fn frontend_origin_with_space_panics() {
        validate_frontend_origin("https://exa mple.com", false);
    }

    #[test]
    fn frontend_origin_accepts_https_in_dev_mode() {
        // dev_mode relaxes the https requirement; it must not *forbid*
        // https (e.g. a dev stack behind TLS).
        validate_frontend_origin("https://dev.example.com", true);
    }

    #[test]
    fn jwt_secret_multibyte_length_counts_bytes_not_chars() {
        // The HS256 minimum is a byte count. 16 two-byte characters is 32
        // bytes and must pass even though it is only 16 chars — pins that
        // the check uses len() (bytes), matching what the HMAC actually
        // keys on.
        validate_jwt_secret(&"é".repeat(16)); // 16 chars × 2 bytes = 32 bytes
    }

    #[test]
    #[should_panic(expected = "JWT_SECRET must be at least 32 bytes")]
    fn jwt_secret_31_bytes_panics() {
        // One byte under the minimum — pins the exact boundary.
        validate_jwt_secret(&"a".repeat(31));
    }

    #[test]
    fn debug_redacts_optional_secrets_when_present() {
        // The existing redaction test leaves google_client_secret and
        // anthropic_api_key as None, so their redaction lines were never
        // exercised with real values. A regression that printed either
        // would leak credentials into every log that formats the config.
        let config = AppConfig {
            aws_region: "us-east-1".into(),
            dynamodb_table_prefix: "test-".into(),
            s3_bucket: "test-bucket".into(),
            redis_url: "redis://localhost:6379".into(),
            oauth_client_id: "client-id".into(),
            oauth_client_secret: "oauth-secret".into(),
            oauth_redirect_uri: "http://localhost/callback".into(),
            jwt_secret: "jwt-secret".into(),
            google_client_id: Some("google-client-id".into()),
            google_client_secret: Some("google-secret-value".into()),
            api_port: 3000,
            frontend_origin: "http://localhost:8080".into(),
            search_index_path: "/tmp/ogrenotes-search-index".into(),
            qdrant_url: None,
            embedding_model_id: "amazon.titan-embed-text-v2:0".into(),
            embedding_dimensions: 1024,
            anthropic_api_key: Some("sk-ant-secret-key-value".into()),
            anthropic_model: "claude-sonnet-4-6".into(),
            admin_emails: Vec::new(),
            dev_mode: false,
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
            security_audit_retention_enabled: false,
            security_audit_retention_days: 90,
            security_audit_retention_hour_utc: 4,
            max_members_per_doc: 200,
            max_members_per_folder: 200,
            max_ws_connections_per_doc: 100,
            max_ws_connections_per_user_per_doc: 5,
            rate_limit_auth_login_per_min: 20,
            rate_limit_auth_refresh_per_min: 60,
            rate_limit_search_per_min: 60,
            rate_limit_sharing_per_min: 30,
            rate_limit_admin_mut_per_min: 30,
            rate_limit_saml_acs_per_min: 10,
            max_pending_updates_bytes: 32 * 1024 * 1024,
            mfa_challenge_max_failures: 5,
            scim_request_rate_limit_per_minute: 100,
            rate_limit_comments_per_min: 30,
            rate_limit_content_write_per_min: 60,
            rate_limit_user_search_per_min: 60,
            rate_limit_ws_upgrade_per_min: 30,
            rate_limit_client_telemetry_per_min: 12,
            rate_limit_rum_per_min: 60,
            rate_limit_import_per_min: 10,
            rate_limit_bulk_export_per_min: 5,
            rate_limit_bulk_op_per_min: 20,
            rate_limit_dev_login_per_min: 100,
            trash_cleanup_enabled: false,
            trash_retention_days: 30,
            trash_cleanup_hour_utc: 3,
            trash_cleanup_dry_run: false,
            job_stream_name: "ogrenotes-jobs".into(),
            worker_concurrency: 4,
            liveapp_strict_validation: "reject".into(),
            liveapp_gate_exempt_doc_ids: std::collections::HashSet::new(),
            liveapp_gate_walk_scope: "canary".into(),
        };
        let debug_output = format!("{config:?}");
        assert!(!debug_output.contains("google-secret-value"));
        assert!(!debug_output.contains("sk-ant-secret-key-value"));
        // The non-secret google_client_id remains visible.
        assert!(debug_output.contains("google-client-id"));
        // table_name() delegates to the shared free function; pin the
        // delegation here since we already have a constructed config.
        assert_eq!(config.table_name(), "test-ogrenote");
    }

}
