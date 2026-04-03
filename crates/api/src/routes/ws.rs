//! WebSocket collaboration endpoints.
//!
//! - `POST /documents/:id/ws-token` — generate a single-use auth token
//! - `GET /documents/:id/ws` — WebSocket upgrade for real-time sync

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade, Query};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use ogrenotes_collab::document::OgreDoc;
use ogrenotes_collab::protocol::{decode_message, encode_message, MessageType};
use ogrenotes_collab::room::Room;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Build the collaboration WebSocket router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/ws-token", post(create_ws_token))
        .route("/{id}/ws", get(ws_upgrade))
}

// ─── Token Generation ───────────────────────────────────────────

#[derive(Serialize)]
struct WsTokenResponse {
    token: String,
}

/// Generate a single-use WebSocket authentication token.
/// The token is stored in Redis with a 30-second TTL.
async fn create_ws_token(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
) -> Result<axum::Json<WsTokenResponse>, ApiError> {
    // Verify user has access to the document
    let _meta = crate::routes::documents::get_verified_doc(&state, &doc_id, &user_id).await?;

    // Generate random token
    let token = nanoid::nanoid!(32);

    // Store in Redis with 30-second TTL
    state
        .redis_pubsub
        .store_ws_token(&token, &user_id, &doc_id, 30)
        .await
        .map_err(|e| ApiError::Internal(format!("Redis error: {e}")))?;

    Ok(axum::Json(WsTokenResponse { token }))
}

// ─── WebSocket Upgrade ──────────────────────────────────────────

#[derive(Deserialize)]
struct WsQuery {
    token: String,
}

/// WebSocket upgrade handler.
/// The client must pass the single-use token as a query parameter.
/// (The token is single-use via Redis GETDEL, so URL logging exposure is time-limited.)
async fn ws_upgrade(
    State(state): State<AppState>,
    Path(doc_id): Path<String>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Validate the single-use token
    let auth = state
        .redis_pubsub
        .validate_ws_token(&query.token)
        .await
        .map_err(|e| ApiError::Internal(format!("Redis error: {e}")))?;

    let Some((user_id, token_doc_id)) = auth else {
        return Err(ApiError::Unauthorized);
    };

    // Token must match the requested document
    if token_doc_id != doc_id {
        return Err(ApiError::Unauthorized);
    }

    // Load or create the collaboration room
    let room = {
        if let Some(existing) = state.room_registry.get(&doc_id) {
            existing
        } else {
            // Load document from storage
            let meta = crate::routes::documents::get_verified_doc(&state, &doc_id, &user_id).await?;
            let mut doc = if let Some(ref s3_key) = meta.snapshot_s3_key {
                let bytes = state
                    .doc_repo
                    .s3()
                    .get_object(s3_key)
                    .await
                    .map_err(|e| ApiError::Internal(format!("S3 error: {e}")))?;
                OgreDoc::from_state_bytes(&bytes)?
            } else {
                OgreDoc::new()
            };

            // Apply pending updates
            let pending = state
                .doc_repo
                .get_pending_updates(&doc_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            for update in &pending {
                let _ = doc.apply_update(&update.update_bytes);
            }

            state.room_registry.get_or_insert(&doc_id, doc)
        }
    };

    // Upgrade to WebSocket
    Ok(ws.on_upgrade(move |socket| handle_ws(socket, room, user_id, state)))
}

// ─── WebSocket Message Loop ─────────────────────────────────────

async fn handle_ws(
    socket: WebSocket,
    room: Arc<Room>,
    user_id: String,
    state: AppState,
) {
    use futures_util::{SinkExt, StreamExt};

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create a channel for this client
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    // Register client in the room
    let client_id = room.next_client_id();
    room.add_client(client_id, user_id.clone(), tx).await;

    tracing::info!(
        doc_id = room.doc_id(),
        client_id,
        user_id = user_id,
        "WebSocket client connected"
    );

    // Send initial sync (server's state vector)
    room.sync_client(client_id).await;

    // Spawn task to forward room messages to WebSocket
    let room_for_send = Arc::clone(&room);
    let mut send_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if ws_sender.send(Message::Binary(data.into())).await.is_err() {
                break;
            }
        }
    });

    // Process incoming messages
    let room_for_recv = Arc::clone(&room);
    let doc_id = room.doc_id().to_string();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Binary(data) => {
                    let data = data.to_vec();
                    // Reject oversized messages to prevent OOM DoS.
                    if data.len() > 1_048_576 {
                        tracing::warn!(client_id, size = data.len(), "WS message too large, dropping");
                        continue;
                    }
                    if let Some((msg_type, payload)) = decode_message(&data) {
                        match msg_type {
                            MessageType::SyncStep1 => {
                                tracing::debug!(client_id, payload_len = payload.len(), "received SyncStep1");
                                if let Ok(diff) = room_for_recv.encode_diff(payload).await {
                                    tracing::debug!(client_id, diff_len = diff.len(), "sending SyncStep2 response");
                                    let response = encode_message(MessageType::SyncStep2, &diff);
                                    room_for_recv.send_to_client(client_id, response).await;
                                }
                            }
                            MessageType::SyncStep2 => {
                                tracing::debug!(client_id, payload_len = payload.len(), "received SyncStep2 (applying, no broadcast)");
                                let _ = room_for_recv.apply_update(payload).await;
                            }
                            MessageType::Update => {
                                tracing::debug!(client_id, payload_len = payload.len(), "received Update");
                                match room_for_recv.apply_update(payload).await {
                                    Ok(()) => {
                                        let num_clients = room_for_recv.client_count().await;
                                        tracing::debug!(client_id, num_clients, "applied update, broadcasting");
                                        let broadcast_msg = encode_message(MessageType::Update, payload);
                                        room_for_recv.broadcast(client_id, broadcast_msg.clone()).await;

                                        let _ = state.redis_pubsub.publish_update(&doc_id, &broadcast_msg).await;

                                        let ts = ogrenotes_common::time::now_usec();
                                        let clock = format!("{}_{}", ts, nanoid::nanoid!(8));
                                        let update = ogrenotes_storage::models::document::DocUpdate {
                                            doc_id: doc_id.clone(),
                                            clock,
                                            update_bytes: payload.to_vec(),
                                            user_id: user_id.clone(),
                                            created_at: ts,
                                        };
                                        let _ = state.doc_repo.append_update(&update).await;
                                    }
                                    Err(e) => {
                                        tracing::warn!(client_id, error = %e, "failed to apply update");
                                    }
                                }
                            }
                            MessageType::Awareness => {
                                // Validate and overwrite user_id before broadcasting
                                // to prevent identity spoofing.
                                if let Some(mut awareness) = ogrenotes_collab::awareness::decode_awareness(payload) {
                                    awareness.user_id = user_id.clone();
                                    let validated = ogrenotes_collab::awareness::encode_awareness(&awareness);
                                    let broadcast_msg = encode_message(MessageType::Awareness, &validated);
                                    room_for_recv.broadcast(client_id, broadcast_msg).await;
                                }
                            }
                            _ => {} // Auth and Error are not expected from client here
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {} // Ignore text, ping, pong
            }
        }
    });

    // Wait for either task to finish (client disconnected).
    // Abort the other task to prevent resource leaks.
    tokio::select! {
        _ = &mut send_task => { recv_task.abort(); },
        _ = &mut recv_task => { send_task.abort(); },
    }

    // Cleanup
    let is_empty = room.remove_client(client_id).await;
    tracing::info!(
        doc_id = room.doc_id(),
        client_id,
        is_empty,
        "WebSocket client disconnected"
    );

    if is_empty {
        // Last client left — compact and remove room
        // TODO: idle compaction (snapshot to S3, prune UPDATE# rows)
        state.room_registry.remove_if_empty(room.doc_id()).await;
    }
}
