// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! WebSocket collaboration endpoints.
//!
//! - `POST /documents/:id/ws-token` — generate a single-use auth token
//! - `GET /documents/:id/ws` — WebSocket upgrade for real-time sync

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequestParts, Path, State, WebSocketUpgrade, Query};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use ogrenotes_collab::document::OgreDoc;
use ogrenotes_collab::protocol::{
    decode_message, encode_foreign_doc_update, encode_message, MessageType,
};
use ogrenotes_collab::redis_pubsub::WsAccess;
use ogrenotes_collab::room::Room;
use ogrenotes_common::metrics::{counter, gauge, histogram, MetricKey};

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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsTokenRequest {
    #[serde(default)]
    client_version: Option<String>,
}

/// Generate a single-use WebSocket authentication token.
/// The token is stored in Redis with a 30-second TTL.
async fn create_ws_token(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(doc_id): Path<String>,
    body: Option<axum::Json<WsTokenRequest>>,
) -> Result<axum::Json<WsTokenResponse>, ApiError> {
    // #111: a View-access user (durable or via a `view` link) may open a
    // *read-only* live subscription. Authorize at View first — that gates
    // who gets a token at all — then decide the token's write authority
    // from whether the same caller also has Edit. The level is baked into
    // the token (server-authored) so the room can reject writes from a
    // read-only session without trusting the client.
    use ogrenotes_storage::models::AccessLevel;
    let meta = crate::routes::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::View,
    ).await?;
    let access = if meta.locked {
        // #140: a locked doc is a doc-wide freeze — mint a read-only token for
        // everyone (including the owner, who unlocks via the lock endpoint, not
        // by editing). The per-frame `read_only_permits_frame` predicate then
        // drops `Update`/`SyncStep2` on this session, so a captured token can
        // never escalate to a write. Sibling of the `put_content` REST guard.
        ogrenotes_collab::redis_pubsub::WsAccess::ReadOnly
    } else if crate::routes::documents::check_doc_access(
        &state, &doc_id, &user_id, AccessLevel::Edit,
    )
    .await
    .is_ok()
    {
        ogrenotes_collab::redis_pubsub::WsAccess::ReadWrite
    } else {
        ogrenotes_collab::redis_pubsub::WsAccess::ReadOnly
    };

    let client_version = body.and_then(|b| b.client_version.clone());

    // Phase 1 observability counters (see design/observability.md
    // §Version-skew tolerance):
    //
    //   - `server.dto.field_absent_total{route,field=client_version}`
    //     fires when an old client (no client_version) sends a token
    //     request. Lets the discrepancy_map agent see the distribution
    //     of upgraded-vs-not clients in flight.
    //
    //   - `ws.skew_client_version_mismatch_total{result}` categorizes
    //     every request. `min_client_version` enforcement lands in
    //     Phase 2 — for now `result` is only `ok`, `absent`, or
    //     `unparseable`, and `unparseable` is empirically rare today
    //     (the client always sends the cargo version). Recording the
    //     distribution now lets us see what's actually deployed before
    //     setting a floor.
    classify_client_version(client_version.as_deref());

    // Generate random token
    let token = nanoid::nanoid!(32);

    // Store in Redis with 30-second TTL
    state
        .redis_pubsub
        .store_ws_token(&token, &user_id, &doc_id, client_version.as_deref(), access, 30)
        .await
        .map_err(|e| ApiError::Internal(format!("Redis error: {e}")))?;

    Ok(axum::Json(WsTokenResponse { token }))
}

/// Emit the WS-token skew counters for `client_version`. Pure side-
/// effect helper extracted from `create_ws_token` so the per-request
/// classification logic isn't tangled with token issuance and so it's
/// unit-testable. `min_client_version` enforcement lives in Phase 2;
/// today the result is one of `{ok, absent, unparseable}`.
fn classify_client_version(client_version: Option<&str>) {
    match client_version {
        None => {
            counter::inc(MetricKey::new(
                "server.dto.field_absent_total",
                &[
                    ("route", "/documents/:id/ws-token"),
                    ("field", "client_version"),
                ],
            ));
            counter::inc(MetricKey::new(
                "ws.skew_client_version_mismatch_total",
                &[("result", "absent")],
            ));
        }
        Some(v) if is_parseable_semver_prefix(v) => {
            counter::inc(MetricKey::new(
                "ws.skew_client_version_mismatch_total",
                &[("result", "ok")],
            ));
        }
        Some(_) => {
            counter::inc(MetricKey::new(
                "ws.skew_client_version_mismatch_total",
                &[("result", "unparseable")],
            ));
        }
    }
}

/// Lightweight semver-prefix check: an ASCII string of digits and
/// dots that starts with a digit. We deliberately avoid pulling a
/// full semver crate just for the skew counter — Phase 2's
/// `min_client_version` enforcement will need real parsing, at which
/// point this helper grows up. Today we just want to distinguish
/// "looks like a version string" from "garbage."
fn is_parseable_semver_prefix(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_digit() || c == '.')
}

// ─── WebSocket Upgrade ──────────────────────────────────────────

/// Hard cap on a single WebSocket message at the transport layer.
/// Closes #43.
///
/// Applied to **both** `WebSocketUpgrade::max_message_size` and
/// `max_frame_size`, which guard distinct attack shapes:
///
///  - `max_message_size` caps the reassembled message after
///    concatenating frames. tokio-tungstenite's default is ~64 MB —
///    without this cap, a malicious authenticated editor can stream
///    a multi-MB logical message broken into many small frames and
///    the runtime buffers all of it before the application check in
///    `handle_ws` fires. The cap also applies to *outbound* frames,
///    so server SyncStep2 broadcasts of large documents must fit
///    within it — that constraint is what set this ceiling.
///  - `max_frame_size` caps each individual frame. tokio-tungstenite's
///    default is also ~16 MB, and a single frame is buffered as it's
///    parsed; a malicious peer could send one giant frame before the
///    message cap fires. Pinning `max_frame_size` to the same ceiling
///    means a legitimate client can use any frame layout up to the
///    cap per frame, since per-frame size cannot exceed the
///    containing message anyway.
///
/// `handle_ws::recv_task` re-asserts `data.len() <= WS_MAX_MESSAGE_BYTES`
/// after decode as defense in depth — see comment there.
///
/// **Sizing**: 64 MiB. Sized to accommodate large documents on initial
/// `SyncStep2` push — the server ships the full encoded yrs state in
/// one frame, and we target supporting ~10 MB+ documents (paste-heavy
/// workloads, sheets with substantive content). 64 MiB matches
/// tokio-tungstenite's reassembled-message default, so we are not
/// raising the upstream-default DoS lever; we are just no longer
/// running 4x below it.
///
/// **Per-connection memory cost**: each in-flight receive of a large
/// frame transiently holds up to this many bytes in the recv buffer
/// before the application loop sees the message. At 64 MiB × concurrent
/// large-frame uploads, the steady-state cost is still trivial (most
/// updates are sub-KB), but a coordinated burst of SyncStep2 pushes
/// could pressure ECS task memory. Verify against the task definition's
/// memory ceiling before raising further.
///
/// The per-`DocUpdate` persistence path is no longer bounded by this
/// ceiling — `DocRepo` routes oversize updates to S3 (#38), so a
/// single paste can be any size up to this WS frame cap.
pub const WS_MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
struct WsQuery {
    /// Legacy auth token carried in the URL query string. Optional now
    /// that the token can ride in the `Sec-WebSocket-Protocol` header
    /// instead (#46) — kept as a fallback for older clients.
    #[serde(default)]
    token: Option<String>,
}

/// #46: the WS auth token rides as a WebSocket subprotocol of the form
/// `ogrenotes-ws-token.<TOKEN>` so it stays out of the URL — and thus
/// out of browser history and HTTP access logs where a query string
/// would land. The server selects and echoes the fixed
/// `ogrenotes-ws` subprotocol (below), so the token itself never
/// appears in the handshake *response* headers either.
const WS_TOKEN_SUBPROTOCOL_PREFIX: &str = "ogrenotes-ws-token.";

/// #46: the fixed subprotocol the server echoes back to satisfy the
/// browser's subprotocol-negotiation check without reflecting the token.
const WS_SENTINEL_SUBPROTOCOL: &str = "ogrenotes-ws";

/// #46: resolve the WS auth token, preferring the `Sec-WebSocket-Protocol`
/// header (token-out-of-URL) and falling back to the legacy `?token=`
/// query param. The header is a comma-separated subprotocol list; the
/// token is the entry prefixed with `ogrenotes-ws-token.`. Empty values
/// are treated as absent.
fn extract_ws_token(
    subprotocol_header: Option<&str>,
    query_token: Option<&str>,
) -> Option<String> {
    if let Some(header) = subprotocol_header {
        for proto in header.split(',') {
            if let Some(tok) = proto.trim().strip_prefix(WS_TOKEN_SUBPROTOCOL_PREFIX) {
                if !tok.is_empty() {
                    return Some(tok.to_string());
                }
            }
        }
    }
    query_token
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
}

/// Reject WebSocket upgrades whose `Origin` header doesn't match the
/// configured `frontend_origin`. CORS does not apply to the WebSocket
/// handshake, so this is the primary defense against cross-site
/// WebSocket hijacking — a different origin's page coercing the
/// browser into upgrading against this server with the user's cookies
/// or a captured ws-token.
///
/// In `dev_mode`, both a missing `Origin` and any non-matching Origin
/// are accepted. This mirrors the dev-mode CORS policy, which uses
/// `AllowOrigin::mirror_request()` to echo whatever origin the browser
/// sent — necessary because tooling like `wasm-pack test --headless`
/// hosts the test page on a randomly-assigned localhost port that we
/// can't pre-register, and `trunk serve` typically runs on a different
/// port than the API. Production (`dev_mode=false`) keeps the strict
/// equality check.
fn validate_ws_origin(
    headers: &HeaderMap,
    expected_origin: &str,
    dev_mode: bool,
) -> Result<(), ApiError> {
    match headers.get(axum::http::header::ORIGIN) {
        Some(origin) => {
            let received = origin.to_str().map_err(|_| {
                tracing::warn!("ws_upgrade rejected: Origin header is not valid UTF-8");
                ApiError::Forbidden
            })?;
            if received == expected_origin {
                Ok(())
            } else if dev_mode {
                tracing::debug!(
                    expected = %expected_origin,
                    received = %received,
                    "ws_upgrade: Origin mismatch tolerated in dev_mode",
                );
                Ok(())
            } else {
                tracing::warn!(
                    expected = %expected_origin,
                    received = %received,
                    "ws_upgrade rejected: Origin mismatch"
                );
                Err(ApiError::Forbidden)
            }
        }
        None if dev_mode => Ok(()),
        None => {
            tracing::warn!("ws_upgrade rejected: missing Origin header (production)");
            Err(ApiError::Forbidden)
        }
    }
}

/// Cold-load a document's `OgreDoc` from its S3 snapshot plus pending
/// DynamoDB updates. The caller is responsible for the access check and the
/// per-document init lock; this owns the storage I/O and the cold-load
/// observability signals:
///   - `s3.failures_total{op=cold_load}` on an S3 error,
///   - `doc.cold_load_duration_ms` on success.
///
/// Both the primary WS-upgrade path and foreign-doc subscription go through
/// here, so the metrics fire identically for both — previously the
/// foreign-subscription path emitted neither, leaving its S3 failures
/// invisible to monitoring. `?` on `get_pending_updates` lets
/// `From<RepoError>` map `TooLarge` to 503 (#91).
async fn cold_load_room_doc(
    state: &AppState,
    doc_id: &str,
    snapshot_s3_key: Option<&str>,
) -> Result<OgreDoc, ApiError> {
    let cold_load_start = std::time::Instant::now();
    let mut doc = if let Some(s3_key) = snapshot_s3_key {
        let bytes = state
            .doc_repo
            .s3()
            .get_object(s3_key)
            .await
            .map_err(|e| {
                counter::inc(MetricKey::new("s3.failures_total", &[("op", "cold_load")]));
                ApiError::Internal(format!("S3 error: {e}"))
            })?;
        OgreDoc::from_state_bytes(&bytes)?
    } else {
        OgreDoc::new()
    };

    let pending = state
        .doc_repo
        .get_pending_updates(doc_id, state.config.max_pending_updates_bytes)
        .await?;
    for update in &pending {
        let _ = doc.apply_update(&update.update_bytes);
    }

    histogram::record(
        MetricKey::new("doc.cold_load_duration_ms", &[]),
        cold_load_start.elapsed().as_secs_f64() * 1000.0,
    );
    Ok(doc)
}

/// WebSocket upgrade handler.
/// The client must pass the single-use token as a query parameter.
/// (The token is single-use via Redis GETDEL, so URL logging exposure is time-limited.)
///
/// We take the raw `Request` rather than the `WebSocketUpgrade`
/// extractor so the Origin / token / cap checks can run BEFORE
/// `WebSocketUpgrade::from_request_parts`. With `WebSocketUpgrade`
/// in the function signature, a request that lacks WS handshake
/// headers (Connection: Upgrade, Sec-WebSocket-Key, etc.) would
/// reject with 400 *before* the handler body runs — masking the
/// 403 (Origin mismatch), 401 (bad token), and 429 (cap reached)
/// responses that downstream clients (and integration tests in
/// `test_ws.rs`) actually depend on.
async fn ws_upgrade(
    State(state): State<AppState>,
    Path(doc_id): Path<String>,
    Query(query): Query<WsQuery>,
    request: axum::extract::Request,
) -> Result<Response, ApiError> {
    let (mut parts, _body) = request.into_parts();
    let headers: &HeaderMap = &parts.headers;

    // Reject cross-origin upgrades before touching Redis or the token.
    validate_ws_origin(headers, &state.config.frontend_origin, state.config.dev_mode)?;

    // #46: resolve the token from the subprotocol header (preferred) or
    // the legacy query string. Absent/empty → 401 before any Redis work.
    let subprotocol_header = headers
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok());
    let token = extract_ws_token(subprotocol_header, query.token.as_deref()).ok_or_else(|| {
        tracing::warn!(doc_id = %doc_id, "ws_upgrade rejected: no auth token");
        ApiError::Unauthorized
    })?;

    // Validate the single-use token
    let auth = state
        .redis_pubsub
        .validate_ws_token(&token)
        .await
        .map_err(|e| {
            tracing::warn!(doc_id = %doc_id, error = %e, "ws_upgrade redis error");
            ApiError::Internal(format!("Redis error: {e}"))
        })?;

    let Some((user_id, token_doc_id, client_version, ws_access)) = auth else {
        tracing::warn!(doc_id = %doc_id, "ws_upgrade rejected: token invalid or expired");
        return Err(ApiError::Unauthorized);
    };

    // M-E7 item 10: per-user WS upgrade rate limit. Runs after
    // token validation so the rate-limit key is the authenticated
    // user_id (not an unauthenticated IP, which would let one user
    // exhaust another's budget by sharing an IP). Bounds reconnect-
    // storm damage from a compromised token; the WS room cap
    // (`max_ws_connections_per_doc`) handles the connection-count
    // dimension separately.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "ws_upgrade",
        &user_id,
        state.config.rate_limit_ws_upgrade_per_min,
        60,
    )
    .await?;

    // Token must match the requested document
    if token_doc_id != doc_id {
        tracing::warn!(
            doc_id = %doc_id,
            token_doc_id = %token_doc_id,
            user_id = %user_id,
            "ws_upgrade rejected: token doc_id mismatch"
        );
        return Err(ApiError::Unauthorized);
    }

    // #140: re-read the lock at upgrade time. `create_ws_token` baked the write
    // authority at mint, but the doc may have been locked in the (≤30s) window
    // between mint and upgrade. Downgrade ReadWrite→ReadOnly when the doc is now
    // locked; never upgrade (a ReadOnly mint stays ReadOnly — access may have
    // been revoked). The per-frame `read_only_permits_frame` predicate then
    // drops this session's CRDT writes. One DDB read on the connection path, not
    // per-frame. This closes the mint→upgrade window; an already-open session
    // keeps its captured authority until it reconnects (documented limitation,
    // matching the #111 token model).
    let ws_access = if ws_access == WsAccess::ReadWrite {
        let meta = crate::routes::documents::check_doc_access(
            &state, &doc_id, &user_id,
            ogrenotes_storage::models::AccessLevel::View,
        ).await?;
        if meta.locked {
            counter::inc(MetricKey::new("doc.locked_write_rejected_total", &[("path", "ws_upgrade")]));
            WsAccess::ReadOnly
        } else {
            ws_access
        }
    } else {
        ws_access
    };

    // Load or create the collaboration room.
    // Use a per-document lock to prevent duplicate S3/DynamoDB loads when
    // multiple clients connect to the same document concurrently.
    let room = {
        if let Some(existing) = state.room_registry.get(&doc_id) {
            existing
        } else {
            let init_lock = state
                .room_init_locks
                .entry(doc_id.clone())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone();
            let _guard = init_lock.lock().await;

            // Re-check after acquiring the lock — another task may have loaded it.
            if let Some(existing) = state.room_registry.get(&doc_id) {
                existing
            } else {
                // Access is checked here (not on the resident fast path
                // above) only to authorize the cold *read* that loads the
                // room — View suffices, since the single-use token already
                // proved View-or-better at mint time and *encodes* the
                // write authority (#111). Write enforcement lives per-frame
                // in `handle_ws`, which drops CRDT updates from a read-only
                // session. Foreign-doc subscription differs — it has no
                // per-doc token and re-checks View on every call.
                let meta = crate::routes::documents::check_doc_access(
                    &state, &doc_id, &user_id,
                    ogrenotes_storage::models::AccessLevel::View,
                ).await?;
                let doc =
                    cold_load_room_doc(&state, &doc_id, meta.snapshot_s3_key.as_deref()).await?;
                state.room_registry.get_or_insert(&doc_id, doc)
            }
        }
    };

    // Enforce concurrent-connection caps (#34) BEFORE on_upgrade so
    // we reject at HTTP layer with 429 + Retry-After rather than
    // accepting the upgrade and immediately closing it. Both checks
    // tolerate a small overshoot under high concurrency (two upgrades
    // can pass the count check before either room.add_client lands)
    // — acceptable for a structural cap, not for a security gate.
    let total_clients = room.client_count().await;
    if total_clients >= state.config.max_ws_connections_per_doc {
        tracing::warn!(
            doc_id = %doc_id,
            user_id = %user_id,
            total_clients,
            cap = state.config.max_ws_connections_per_doc,
            "ws_upgrade rejected: per-document connection cap reached"
        );
        return Err(ApiError::TooManyRequests {
            message: format!(
                "Document has reached the concurrent-connection cap of {}; retry after some clients disconnect.",
                state.config.max_ws_connections_per_doc
            ),
            retry_after_secs: 30,
        });
    }
    let user_clients = room.client_count_for_user(&user_id).await;
    if user_clients >= state.config.max_ws_connections_per_user_per_doc {
        tracing::warn!(
            doc_id = %doc_id,
            user_id = %user_id,
            user_clients,
            cap = state.config.max_ws_connections_per_user_per_doc,
            "ws_upgrade rejected: per-user-per-document connection cap reached"
        );
        return Err(ApiError::TooManyRequests {
            message: format!(
                "You have reached the concurrent-connection cap of {} for this document; close other tabs to free a slot.",
                state.config.max_ws_connections_per_user_per_doc
            ),
            retry_after_secs: 30,
        });
    }

    // Upgrade to WebSocket. Done manually (rather than via the
    // `WebSocketUpgrade` extractor in the function signature) so the
    // Origin / token / cap checks above can return their specific
    // 403 / 401 / 429 statuses before `WebSocketUpgrade` would
    // otherwise reject a non-handshake request with 400 — see the
    // doc comment on this function.
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &state)
        .await
        .map_err(|rej| {
            tracing::warn!(
                doc_id = %doc_id,
                user_id = %user_id,
                "ws_upgrade rejected: not a valid WebSocket handshake",
            );
            ApiError::BadRequest(format!("not a websocket upgrade request: {rej}"))
        })?;
    // Pin both transport-layer caps before upgrading (#43). Without
    // these, the tokio-tungstenite defaults apply (~64 MB / message,
    // ~16 MB / frame) and the bytes hit the heap during frame
    // parsing or message reassembly before the application check in
    // `handle_ws` sees them. See the doc comment on
    // `WS_MAX_MESSAGE_BYTES` for the per-cap rationale.
    let ws = ws
        .max_message_size(WS_MAX_MESSAGE_BYTES)
        .max_frame_size(WS_MAX_MESSAGE_BYTES)
        // #46: when the client offered the token via subprotocol, it also
        // offers the fixed `ogrenotes-ws` sentinel — select+echo that one
        // so the browser's negotiation check passes without reflecting the
        // token. Legacy query-token clients offer no subprotocol, so this
        // is a no-op for them.
        .protocols([WS_SENTINEL_SUBPROTOCOL]);
    Ok(ws.on_upgrade(move |socket| {
        handle_ws(socket, room, user_id, client_version, ws_access, state)
    }))
}

// ─── Cross-document foreign-room subscriptions ────────────────────

/// One active foreign-doc subscription on a single WS connection.
/// `room` is the foreign doc's `Room`; `client_id` is the id this
/// connection holds inside that room (registered via
/// `room.add_client`); `forward_task` pumps updates from the foreign
/// room's broadcast channel onto the primary connection's `tx`,
/// re-wrapping each one as a `ForeignDocUpdate` frame so the client
/// can route it to the right yrs::Doc sidecar.
struct ForeignSubscription {
    room: Arc<Room>,
    client_id: u64,
    forward_task: tokio::task::JoinHandle<()>,
}

/// Ensure the foreign doc's `Room` is loaded (cold-load from S3 +
/// pending updates if necessary), gated by a per-id init lock to
/// avoid concurrent cold-loads. Re-checks the *caller's* read access
/// to the foreign doc on every call — even if the room is already
/// resident in memory (loaded earlier by a different user with
/// different perms), the subscribing user must independently have
/// View access. Returns `Forbidden` / `NotFound` from
/// `check_doc_access` for ineligible callers.
async fn ensure_foreign_room_loaded(
    state: &AppState,
    doc_id: &str,
    user_id: &str,
) -> Result<Arc<Room>, ApiError> {
    use crate::routes::documents::check_doc_access;
    use ogrenotes_storage::models::AccessLevel;

    if let Some(existing) = state.room_registry.get(doc_id) {
        let _meta = check_doc_access(state, doc_id, user_id, AccessLevel::View).await?;
        return Ok(existing);
    }
    let init_lock = state
        .room_init_locks
        .entry(doc_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = init_lock.lock().await;
    if let Some(existing) = state.room_registry.get(doc_id) {
        let _meta = check_doc_access(state, doc_id, user_id, AccessLevel::View).await?;
        return Ok(existing);
    }
    let meta = check_doc_access(state, doc_id, user_id, AccessLevel::View).await?;
    let doc = cold_load_room_doc(state, doc_id, meta.snapshot_s3_key.as_deref()).await?;
    Ok(state.room_registry.get_or_insert(doc_id, doc))
}

// ─── WebSocket Message Loop ─────────────────────────────────────

/// #111: whether an inbound frame of `msg_type` from a session holding
/// `ws_access` is allowed to mutate the doc. The two client→server CRDT
/// write frames — `Update` (applies + broadcasts + persists) and
/// `SyncStep2` (merges the client's state into the server doc) — are the
/// only writes, so they're the only frames a `ReadOnly` session is denied.
/// Everything else is a read or presence-only frame (`SyncStep1` is a
/// server→client diff response, `Ping` a keepalive, `Awareness` ephemeral
/// cursor state, foreign-doc subscribe/unsubscribe are read subscriptions)
/// and is always permitted. Pure so the security rule has a direct unit
/// test even though the full WS message loop needs a live socket.
fn read_only_permits_frame(ws_access: WsAccess, msg_type: MessageType) -> bool {
    !matches!(
        (ws_access, msg_type),
        (WsAccess::ReadOnly, MessageType::Update | MessageType::SyncStep2)
    )
}

/// #58/T-2: per-user rate limit for WS frames that hit the persist path
/// (`Update` / `SyncStep2`). Returns `true` when the caller has exceeded
/// `rate_limit_ws_messages_per_min` and the frame must not be applied.
/// `ws_upgrade` bounds connection establishment; this bounds per-message
/// writes once a socket is open. A legit client debounces outgoing updates
/// (~16ms), so the generous default never clips real editing while it caps a
/// client flooding the persist path at socket speed. Fails open on a Redis
/// error, matching `rate_limit::enforce`.
async fn ws_message_rate_limited(state: &AppState, user_id: &str) -> bool {
    if crate::middleware::rate_limit::enforce(
        &state.redis,
        "ws_message",
        user_id,
        state.config.rate_limit_ws_messages_per_min,
        60,
    )
    .await
    .is_err()
    {
        counter::inc(MetricKey::new("ws.rate_limited_total", &[]));
        tracing::warn!(%user_id, "ws message rate limit exceeded; closing connection");
        true
    } else {
        false
    }
}

async fn handle_ws(
    socket: WebSocket,
    room: Arc<Room>,
    user_id: String,
    client_version: Option<String>,
    ws_access: ogrenotes_collab::redis_pubsub::WsAccess,
    state: AppState,
) {
    use futures_util::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicU64, Ordering};

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create a channel for this client
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    // Per-foreign-doc forward tasks pump updates onto the same WS via
    // a clone of `tx`. Cloning before the move into `room.add_client`
    // lets us reuse the same channel for primary + foreign traffic
    // (so the WS frames stay ordered and the client only deals with
    // a single inbound stream).
    let tx_for_foreign = tx.clone();

    // Active foreign-doc subscriptions on this connection. Mutex
    // because the recv task owns mutate-on-subscribe / unsubscribe
    // and the disconnect path needs to drain them on cleanup.
    let foreign_subs: Arc<tokio::sync::Mutex<std::collections::HashMap<String, ForeignSubscription>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));

    // Register client in the room
    let client_id = room.next_client_id();
    room.add_client(client_id, user_id.clone(), tx).await;

    let connected_at = std::time::Instant::now();
    let messages_in = Arc::new(AtomicU64::new(0));
    let messages_out = Arc::new(AtomicU64::new(0));
    // Per-type inbound counters. Tells us whether a "quiet" session was
    // actually text-only-missing (lots of Awareness, zero Update) or
    // completely silent (zero of everything), which points at different
    // client-side regressions.
    let sync1_in = Arc::new(AtomicU64::new(0));
    let sync2_in = Arc::new(AtomicU64::new(0));
    let update_in = Arc::new(AtomicU64::new(0));
    let awareness_in = Arc::new(AtomicU64::new(0));
    let ping_in = Arc::new(AtomicU64::new(0));
    let other_in = Arc::new(AtomicU64::new(0));
    let user_id_for_log = user_id.clone();

    counter::inc(MetricKey::new("ws.connected_total", &[]));
    gauge::add(MetricKey::new("ws.active_connections", &[]), 1);

    tracing::info!(
        event_type = "ws_connected",
        doc_id = room.doc_id(),
        client_id,
        user_id = %user_id,
        client_version = ?client_version,
        "ws_client_connected"
    );

    // Send initial sync (server's state vector)
    room.sync_client(client_id).await;

    // Prime this client with every other collaborator's most recent
    // awareness (cursor + selection). Without this, a client that
    // idle-disconnected and reconnected wouldn't see existing cursors
    // until those users moved.
    for payload in room.awareness_snapshot(client_id).await {
        let msg = encode_message(MessageType::Awareness, &payload);
        room.send_to_client(client_id, msg).await;
    }

    // Spawn task to forward room messages to WebSocket
    let _room_for_send = Arc::clone(&room);
    let messages_out_for_send = Arc::clone(&messages_out);
    let mut send_task = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if ws_sender.send(Message::Binary(data.into())).await.is_err() {
                break;
            }
            messages_out_for_send.fetch_add(1, Ordering::Relaxed);
        }
    });

    // Process incoming messages
    let room_for_recv = Arc::clone(&room);
    let doc_id = room.doc_id().to_string();
    let client_version_for_recv = client_version;
    let messages_in_for_recv = Arc::clone(&messages_in);
    // Phase 2a — LiveApp attribute pre-apply gate mode. Read once
    // per session; the flag is a rollout knob, not something
    // that changes mid-connection. `Log` default matches AppConfig.
    //
    // gap-001 exemption: if this doc's id is in the operator-set
    // exempt list (LIVEAPP_GATE_EXEMPT_DOC_IDS env var), force the
    // gate to `Off` for this session. Emits a metric so operators
    // can see the exemption is firing.
    let liveapp_mode = if state.config.liveapp_gate_exempt_doc_ids.contains(&doc_id) {
        counter::inc(MetricKey::new(
            "liveapp.gate_exempted_total",
            &[("path", "ws"), ("doc_id", doc_id.as_str())],
        ));
        ogrenotes_collab::room::LiveAppValidationMode::Off
    } else {
        ogrenotes_collab::room::LiveAppValidationMode::from_env_value(
            Some(state.config.liveapp_strict_validation.as_str()),
        )
    };
    // Phase 3 — resolve the walk scope for this session. Read
    // once and pass through; a per-doc exemption does not affect
    // the scope choice.
    let liveapp_scope = ogrenotes_collab::blocks::WalkScope::from_env_value(
        Some(state.config.liveapp_gate_walk_scope.as_str()),
    );
    // Foreign-doc subscribe paths need the AppState (auth +
    // room_registry) and the user id to gate access. Clone explicitly
    // so the recv task moves its own copy and the outer disconnect
    // cleanup (line ~622: `state.room_registry.remove_if_empty`) can
    // still see `state` via its own disjoint capture.
    let state_for_recv = state.clone();
    let user_id_for_recv = user_id.clone();
    let foreign_subs_for_recv = Arc::clone(&foreign_subs);
    let sync1_in_for_recv = Arc::clone(&sync1_in);
    let sync2_in_for_recv = Arc::clone(&sync2_in);
    let update_in_for_recv = Arc::clone(&update_in);
    let awareness_in_for_recv = Arc::clone(&awareness_in);
    let ping_in_for_recv = Arc::clone(&ping_in);
    let other_in_for_recv = Arc::clone(&other_in);
    // Edit-activity dedupe goes through `state.edit_activity_debouncer`
    // — a process-shared DashMap keyed by (doc_id, user_id) — so the
    // cooldown spans REST `put_content` saves AND every WS session the
    // user has open. Without that shared state, an autosave via REST
    // while a WS client is also pushing updates would double-write Edit
    // rows in the same window.
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            match msg {
                Message::Binary(data) => {
                    messages_in_for_recv.fetch_add(1, Ordering::Relaxed);
                    let data = data.to_vec();
                    // Defense in depth — the transport layer
                    // (`WebSocketUpgrade::max_message_size`) already
                    // rejects frames above WS_MAX_MESSAGE_BYTES with
                    // a Close frame before the bytes reach this loop,
                    // but we re-assert the cap here so an upstream
                    // reconfiguration (or a non-Axum server runner)
                    // can never silently regress to the
                    // tokio-tungstenite default of ~64 MB.
                    if data.len() > WS_MAX_MESSAGE_BYTES {
                        tracing::warn!(client_id, size = data.len(), "WS message too large, dropping");
                        continue;
                    }
                    if let Some((msg_type, payload)) = decode_message(&data) {
                        match msg_type {
                            MessageType::SyncStep1 => {
                                sync1_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "sync1")]));
                                tracing::debug!(client_id, payload_len = payload.len(), "received SyncStep1");
                                if let Ok(diff) = room_for_recv.encode_diff(payload).await {
                                    tracing::debug!(client_id, diff_len = diff.len(), "sending SyncStep2 response");
                                    let response = encode_message(MessageType::SyncStep2, &diff);
                                    room_for_recv.send_to_client(client_id, response).await;
                                }
                            }
                            MessageType::SyncStep2 => {
                                sync2_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "sync2")]));
                                // #111: SyncStep2 merges the client's state
                                // into the server doc — a write path — so a
                                // read-only session's frame is dropped. The
                                // viewer's own SyncStep1→2 *read* (server
                                // sends them the diff) is unaffected.
                                if !read_only_permits_frame(ws_access, MessageType::SyncStep2) {
                                    counter::inc(MetricKey::new(
                                        "ws.readonly_write_rejected_total",
                                        &[("type", "sync2")],
                                    ));
                                    tracing::debug!(client_id, "dropping SyncStep2 from read-only session (#111)");
                                } else if ws_message_rate_limited(&state_for_recv, &user_id_for_recv).await {
                                    // #58/T-2: over the per-user persist rate —
                                    // close (break) so the client reconnects and
                                    // resyncs cleanly instead of losing frames.
                                    break;
                                } else {
                                    tracing::debug!(client_id, payload_len = payload.len(), "received SyncStep2 (applying, no broadcast)");
                                    if let Err(e) = room_for_recv
                                        .apply_update_gated(payload, liveapp_mode, liveapp_scope)
                                        .await
                                    {
                                        // Only the LiveApp gate produces a
                                        // signal we want the client to see;
                                        // real yrs decode / apply failures
                                        // are handled elsewhere. See #163
                                        // for the state-recovery follow-up
                                        // (Option C in the finding-#5 plan).
                                        if let ogrenotes_collab::document::DocError::LiveAppRejected(msg) = &e {
                                            let err_msg = encode_message(
                                                MessageType::Error,
                                                format!("liveapp-rejected:{msg}").as_bytes(),
                                            );
                                            room_for_recv
                                                .send_to_client(client_id, err_msg)
                                                .await;
                                        }
                                        tracing::debug!(client_id, error = %e, "SyncStep2 rejected by liveapp gate");
                                    }
                                }
                            }
                            MessageType::Update => {
                                update_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "update")]));
                                // #111: read-only sessions cannot mutate the
                                // doc — drop the Update before any apply /
                                // broadcast / publish / persist / activity
                                // write. Server-side enforcement: a captured
                                // read-only token can never escalate to write.
                                if !read_only_permits_frame(ws_access, MessageType::Update) {
                                    counter::inc(MetricKey::new(
                                        "ws.readonly_write_rejected_total",
                                        &[("type", "update")],
                                    ));
                                    tracing::debug!(client_id, "dropping Update from read-only session (#111)");
                                    continue;
                                }
                                // #58/T-2: bound the per-user persist rate.
                                // Close (break) on breach rather than drop —
                                // a dropped Update silently desyncs the
                                // client's CRDT; a close forces a clean resync.
                                if ws_message_rate_limited(&state_for_recv, &user_id_for_recv).await {
                                    break;
                                }
                                tracing::debug!(client_id, payload_len = payload.len(), "received Update");
                                let apply_start = std::time::Instant::now();
                                match room_for_recv
                                    .apply_update_gated(payload, liveapp_mode, liveapp_scope)
                                    .await
                                {
                                    Ok(report) => {
                                        // gap-003: emit a SecurityAudit
                                        // row per removed LiveApp sub-
                                        // node. Fire after broadcast so
                                        // the row lands even if the
                                        // audit write is slow.
                                        for del in &report.deletions {
                                            crate::routes::audit::record_security_event_by_actor(
                                                &state_for_recv,
                                                &user_id,
                                                &user_id,
                                                ogrenotes_storage::models::security_audit::SecurityAuditAction::LiveAppNodeDeleted {
                                                    doc_id: doc_id.clone(),
                                                    node_type: del.node_type.tag_name().to_string(),
                                                    block_id: del.block_id.clone(),
                                                },
                                            );
                                        }
                                        histogram::record(
                                            MetricKey::new("ws.update_apply_latency_ms", &[]),
                                            apply_start.elapsed().as_secs_f64() * 1000.0,
                                        );
                                        let num_clients = room_for_recv.client_count().await;
                                        tracing::debug!(client_id, num_clients, "applied update, broadcasting");
                                        let broadcast_msg = encode_message(MessageType::Update, payload);
                                        room_for_recv.broadcast(client_id, broadcast_msg.clone()).await;

                                        if let Err(e) = state.redis_pubsub.publish_update(&doc_id, &broadcast_msg).await {
                                            counter::inc(MetricKey::new("redis.publish_failures_total", &[]));
                                            tracing::warn!(error = %e, "redis publish failed");
                                        }

                                        let ts = ogrenotes_common::time::now_usec();
                                        let clock = format!("{}_{}", ts, nanoid::nanoid!(8));
                                        let update = ogrenotes_storage::models::document::DocUpdate {
                                            doc_id: doc_id.clone(),
                                            clock,
                                            update_bytes: payload.to_vec(),
                                            user_id: user_id.clone(),
                                            created_at: ts,
                                            client_version: client_version_for_recv.clone(),
                                        };
                                        // `DocRepo::append_update` internally routes
                                        // payloads above its inline-blob threshold to
                                        // S3 so DynamoDB's 400 KB item cap can't
                                        // silently swallow a large paste (#38). A
                                        // failure here is durable data loss — the
                                        // in-memory + broadcast change has already
                                        // happened and the reader will not see it on
                                        // reload — so we surface it as MSG_ERROR
                                        // instead of the historical log-and-drop.
                                        if let Err(e) = state_for_recv.doc_repo.append_update(&update).await {
                                            counter::inc(MetricKey::new(
                                                "ws.update_persist_failures_total",
                                                &[],
                                            ));
                                            counter::inc(MetricKey::new(
                                                "dynamo.write_failures_total",
                                                &[("op", "append_update")],
                                            ));
                                            tracing::error!(
                                                error = %e,
                                                doc_id = %doc_id,
                                                payload_len = payload.len(),
                                                "append_update failed — notifying client",
                                            );
                                            // Opaque payload — the full RepoError
                                            // (which can carry AWS SDK service-error
                                            // strings, bucket names, table names) is
                                            // already logged server-side at the
                                            // tracing::error above. The client only
                                            // needs the code to react.
                                            let err_msg = encode_message(
                                                MessageType::Error,
                                                b"persist-failed",
                                            );
                                            room_for_recv
                                                .send_to_client(client_id, err_msg)
                                                .await;
                                        }

                                        // Edit activity write — gated by the
                                        // shared debouncer so REST saves and
                                        // every WS session share one window
                                        // per (doc, user).
                                        if state.edit_activity_debouncer.try_record(&doc_id, &user_id, ts) {
                                            let activity_repo = state.activity_repo.clone();
                                            let act_doc_id = doc_id.clone();
                                            let act_user_id = user_id.clone();
                                            tokio::spawn(async move {
                                                let activity = ogrenotes_storage::models::activity::Activity {
                                                    activity_id: nanoid::nanoid!(16),
                                                    doc_id: act_doc_id,
                                                    event_type: ogrenotes_storage::models::activity::ActivityEventType::Edit,
                                                    actor_id: act_user_id,
                                                    detail: "{}".to_string(),
                                                    created_at: ts,
                                                };
                                                let _ = activity_repo.create(&activity).await;
                                            });
                                        }
                                    }
                                    Err(e) => {
                                        counter::inc(MetricKey::new("ws.update_apply_failures_total", &[]));
                                        tracing::warn!(client_id, error = %e, "failed to apply update");
                                        // Phase 2a Option-A signal: send an
                                        // error frame back on liveapp reject
                                        // so the frontend can show a "your
                                        // change wasn't saved" toast. The
                                        // client's local yrs still holds the
                                        // rejected write; without this
                                        // signal, the divergence would only
                                        // surface on refresh. See #163 for
                                        // the state-recovery follow-up
                                        // (Option C in the finding-#5 plan).
                                        if let ogrenotes_collab::document::DocError::LiveAppRejected(msg) = &e {
                                            let err_msg = encode_message(
                                                MessageType::Error,
                                                format!("liveapp-rejected:{msg}").as_bytes(),
                                            );
                                            room_for_recv
                                                .send_to_client(client_id, err_msg)
                                                .await;
                                        }
                                    }
                                }
                            }
                            MessageType::Ping => {
                                // Application-level keepalive — no broadcast,
                                // no persistence. The frame's mere existence
                                // resets the ALB idle timer; the counter just
                                // lets us verify in CloudWatch that the client
                                // heartbeat is flowing.
                                ping_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "ping")]));
                            }
                            MessageType::Awareness => {
                                awareness_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "awareness")]));
                                // Validate and overwrite user_id before broadcasting
                                // to prevent identity spoofing.
                                if let Some(mut awareness) = ogrenotes_collab::awareness::decode_awareness(payload) {
                                    awareness.user_id = user_id.clone();
                                    let validated = ogrenotes_collab::awareness::encode_awareness(&awareness);
                                    let broadcast_msg = encode_message(MessageType::Awareness, &validated);
                                    room_for_recv.broadcast(client_id, broadcast_msg).await;
                                    // Cache for snapshot-on-join so a future
                                    // reconnecting client sees this cursor
                                    // without having to wait for a move.
                                    room_for_recv.store_awareness(client_id, validated).await;
                                }
                            }
                            MessageType::SubscribeForeignDoc => {
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "subscribe_foreign")]));
                                let foreign_id = match std::str::from_utf8(payload) {
                                    Ok(s) => s.trim().to_string(),
                                    Err(_) => continue,
                                };
                                if foreign_id.is_empty() || foreign_id == doc_id { continue; }
                                // Hold the `foreign_subs` lock across the
                                // duplicate-check + cold-load + insert
                                // sequence. Two reasons:
                                //
                                // 1. Two SubscribeForeignDoc messages for
                                //    the same id arriving in close
                                //    succession on the same connection
                                //    would otherwise both pass the
                                //    `contains_key` check and both call
                                //    `add_client`, leaking the loser's
                                //    client handle when the second
                                //    `insert` overwrites the first.
                                //
                                // 2. The disconnect cleanup at the bottom
                                //    of `handle_ws` drains this map. By
                                //    inserting into `foreign_subs` BEFORE
                                //    calling `add_client`, a mid-flight
                                //    abort (recv task aborted between
                                //    `add_client` and `insert`) doesn't
                                //    leak — cleanup still finds the entry
                                //    and calls `remove_client`, which is
                                //    a HashMap::remove no-op if the
                                //    client was never actually inserted.
                                let mut subs = foreign_subs_for_recv.lock().await;
                                if subs.contains_key(&foreign_id) { continue; }
                                match ensure_foreign_room_loaded(
                                    &state_for_recv, &foreign_id, &user_id_for_recv,
                                ).await {
                                    Ok(foreign_room) => {
                                        let foreign_client_id = foreign_room.next_client_id();
                                        let (foreign_tx, mut foreign_rx) =
                                            tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
                                        let primary_tx = tx_for_foreign.clone();
                                        let foreign_id_for_task = foreign_id.clone();
                                        let forward_task = tokio::spawn(async move {
                                            while let Some(bytes) = foreign_rx.recv().await {
                                                // Foreign rooms broadcast already-encoded
                                                // messages — we only forward Updates, since
                                                // the client only needs CRDT data to refresh
                                                // the cross-doc cache. Awareness, comments,
                                                // etc. from foreign rooms are intentionally
                                                // dropped: the consumer doc isn't displaying
                                                // those collaborator cursors.
                                                if let Some((MessageType::Update, update_bytes)) =
                                                    decode_message(&bytes)
                                                {
                                                    let wrapped_payload = encode_foreign_doc_update(
                                                        &foreign_id_for_task, update_bytes,
                                                    );
                                                    let wrapped = encode_message(
                                                        MessageType::ForeignDocUpdate,
                                                        &wrapped_payload,
                                                    );
                                                    if primary_tx.send(wrapped).is_err() { break; }
                                                }
                                            }
                                        });
                                        // Insert tracking BEFORE add_client
                                        // so an abort during add_client
                                        // can be cleaned up by the
                                        // disconnect path. Clone the room
                                        // Arc since the entry consumes one.
                                        let foreign_room_for_add = Arc::clone(&foreign_room);
                                        subs.insert(
                                            foreign_id.clone(),
                                            ForeignSubscription {
                                                room: foreign_room,
                                                client_id: foreign_client_id,
                                                forward_task,
                                            },
                                        );
                                        drop(subs); // release before the add_client await
                                        foreign_room_for_add.add_client(
                                            foreign_client_id,
                                            user_id_for_recv.clone(),
                                            foreign_tx,
                                        ).await;
                                    }
                                    Err(_) => {
                                        // Forbidden / NotFound / etc. — surface as Error
                                        // frame so the client maps to #REF! locally.
                                        drop(subs);
                                        let err_msg = encode_message(
                                            MessageType::Error,
                                            format!("foreign-doc-subscribe-denied:{foreign_id}").as_bytes(),
                                        );
                                        room_for_recv.send_to_client(client_id, err_msg).await;
                                    }
                                }
                            }
                            MessageType::UnsubscribeForeignDoc => {
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "unsubscribe_foreign")]));
                                let foreign_id = match std::str::from_utf8(payload) {
                                    Ok(s) => s.trim().to_string(),
                                    Err(_) => continue,
                                };
                                if let Some(sub) = foreign_subs_for_recv.lock().await.remove(&foreign_id) {
                                    sub.forward_task.abort();
                                    sub.room.remove_client(sub.client_id).await;
                                }
                            }
                            _ => {
                                other_in_for_recv.fetch_add(1, Ordering::Relaxed);
                                counter::inc(MetricKey::new("ws.messages_total", &[("type", "other")]));
                            }
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
    room.forget_awareness(client_id).await;
    // #9: tell the remaining peers to drop this user's cursor, otherwise
    // it lingers frozen at its last position until they refresh. Skip if
    // the room is now empty (no one to notify) or if this user still has
    // another tab/connection here (multi-tab — their cursor is still
    // live). Awareness is local-broadcast only, so this matches its scope.
    if !is_empty && room.client_count_for_user(&user_id_for_log).await == 0 {
        let leave = encode_message(MessageType::AwarenessLeave, user_id_for_log.as_bytes());
        room.broadcast(client_id, leave).await;
    }
    // Drain any active foreign-doc subscriptions: abort the forward
    // tasks and remove this connection's client handle from each
    // foreign room. The forward channel closes when the foreign_tx
    // is dropped, but explicit `remove_client` triggers the room's
    // `remove_if_empty` accounting — important so foreign rooms
    // shed reference counts cleanly when the last subscriber leaves.
    let mut foreign = foreign_subs.lock().await;
    for (_, sub) in foreign.drain() {
        sub.forward_task.abort();
        sub.room.remove_client(sub.client_id).await;
    }
    drop(foreign);
    let duration_ms = connected_at.elapsed().as_millis() as u64;
    counter::inc(MetricKey::new("ws.disconnected_total", &[]));
    gauge::add(MetricKey::new("ws.active_connections", &[]), -1);
    histogram::record(
        MetricKey::new("ws.session_duration_ms", &[]),
        duration_ms as f64,
    );
    tracing::info!(
        event_type = "ws_disconnected",
        doc_id = room.doc_id(),
        client_id,
        user_id = %user_id_for_log,
        is_empty,
        duration_ms,
        messages_in = messages_in.load(Ordering::Relaxed),
        messages_out = messages_out.load(Ordering::Relaxed),
        sync1_in = sync1_in.load(Ordering::Relaxed),
        sync2_in = sync2_in.load(Ordering::Relaxed),
        update_in = update_in.load(Ordering::Relaxed),
        awareness_in = awareness_in.load(Ordering::Relaxed),
        ping_in = ping_in.load(Ordering::Relaxed),
        other_in = other_in.load(Ordering::Relaxed),
        "ws_client_disconnected"
    );

    if is_empty {
        // Last client left — snapshot + prune the op log if there are
        // pending updates, otherwise just drop the empty room. This is
        // what keeps a WS-only-edited doc's UPDATE# log from growing
        // without bound: the periodic compactor can't see a room once
        // it's removed here, so the pruning has to happen on the way out.
        crate::compaction::compact_or_remove_on_empty(
            &state.room_registry,
            &state.doc_repo,
            room.doc_id(),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with_origin(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::ORIGIN, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn read_only_session_is_denied_both_crdt_write_frames() {
        // #111: the two client→server CRDT write frames are the only frames
        // a read-only session may not send. This is the security invariant
        // the `handle_ws` Update/SyncStep2 arms enforce.
        assert!(!read_only_permits_frame(WsAccess::ReadOnly, MessageType::Update));
        assert!(!read_only_permits_frame(WsAccess::ReadOnly, MessageType::SyncStep2));
    }

    #[test]
    fn read_only_session_keeps_read_and_presence_frames() {
        // Read/presence frames must still flow for a read-only viewer —
        // otherwise they'd get no live updates (the whole point of #111).
        for mt in [
            MessageType::SyncStep1,
            MessageType::Ping,
            MessageType::Awareness,
            MessageType::SubscribeForeignDoc,
            MessageType::UnsubscribeForeignDoc,
        ] {
            assert!(
                read_only_permits_frame(WsAccess::ReadOnly, mt),
                "read-only session must still be allowed {mt:?}",
            );
        }
    }

    #[test]
    fn read_write_session_permits_every_frame() {
        // An Edit/Own session is never gated.
        for mt in [
            MessageType::SyncStep1,
            MessageType::SyncStep2,
            MessageType::Update,
            MessageType::Ping,
            MessageType::Awareness,
            MessageType::SubscribeForeignDoc,
            MessageType::UnsubscribeForeignDoc,
        ] {
            assert!(read_only_permits_frame(WsAccess::ReadWrite, mt));
        }
    }

    #[test]
    fn matching_origin_passes() {
        let headers = headers_with_origin("https://app.ogrenotes.example");
        assert!(validate_ws_origin(&headers, "https://app.ogrenotes.example", false).is_ok());
    }

    // ── #46: ws-token resolution from subprotocol vs query ──────────

    #[test]
    fn ws_token_prefers_subprotocol_over_query() {
        // When both are present, the subprotocol header wins — it's the
        // path that keeps the token out of the URL.
        let header = "ogrenotes-ws-token.SUBPROTO, ogrenotes-ws";
        assert_eq!(
            extract_ws_token(Some(header), Some("QUERY")).as_deref(),
            Some("SUBPROTO")
        );
    }

    #[test]
    fn ws_token_falls_back_to_query_for_legacy_clients() {
        // No subprotocol offered (older client) → use the query token.
        assert_eq!(
            extract_ws_token(None, Some("QUERY")).as_deref(),
            Some("QUERY")
        );
        // A subprotocol header without the token entry also falls back.
        assert_eq!(
            extract_ws_token(Some("ogrenotes-ws"), Some("QUERY")).as_deref(),
            Some("QUERY")
        );
    }

    #[test]
    fn ws_token_absent_or_empty_yields_none() {
        assert_eq!(extract_ws_token(None, None), None);
        assert_eq!(extract_ws_token(None, Some("")), None);
        // Empty token after the prefix is treated as absent, then the
        // (also empty) query falls through to None.
        assert_eq!(extract_ws_token(Some("ogrenotes-ws-token."), Some("")), None);
    }

    #[test]
    fn ws_token_parses_single_subprotocol_without_sentinel() {
        assert_eq!(
            extract_ws_token(Some("ogrenotes-ws-token.ONLY"), None).as_deref(),
            Some("ONLY")
        );
    }

    #[test]
    fn ws_subprotocol_constants_match_frontend_literals() {
        // #115 / cross-target schema agreement: the subprotocol strings are
        // mirrored as inline literals in frontend/src/collab/ws_client.rs
        // (the WASM target). A rename on one side only would silently drop
        // new clients to the legacy `?token=` path — which they no longer
        // send — so they'd fail to connect with no other signal. Assert the
        // frontend source still carries both constants so the drift fails
        // CI here on the backend side, the same way editor schema duality
        // is enforced.
        let frontend_src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../frontend/src/collab/ws_client.rs"
        ))
        .expect("frontend ws_client.rs must be readable from the api crate tests");

        assert!(
            frontend_src.contains(WS_TOKEN_SUBPROTOCOL_PREFIX),
            "frontend ws_client.rs must use the token-subprotocol prefix {WS_TOKEN_SUBPROTOCOL_PREFIX:?}"
        );
        // Quoted so the match is the sentinel literal itself, not the
        // `ogrenotes-ws` substring inside the token-prefix literal.
        assert!(
            frontend_src.contains(&format!("\"{WS_SENTINEL_SUBPROTOCOL}\"")),
            "frontend ws_client.rs must offer the sentinel subprotocol {WS_SENTINEL_SUBPROTOCOL:?}"
        );
    }

    #[test]
    fn mismatched_origin_rejected() {
        let headers = headers_with_origin("https://attacker.example");
        let err = validate_ws_origin(&headers, "https://app.ogrenotes.example", false)
            .expect_err("expected Origin mismatch to fail");
        assert!(matches!(err, ApiError::Forbidden));
    }

    #[test]
    fn missing_origin_rejected_in_production() {
        let headers = HeaderMap::new();
        let err = validate_ws_origin(&headers, "https://app.ogrenotes.example", false)
            .expect_err("expected missing Origin to fail in production");
        assert!(matches!(err, ApiError::Forbidden));
    }

    #[test]
    fn missing_origin_allowed_in_dev_mode() {
        let headers = HeaderMap::new();
        assert!(validate_ws_origin(&headers, "http://localhost:8080", true).is_ok());
    }

    #[test]
    fn dev_mode_accepts_any_origin() {
        // dev_mode also relaxes the strict-equality check (in addition to
        // tolerating a missing Origin) so that wasm-pack-served test
        // pages on randomly-assigned localhost ports and `trunk serve`
        // on a non-matching port can both upgrade. Mirrors the dev-mode
        // CORS policy (`AllowOrigin::mirror_request()` in `main.rs`).
        // Production stays strict via `dev_mode=false`, asserted by
        // `mismatched_origin_rejected` above.
        let headers = headers_with_origin("https://attacker.example");
        assert!(validate_ws_origin(&headers, "http://localhost:8080", true).is_ok());
    }

    #[test]
    fn non_utf8_origin_rejected() {
        let mut headers = HeaderMap::new();
        // Bytes 0x80-0xFF are legal HTTP header octets per the `http` crate
        // (HeaderValue::from_bytes accepts them) but are not valid UTF-8 when
        // read via to_str(), so this exercises the to_str() branch.
        headers.insert(
            axum::http::header::ORIGIN,
            HeaderValue::from_bytes(b"\x80\x81").unwrap(),
        );
        let err = validate_ws_origin(&headers, "https://app.ogrenotes.example", false)
            .expect_err("non-UTF-8 Origin must be rejected");
        assert!(matches!(err, ApiError::Forbidden));
    }

    #[test]
    fn null_origin_string_rejected() {
        // Browsers send the literal string "null" for sandboxed iframes,
        // data: URLs, and some file:// contexts. String equality against a
        // real frontend_origin must reject this.
        let headers = headers_with_origin("null");
        let err = validate_ws_origin(&headers, "https://app.ogrenotes.example", false)
            .expect_err("Origin: null must be rejected by string-equality");
        assert!(matches!(err, ApiError::Forbidden));
    }

    #[test]
    fn semver_prefix_accepts_typical_cargo_versions() {
        assert!(is_parseable_semver_prefix("0.1.2"));
        assert!(is_parseable_semver_prefix("1.0.0"));
        assert!(is_parseable_semver_prefix("12.34.56"));
        // Phase 2 may attach a build SHA suffix, but `client_version`
        // remains pure semver. The helper only validates the prefix
        // shape; anything strictly digits + dots passes.
        assert!(is_parseable_semver_prefix("0.1"));
    }

    #[test]
    fn semver_prefix_rejects_garbage() {
        assert!(!is_parseable_semver_prefix(""));
        assert!(!is_parseable_semver_prefix("v1.0.0"));      // leading v
        assert!(!is_parseable_semver_prefix("alpha"));
        assert!(!is_parseable_semver_prefix("1.0.0-rc1"));   // suffix not allowed by Phase 1 helper
        assert!(!is_parseable_semver_prefix(" 1.0.0"));      // leading whitespace
    }
}
