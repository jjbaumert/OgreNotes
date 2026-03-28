use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

const STORAGE_KEY: &str = "ogrenotes_auth";
const ACCESS_TOKEN_TTL_MS: f64 = 900_000.0; // 15 minutes

// Auth state stored in thread-local (WASM is single-threaded) with
// localStorage backing so it survives page reloads.
thread_local! {
    static AUTH_STATE: RefCell<Option<AuthState>> = RefCell::new(load_from_storage());
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthState {
    pub access_token: String,
    pub refresh_token: String,
    pub session_id: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    /// Unix timestamp in milliseconds when the access token expires.
    /// Old localStorage entries without this field deserialize as 0.0 (= expired).
    #[serde(default)]
    pub expires_at: f64,
}

/// Get the current time as Unix milliseconds.
fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

fn load_from_storage() -> Option<AuthState> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let json = storage.get_item(STORAGE_KEY).ok()??;
    let state: AuthState = serde_json::from_str(&json).ok()?;
    if now_ms() >= state.expires_at {
        let _ = storage.remove_item(STORAGE_KEY);
        return None;
    }
    Some(state)
}

fn save_to_storage(state: &AuthState) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        if let Ok(json) = serde_json::to_string(state) {
            let _ = storage.set_item(STORAGE_KEY, &json);
        }
    }
}

fn clear_storage() {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = storage.remove_item(STORAGE_KEY);
    }
}

/// Set auth state after login.
pub fn set_auth(state: AuthState) {
    save_to_storage(&state);
    AUTH_STATE.with(|s| *s.borrow_mut() = Some(state));
}

/// Get the current access token.
pub fn get_token() -> Option<String> {
    AUTH_STATE.with(|s| s.borrow().as_ref().map(|a| a.access_token.clone()))
}

/// Get the full auth state.
pub fn get_auth() -> Option<AuthState> {
    AUTH_STATE.with(|s| s.borrow().clone())
}

/// Check if the user is authenticated.
pub fn is_authenticated() -> bool {
    AUTH_STATE.with(|s| s.borrow().is_some())
}

/// Clear auth state on logout.
pub fn clear_auth() {
    clear_storage();
    AUTH_STATE.with(|s| *s.borrow_mut() = None);
}

const API_BASE: &str = "/api/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub session_id: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
}

/// POST /auth/dev-login -- for local development only.
pub async fn dev_login(email: &str, name: &str) -> Result<AuthState, ApiClientError> {
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

    if !resp.ok() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    let token_resp: TokenResponse = resp
        .json()
        .await
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;

    let auth = AuthState {
        access_token: token_resp.access_token,
        refresh_token: token_resp.refresh_token,
        session_id: token_resp.session_id,
        user_id: token_resp.user_id,
        email: token_resp.email,
        name: token_resp.name,
        expires_at: now_ms() + ACCESS_TOKEN_TTL_MS,
    };

    set_auth(auth.clone());
    Ok(auth)
}

/// Make an authenticated GET request and deserialize JSON response.
pub async fn api_get<T: DeserializeOwned>(path: &str) -> Result<T, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::get(&url);

    if let Some(token) = get_token() {
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

    if let Some(token) = get_token() {
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

/// Make an authenticated PATCH request with JSON body.
pub async fn api_patch<B: Serialize>(path: &str, body: &B) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::patch(&url).header("Content-Type", "application/json");

    if let Some(token) = get_token() {
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
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    Ok(())
}

/// Make an authenticated DELETE request.
pub async fn api_delete(path: &str) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::delete(&url);

    if let Some(token) = get_token() {
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
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    Ok(())
}

/// Make an authenticated PUT request with raw bytes.
pub async fn api_put_bytes(path: &str, data: &[u8]) -> Result<(), ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::put(&url).header("Content-Type", "application/octet-stream");

    if let Some(token) = get_token() {
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
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    Ok(())
}

/// Make an authenticated GET request for raw bytes.
pub async fn api_get_bytes(path: &str) -> Result<Vec<u8>, ApiClientError> {
    let url = format!("{API_BASE}{path}");
    let mut req = Request::get(&url);

    if let Some(token) = get_token() {
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
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    let bytes = resp
        .binary()
        .await
        .map_err(|e| ApiClientError::Network(e.to_string()))?;

    Ok(bytes)
}

async fn handle_response<T: DeserializeOwned>(resp: Response) -> Result<T, ApiClientError> {
    if resp.status() == 401 {
        redirect_to_login();
        return Err(ApiClientError::Unauthorized);
    }

    if !resp.ok() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(ApiClientError::Http(status, text));
    }

    let data: T = resp
        .json()
        .await
        .map_err(|e| ApiClientError::Deserialize(e.to_string()))?;

    Ok(data)
}

fn redirect_to_login() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href("/login");
    }
}

#[derive(Debug, Clone)]
pub enum ApiClientError {
    Network(String),
    Serialize(String),
    Deserialize(String),
    Http(u16, String),
    Unauthorized,
}

impl std::fmt::Display for ApiClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiClientError::Network(e) => write!(f, "Network error: {e}"),
            ApiClientError::Serialize(e) => write!(f, "Serialize error: {e}"),
            ApiClientError::Deserialize(e) => write!(f, "Deserialize error: {e}"),
            ApiClientError::Http(status, body) => write!(f, "HTTP {status}: {body}"),
            ApiClientError::Unauthorized => write!(f, "Unauthorized"),
        }
    }
}
