use std::net::SocketAddr;

use axum::http::{Method, header};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use fred::prelude::*;
use ogrenotes_common::config::AppConfig;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::s3::S3Client;

mod error;
mod middleware;
mod routes;
mod state;

use state::AppState;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .json()
        .init();

    // Load configuration
    let config = AppConfig::from_env();
    tracing::info!(config = ?config, "starting OgreNotes API server");

    // Build AWS clients
    let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(config.aws_region.clone()))
        .load()
        .await;

    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let s3_client = aws_sdk_s3::Client::new(&aws_config);

    let dynamo = DynamoClient::new(dynamo_client, config.table_name());
    let s3 = S3Client::new(s3_client, config.s3_bucket.clone());

    // Connect to Redis
    let redis_config = fred::types::RedisConfig::from_url(&config.redis_url)
        .expect("invalid REDIS_URL");
    let redis_client = fred::prelude::RedisClient::new(redis_config, None, None, None);
    redis_client.connect();
    redis_client
        .wait_for_connect()
        .await
        .expect("failed to connect to Redis");
    tracing::info!("connected to Redis at {}", config.redis_url);

    let redis_pubsub = ogrenotes_collab::redis_pubsub::RedisPubSub::new(
        std::sync::Arc::new(redis_client),
    );

    // Build app state
    let state = AppState::new(config.clone(), dynamo, s3, redis_pubsub);

    // Build CORS layer with explicit headers (not Any, which is rejected with credentials)
    let cors = CorsLayer::new()
        .allow_origin(
            config
                .frontend_origin
                .parse::<axum::http::HeaderValue>()
                .expect("invalid FRONTEND_ORIGIN"),
        )
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

    // Build router
    let app = routes::api_router()
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

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
