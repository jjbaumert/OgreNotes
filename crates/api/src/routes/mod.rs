pub mod auth;
pub mod chat;
pub mod comments;
pub mod documents;
pub mod folders;
pub mod history;
pub mod notifications;
pub mod sharing;
pub mod users;
pub mod ws;

use axum::Router;
use crate::state::AppState;

/// Build the complete API router.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .nest("/api/v1/auth", auth::router())
        .nest("/api/v1/users", users::router())
        .nest("/api/v1/documents", documents::router().merge(ws::router()).merge(comments::doc_router()).merge(history::router()))
        .nest("/api/v1/threads", comments::thread_router())
        .nest("/api/v1/chats", chat::router())
        .nest("/api/v1/notifications", notifications::router())
        .nest("/api/v1/folders", folders::router().merge(sharing::router()))
}
