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
        // Cap the map to prevent memory DoS from parallel login floods.
        if flows.len() >= 10_000 {
            return Err(ApiError::BadRequest(
                "Too many pending login requests. Try again later.".to_string(),
            ));
        }
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
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
) -> Result<Response, ApiError> {
    let code_verifier = {
        let mut flows = PENDING_FLOWS.lock().unwrap();
        let flow = flows
            .remove(&params.state)
            .ok_or(ApiError::BadRequest(
                "Invalid or expired state parameter".to_string(),
            ))?;
        flow.code_verifier
    };

    // Exchange authorization code for GitHub access token
    let github_token = exchange_code_for_token(
        &params.code,
        &code_verifier,
        &state.config.oauth_client_id,
        &state.config.oauth_client_secret,
        &state.config.oauth_redirect_uri,
    )
    .await?;

    // Fetch user profile from GitHub
    let gh_profile = fetch_github_profile(&github_token).await?;

    // Find or create user in our database
    let profile = ogrenotes_auth::user::OAuthProfile {
        email: gh_profile.email,
        name: gh_profile.name,
        avatar_url: gh_profile.avatar_url,
    };

    let user = ogrenotes_auth::user::find_or_create_user(
        &state.user_repo,
        &state.folder_repo,
        &profile,
    )
    .await?;

    // Create session and generate tokens
    let session = ogrenotes_auth::session::create_session(
        &state.session_repo,
        &user.user_id,
        Some("github-oauth"),
    )
    .await?;

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    // Redirect to frontend with tokens in URL fragment (not query params for security)
    let redirect_url = format!(
        "{}/#access_token={}&refresh_token={}&session_id={}&user_id={}&email={}&name={}",
        state.config.frontend_origin,
        urlencoding::encode(&access_token),
        urlencoding::encode(&session.refresh_token),
        urlencoding::encode(&session.session_id),
        urlencoding::encode(&user.user_id),
        urlencoding::encode(&user.email),
        urlencoding::encode(&user.name),
    );

    Ok(Redirect::temporary(&redirect_url).into_response())
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
/// Requires DEV_MODE=true in environment; returns 404 otherwise.
async fn dev_login(
    State(state): State<AppState>,
    Json(req): Json<DevLoginRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    if !state.config.dev_mode {
        return Err(ApiError::NotFound("Not found".to_string()));
    }
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

// ─── GitHub OAuth helpers ──────────────────────────────────────

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

async fn exchange_code_for_token(
    code: &str,
    code_verifier: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<String, ApiError> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", code_verifier),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("GitHub token exchange failed: {e}")))?;

    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ApiError::Internal(format!(
            "GitHub token exchange error: {body}"
        )));
    }

    let token_resp: GitHubTokenResponse = resp
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to parse GitHub token: {e}")))?;

    Ok(token_resp.access_token)
}

struct GitHubProfile {
    email: String,
    name: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubUser {
    email: Option<String>,
    name: Option<String>,
    login: String,
    avatar_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

async fn fetch_github_profile(access_token: &str) -> Result<GitHubProfile, ApiError> {
    let client = reqwest::Client::new();

    let user: GitHubUser = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "OgreNotes")
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("GitHub user fetch failed: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to parse GitHub user: {e}")))?;

    // If email is not public, fetch from /user/emails
    let email = if let Some(email) = user.email {
        email
    } else {
        let emails: Vec<GitHubEmail> = client
            .get("https://api.github.com/user/emails")
            .header("Authorization", format!("Bearer {access_token}"))
            .header("User-Agent", "OgreNotes")
            .send()
            .await
            .map_err(|e| ApiError::Internal(format!("GitHub emails fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to parse GitHub emails: {e}")))?;

        emails
            .iter()
            .find(|e| e.primary && e.verified)
            .or_else(|| emails.iter().find(|e| e.verified))
            .map(|e| e.email.clone())
            .ok_or_else(|| ApiError::BadRequest("No verified email found".to_string()))?
    };

    Ok(GitHubProfile {
        email,
        name: user.name.unwrap_or(user.login),
        avatar_url: user.avatar_url,
    })
}
