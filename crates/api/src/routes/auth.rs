use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::ApiError;
use crate::state::AppState;

/// In-memory store for pending OAuth flows (PKCE verifier + state).
/// In production, use a signed cookie or Redis with short TTL.
static PENDING_FLOWS: std::sync::LazyLock<Mutex<HashMap<String, PendingFlow>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

struct PendingFlow {
    code_verifier: String,
    created_at: i64,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", get(login))
        .route("/callback", get(callback))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/dev-login", post(dev_login))
}

/// GET /auth/login -- redirect to OAuth provider.
async fn login(State(state): State<AppState>) -> Result<Response, ApiError> {
    let pkce = ogrenotes_auth::oauth::generate_pkce();
    let auth_state = ogrenotes_auth::oauth::generate_state();

    {
        let mut flows = PENDING_FLOWS.lock().unwrap();
        let now = ogrenotes_common::time::now_usec();
        let ten_minutes_usec = 10 * 60 * 1_000_000;
        flows.retain(|_, f| now - f.created_at < ten_minutes_usec);
        flows.insert(
            auth_state.clone(),
            PendingFlow {
                code_verifier: pkce.code_verifier.clone(),
                created_at: now,
            },
        );
    }

    let url = ogrenotes_auth::oauth::build_authorization_url(
        "https://github.com/login/oauth/authorize",
        &state.config.oauth_client_id,
        &state.config.oauth_redirect_uri,
        &pkce,
        &auth_state,
        &["user:email"],
    );

    Ok(Redirect::temporary(&url).into_response())
}

#[derive(Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub session_id: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
}

/// GET /auth/callback -- exchange OAuth code for tokens.
async fn callback(
    State(_state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
) -> Result<Json<TokenResponse>, ApiError> {
    let _code_verifier = {
        let mut flows = PENDING_FLOWS.lock().unwrap();
        let flow = flows
            .remove(&params.state)
            .ok_or(ApiError::BadRequest(
                "Invalid or expired state parameter".to_string(),
            ))?;
        flow.code_verifier
    };

    let _ = params.code;

    Err(ApiError::BadRequest(
        "OAuth token exchange not yet implemented -- use POST /api/v1/auth/dev-login for development"
            .to_string(),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest {
    refresh_token: String,
    user_id: String,
    session_id: String,
}

/// POST /auth/refresh -- refresh access token and rotate refresh token.
async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let new_refresh = ogrenotes_auth::session::rotate_refresh_token(
        &state.session_repo,
        &req.user_id,
        &req.session_id,
        &req.refresh_token,
    )
    .await?;

    let user = state
        .user_repo
        .get_by_id(&req.user_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?
        .ok_or(ApiError::Unauthorized)?;

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    Ok(Json(TokenResponse {
        access_token,
        refresh_token: new_refresh,
        session_id: req.session_id,
        user_id: user.user_id,
        email: user.email,
        name: user.name,
    }))
}

/// POST /auth/logout -- revoke session.
async fn logout(
    State(state): State<AppState>,
    crate::middleware::auth::AuthUser { user_id, .. }: crate::middleware::auth::AuthUser,
) -> Result<StatusCode, ApiError> {
    state
        .session_repo
        .delete_all_for_user(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevLoginRequest {
    email: String,
    #[serde(default = "default_dev_name")]
    name: String,
}

fn default_dev_name() -> String {
    "Dev User".to_string()
}

/// POST /auth/dev-login -- create or find a user and return tokens directly.
/// For local development only. Bypasses OAuth.
async fn dev_login(
    State(state): State<AppState>,
    Json(req): Json<DevLoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let profile = ogrenotes_auth::user::OAuthProfile {
        email: req.email,
        name: req.name,
        avatar_url: None,
    };

    let user = ogrenotes_auth::user::find_or_create_user(
        &state.user_repo,
        &state.folder_repo,
        &profile,
    )
    .await?;

    let session = ogrenotes_auth::session::create_session(
        &state.session_repo,
        &user.user_id,
        Some("dev-login"),
    )
    .await?;

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    Ok(Json(TokenResponse {
        access_token,
        refresh_token: session.refresh_token,
        session_id: session.session_id,
        user_id: user.user_id,
        email: user.email,
        name: user.name,
    }))
}
