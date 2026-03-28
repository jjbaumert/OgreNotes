use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/me", get(get_me))
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
