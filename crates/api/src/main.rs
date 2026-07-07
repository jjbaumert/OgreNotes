// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::{header, Method};
use tower_http::cors::CorsLayer;
use tower_http::trace::{DefaultOnFailure, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;

use fred::prelude::*;
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::s3::S3Client;

use ogrenotes_api::compaction;
use ogrenotes_api::observability;
use ogrenotes_api::routes;
use ogrenotes_api::state::AppState;

#[tokio::main]
async fn main() {
    // Initialise the metrics recorder before any logs fire, so the
    // log-event counter layer below sees a live recorder from the first event.
    ogrenotes_common::metrics::init();

    // Initialize tracing
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer().json())
        .with(ogrenotes_common::metrics::LogEventCounterLayer)
        .init();

    // Load configuration
    let config = AppConfig::from_env();

    // --mode=worker dispatches to the async-job consumer loop instead
    // of the HTTP server. Same binary, same image — ECS task
    // definition selects between modes via the command field. See
    // `ogrenotes_api::worker_mode` for the loop + dispatch.
    let worker_mode = std::env::args().any(|a| a == "--mode=worker");
    if worker_mode {
        tracing::info!(config = ?config, "starting OgreNotes worker");
        ogrenotes_api::worker_mode::run(config).await;
        return;
    }

    tracing::info!(config = ?config, "starting OgreNotes API server");

    // Loud warning if dev-only surface is active. Intentionally at warn
    // level so it's hard to miss in aggregated logs; pair with the
    // `dev-login` cargo feature being off in production builds.
    if config.dev_mode {
        tracing::warn!(
            "DEV_MODE=true is active — /auth/dev-login (if compiled in) \
             will issue tokens for any email. This MUST NOT be enabled in \
             production. Rebuild with `--no-default-features` to compile \
             the endpoint out entirely."
        );
    }

    // Build AWS clients
    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.clone()))
        .load()
        .await;

    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let dynamo = DynamoClient::new(dynamo_client, config.table_name());
    let s3 = S3Client::new(s3_client, config.s3_bucket.clone());

    // Connect to Redis (optional — pub/sub features degrade gracefully without it).
    // We build two clients: the command client (SET/GETDEL/PUBLISH etc) and a
    // dedicated SubscriberClient. RESP2 locks a subscribed connection out of
    // non-pubsub commands, so they must be separate connections.
    let redis_config = fred::types::RedisConfig::from_url(&config.redis_url)
        .expect("invalid REDIS_URL");
    let redis_client = std::sync::Arc::new(
        fred::prelude::RedisClient::new(redis_config.clone(), None, None, None),
    );
    redis_client.connect();
    let redis_connected = match redis_client.wait_for_connect().await {
        Ok(()) => {
            tracing::info!("connected to Redis at {}", config.redis_url);
            true
        }
        Err(e) => {
            tracing::warn!(
                "Redis connection failed: {e} — pub/sub and cross-instance sync will not work. \
                 This is expected for single-task deployments without Redis."
            );
            false
        }
    };

    // Subscriber for cross-instance document update fanout. Only built if
    // the command client connected successfully; failures here are
    // non-fatal (the server still serves clients — it just misses edits
    // originating on other instances).
    let redis_subscriber: Option<fred::clients::SubscriberClient> = if redis_connected {
        let builder = fred::types::Builder::from_config(redis_config);
        match builder.build_subscriber_client() {
            Ok(sub) => {
                sub.connect();
                match sub.wait_for_connect().await {
                    Ok(()) => {
                        // Auto-resubscribe to our channels after any reconnect.
                        let _ = sub.manage_subscriptions();
                        Some(sub)
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Redis subscriber connect failed: {e} — cross-instance fanout disabled"
                        );
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Redis subscriber build failed: {e} — cross-instance fanout disabled"
                );
                None
            }
        }
    } else {
        None
    };

    // Initialize search index
    let search_index = ogrenotes_search::SearchIndex::open_or_create(
        std::path::Path::new(&config.search_index_path),
    )
    .expect("failed to open search index");

    // Initialize embedding pipeline (optional — disabled when QDRANT_URL is not set)
    let embedding_pipeline = if let Some(ref qdrant_url) = config.qdrant_url {
        let bedrock_client = aws_sdk_bedrockruntime::Client::new(&aws_config);
        let embedder = ogrenotes_embeddings::bedrock::BedrockEmbedder::new(
            bedrock_client,
            config.embedding_model_id.clone(),
            config.embedding_dimensions,
        );
        let store = ogrenotes_embeddings::VectorStore::new(
            qdrant_url,
            "ogrenotes",
            config.embedding_dimensions,
        )
        .await
        .expect("failed to connect to Qdrant");
        tracing::info!(qdrant_url, "embedding pipeline initialized");
        Some(ogrenotes_embeddings::EmbeddingPipeline::new(embedder, store))
    } else {
        tracing::info!("embedding pipeline disabled (QDRANT_URL not set)");
        None
    };

    // Initialize Claude API client (optional — ask endpoint returns 503 when not set)
    let claude_client: Option<std::sync::Arc<dyn ogrenotes_api::claude::ClaudeMessages>> =
        config.anthropic_api_key.as_ref().map(|key| {
            tracing::info!("Claude API client initialized");
            std::sync::Arc::new(ogrenotes_api::claude::ClaudeClient::new(
                key.clone(),
                config.anthropic_model.clone(),
            )) as std::sync::Arc<dyn ogrenotes_api::claude::ClaudeMessages>
        });

    // Initialize the async-job producer. Same Redis client family the
    // rest of the stack already uses (the worker-mode entrypoint
    // builds its own consumer-side client). Disabled when Redis isn't
    // connected — the /jobs route returns 503 in that state.
    let job_producer: Option<std::sync::Arc<dyn ogrenotes_worker::JobProducer>> =
        if redis_connected {
            // Rebuild a dedicated RedisClient for the queue handle. The
            // pub/sub client above is locked out of non-pubsub commands
            // on RESP2; we need a clean handle for XADD/XREADGROUP/etc.
            let job_redis_config =
                fred::types::RedisConfig::from_url(&config.redis_url)
                    .expect("invalid REDIS_URL");
            let job_client =
                fred::prelude::RedisClient::new(job_redis_config, None, None, None);
            job_client.connect();
            match job_client.wait_for_connect().await {
                Ok(()) => {
                    match ogrenotes_worker::JobQueue::new(
                        std::sync::Arc::new(job_client),
                        config.job_stream_name.clone(),
                    )
                    .await
                    {
                        Ok(queue) => {
                            tracing::info!(
                                stream = %config.job_stream_name,
                                "job queue producer initialized",
                            );
                            Some(std::sync::Arc::new(queue)
                                as std::sync::Arc<dyn ogrenotes_worker::JobProducer>)
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "job queue init failed; /jobs disabled");
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "job queue redis connect failed; /jobs disabled");
                    None
                }
            }
        } else {
            None
        };

    // Build app state
    let state = AppState::new(
        config.clone(),
        dynamo,
        s3,
        redis_client,
        search_index,
        embedding_pipeline,
        claude_client,
        job_producer,
    );

    // Start background compaction task (checks every 60s, compacts rooms idle > 5min).
    let _compaction_handle = compaction::spawn_compaction_task(
        state.room_registry.clone(),
        state.doc_repo.clone(),
        std::time::Duration::from_secs(60),
        5 * 60 * 1000, // 5 minutes in ms
    );

    // Start the daily digest scheduler. Safe when EMAIL_DIGEST_ENABLED
    // is false — each tick short-circuits on the config flag.
    let _digest_handle = ogrenotes_api::digest::spawn_scheduler(state.clone());

    // Start the daily SecurityAudit retention worker. Safe when
    // SECURITY_AUDIT_RETENTION_ENABLED is false — each tick
    // short-circuits on the config flag. (Phase 4 M-E6 piece D.)
    let _audit_retention_handle =
        ogrenotes_api::audit_retention::spawn_scheduler(state.clone());

    // Start the daily trash-cleanup worker. Safe when
    // TRASH_CLEANUP_ENABLED is false. (Phase 4 M-E7 item 9.)
    let _trash_cleanup_handle =
        ogrenotes_api::trash_cleanup::spawn_scheduler(state.clone());

    // Start the Redis pub/sub subscriber: receive updates from other API
    // instances and fan them out to local clients of the same room.
    // Required for correctness when running with >1 API instance; no-op
    // (and not started) when Redis is unavailable or we're running solo.
    let _redis_sub_handle = if let Some(sub) = redis_subscriber {
        match state
            .redis_pubsub
            .spawn_subscriber(sub, state.room_registry.clone())
            .await
        {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "failed to start Redis pub/sub subscriber — cross-instance sync disabled"
                );
                None
            }
        }
    } else {
        None
    };

    // Start the EMF emitter (every 60s) and the state sampler (every 30s).
    observability::spawn(state.clone(), config.deploy_env.clone(), state.rolling_users.clone());

    // Build CORS layer with explicit headers (not Any, which is rejected with credentials).
    //
    // Origin policy:
    //   - production (`dev_mode=false`): pin to the configured
    //     `FRONTEND_ORIGIN`. Single tenant, single SPA — anything else
    //     is hostile.
    //   - dev / CI (`dev_mode=true`): mirror whatever Origin the
    //     browser sent. The wasm-pack test harness assigns a random
    //     localhost port per run, so a fixed origin can't cover it;
    //     mirroring lets `cargo wasm-pack test --headless --firefox`
    //     authenticate against a sibling axum instance, and is also
    //     friendly to local dev where you might serve the frontend
    //     from `trunk serve` on one port and the API on another.
    //     Mirroring is credential-compatible (echoes the origin in
    //     `Access-Control-Allow-Origin`), so refresh-cookie auth
    //     keeps working in dev.
    let allow_origin = if config.dev_mode {
        tower_http::cors::AllowOrigin::mirror_request()
    } else {
        tower_http::cors::AllowOrigin::exact(
            config
                .frontend_origin
                .parse::<axum::http::HeaderValue>()
                .expect("invalid FRONTEND_ORIGIN"),
        )
    };
    let cors = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT])
        .allow_credentials(true);

    // Build router — serve API routes, then fall back to static frontend files
    let mut app = routes::api_router()
        .with_state(state);

    // #48: pending OAuth-flow state lives in Redis now (keyed
    // `oauth_flow:<state>` with a TTL), so there is no in-memory map to
    // sweep — expiry is handled by Redis and the per-IP login rate limit.

    // Serve frontend static files if the directory exists (production builds
    // copy the Trunk output into /app/frontend/dist inside the Docker image).
    // For SPA routing, any path that doesn't match a static file returns index.html.
    //
    // We also peek at index.html here to compute the SHA-256 hashes of
    // its inline `<script>` blocks (Trunk emits one to bootstrap the
    // WASM); those hashes get appended to the CSP `script-src` so the
    // browser will execute the bootstrap. Without this the page loads,
    // CSP blocks the inline `<script type="module">…import init…</script>`,
    // and the WASM never mounts. Hashes are computed once at startup —
    // the dist is baked into the image, so they're stable per deploy.
    let static_dir_path = std::env::var("FRONTEND_DIST")
        .unwrap_or_else(|_| "/app/frontend/dist".to_string());
    let static_dir = std::path::Path::new(&static_dir_path);
    let mut inline_script_hashes: Vec<String> = Vec::new();
    if static_dir.exists() {
        tracing::info!("serving frontend from {}", static_dir.display());
        let index_path = static_dir.join("index.html");
        // Read index.html once: its bytes feed both the CSP inline-script
        // hashing and the SPA fallback below (served from memory, since
        // the dist is baked into the image and immutable per deploy).
        let index_bytes: std::sync::Arc<Vec<u8>> = match std::fs::read_to_string(&index_path) {
            Ok(html) => {
                inline_script_hashes = routes::compute_inline_script_hashes(&html);
                tracing::info!(
                    count = inline_script_hashes.len(),
                    "computed CSP allow-list hashes for inline <script> blocks in index.html",
                );
                std::sync::Arc::new(html.into_bytes())
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %index_path.display(),
                    "could not read index.html for CSP hash computation; inline scripts will be CSP-blocked",
                );
                std::sync::Arc::new(Vec::new())
            }
        };

        // SPA fallback for paths that match no static file. Navigations
        // (Accept: text/html) get the shell so client-side routes work;
        // every other miss — notably a stale client's request for a
        // content-hashed asset from a previous deploy — 404s, so the
        // browser fails it hard instead of hashing an HTML body against
        // the asset's Subresource-Integrity attribute. See
        // `routes::spa_fallback_should_serve_shell`.
        let spa_fallback = {
            use axum::handler::HandlerWithoutStateExt;
            use axum::response::IntoResponse;
            let index_bytes = index_bytes.clone();
            (move |req: axum::extract::Request| {
                let index_bytes = index_bytes.clone();
                async move {
                    let accept = req
                        .headers()
                        .get(axum::http::header::ACCEPT)
                        .and_then(|v| v.to_str().ok());
                    if index_bytes.is_empty()
                        || !routes::spa_fallback_should_serve_shell(accept)
                    {
                        return axum::http::StatusCode::NOT_FOUND.into_response();
                    }
                    (
                        [(
                            axum::http::header::CONTENT_TYPE,
                            "text/html; charset=utf-8",
                        )],
                        (*index_bytes).clone(),
                    )
                        .into_response()
                }
            })
            .into_service()
        };

        let serve_dir = tower_http::services::ServeDir::new(static_dir).fallback(spa_fallback);
        app = app.fallback_service(serve_dir);
    }
    let csp = routes::build_csp(&inline_script_hashes);

    // Per-request INFO logging with a correlation id. Every log line emitted
    // inside a handler inherits the `request_id` field via the span, so the
    // frontend-doctor and aws-diagnostic agents can correlate a browser
    // request to its server-side trace by timestamp or id.
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|req: &axum::http::Request<_>| {
            tracing::info_span!(
                "http",
                request_id = nanoid::nanoid!(12),
                method = %req.method(),
                path = %req.uri().path(),
                user_id = tracing::field::Empty,
            )
        })
        .on_request(|_req: &axum::http::Request<_>, _span: &tracing::Span| {
            tracing::info!("request_start");
        })
        .on_response(
            |res: &axum::http::Response<_>, latency: Duration, _span: &tracing::Span| {
                tracing::info!(
                    status = res.status().as_u16(),
                    latency_ms = latency.as_millis() as u64,
                    "request_end"
                );
            },
        )
        .on_failure(DefaultOnFailure::new().level(Level::WARN));

    // ─── Security response headers (#35) ──────────────────────────
    //
    // Defense-in-depth: every primary defense (CORS, AuthUser, ACL) is
    // upstream of these, but a missing header amplifies the blast
    // radius of any successful XSS or clickjacking attempt. The CSP
    // directive list and per-header rationale live next to
    // `apply_security_headers` in `routes/mod.rs` so the test
    // harness applies the same policy. We attach the layer here at
    // the outermost position so static-file responses (the Leptos
    // WASM bundle, /index.html, fonts) get the headers too.
    let app = routes::apply_security_headers(
        app
            .layer(axum::middleware::from_fn(
                ogrenotes_api::middleware::metrics::track,
            ))
            .layer(cors)
            .layer(trace_layer),
        config.dev_mode,
        &csp,
    );

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.api_port));
    tracing::info!(%addr, "listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    axum::serve(listener, app)
        .await
        .expect("server error");
}
