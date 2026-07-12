// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};

use ogrenotes_common::metrics::{counter, MetricKey};
use ogrenotes_storage::models::security_audit::SecurityAuditAction;

use crate::error::ApiError;
use crate::routes::audit::record_security_event;
use crate::state::AppState;

/// Pending OAuth flow state, persisted in Redis between `/auth/login`
/// and the provider callback under `oauth_flow:<state>` (#48). Holds the
/// PKCE code_verifier and the provider that minted the flow. Stored as
/// Redis rather than a process-local map so the callback can land on any
/// instance — a multi-task deploy previously failed when login started
/// on one task and the callback hit another. Expiry is the Redis TTL.
#[derive(Serialize, Deserialize)]
struct PendingFlow {
    code_verifier: String,
    provider: String,
}

/// TTL on a pending OAuth flow — bounds the login round-trip, generous
/// enough for a slow IdP consent screen.
const OAUTH_FLOW_TTL_SECS: u64 = 10 * 60;

/// Client-facing error message for any OAuth provider failure. The specific
/// cause (bad code, provider 5xx, parse error, network timeout) is logged
/// server-side but never included in the response — provider internals are
/// deployment-sensitive and leak nothing useful to the caller.
const GENERIC_OAUTH_ERR: &str = "OAuth provider error";

// ─── Refresh-token cookie (#33) ─────────────────────────────────
//
// The refresh token never reaches JS — it lives only in an
// `HttpOnly; Secure; SameSite=Strict` cookie scoped to /api/v1/auth.
// JS sees just the access token (15-min TTL); a future XSS that
// reads `localStorage` finds nothing.
//
// The cookie carries the (user_id, session_id, refresh_token) triple
// because rotate_refresh_token needs all three to find and rotate
// the right session row. A GSI by token-hash would let us drop two
// of those, but the cookie is still HttpOnly so embedding them is
// equivalent in confidentiality.

/// Cookie name. Keep stable — older sessions still send it on
/// upgrade if anyone else mints them.
const REFRESH_COOKIE_NAME: &str = "ogrenotes_refresh";
/// Cookie scope — only sent to auth endpoints. Keeps it off ALB
/// access logs for every other route.
const REFRESH_COOKIE_PATH: &str = "/api/v1/auth";
/// Cookie lifetime — matches the refresh-token TTL in `crates/auth`.
const REFRESH_COOKIE_MAX_AGE_SECS: u64 = 30 * 24 * 60 * 60;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshCookiePayload {
    user_id: String,
    session_id: String,
    refresh_token: String,
}

fn encode_refresh_cookie_value(
    user_id: &str,
    session_id: &str,
    refresh_token: &str,
) -> String {
    let payload = RefreshCookiePayload {
        user_id: user_id.to_string(),
        session_id: session_id.to_string(),
        refresh_token: refresh_token.to_string(),
    };
    let json = serde_json::to_vec(&payload).expect("infallible serialize");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn decode_refresh_cookie_value(value: &str) -> Option<RefreshCookiePayload> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Build the `Set-Cookie` header value for a successful login or refresh.
/// `dev_mode = true` drops the `Secure` flag so localhost HTTP works.
fn set_refresh_cookie_header(value: &str, dev_mode: bool) -> HeaderValue {
    let secure = if dev_mode { "" } else { "; Secure" };
    let s = format!(
        "{name}={value}; HttpOnly; SameSite=Strict; Path={path}; Max-Age={max_age}{secure}",
        name = REFRESH_COOKIE_NAME,
        path = REFRESH_COOKIE_PATH,
        max_age = REFRESH_COOKIE_MAX_AGE_SECS,
    );
    HeaderValue::from_str(&s).expect("ascii-only Set-Cookie value")
}

/// Build the `Set-Cookie` header value that clears the cookie on logout.
fn clear_refresh_cookie_header(dev_mode: bool) -> HeaderValue {
    let secure = if dev_mode { "" } else { "; Secure" };
    let s = format!(
        "{name}=; HttpOnly; SameSite=Strict; Path={path}; Max-Age=0{secure}",
        name = REFRESH_COOKIE_NAME,
        path = REFRESH_COOKIE_PATH,
    );
    HeaderValue::from_str(&s).expect("ascii-only Set-Cookie value")
}

/// Pull the refresh-cookie payload out of an inbound `Cookie` header.
/// Returns `None` if the cookie is missing or malformed — callers fall
/// back to the JSON-body path (transitional; will be removed once the
/// frontend ships the cookie-only flow).
fn read_refresh_cookie(headers: &HeaderMap) -> Option<RefreshCookiePayload> {
    let cookie_header = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    // The Cookie header carries `name1=v1; name2=v2; …`. RFC 6265 allows
    // multiple Cookie headers but axum coalesces; splitting on `;` is
    // sufficient for either shape.
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(rest) = pair.strip_prefix(REFRESH_COOKIE_NAME) {
            if let Some(value) = rest.strip_prefix('=') {
                return decode_refresh_cookie_value(value);
            }
        }
    }
    None
}

/// Caps on provider-supplied profile fields before they reach DynamoDB or
/// any log. These are generous — the point is to stop a malicious or
/// misbehaving provider from submitting MB-sized values, not to enforce
/// product rules.
pub(crate) const MAX_EMAIL_LEN: usize = 320; // RFC 5321
pub(crate) const MAX_NAME_LEN: usize = 256;
pub(crate) const MAX_AVATAR_URL_LEN: usize = 2048;

/// Truncate (not reject) UTF-8 fields on an `OAuthProfile` to their caps.
/// Truncation stops at a char boundary so we never produce invalid UTF-8.
fn sanitize_profile(profile: ogrenotes_auth::user::OAuthProfile) -> ogrenotes_auth::user::OAuthProfile {
    ogrenotes_auth::user::OAuthProfile {
        email: truncate_chars(profile.email, MAX_EMAIL_LEN),
        name: truncate_chars(profile.name, MAX_NAME_LEN),
        avatar_url: profile.avatar_url.map(|u| truncate_chars(u, MAX_AVATAR_URL_LEN)),
        provider: profile.provider,
        provider_subject_id: profile
            .provider_subject_id
            .map(|s| truncate_chars(s, MAX_NAME_LEN)),
    }
}

pub(crate) fn truncate_chars(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk back from max_bytes to the previous char boundary.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

pub fn router() -> Router<AppState> {
    let r = Router::new()
        // Legacy routes (default to GitHub)
        .route("/login", get(login_github))
        .route("/callback", get(callback))
        // Provider-specific routes
        .route("/login/{provider}", get(login_provider))
        .route("/callback/{provider}", get(callback_provider))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout));

    // `dev-login` is only compiled in when the feature is enabled.
    // Production builds with `--no-default-features` cannot reach this
    // handler at all — defence in depth over the runtime `config.dev_mode`
    // gate inside the handler itself.
    #[cfg(feature = "dev-login")]
    let r = r.route("/dev-login", post(dev_login));

    // Phase 4 M-E3: `/auth/mfa/*` lives in its own module.
    // Phase 4 M-E4: `/auth/saml/*` likewise.
    r.nest("/mfa", crate::routes::mfa::router())
        .nest("/saml", crate::routes::saml::router())
}

// ─── Login ─────────────────────────────────────────────────────

/// GET /auth/login — redirect to GitHub (legacy, backwards-compatible).
async fn login_github(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    enforce_login_rate_limit(&state, &headers).await?;
    start_oauth_flow(&state, "github").await
}

/// GET /auth/login/:provider — redirect to the chosen OAuth provider.
async fn login_provider(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    enforce_login_rate_limit(&state, &headers).await?;
    start_oauth_flow(&state, &provider).await
}

/// Per-IP rate limit on OAuth-init endpoints (#36). Mitigates a
/// pending-flow flooding attack: without it, an unbounded init loop
/// could write OAuth-state rows to Redis as fast as it can issue
/// requests. Each row is small and TTL-bounded (#48), but the rate
/// limit is the real guard against churn.
async fn enforce_login_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let ip = crate::middleware::rate_limit::ip_identifier(headers);
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "auth_login",
        &ip,
        state.config.rate_limit_auth_login_per_min,
        60,
    )
    .await
}

async fn start_oauth_flow(state: &AppState, provider: &str) -> Result<Response, ApiError> {
    let (authorize_url, client_id, scopes): (&str, &str, &[&str]) = match provider {
        "github" => (
            "https://github.com/login/oauth/authorize",
            &state.config.oauth_client_id,
            &["user:email"],
        ),
        "google" => {
            let client_id = state.config.google_client_id.as_deref().ok_or_else(|| {
                ApiError::BadRequest("Google OAuth is not configured".to_string())
            })?;
            (
                "https://accounts.google.com/o/oauth2/v2/auth",
                client_id,
                &["openid", "email", "profile"],
            )
        }
        _ => return Err(ApiError::BadRequest(format!("Unknown provider: {provider}"))),
    };

    let pkce = ogrenotes_auth::oauth::generate_pkce();
    let auth_state = ogrenotes_auth::oauth::generate_state();

    // Build the callback URI for this provider
    let redirect_uri = provider_callback_uri(&state.config.oauth_redirect_uri, provider);

    // Persist the pending flow in Redis keyed by the random state token.
    // Expiry is the TTL; the per-IP login rate limit (enforce_login_rate_limit)
    // bounds flood attempts, replacing the former 10k in-memory cap +
    // sweeper. Fail closed: if the store write fails the user can't
    // complete login, which is safer than redirecting to the IdP with no
    // way to validate the callback.
    let pending = PendingFlow {
        code_verifier: pkce.code_verifier.clone(),
        provider: provider.to_string(),
    };
    let pending_json = serde_json::to_string(&pending)
        .map_err(|_| ApiError::Internal("OAuth state encode".to_string()))?;
    state
        .redis_session
        .store_oauth_flow(&auth_state, &pending_json, OAUTH_FLOW_TTL_SECS)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, provider, "failed to store OAuth flow in Redis");
            ApiError::Internal("OAuth state store".to_string())
        })?;

    // Build the authorization URL. GitHub does not support PKCE, so we skip
    // the code_challenge for GitHub (the code_verifier is still stored but
    // won't be sent during token exchange).
    let url = if provider == "github" {
        // GitHub: no PKCE — just client_id, redirect_uri, state, scope
        format!(
            "{authorize_url}?\
             client_id={}&\
             redirect_uri={}&\
             response_type=code&\
             state={}&\
             scope={}",
            urlencoding::encode(client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(&auth_state),
            urlencoding::encode(&scopes.join(" ")),
        )
    } else {
        // Google and others: full PKCE
        ogrenotes_auth::oauth::build_authorization_url(
            authorize_url,
            client_id,
            &redirect_uri,
            &pkce,
            &auth_state,
            scopes,
        )
    };

    Ok(Redirect::temporary(&url).into_response())
}

/// Derive the callback URI for a given provider from the configured redirect URI.
/// GitHub uses the configured URI as-is (backwards compatible).
/// Other providers append `/{provider}` to the base URI.
fn provider_callback_uri(base_redirect_uri: &str, provider: &str) -> String {
    if provider == "github" {
        base_redirect_uri.to_string()
    } else {
        let base = base_redirect_uri.trim_end_matches('/');
        format!("{base}/{provider}")
    }
}

// ─── Callback ──────────────────────────────────────────────────

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
    /// Phase 4 M-E3: set to `Some(true)` when the user's default
    /// workspace has `mfa_required = true` and the user hasn't yet
    /// enrolled. The frontend uses this to redirect the just-logged-
    /// in user to the enrollment page before they can navigate
    /// elsewhere. Absent on every other path (skip_serializing_if)
    /// so the wire shape stays clean for the common case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_enrollment_required: Option<bool>,
    /// Phase 5 M-P2: the user's stored UI preferences, delivered on
    /// the auth response so the frontend applies locale/theme/a11y on
    /// boot without a separate /users/me fetch. Additive + optional;
    /// omitted when the user has no stored prefs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_prefs: Option<ogrenotes_storage::models::user::UiPrefs>,
}

/// Phase 4 M-E3: response shape for the OAuth/dev-login → MFA
/// challenge interstitial. Sent with HTTP 202 (Accepted) when the
/// user is enrolled in MFA and must complete the challenge step
/// before a session is issued. The handle is the opaque key the
/// frontend echoes to `POST /auth/mfa/challenge`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MfaPendingResponse {
    pub handle: String,
}

/// TTL on the Redis-backed MfaPending row. 60 seconds is long enough
/// for a user to switch windows to their authenticator and short
/// enough that an abandoned challenge expires quickly. Echoes
/// directly into the `Expiration::EX` argument on the SET.
const MFA_PENDING_TTL_SECS: u64 = 60;

/// Mint an opaque 32-byte handle (base64-url-no-pad). 256 bits of
/// entropy makes guessing computationally implausible within the 60s
/// window even if the attacker can fire many parallel
/// `/auth/mfa/challenge` requests.
fn mint_mfa_handle() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let bytes: [u8; 32] = rand::random();
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Mint a one-shot MFA challenge handle, persist it to Redis, and
/// return a 302 redirect to the frontend's MFA challenge page.
/// Shared by every primary-auth flow (OAuth callback + SAML ACS)
/// when the user has enrolled MFA — the second factor must verify
/// before any session is minted.
///
/// `Err` is a raw Redis storage failure; the caller decides how to
/// map it (OAuth → Internal, SAML → Unauthorized to fail closed).
/// The caller is also responsible for logging with its own context
/// (the helper does not know whether the trigger was OAuth or SAML,
/// nor which workspace if any).
pub(crate) async fn redirect_to_mfa_challenge(
    state: &AppState,
    user_id: &str,
) -> Result<Response, fred::error::RedisError> {
    let handle = mint_mfa_handle();
    state
        .redis_session
        .store_mfa_pending(&handle, user_id, MFA_PENDING_TTL_SECS)
        .await?;
    let redirect_url = format!(
        "{}/auth/mfa-challenge?handle={}",
        state.config.frontend_origin,
        urlencoding::encode(&handle),
    );
    Ok(Redirect::temporary(&redirect_url).into_response())
}

/// The closed set of auth methods written to the Session row's
/// `auth_method` field. Typed (rather than a `&str`) so a future
/// caller can't write `"github_oauth"` (underscore) and silently
/// emit a row that diverges from every other in the same table —
/// admin-audit grep / metric labels expect stable tags.
///
/// Two values are out-of-scope for `issue_session_response`
/// because their handlers predate it: `"dev-login"` is built
/// inline in `dev_login`, and `"<provider>-oauth"` is built inline
/// in `handle_callback`. When those paths are folded into the
/// helper, extend this enum to include them.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SessionSource {
    /// User completed the MFA challenge step with a TOTP code.
    MfaTotp,
    /// User completed the MFA challenge with a single-use recovery
    /// code.
    MfaRecovery,
    /// User authenticated via a workspace's SAML IdP (Phase 4
    /// M-E4). `auth_method` on the Session row carries this tag so
    /// admin-audit / log-shipping pipelines can distinguish SAML
    /// sessions from OAuth sessions.
    Saml,
}

impl SessionSource {
    fn as_str(self) -> &'static str {
        match self {
            SessionSource::MfaTotp => "mfa-totp",
            SessionSource::MfaRecovery => "mfa-recovery",
            SessionSource::Saml => "saml",
        }
    }
}

/// Build the full post-login response (session row, access JWT,
/// refresh cookie, TokenResponse body) for a successfully-
/// authenticated user. The MFA challenge + recovery handlers call
/// this after the second factor verifies; dev_login + the OAuth
/// callback don't use it yet (their inline paths predate this
/// helper) — future cleanup could fold them in.
pub(crate) async fn issue_session_response(
    state: &AppState,
    user: &ogrenotes_storage::models::user::User,
    source: SessionSource,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;

    let session = ogrenotes_auth::session::create_session(
        &state.session_repo,
        &user.user_id,
        Some(source.as_str()),
    )
    .await?;

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    let cookie_value = encode_refresh_cookie_value(
        &user.user_id,
        &session.session_id,
        &session.refresh_token,
    );
    let set_cookie = set_refresh_cookie_header(&cookie_value, state.config.dev_mode);

    let mfa_enrollment_required =
        crate::auth_policy::mfa_enrollment_required_for(state, user).await;

    Ok((
        [(axum::http::header::SET_COOKIE, set_cookie)],
        Json(TokenResponse {
            access_token,
            refresh_token: session.refresh_token,
            session_id: session.session_id,
            user_id: user.user_id.clone(),
            email: user.email.clone(),
            name: user.name.clone(),
            mfa_enrollment_required,
            ui_prefs: user.ui_prefs.clone(),
        }),
    )
        .into_response())
}

/// GET /auth/callback — handle OAuth callback (detects provider from stored state).
async fn callback(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
) -> Result<Response, ApiError> {
    handle_callback(state, params).await
}

/// GET /auth/callback/:provider — handle OAuth callback (validates provider matches stored flow).
async fn callback_provider(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    axum::extract::Query(params): axum::extract::Query<CallbackParams>,
) -> Result<Response, ApiError> {
    // Validate that the provider in the URL matches the one stored in the
    // pending flow. Prevents cross-provider state replay attacks. Peek
    // (don't consume) here — handle_callback does the single-use take.
    match state.redis_session.peek_oauth_flow(&params.state).await {
        Ok(Some(flow_json)) => {
            let flow: PendingFlow = serde_json::from_str(&flow_json).map_err(|_| {
                ApiError::BadRequest("Invalid or expired state parameter".to_string())
            })?;
            if flow.provider != provider {
                return Err(ApiError::BadRequest("Provider mismatch".to_string()));
            }
        }
        Ok(None) => {
            return Err(ApiError::BadRequest(
                "Invalid or expired state parameter".to_string(),
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "failed to read OAuth flow from Redis");
            return Err(ApiError::Internal("OAuth state store".to_string()));
        }
    }
    handle_callback(state, params).await
}

async fn handle_callback(state: AppState, params: CallbackParams) -> Result<Response, ApiError> {
    let (code_verifier, provider) = {
        // Single-use take: GETDEL atomically reads and erases the flow so
        // two parallel callbacks for one state can't both proceed.
        let flow_json = state
            .redis_session
            .take_oauth_flow(&params.state)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "failed to take OAuth flow from Redis");
                ApiError::Internal("OAuth state store".to_string())
            })?
            .ok_or_else(|| {
                counter::inc(MetricKey::new(
                    "user.login_total",
                    &[("provider", "unknown"), ("result", "fail")],
                ));
                ApiError::BadRequest("Invalid or expired state parameter".to_string())
            })?;
        let flow: PendingFlow = serde_json::from_str(&flow_json).map_err(|_| {
            ApiError::BadRequest("Invalid or expired state parameter".to_string())
        })?;
        (flow.code_verifier, flow.provider)
    };

    let redirect_uri = provider_callback_uri(&state.config.oauth_redirect_uri, &provider);

    // Exchange code for access token and fetch profile (provider-specific)
    let profile = match provider.as_str() {
        "github" => {
            let token = exchange_github_token(
                &params.code,
                &code_verifier,
                &state.config.oauth_client_id,
                &state.config.oauth_client_secret,
                &redirect_uri,
            )
            .await?;
            fetch_github_profile(&token).await?
        }
        "google" => {
            let client_id = state.config.google_client_id.as_deref().ok_or_else(|| {
                ApiError::Internal("Google OAuth not configured".to_string())
            })?;
            let client_secret =
                state.config.google_client_secret.as_deref().ok_or_else(|| {
                    ApiError::Internal("Google OAuth not configured".to_string())
                })?;
            let token = exchange_google_token(
                &params.code,
                &code_verifier,
                client_id,
                client_secret,
                &redirect_uri,
            )
            .await?;
            fetch_google_profile(&token).await?
        }
        _ => return Err(ApiError::Internal(format!("Unknown provider: {provider}"))),
    };

    // Shared post-login logic: find/create user, check disabled, auto-admin, create session.
    // Profile fields are truncated before reaching storage — a misbehaving
    // provider cannot push MB-sized values into DynamoDB or logs.
    let auth_provider = match provider.as_str() {
        "github" => ogrenotes_storage::models::user::AuthProvider::Github,
        "google" => ogrenotes_storage::models::user::AuthProvider::Google,
        _ => return Err(ApiError::Internal(format!("Unknown provider: {provider}"))),
    };
    let oauth_profile = sanitize_profile(ogrenotes_auth::user::OAuthProfile {
        email: profile.email,
        name: profile.name,
        avatar_url: profile.avatar_url,
        provider: auth_provider,
        // Provider subject id (GitHub numeric id / Google sub) — find_or_create_user
        // compares it on same-provider logins to catch email reassignment.
        provider_subject_id: profile.subject_id,
    });

    let mut user = ogrenotes_auth::user::find_or_create_user(
        &state.user_repo,
        &state.folder_repo,
        &state.workspace_repo,
        &oauth_profile,
    )
    .await?;

    if user.is_disabled {
        record_security_event(
            &state,
            &user.user_id,
            SecurityAuditAction::LoginFailure {
                reason: "disabled".to_string(),
            },
        );
        return Err(ApiError::Forbidden);
    }

    crate::auth_policy::apply_admin_email_promotion(&state, &mut user).await;

    // gap-003: a cross-provider link — this login authenticated via
    // `auth_provider` but resolved to an existing account created via a
    // different OAuth provider. find_or_create_user allows it (both providers
    // verify the email), but record it distinctly so the merge isn't invisible
    // behind a plain LoginSuccess. Emitted before the MFA branch so it's
    // captured even when an MFA challenge follows.
    if user.provider != auth_provider {
        let prov =
            |p: ogrenotes_storage::models::user::AuthProvider| format!("{p:?}").to_lowercase();
        record_security_event(
            &state,
            &user.user_id,
            SecurityAuditAction::AccountLinked {
                from_provider: prov(user.provider),
                to_provider: prov(auth_provider),
            },
        );
    }

    // Phase 4 M-E3: enrolled-MFA users go through the challenge step
    // before a session is minted. Redirect to a frontend route that
    // can read the handle from the query string and post it to
    // /auth/mfa/challenge — no cookie is set yet (the user isn't
    // authenticated until the second factor verifies). The shared
    // helper does the same mint+store+redirect on the SAML path.
    //
    // No LoginSuccess audit yet — the MFA challenge has to pass first.
    // The MFA challenge handler emits MfaVerify { ok: true } which
    // serves the same forensic purpose.
    if user.mfa_enrolled_at.is_some() {
        return redirect_to_mfa_challenge(&state, &user.user_id).await.map_err(|e| {
            tracing::error!(error = %e, user_id = %user.user_id, "OAuth: failed to store MFA pending state");
            ApiError::Internal("MFA handoff".to_string())
        });
    }

    record_security_event(
        &state,
        &user.user_id,
        SecurityAuditAction::LoginSuccess,
    );

    let session = ogrenotes_auth::session::create_session(
        &state.session_repo,
        &user.user_id,
        Some(&format!("{provider}-oauth")),
    )
    .await?;

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    // The redirect retains the URL-fragment payload for transitional
    // compatibility with frontends that still read tokens from the
    // hash. The cookie below is the new path; once the frontend ships
    // the cookie-only flow, drop refresh_token from the fragment.
    let redirect_url = format!(
        "{}/auth/complete#access_token={}&refresh_token={}&session_id={}&user_id={}&email={}&name={}",
        state.config.frontend_origin,
        urlencoding::encode(&access_token),
        urlencoding::encode(&session.refresh_token),
        urlencoding::encode(&session.session_id),
        urlencoding::encode(&user.user_id),
        urlencoding::encode(&user.email),
        urlencoding::encode(&user.name),
    );

    let cookie_value = encode_refresh_cookie_value(
        &user.user_id,
        &session.session_id,
        &session.refresh_token,
    );
    let set_cookie = set_refresh_cookie_header(&cookie_value, state.config.dev_mode);

    counter::inc(MetricKey::new(
        "user.login_total",
        &[("provider", provider.as_str()), ("result", "ok")],
    ));
    tracing::info!(
        event_type = "user_login",
        provider = %provider,
        user_id = %user.user_id,
        "user logged in"
    );

    Ok((
        [(axum::http::header::SET_COOKIE, set_cookie)],
        Redirect::temporary(&redirect_url),
    )
        .into_response())
}

// ─── Refresh + Logout ──────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest {
    /// Optional only because the cookie path doesn't carry a body.
    /// When the cookie is present and valid it overrides the body,
    /// even if the body also carries a (possibly stale) token.
    refresh_token: Option<String>,
    user_id: Option<String>,
    session_id: Option<String>,
}

/// POST /auth/refresh — refresh access token and rotate refresh token.
///
/// The cookie path (#33) is preferred: when the request carries a
/// valid `ogrenotes_refresh` cookie we read (user_id, session_id,
/// refresh_token) from it and ignore any body. The body path remains
/// for transitional clients still on the localStorage flow; it will
/// be removed once the frontend ships cookie-only.
async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<RefreshRequest>>,
) -> Result<Response, ApiError> {
    // Per-IP rate limit (#36). A leaked refresh cookie / token can
    // otherwise be replayed against this endpoint until rotation
    // completes; the limit caps replay attempts per minute.
    let ip = crate::middleware::rate_limit::ip_identifier(&headers);
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "auth_refresh",
        &ip,
        state.config.rate_limit_auth_refresh_per_min,
        60,
    )
    .await?;

    // Resolve the credentials. Cookie wins over body. Logging when
    // both are present helps us tell when the body path is safe to
    // remove (during the transition window the frontend may send
    // both for a release or two).
    let (user_id, session_id, refresh_token) = match read_refresh_cookie(&headers) {
        Some(c) => {
            if let Some(Json(b)) = body.as_ref() {
                if b.refresh_token.is_some() || b.user_id.is_some() || b.session_id.is_some() {
                    tracing::debug!(
                        "refresh: cookie present with non-empty body fields — body ignored (transitional)"
                    );
                }
            }
            (c.user_id, c.session_id, c.refresh_token)
        }
        None => match body.map(|Json(r)| r).unwrap_or_default() {
            RefreshRequest {
                refresh_token: Some(rt),
                user_id: Some(uid),
                session_id: Some(sid),
            } => (uid, sid, rt),
            _ => return Err(ApiError::Unauthorized),
        },
    };

    let new_refresh = ogrenotes_auth::session::rotate_refresh_token(
        &state.session_repo,
        &user_id,
        &session_id,
        &refresh_token,
    )
    .await
    .map_err(|e| {
        counter::inc(MetricKey::new(
            "user.refresh_total",
            &[("result", "fail")],
        ));
        // Refresh-token reuse is the high-signal event in this
        // branch — it means an old refresh token (already rotated
        // server-side) was presented again, which is either a bug
        // or a stolen-token replay. The L3 session helper has
        // already revoked every session for this user; we add the
        // durable audit row here at L4 because L3 doesn't have
        // SecurityAuditRepo wired through.
        if matches!(e, ogrenotes_auth::jwt::AuthError::RefreshTokenReused) {
            record_security_event(
                &state,
                &user_id,
                SecurityAuditAction::SessionRevoked {
                    reason: "refresh_reuse_detected".to_string(),
                },
            );
        }
        e
    })?;

    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|_| ApiError::Unauthorized)?
        .ok_or(ApiError::Unauthorized)?;

    counter::inc(MetricKey::new(
        "user.refresh_total",
        &[("result", "ok")],
    ));

    let access_token = ogrenotes_auth::jwt::create_access_token(
        &user.user_id,
        &user.email,
        &state.config.jwt_secret,
    )?;

    // Rotate the cookie alongside the rotated token. Always re-issue
    // it so old cookies (post-rotation) become invalid even if the
    // browser caches them — the underlying refresh-token hash has
    // already been replaced server-side by rotate_refresh_token.
    let cookie_value =
        encode_refresh_cookie_value(&user.user_id, &session_id, &new_refresh);
    let set_cookie = set_refresh_cookie_header(&cookie_value, state.config.dev_mode);

    Ok((
        [(axum::http::header::SET_COOKIE, set_cookie)],
        Json(TokenResponse {
            access_token,
            refresh_token: new_refresh,
            session_id,
            user_id: user.user_id,
            email: user.email,
            name: user.name,
            // Refresh doesn't re-trigger enrollment flow. A
            // workspace that flips `mfa_required = true` after the
            // session was minted is enforced on the NEXT login, not
            // mid-session — matches the plan's "must enroll on next
            // login" semantics.
            mfa_enrollment_required: None,
            ui_prefs: user.ui_prefs.clone(),
        }),
    )
        .into_response())
}

/// POST /auth/logout — revoke session and clear the refresh cookie.
///
/// Accepts either a Bearer access token (the legacy/normal path) OR
/// the refresh cookie alone. The cookie-only fallback exists because
/// in the cookie-only frontend flow a browser restart loses the JS-
/// memory access token but retains the cookie — without this fallback
/// such a session would be unable to log itself out.
///
/// Logout intentionally bypasses the `AuthUser` extractor's live
/// User-row check (disabled / not-found): revoking all sessions for
/// a user_id is idempotent and safe even when the row no longer
/// reflects an active account.
async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // Bearer wins over cookie when both are present — same precedence
    // as `refresh`. JWT validation here is a sanity check; we extract
    // the subject and use it to revoke. A valid signature is enough,
    // we don't need to confirm the user still exists.
    let user_id = if let Some(token) = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if token.is_empty() || token.len() > 4096 {
            return Err(ApiError::Unauthorized);
        }
        let claims = ogrenotes_auth::jwt::validate_token(token, &state.config.jwt_secret)
            .map_err(|_| ApiError::Unauthorized)?;
        claims.sub
    } else {
        read_refresh_cookie(&headers)
            .ok_or(ApiError::Unauthorized)?
            .user_id
    };

    state
        .session_repo
        .delete_all_for_user(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    counter::inc(MetricKey::new("user.logout_total", &[]));
    tracing::info!(event_type = "user_logout", %user_id, "user logged out");

    // Clear the refresh cookie via Max-Age=0 so the browser drops it
    // immediately. Without this, the cookie would linger until its
    // 30-day Max-Age and (post-revocation) would 401 every refresh
    // until then — surprising UX even though it's not a security gap.
    let clear = clear_refresh_cookie_header(state.config.dev_mode);
    Ok((
        [(axum::http::header::SET_COOKIE, clear)],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

// ─── Dev Login ─────────────────────────────────────────────────

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

/// Dev-login rate limit: fixed-window, 100 attempted logins per minute
/// keyed by `X-Forwarded-For` (one global "unknown" bucket when the
/// header is absent). This is deliberately cheap and coarse — the real
/// protection is the `dev-login` cargo feature gate and the runtime
/// `config.dev_mode` check; the limiter's job is just to cap damage to
/// (user-creation-spam, not full DoS) if both of those fail.
///
/// 100/min is well above what a test suite produces and well below what
/// an attacker trying to spray-create accounts would do.
// M-E7 item 10 — dev-login rate limit consolidated onto the
// Redis-backed `rate_limit::enforce` module. Previously a process-
// local DashMap counter; the new shape uses the same machinery as
// every other gate so cap + window + metric tags are uniform.

/// POST /auth/dev-login — bypass OAuth for local development.
#[cfg(feature = "dev-login")]
async fn dev_login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<DevLoginRequest>,
) -> Result<Response, ApiError> {
    if !state.config.dev_mode {
        return Err(ApiError::NotFound("Not found".to_string()));
    }

    // Rate-limit per X-Forwarded-For (best-effort; "unknown" bucket
    // when the header is missing). Now routed through the shared
    // `rate_limit::enforce` module — see the M-E7 piece above for
    // the consolidation rationale.
    let ip_key = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "dev_login",
        &ip_key,
        state.config.rate_limit_dev_login_per_min,
        60,
    )
    .await?;
    let profile = sanitize_profile(ogrenotes_auth::user::OAuthProfile {
        email: req.email,
        name: req.name,
        avatar_url: None,
        provider: ogrenotes_storage::models::user::AuthProvider::Dev,
        provider_subject_id: None,
    });

    let mut user = ogrenotes_auth::user::find_or_create_user(
        &state.user_repo,
        &state.folder_repo,
        &state.workspace_repo,
        &profile,
    )
    .await?;

    if user.is_disabled {
        record_security_event(
            &state,
            &user.user_id,
            SecurityAuditAction::LoginFailure {
                reason: "disabled".to_string(),
            },
        );
        return Err(ApiError::Forbidden);
    }

    crate::auth_policy::apply_admin_email_promotion(&state, &mut user).await;

    // dev-login is the test/dev shortcut; auto-open the ask
    // policy to `SystemOrByok` so existing test suites and local
    // exploration don't trip the admin-controlled gate.
    // Production OAuth users still default to `Disabled` and
    // need an explicit admin flip via
    // PUT /api/v1/admin/users/:id/ask-policy.
    use ogrenotes_storage::models::user::AskPolicy;
    if user.ask_policy() == AskPolicy::Disabled {
        user.ask_policy = Some(AskPolicy::SystemOrByok);
        let _ = state
            .user_repo
            .set_ask_policy(&user.user_id, AskPolicy::SystemOrByok)
            .await;
    }

    // Phase 4 M-E3: if the user has completed MFA enrollment, hand
    // off to the challenge step instead of minting a session. The
    // pending-MFA row holds the user_id under an opaque handle for
    // 60s; the frontend echoes the handle to POST /auth/mfa/challenge.
    // Tests that don't want this branch should leave mfa_enrolled_at
    // None on the dev-login subject.
    if user.mfa_enrolled_at.is_some() {
        let handle = mint_mfa_handle();
        if let Err(e) = state
            .redis_session
            .store_mfa_pending(&handle, &user.user_id, MFA_PENDING_TTL_SECS)
            .await
        {
            tracing::error!(error = %e, "failed to store MFA pending state");
            return Err(ApiError::Internal("MFA handoff".to_string()));
        }
        return Ok((
            axum::http::StatusCode::ACCEPTED,
            Json(MfaPendingResponse { handle }),
        )
            .into_response());
    }

    record_security_event(
        &state,
        &user.user_id,
        SecurityAuditAction::LoginSuccess,
    );

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

    let cookie_value = encode_refresh_cookie_value(
        &user.user_id,
        &session.session_id,
        &session.refresh_token,
    );
    let set_cookie = set_refresh_cookie_header(&cookie_value, state.config.dev_mode);

    let mfa_enrollment_required =
        crate::auth_policy::mfa_enrollment_required_for(&state, &user).await;

    Ok((
        [(axum::http::header::SET_COOKIE, set_cookie)],
        Json(TokenResponse {
            access_token,
            refresh_token: session.refresh_token,
            session_id: session.session_id,
            user_id: user.user_id,
            email: user.email,
            name: user.name,
            mfa_enrollment_required,
            ui_prefs: user.ui_prefs.clone(),
        }),
    )
        .into_response())
}

// ─── Provider: GitHub ──────────────────────────────────────────

struct OAuthProfile {
    email: String,
    name: String,
    avatar_url: Option<String>,
    /// Provider-stable subject id (GitHub numeric id, Google `sub`/`id`).
    /// Threaded to `find_or_create_user` to detect email reassignment.
    subject_id: Option<String>,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

async fn exchange_github_token(
    code: &str,
    _code_verifier: &str, // GitHub does not support PKCE; kept for API symmetry
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
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "GitHub token exchange transport error");
            ApiError::Internal(GENERIC_OAUTH_ERR.to_string())
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(
            status = %status,
            body = %body,
            "GitHub token exchange non-2xx"
        );
        return Err(ApiError::Internal(GENERIC_OAUTH_ERR.to_string()));
    }

    let token_resp: GitHubTokenResponse = resp.json().await.map_err(|e| {
        tracing::warn!(error = %e, "failed to parse GitHub token response");
        ApiError::Internal(GENERIC_OAUTH_ERR.to_string())
    })?;

    Ok(token_resp.access_token)
}

#[derive(Deserialize)]
struct GitHubUser {
    /// Stable numeric account id — the GitHub OAuth subject.
    id: i64,
    // `email` is intentionally not read: the address is always resolved from
    // /user/emails with a verified check (security review gap-001).
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

async fn fetch_github_profile(access_token: &str) -> Result<OAuthProfile, ApiError> {
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

    // Always resolve the email from /user/emails and require it be GitHub-
    // verified — never trust the public `user.email` field unconditionally.
    // With account linking, an unverified address could otherwise merge into an
    // existing account (security review gap-001). Needs the `user:email` scope.
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

    let email = emails
        .iter()
        .find(|e| e.primary && e.verified)
        .or_else(|| emails.iter().find(|e| e.verified))
        .map(|e| e.email.clone())
        .ok_or_else(|| ApiError::BadRequest("No verified email found".to_string()))?;

    Ok(OAuthProfile {
        email,
        name: user.name.unwrap_or(user.login),
        avatar_url: user.avatar_url,
        subject_id: Some(user.id.to_string()),
    })
}

// ─── Provider: Google ──────────────────────────────────────────

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

async fn exchange_google_token(
    code: &str,
    code_verifier: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<String, ApiError> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("code_verifier", code_verifier),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "Google token exchange transport error");
            ApiError::Internal(GENERIC_OAUTH_ERR.to_string())
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(
            status = %status,
            body = %body,
            "Google token exchange non-2xx"
        );
        return Err(ApiError::Internal(GENERIC_OAUTH_ERR.to_string()));
    }

    let token_resp: GoogleTokenResponse = resp.json().await.map_err(|e| {
        tracing::warn!(error = %e, "failed to parse Google token response");
        ApiError::Internal(GENERIC_OAUTH_ERR.to_string())
    })?;

    Ok(token_resp.access_token)
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    email: Option<String>,
    // Stable subject id: the v2 userinfo endpoint returns it as `id`; the OIDC
    // userinfo endpoint calls it `sub`. Accept either.
    #[serde(alias = "sub")]
    id: Option<String>,
    // The OAuth2 v2 userinfo endpoint (used below) returns this as
    // `verified_email`; the OIDC userinfo endpoint calls it `email_verified`.
    // Accept either so the verified-email check doesn't reject every user.
    #[serde(alias = "verified_email")]
    email_verified: Option<bool>,
    name: Option<String>,
    picture: Option<String>,
}

async fn fetch_google_profile(access_token: &str) -> Result<OAuthProfile, ApiError> {
    let client = reqwest::Client::new();

    let user: GoogleUserInfo = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Google userinfo fetch failed: {e}")))?
        .json()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to parse Google userinfo: {e}")))?;

    if user.email_verified != Some(true) {
        return Err(ApiError::BadRequest(
            "Google email is not verified".to_string(),
        ));
    }

    let email = user
        .email
        .ok_or_else(|| ApiError::BadRequest("No email in Google profile".to_string()))?;

    Ok(OAuthProfile {
        email,
        name: user.name.unwrap_or_else(|| "Google User".to_string()),
        avatar_url: user.picture,
        subject_id: user.id,
    })
}

#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn refresh_cookie_roundtrip() {
        let v = encode_refresh_cookie_value("user-123", "sess-abc", "REFRESH-TOKEN-PAYLOAD");
        let decoded = decode_refresh_cookie_value(&v).expect("must round-trip");
        assert_eq!(decoded.user_id, "user-123");
        assert_eq!(decoded.session_id, "sess-abc");
        assert_eq!(decoded.refresh_token, "REFRESH-TOKEN-PAYLOAD");
    }

    #[test]
    fn google_userinfo_maps_v2_verified_email_field() {
        // Regression: the OAuth2 v2 userinfo endpoint returns `verified_email`,
        // but the struct field is `email_verified`. Without the serde alias the
        // field was always None, rejecting every Google login with
        // "Google email is not verified".
        let v2 = serde_json::json!({
            "email": "u@example.com",
            "verified_email": true,
            "name": "U",
            "picture": "https://example.com/p.png",
        });
        let parsed: GoogleUserInfo = serde_json::from_value(v2).unwrap();
        assert_eq!(parsed.email_verified, Some(true));

        // OIDC userinfo uses `email_verified` directly — must still work.
        let oidc = serde_json::json!({ "email": "u@example.com", "email_verified": true });
        let parsed: GoogleUserInfo = serde_json::from_value(oidc).unwrap();
        assert_eq!(parsed.email_verified, Some(true));
    }

    #[test]
    fn malformed_cookie_decodes_to_none() {
        assert!(decode_refresh_cookie_value("not-base64-!@#$%").is_none());
        // Valid base64 but not valid JSON
        let bogus = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"not json");
        assert!(decode_refresh_cookie_value(&bogus).is_none());
    }

    #[test]
    fn set_cookie_header_omits_secure_in_dev_mode() {
        let v = "abc";
        let dev = set_refresh_cookie_header(v, true);
        let prod = set_refresh_cookie_header(v, false);
        assert!(!dev.to_str().unwrap().contains("Secure"));
        assert!(prod.to_str().unwrap().contains("Secure"));
        // HttpOnly and SameSite=Strict always present.
        for h in [&dev, &prod] {
            let s = h.to_str().unwrap();
            assert!(s.contains("HttpOnly"), "missing HttpOnly: {s}");
            assert!(s.contains("SameSite=Strict"), "missing SameSite=Strict: {s}");
            assert!(s.contains("Path=/api/v1/auth"), "wrong Path: {s}");
        }
    }

    #[test]
    fn clear_cookie_header_uses_max_age_zero() {
        let h = clear_refresh_cookie_header(false);
        let s = h.to_str().unwrap();
        assert!(s.contains("Max-Age=0"), "missing Max-Age=0: {s}");
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("SameSite=Strict"));
        assert!(s.contains("Secure"));
    }

    #[test]
    fn read_refresh_cookie_picks_correct_value_among_others() {
        let mut headers = HeaderMap::new();
        let payload =
            encode_refresh_cookie_value("user-x", "sess-y", "tok-z");
        let cookie = format!(
            "other_thing=hello; {name}={payload}; another=bye",
            name = REFRESH_COOKIE_NAME,
        );
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&cookie).unwrap(),
        );
        let got = read_refresh_cookie(&headers).expect("must find cookie");
        assert_eq!(got.user_id, "user-x");
        assert_eq!(got.refresh_token, "tok-z");
    }

    #[test]
    fn read_refresh_cookie_returns_none_for_missing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("session=foo; theme=dark"),
        );
        assert!(read_refresh_cookie(&headers).is_none());
    }

    #[test]
    fn read_refresh_cookie_does_not_match_prefix_collisions() {
        // A cookie named `ogrenotes_refresh_other` must not be read as
        // the refresh cookie. Without the explicit `=` check below the
        // strip_prefix would silently match.
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("ogrenotes_refresh_other=trickery"),
        );
        assert!(read_refresh_cookie(&headers).is_none());
    }

    #[test]
    fn read_refresh_cookie_only_searches_first_cookie_header() {
        // Documents the contract: HTTP/1.1 allows multiple `Cookie`
        // headers but RFC 6265 says clients SHOULD send one, and HTTP/2
        // requires it. Browsers always coalesce. axum's `HeaderMap::get`
        // returns only the first value, so a payload arriving in the
        // *second* header would be missed — this test pins that
        // behavior so a future regression (e.g. switching to
        // `get_all().iter()`) is intentional.
        let payload = encode_refresh_cookie_value("u", "s", "t");
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("other=foo"),
        );
        headers.append(
            axum::http::header::COOKIE,
            HeaderValue::from_str(&format!("ogrenotes_refresh={payload}")).unwrap(),
        );
        assert!(
            read_refresh_cookie(&headers).is_none(),
            "second Cookie header is not searched; if you want it searched, change to get_all()"
        );
    }
}
