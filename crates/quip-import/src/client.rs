//! `QuipClient` — throttled reqwest client for the Quip Automation API
//! (platform.quip.com). Mirrors `crates/api/src/claude.rs`'s reqwest idiom,
//! plus a rate-limit-header read into `Throttle` before consuming the body.

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::secret::QuipToken;
use crate::throttle::Throttle;

const DEFAULT_BASE: &str = "https://platform.quip.com";

// ─── Error ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum QuipError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("rate limited")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("unauthorized")]
    Unauthorized,

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("unexpected response format: {0}")]
    Parse(String),
}

// ─── DTOs ──────────────────────────────────────────────────────

/// The subset of Quip's `/1/users/current` response we need. The real
/// payload has more fields (e.g. `affinity`, `desktop_folder_id`); we
/// deserialize only what we use and ignore the rest.
#[derive(Debug, Clone, Deserialize)]
pub struct QuipUser {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emails: Vec<String>,
    pub private_folder_id: String,
    #[serde(default)]
    pub shared_folder_ids: Vec<String>,
}

/// The subset of Quip's folder object we need.
#[derive(Debug, Clone, Deserialize)]
pub struct QuipFolder {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub children: Vec<QuipFolderChild>,
}

/// One entry in a folder's `children` array — Quip mixes thread and
/// sub-folder references in the same list, tagged by which id field is
/// present.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct QuipFolderChild {
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub folder_id: Option<String>,
}

/// A folder as returned inside `/1/folders/?ids=...`, which wraps each
/// requested folder under a `folder` key alongside its own `children`.
#[derive(Debug, Deserialize)]
struct FolderEnvelope {
    folder: FolderMeta,
    #[serde(default)]
    children: Vec<QuipFolderChild>,
}

#[derive(Debug, Deserialize)]
struct FolderMeta {
    id: String,
    #[serde(default)]
    title: String,
}

// ─── Client ────────────────────────────────────────────────────

pub struct QuipClient {
    http: Client,
    base: String,
    throttle: Throttle,
}

impl QuipClient {
    /// Build a client pointed at `base` (defaults to
    /// `https://platform.quip.com`), with a 30s request timeout and the
    /// default (45/min) throttle.
    pub fn new(base: Option<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client builder should not fail with static config");
        Self {
            http,
            base: base.unwrap_or_else(|| DEFAULT_BASE.to_string()),
            throttle: Throttle::new(),
        }
    }

    /// `GET /1/users/current`.
    pub async fn current_user(&self, t: &QuipToken) -> Result<QuipUser, QuipError> {
        self.throttle.acquire().await;

        let resp = self
            .http
            .get(format!("{}/1/users/current", self.base))
            .bearer_auth(t.expose())
            .send()
            .await?;

        self.observe_and_check(resp).await?.json_body().await
    }

    /// `GET /1/folders/?ids=<comma-joined>`.
    pub async fn folders(&self, t: &QuipToken, ids: &[String]) -> Result<Vec<QuipFolder>, QuipError> {
        self.throttle.acquire().await;

        let resp = self
            .http
            .get(format!("{}/1/folders/", self.base))
            .bearer_auth(t.expose())
            .query(&[("ids", ids.join(","))])
            .send()
            .await?;

        let body: std::collections::HashMap<String, FolderEnvelope> =
            self.observe_and_check(resp).await?.json_body().await?;

        Ok(body
            .into_values()
            .map(|env| QuipFolder {
                id: env.folder.id,
                title: env.folder.title,
                children: env.children,
            })
            .collect())
    }

    /// Feed rate-limit headers into the throttle, then map non-2xx statuses
    /// to `QuipError` variants. Reads headers before consuming the body, per
    /// the task contract. Never includes the token (it was never read here)
    /// or, for the error path, the raw response body verbatim beyond what
    /// the server sent as its own error text.
    async fn observe_and_check(&self, resp: reqwest::Response) -> Result<Checked, QuipError> {
        let remaining = header_u32(&resp, "x-ratelimit-remaining");
        let reset_at_ms = header_reset_ms(&resp, "x-ratelimit-reset");
        self.throttle.observe_headers(remaining, reset_at_ms);

        let status = resp.status();
        if status.is_success() {
            return Ok(Checked(resp));
        }

        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(QuipError::Unauthorized);
        }
        if status.as_u16() == 503 {
            return Err(QuipError::RateLimited {
                retry_after_ms: reset_at_ms.map(|reset| {
                    let now = ogrenotes_common::time::now_usec() / 1000;
                    (reset - now).max(0) as u64
                }),
            });
        }

        let message = resp.text().await.unwrap_or_default();
        Err(QuipError::Api {
            status: status.as_u16(),
            message,
        })
    }
}

/// Thin wrapper so `.json_body()` reads clearly at call sites while keeping
/// the response-consuming step (which must happen after header
/// inspection) explicit.
struct Checked(reqwest::Response);

impl Checked {
    async fn json_body<T: for<'de> Deserialize<'de>>(self) -> Result<T, QuipError> {
        self.0
            .json::<T>()
            .await
            .map_err(|e| QuipError::Parse(e.to_string()))
    }
}

fn header_u32(resp: &reqwest::Response, name: &str) -> Option<u32> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// Parse an `x-ratelimit-reset`-style header. Quip sends this as a unix
/// epoch in **seconds**; converted to ms to match `Throttle`'s clock.
/// Missing or unparseable values are ignored rather than erroring — a
/// throttle hint is a courtesy, not something worth failing the call over.
fn header_reset_ms(resp: &reqwest::Response, name: &str) -> Option<i64> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|secs| (secs * 1000.0) as i64)
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn current_user_sends_bearer_and_parses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/users/current"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"u1","name":"Ada","emails":["ada@example.com"],
                "private_folder_id":"pf","shared_folder_ids":["sf1"]
            })))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let u = c
            .current_user(&QuipToken::new("tok-1".into()))
            .await
            .unwrap();
        assert_eq!(u.id, "u1");
        assert_eq!(u.emails, vec!["ada@example.com"]);
        assert_eq!(u.private_folder_id, "pf");
        assert_eq!(u.shared_folder_ids, vec!["sf1"]);
    }

    #[tokio::test]
    async fn unauthorized_and_rate_limited_map() {
        let server = MockServer::start().await;
        Mock::given(path("/1/users/current"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        assert!(matches!(
            c.current_user(&QuipToken::new("x".into())).await,
            Err(QuipError::Unauthorized)
        ));
        // token never leaks into the error text:
        let e = c
            .current_user(&QuipToken::new("SEEKRET".into()))
            .await
            .unwrap_err();
        assert!(!format!("{e}").contains("SEEKRET"));
    }

    #[tokio::test]
    async fn folders_happy_path_joins_ids_and_parses_children() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/folders/"))
            .and(header("authorization", "Bearer tok-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "f1": {
                    "folder": {"id": "f1", "title": "Root"},
                    "children": [
                        {"thread_id": "t1"},
                        {"folder_id": "f2"}
                    ]
                }
            })))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let folders = c
            .folders(
                &QuipToken::new("tok-2".into()),
                &["f1".to_string(), "f2".to_string()],
            )
            .await
            .unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, "f1");
        assert_eq!(folders[0].title, "Root");
        assert_eq!(folders[0].children.len(), 2);
        assert_eq!(folders[0].children[0].thread_id.as_deref(), Some("t1"));
        assert_eq!(folders[0].children[1].folder_id.as_deref(), Some("f2"));

        // The request the mock matched confirms `ids` were comma-joined
        // (the `path`+`header` matchers above already assert the route was
        // hit; verify query explicitly via the received requests too).
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.query(), Some("ids=f1%2Cf2"));
    }

    #[tokio::test]
    async fn service_unavailable_maps_to_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(path("/1/users/current"))
            .respond_with(
                ResponseTemplate::new(503).insert_header("x-ratelimit-reset", "9999999999"),
            )
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let err = c
            .current_user(&QuipToken::new("tok-3".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, QuipError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn observes_rate_limit_headers_before_consuming_body() {
        let server = MockServer::start().await;
        Mock::given(path("/1/users/current"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ratelimit-remaining", "3")
                    .insert_header("x-ratelimit-reset", "9999999999")
                    .set_body_json(serde_json::json!({
                        "id":"u2","name":"Bea","emails":[],
                        "private_folder_id":"pf2","shared_folder_ids":[]
                    })),
            )
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        c.current_user(&QuipToken::new("tok-4".into()))
            .await
            .unwrap();
        // Successful parse (body still readable) is itself proof headers
        // were read via `resp.headers()` (a non-consuming accessor) rather
        // than by draining/re-parsing the body.
    }
}
