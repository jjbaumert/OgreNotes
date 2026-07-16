// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

const ACCESS_TOKEN_TTL_MS: f64 = 900_000.0; // 15 minutes

// Auth state lives ONLY in the JS heap (this thread-local) for the
// lifetime of the page. The refresh token is in an HttpOnly cookie
// that JS cannot read; on page load we hydrate the access token via
// a refresh call (the cookie is automatically attached to same-origin
// fetches). When the page closes, JS state is gone — only the cookie
// (server-side hashed) persists. Closes the JS-readable-tokens
// branch of #33.
thread_local! {
    static AUTH_STATE: RefCell<Option<AuthState>> = const { RefCell::new(None) };
}

/// In-memory auth state. The refresh token is intentionally NOT here —
/// it lives in an `HttpOnly; Secure; SameSite=Strict` cookie that the
/// browser auto-attaches to `/api/v1/auth/*` requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthState {
    pub access_token: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    /// Unix timestamp in milliseconds when the access token expires.
    pub expires_at: f64,
}

/// Get the current time as Unix milliseconds.
pub fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

/// Set auth state after login or successful refresh.
pub fn set_auth(state: AuthState) {
    AUTH_STATE.with(|s| *s.borrow_mut() = Some(state));
}

/// Get the current access token, or `None` if expired/missing.
pub fn get_token() -> Option<String> {
    AUTH_STATE.with(|s| {
        s.borrow().as_ref().and_then(|a| {
            if now_ms() < a.expires_at {
                Some(a.access_token.clone())
            } else {
                None
            }
        })
    })
}

/// Get the full auth state.
pub fn get_auth() -> Option<AuthState> {
    AUTH_STATE.with(|s| s.borrow().clone())
}

/// Check if the user is authenticated (has any in-memory state).
///
/// Caller must ensure `try_hydrate_from_cookie()` has been awaited at
/// app boot before relying on this — otherwise it returns `false`
/// even when a valid refresh cookie is present.
pub fn is_authenticated() -> bool {
    AUTH_STATE.with(|s| s.borrow().is_some())
}

/// Clear in-memory auth state.
pub fn clear_auth() {
    AUTH_STATE.with(|s| *s.borrow_mut() = None);
}

/// Log out: revoke the server-side session and clear in-memory state.
///
/// The cookie carries identity to the backend, which clears it via
/// `Set-Cookie: …; Max-Age=0`. We always send Bearer when present
/// (the backend prefers it), but cookie-only also works for the
/// post-restart case where the JS-memory access token is gone but
/// the cookie remains.
pub async fn logout() {
    let url = format!("{API_BASE}/auth/logout");
    let mut req = Request::post(&url);
    if let Some(token) = AUTH_STATE.with(|s| s.borrow().as_ref().map(|a| a.access_token.clone())) {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let _ = req.send().await;
    clear_auth();
}

/// Hydrate the in-memory access token from the refresh cookie at boot.
/// Returns the decoded response on success (`None` only when the refresh
/// itself failed), so callers can both (a) treat `Some` as "authenticated"
/// and (b) read `.ui_prefs` for the boot-time prefs application. The
/// access token is installed inside `refresh_token_inner` before this
/// returns.
pub async fn try_hydrate_from_cookie() -> Option<TokenResponse> {
    refresh_token_inner().await
}

/// POST /auth/refresh via the HttpOnly cookie. Returns the decoded
/// TokenResponse on success (and installs the access token), else None.
/// Shared by boot hydration (which wants the ui_prefs) and mid-session
/// token recovery (which ignores the body).
///
/// The body carries no fields — we send `"{}"` (an empty JSON object)
/// rather than truly no body because the backend extractor accepts
/// both shapes today, and `"{}"` is an unambiguous wire-level
/// fingerprint for "cookie path" in CloudWatch traces. Once the
/// backend drops the legacy body-fallback, this can become a
/// bodyless POST.
async fn refresh_token_inner() -> Option<TokenResponse> {
    let resp = Request::post(&format!("{API_BASE}/auth/refresh"))
        .header("Content-Type", "application/json")
        .body("{}")
        .ok()?
        .send()
        .await
        .ok()?;
    if !resp.ok() {
        return None;
    }
    // The backend response carries `refreshToken` and `sessionId` for
    // transitional clients; we deliberately ignore them — the cookie
    // path is canonical now and those fields will be removed once
    // every deployed frontend ships this version.
    let token_resp: TokenResponse = resp.json().await.ok()?;
    set_auth(AuthState {
        access_token: token_resp.access_token.clone(),
        user_id: token_resp.user_id.clone(),
        email: token_resp.email.clone(),
        name: token_resp.name.clone(),
        expires_at: now_ms() + ACCESS_TOKEN_TTL_MS,
    });
    Some(token_resp)
}

/// Return a valid access token, refreshing proactively if expired.
/// Mid-session path — deliberately ignores ui_prefs (a silent token
/// renewal must not re-apply locale/theme).
async fn try_refresh_token() -> bool {
    refresh_token_inner().await.is_some()
}

/// Return a valid access token, refreshing proactively if expired.
async fn ensure_token() -> Option<String> {
    if let Some(token) = get_token() {
        return Some(token);
    }
    if try_refresh_token().await {
        return get_token();
    }
    None
}

pub(super) const API_BASE: &str = "/api/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    pub access_token: String,
    /// Unused by the cookie-flow frontend — kept on the wire for
    /// transitional compatibility with older clients during rollout.
    #[serde(default)]
    pub refresh_token: String,
    /// Same — unused by this frontend.
    #[serde(default)]
    pub session_id: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    /// Phase 4 M-E3 piece D: `Some(true)` when the user's default
    /// workspace requires MFA and the user hasn't yet enrolled.
    /// `serde(default)` because the backend omits the field on the
    /// common no-MFA path (skip_serializing_if_none).
    #[serde(default)]
    pub mfa_enrollment_required: Option<bool>,
    /// The user's stored UI prefs (Phase 5 M-P2). `serde(default)` so
    /// older servers that omit the field still decode.
    #[serde(default)]
    pub ui_prefs: Option<UiPrefsDto>,
}

/// Slim decode of the server `UiPrefs` blob delivered on the auth
/// response. Mirrors the backend camelCase shape; only the fields the
/// boot path applies. Per the per-consumer-slim-decode pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPrefsDto {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub dyslexic_font: Option<bool>,
    #[serde(default)]
    pub reduce_motion: Option<bool>,
    /// Document typography theme id (#59 T-12); `None` / "default" ⇒ Inter.
    #[serde(default)]
    pub doc_theme: Option<String>,
}

/// The body shape the server returns on a 202 from `/auth/dev-login`
/// (and the redirect target on the OAuth callback). The user needs
/// to submit a TOTP via `/auth/mfa/challenge` before a session is
/// minted.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MfaPendingResponse {
    pub handle: String,
}

/// Outcome of a login attempt. The frontend dispatches on this to
/// pick the next route: home, MFA enrollment page, or MFA challenge
/// page.
#[derive(Debug, Clone)]
pub enum LoginOutcome {
    /// 200 + TokenResponse. The session is minted; the cookie is set.
    /// `mfa_enrollment_required` is true when the workspace requires
    /// MFA but the user hasn't enrolled — the frontend should route
    /// to the enrollment page before showing the app.
    Authenticated {
        auth: AuthState,
        mfa_enrollment_required: bool,
    },
    /// 202 + MfaPendingResponse. The user is enrolled in MFA; submit
    /// a TOTP via `/auth/mfa/challenge?handle=...` to complete login.
    MfaRequired { handle: String },
}

/// POST /auth/dev-login -- for local development only.
///
/// Returns the polymorphic [`LoginOutcome`]: either a fully
/// authenticated session (with a possible enrollment-required hint),
/// or the MFA-challenge handle when the user is already enrolled.
/// The 202 (Accepted) path is what the Phase 4 M-E3 server returns
/// for MFA-enrolled users.
pub async fn dev_login(email: &str, name: &str) -> Result<LoginOutcome, ApiClientError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DevLoginBody {
        email: String,
        name: String,
    }

    let body = DevLoginBody {
        email: email.to_string(),
        name: name.to_string(),
    };

    let body_str =
        serde_json::to_string(&body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;

    let resp = Request::post(&format!("{API_BASE}/auth/dev-login"))
        .header("Content-Type", "application/json")
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    // 202 ACCEPTED is the MFA-pending interstitial — body is
    // `{ handle }`, NOT a TokenResponse. The cookie is intentionally
    // NOT set on this response (the user isn't authenticated yet).
    if resp.status() == 202 {
        let pending: MfaPendingResponse = resp
            .json()
            .await
            .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;
        return Ok(LoginOutcome::MfaRequired { handle: pending.handle });
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;

    // The Set-Cookie header on this response carries the refresh
    // token; the browser stores it automatically. We persist only
    // the access token + user identity in JS memory.
    let auth = AuthState {
        access_token: token_resp.access_token,
        user_id: token_resp.user_id,
        email: token_resp.email,
        name: token_resp.name,
        expires_at: now_ms() + ACCESS_TOKEN_TTL_MS,
    };

    set_auth(auth.clone());
    Ok(LoginOutcome::Authenticated {
        auth,
        mfa_enrollment_required: token_resp.mfa_enrollment_required.unwrap_or(false),
    })
}

/// Store a TokenResponse received from `/auth/mfa/challenge` or
/// `/auth/mfa/recovery` as the active auth state. Lives here (not
/// in api/mfa.rs) so the MFA pages don't reach into client::
/// internals to copy the same eight lines.
pub fn set_auth_from_token(token: &TokenResponse) {
    set_auth(AuthState {
        access_token: token.access_token.clone(),
        user_id: token.user_id.clone(),
        email: token.email.clone(),
        name: token.name.clone(),
        expires_at: now_ms() + ACCESS_TOKEN_TTL_MS,
    });
}

/// Make an authenticated GET request and deserialize JSON response.
pub async fn api_get<T: DeserializeOwned>(path: &str) -> Result<T, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::get(&url);

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;
    handle_response(resp).await
}

/// Make an authenticated POST request with JSON body.
pub async fn api_post<T: DeserializeOwned, B: Serialize>(
    path: &str,
    body: &B,
) -> Result<T, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::post(&url).header("Content-Type", "application/json");

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let body_str =
        serde_json::to_string(body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let resp = req
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    handle_response(resp).await
}

/// Make an authenticated POST request with JSON body, expecting no response body.
pub async fn api_post_empty<B: Serialize>(path: &str, body: &B) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::post(&url).header("Content-Type", "application/json");

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let body_str =
        serde_json::to_string(body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let resp = req
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    Ok(())
}

/// Make an authenticated PUT request with JSON body, expecting no response body.
/// Mirrors `api_post_empty` / `api_patch`; used by admin set-ask-enabled.
pub async fn api_put<B: Serialize>(path: &str, body: &B) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::put(&url).header("Content-Type", "application/json");

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let body_str =
        serde_json::to_string(body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let resp = req
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    Ok(())
}

/// Make an authenticated PATCH request with JSON body.
pub async fn api_patch<B: Serialize>(path: &str, body: &B) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::patch(&url).header("Content-Type", "application/json");

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let body_str =
        serde_json::to_string(body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let resp = req
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    Ok(())
}

/// Make an authenticated DELETE request with a JSON body. Routes
/// through `ensure_token()` so an expired in-memory access token is
/// refreshed from the HttpOnly cookie before the call goes out —
/// otherwise the caller risks sending `Authorization: Bearer ` (empty)
/// and surfacing a generic 401 to the user with no recovery path.
pub async fn api_delete_with_body<B: Serialize>(
    path: &str,
    body: &B,
) -> Result<(), ApiClientError> {
    let token = ensure_token().await.ok_or(ApiClientError::Unauthorized)?;
    let body_str =
        serde_json::to_string(body).map_err(|e| ApiClientError::Serialize(e.to_string()))?;
    let url = format!("{API_BASE}{path}");
    let resp = Request::delete(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"))
        .body(body_str)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;
    if !resp.ok() {
        return Err(http_error(&resp));
    }
    Ok(())
}

/// Make an authenticated DELETE request.
pub async fn api_delete(path: &str) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::delete(&url);

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    Ok(())
}

/// Make an authenticated PUT request with raw bytes.
pub async fn api_put_bytes(path: &str, data: &[u8]) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::put(&url).header("Content-Type", "application/octet-stream");

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let resp = req
        .body(data.to_vec())
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    Ok(())
}

/// Make an authenticated GET request for raw bytes.
pub async fn api_get_bytes(path: &str) -> Result<Vec<u8>, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::get(&url);

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    let bytes = resp
        .binary()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    Ok(bytes)
}

/// POST a `multipart/form-data` body (a file upload). The browser sets
/// the `Content-Type` with its boundary from the `FormData`, so we must
/// NOT set that header ourselves — doing so would omit the boundary and
/// the server's multipart parser would reject the body.
pub async fn api_post_multipart<T: DeserializeOwned>(
    path: &str,
    form: web_sys::FormData,
) -> Result<T, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::post(&url);

    if let Some(token) = ensure_token().await {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }

    let resp = req
        .body(form)
        .map_err(|e| ApiClientError::Network(e.to_string()))?
        .send()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    handle_response(resp).await
}

async fn handle_response<T: DeserializeOwned>(resp: Response) -> Result<T, ApiClientError> {
    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        return Err(http_error(&resp));
    }

    let data: T = resp
        .json()
        .await
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;

    Ok(data)
}

/// Whether a 401 should bounce the browser to `/login`.
///
/// Returns false when we're already on the login page. Every authed
/// request routes its 401 through `redirect_to_login`, and the app's
/// bootstrap fires authed probes on *every* load — `/users/me` for
/// stored prefs, the RUM beacon, the telemetry flush. On a logged-out
/// `/login` load those 401 immediately; without this guard each one
/// calls `set_href("/login")`, which reloads the page, which re-runs the
/// bootstrap, which 401s again — a tight reload loop (the symptom after a
/// failed login). Pure + string-typed so it's unit-testable without a DOM.
fn should_redirect_to_login(pathname: &str) -> bool {
    pathname != "/login"
}

fn redirect_to_login() {
    if let Some(window) = web_sys::window() {
        let pathname = window.location().pathname().unwrap_or_default();
        if should_redirect_to_login(&pathname) {
            let _ = window.location().set_href("/login");
        }
    }
}

#[derive(Debug, Clone)]
pub enum ApiClientError {
    Network(String),
    Serialize(String),
    Deserialize(String),
    /// A non-2xx HTTP response. The second field is an *optional, safe
    /// detail* — the `x-request-id` correlation header, or a message
    /// from a structured DTO the backend explicitly opted into — but
    /// NEVER the raw error response body. #47: backend error bodies are
    /// treated as opaque so a detail-laden message (a stray email
    /// address, a stack fragment, a parser echo) can never reach a
    /// toast, the console, or frontend logs. Match on the status code.
    Http(u16, Option<String>),
    Unauthorized,
}

impl std::fmt::Display for ApiClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiClientError::Network(e) => write!(f, "Network error: {e}"),
            ApiClientError::Serialize(e) => write!(f, "Serialize error: {e}"),
            ApiClientError::Deserialize(e) => write!(f, "Deserialize error: {e}"),
            ApiClientError::Http(status, Some(detail)) => write!(f, "HTTP {status}: {detail}"),
            ApiClientError::Http(status, None) => write!(f, "HTTP {status}"),
            ApiClientError::Unauthorized => write!(f, "Unauthorized"),
        }
    }
}

/// Build an opaque HTTP error from a non-2xx response.
///
/// #47: we deliberately do NOT read `resp.text()` into the error, so a
/// detail-laden backend message (a stray email address, a stack
/// fragment, a Tantivy parse echo) can never reach the UI or logs. Only
/// the status code and the `x-request-id` correlation header the
/// backend's TraceLayer emits are captured, so support can still trace
/// the failure server-side.
pub(crate) fn http_error(resp: &Response) -> ApiClientError {
    ApiClientError::Http(resp.status(), resp.headers().get("x-request-id"))
}

#[cfg(test)]
mod tests {
    use super::{should_redirect_to_login, ApiClientError};

    #[test]
    fn http_error_display_carries_no_body() {
        // #47: the Http variant holds only a status + opaque request-id,
        // never the backend response body. Display must surface the
        // status (and a support reference) without any body text —
        // the type (Option<String> = request id) makes a body
        // unrepresentable here.
        let with_id = ApiClientError::Http(500, Some("req-abc123".to_string()));
        assert_eq!(with_id.to_string(), "HTTP 500: req-abc123");

        let without_id = ApiClientError::Http(404, None);
        assert_eq!(without_id.to_string(), "HTTP 404");
    }

    #[test]
    fn does_not_redirect_when_already_on_login() {
        // The regression: a 401 from a bootstrap probe on /login must not
        // bounce back to /login (which would reload → 401 → loop).
        assert!(!should_redirect_to_login("/login"));
    }

    #[test]
    fn redirects_from_other_paths() {
        assert!(should_redirect_to_login("/"));
        assert!(should_redirect_to_login("/d/abc123/doc"));
        assert!(should_redirect_to_login("/home"));
    }
}
