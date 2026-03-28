pub mod auth;
pub mod documents;
pub mod folders;
pub mod users;

use axum::Router;
use crate::state::AppState;

/// Build the complete API router.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .nest("/api/v1/auth", auth::router())
        .nest("/api/v1/users", users::router())
        .nest("/api/v1/documents", documents::router())
        .nest("/api/v1/folders", folders::router())
}
