use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(get_me))
        .route("/search", get(search_users))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserResponse {
    user_id: String,
    name: String,
    email: String,
    avatar_url: Option<String>,
    home_folder_id: String,
    private_folder_id: String,
    trash_folder_id: String,
    created_at: i64,
}

/// GET /users/me -- current user profile.
async fn get_me(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<Json<UserResponse>, ApiError> {
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    Ok(Json(UserResponse {
        user_id: user.user_id,
        name: user.name,
        email: user.email,
        avatar_url: user.avatar_url,
        home_folder_id: user.home_folder_id,
        private_folder_id: user.private_folder_id,
        trash_folder_id: user.trash_folder_id,
        created_at: user.created_at,
    }))
}

#[derive(Deserialize)]
struct SearchQuery {
    /// Exact email lookup (used by ShareDialog).
    #[serde(default)]
    email: Option<String>,
    /// Substring search across email and name (used by @menu).
    #[serde(default)]
    q: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    user_id: String,
    name: String,
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    users: Vec<SearchResult>,
}

/// GET /users/search?email=...&q=... -- search for users.
/// Use `email` for exact email lookup, `q` for substring search.
async fn search_users(
    State(state): State<AppState>,
    AuthUser { .. }: AuthUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    // Exact email lookup takes priority.
    if let Some(email) = query.email {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(Json(SearchResponse { users: Vec::new() }));
        }
        return match state.user_repo.get_by_email(&email).await {
            Ok(Some(user)) => Ok(Json(SearchResponse {
                users: vec![SearchResult {
                    user_id: user.user_id,
                    name: user.name,
                    email: user.email,
                }],
            })),
            Ok(None) => Ok(Json(SearchResponse { users: Vec::new() })),
            Err(e) => Err(ApiError::Internal(e.to_string())),
        };
    }

    // Substring search across email and name.
    if let Some(q) = query.q {
        let q = q.trim().to_string();
        if q.is_empty() {
            return Ok(Json(SearchResponse { users: Vec::new() }));
        }
        return match state.user_repo.search_users(&q).await {
            Ok(users) => Ok(Json(SearchResponse {
                users: users
                    .into_iter()
                    .map(|u| SearchResult {
                        user_id: u.user_id,
                        name: u.name,
                        email: u.email,
                    })
                    .collect(),
            })),
            Err(e) => Err(ApiError::Internal(e.to_string())),
        };
    }

    Ok(Json(SearchResponse { users: Vec::new() }))
}
