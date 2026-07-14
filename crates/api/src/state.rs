// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use fred::prelude::RedisClient;
use ogrenotes_collab::redis_pubsub::RedisPubSub;
use ogrenotes_collab::room::RoomRegistry;
use ogrenotes_common::config::AppConfig;
use crate::redis_session::RedisSessionStore;
use ogrenotes_common::metrics::RollingUsers;
use crate::claude::ClaudeMessages;
use crate::edit_activity::EditActivityDebouncer;
use crate::folder_inherit_cache::FolderInheritCache;
use crate::middleware::activity::ActivityTracker;
use ogrenotes_embeddings::EmbeddingPipeline;
use ogrenotes_notify::{EmailCapRepo, EmailService, NoopSender, SmtpSender};
use ogrenotes_search::SearchIndex;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::repo::activity_repo::ActivityRepo;
use ogrenotes_storage::repo::admin_audit_repo::AdminAuditRepo;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::mfa_recovery_repo::MfaRecoveryRepo;
use ogrenotes_storage::repo::notification_repo::NotificationRepo;
use ogrenotes_storage::repo::security_audit_repo::SecurityAuditRepo;
use ogrenotes_storage::repo::session_repo::SessionRepo;
use ogrenotes_storage::repo::snapshot_repo::SnapshotRepo;
use ogrenotes_storage::repo::thread_repo::ThreadRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;
use ogrenotes_storage::repo::template_gallery_repo::TemplateGalleryRepo;
use ogrenotes_storage::repo::workspace_saml_config_repo::WorkspaceSamlConfigRepo;
use ogrenotes_storage::repo::workspace_scim_token_repo::WorkspaceScimTokenRepo;
use ogrenotes_storage::s3::S3Client;
use std::sync::Arc;

/// Per-document lock for serializing room initialization.
/// Prevents duplicate S3/DynamoDB loads when multiple WebSocket clients
/// connect to the same document concurrently.
pub type RoomInitLocks = Arc<dashmap::DashMap<String, Arc<tokio::sync::Mutex<()>>>>;

/// Shared application state passed to all Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub user_repo: Arc<UserRepo>,
    pub activity_repo: Arc<ActivityRepo>,
    pub admin_audit_repo: Arc<AdminAuditRepo>,
    pub security_audit_repo: Arc<SecurityAuditRepo>,
    pub mfa_recovery_repo: Arc<MfaRecoveryRepo>,
    pub doc_repo: Arc<DocRepo>,
    pub folder_repo: Arc<FolderRepo>,
    pub session_repo: Arc<SessionRepo>,
    pub thread_repo: Arc<ThreadRepo>,
    pub notification_repo: Arc<NotificationRepo>,
    pub snapshot_repo: Arc<SnapshotRepo>,
    pub workspace_repo: Arc<WorkspaceRepo>,
    pub workspace_saml_config_repo: Arc<WorkspaceSamlConfigRepo>,
    pub workspace_scim_token_repo: Arc<WorkspaceScimTokenRepo>,
    pub template_gallery_repo: Arc<TemplateGalleryRepo>,
    pub room_registry: Arc<RoomRegistry>,
    pub redis_pubsub: Arc<RedisPubSub>,
    /// Shared Redis command client. The single source for the raw
    /// fixed-window counter operations (rate limiting, `/ask` quota,
    /// MFA-failure throttling) and the backing handle that
    /// `redis_pubsub` and `redis_session` are both built from.
    pub redis: Arc<RedisClient>,
    /// Short-lived auth-flow Redis state (MFA pending handles, SAML
    /// replay/AuthnRequest tracking). Extracted from `redis_pubsub`
    /// so the collaboration crate carries no auth-domain knowledge
    /// (#97).
    pub redis_session: Arc<RedisSessionStore>,
    pub room_init_locks: RoomInitLocks,
    pub search_index: Arc<SearchIndex>,
    pub embedding_pipeline: Option<Arc<EmbeddingPipeline>>,
    pub claude_client: Option<Arc<dyn ClaudeMessages>>,
    /// Async-job queue producer. None when Redis isn't available
    /// (single-task dev stack without docker compose up). The /jobs
    /// route returns 503 ServiceUnavailable in that case; specific
    /// job-producing routes (DOCX/PDF import in M-6.5/6) should
    /// also degrade rather than hang. Wraps a `JobQueue` in prod.
    pub job_producer: Option<Arc<dyn ogrenotes_worker::JobProducer>>,
    pub rolling_users: Arc<RollingUsers>,
    pub email_service: Arc<EmailService>,
    pub activity_tracker: Arc<ActivityTracker>,
    pub edit_activity_debouncer: Arc<EditActivityDebouncer>,
    /// #37: short-TTL cache of folder `inherit_mode` for the REST access
    /// path. The WS-connect check bypasses it (authoritative). Invalidated
    /// on folder update/delete in `routes/folders.rs`.
    pub folder_inherit_cache: Arc<FolderInheritCache>,
}

impl AppState {
    pub fn new(
        config: AppConfig,
        dynamo: DynamoClient,
        s3: S3Client,
        redis: Arc<RedisClient>,
        search_index: SearchIndex,
        embedding_pipeline: Option<EmbeddingPipeline>,
        claude_client: Option<Arc<dyn ClaudeMessages>>,
        job_producer: Option<Arc<dyn ogrenotes_worker::JobProducer>>,
    ) -> Self {
        // Phase 4 M-E3: surface a misconfigured MFA key at startup
        // instead of on the first enroll/verify request. We only
        // WARN (don't panic) so dev/test stacks without the env var
        // still boot — every MFA route returns 500 in that state,
        // which is the same behavior as before this check landed.
        match ogrenotes_auth::mfa::load_key() {
            Ok(_) => tracing::info!("MFA_ENCRYPTION_KEY loaded; MFA routes ready"),
            Err(e) => tracing::warn!(
                error = ?e,
                "MFA_ENCRYPTION_KEY missing or malformed; \
                 /auth/mfa/* routes will return 500 until configured"
            ),
        }

        let activity_repo = Arc::new(ActivityRepo::new(dynamo.clone()));
        let admin_audit_repo = Arc::new(AdminAuditRepo::new(dynamo.clone()));
        let security_audit_repo = Arc::new(SecurityAuditRepo::new(dynamo.clone()));
        let mfa_recovery_repo = Arc::new(MfaRecoveryRepo::new(dynamo.clone()));
        let user_repo = Arc::new(UserRepo::new(dynamo.clone()));
        let doc_repo = Arc::new(DocRepo::new(dynamo.clone(), s3));
        let folder_repo = Arc::new(FolderRepo::new(dynamo.clone()));
        let thread_repo = Arc::new(ThreadRepo::new(dynamo.clone()));
        let notification_repo = Arc::new(NotificationRepo::new(dynamo.clone()));
        let snapshot_repo = Arc::new(SnapshotRepo::new(dynamo.clone()));
        let workspace_repo = Arc::new(WorkspaceRepo::new(dynamo.clone()));
        let workspace_saml_config_repo =
            Arc::new(WorkspaceSamlConfigRepo::new(dynamo.clone()));
        let workspace_scim_token_repo =
            Arc::new(WorkspaceScimTokenRepo::new(dynamo.clone()));
        let template_gallery_repo = Arc::new(TemplateGalleryRepo::new(dynamo.clone()));
        let cap_repo = Arc::new(EmailCapRepo::new(
            dynamo.clone(),
            config.email_daily_cap,
        ));
        let session_repo = Arc::new(SessionRepo::new(dynamo));

        // Pick the email transport at startup. A falsy EMAIL_ENABLED or a
        // SMTP build error both fall back to the no-op sender *and flip
        // effective_enabled to false*. Keeping `enabled=true` with a
        // NoopSender would silently drain every user's daily cap while
        // delivering nothing — the `SendOutcome::Sent` would be a lie.
        let (sender, effective_enabled): (Arc<dyn ogrenotes_notify::EmailSender>, bool) =
            if config.email_enabled {
                match SmtpSender::new(
                    &config.smtp_host,
                    config.smtp_port,
                    config.smtp_username.as_deref(),
                    config.smtp_password.as_deref(),
                    config.smtp_starttls,
                ) {
                    Ok(s) => (Arc::new(s), true),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            "SMTP sender build failed; disabling email delivery",
                        );
                        (Arc::new(NoopSender), false)
                    }
                }
            } else {
                (Arc::new(NoopSender), false)
            };
        // Email-link HMAC signing key (#40). Reusing jwt_secret is
        // safe because the HMAC input carries a `notif:v1:` domain
        // separator — see `templates::build_notif_param`. Rotating
        // jwt_secret retires every outstanding email-link token in
        // one move, which matches the "if you suspect compromise,
        // invalidate everything" property the secret already has
        // for sessions.
        let notif_secret = config.jwt_secret.as_bytes().to_vec();
        let email_service = Arc::new(EmailService::new(
            sender,
            user_repo.clone(),
            cap_repo.clone(),
            config.email_from_address.clone(),
            config.frontend_origin.clone(),
            notif_secret,
            effective_enabled,
        ));
        let activity_tracker = Arc::new(ActivityTracker::new());

        // Build the collaboration pub/sub and the auth-flow session
        // store over the one shared command client (#97). Both clone
        // the Arc — they share the connection; only their Redis
        // key surfaces differ.
        let redis_pubsub = Arc::new(RedisPubSub::new(redis.clone()));
        let redis_session = Arc::new(RedisSessionStore::new(redis.clone()));

        Self {
            config: Arc::new(config),
            activity_repo,
            admin_audit_repo,
            security_audit_repo,
            mfa_recovery_repo,
            user_repo,
            doc_repo,
            folder_repo,
            session_repo,
            thread_repo,
            notification_repo,
            snapshot_repo,
            workspace_repo,
            workspace_saml_config_repo,
            workspace_scim_token_repo,
            template_gallery_repo,
            room_registry: Arc::new(RoomRegistry::new()),
            redis_pubsub,
            redis,
            redis_session,
            room_init_locks: Arc::new(dashmap::DashMap::new()),
            search_index: Arc::new(search_index),
            embedding_pipeline: embedding_pipeline.map(Arc::new),
            claude_client,
            job_producer,
            rolling_users: RollingUsers::new(),
            email_service,
            activity_tracker,
            edit_activity_debouncer: Arc::new(EditActivityDebouncer::new()),
            folder_inherit_cache: Arc::new(FolderInheritCache::new()),
        }
    }
}
