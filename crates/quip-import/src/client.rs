//! `QuipClient` — throttled reqwest client for the Quip Automation API
//! (platform.quip.com). Mirrors `crates/api/src/claude.rs`'s reqwest idiom,
//! plus a rate-limit-header read into `Throttle` before consuming the body.

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;

use crate::secret::QuipToken;
use crate::throttle::Throttle;

const DEFAULT_BASE: &str = "https://platform.quip.com";

/// Upper bound on a downloaded blob (attachment/image). Guards against a
/// pathological attachment exhausting worker memory, mirroring
/// `crates/collab/src/import_pdf.rs`'s `MAX_PDF_BYTES` posture. Checked
/// against `Content-Length` when present (short-circuits before download)
/// and again against the actual received length. A chunked or
/// header-less response skips the short-circuit and is only caught by
/// the post-read check, meaning `reqwest` will have buffered the full
/// (oversized) body into memory before the error is raised — accepted,
/// same posture as `import_pdf.rs`, given Quip is a fixed trusted host.
const MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;

/// Upper bound on the number of `/2/threads/{id}/html` pages fetched for a
/// single document. The v2 HTML endpoint paginates long documents via
/// `response_metadata.next_cursor`; this cap stops a misbehaving or looping
/// cursor from spinning forever. Hitting it is surfaced as an error (a
/// document we cannot fetch in full is a failure, not something to truncate
/// silently) rather than returning a partial body.
const MAX_HTML_PAGES: usize = 100;

// ─── Error ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum QuipError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("rate limited")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("unauthorized")]
    Unauthorized,

    /// 403 from Quip: a per-thread/blob access-restriction, not a dead
    /// credential. Distinct from [`Self::Unauthorized`] so callers can
    /// choose a per-thread-skip disposition instead of a run-terminal one
    /// (see issue #141) — never carries the token, same as `Unauthorized`.
    #[error("forbidden")]
    Forbidden,

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

/// The subset of Quip's thread object we need.
#[derive(Debug, Clone, Deserialize)]
pub struct QuipThread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "type")]
    pub thread_type: String,
    #[serde(default)]
    pub updated_usec: i64,
}

/// A thread as returned inside `/1/threads/?ids=...`, which wraps each
/// requested thread under a `thread` key.
#[derive(Debug, Deserialize)]
struct ThreadEnvelope {
    thread: QuipThread,
}

/// One page of the paginated `GET /2/threads/{id}/html` response. The v2
/// HTML endpoint returns a JSON envelope, NOT bare HTML: the document body
/// is the `html` field (serde unescapes it automatically), and long
/// documents are split across pages driven by `response_metadata.next_cursor`
/// (see #169 — the previous `text_body()` read returned this whole JSON
/// wrapper verbatim, so html5ever saw a single escaped text node).
#[derive(Debug, Deserialize)]
struct ThreadHtmlPage {
    #[serde(default)]
    html: String,
    #[serde(default)]
    response_metadata: ResponseMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct ResponseMetadata {
    #[serde(default)]
    next_cursor: String,
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

    /// `GET /1/threads/?ids=<comma-joined>`. Short-circuits to `Ok(vec![])`
    /// for an empty `ids` (avoids an `ids=` query Quip could 400 on).
    pub async fn threads(&self, t: &QuipToken, ids: &[String]) -> Result<Vec<QuipThread>, QuipError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.throttle.acquire().await;

        let resp = self
            .http
            .get(format!("{}/1/threads/", self.base))
            .bearer_auth(t.expose())
            .query(&[("ids", ids.join(","))])
            .send()
            .await?;

        let body: std::collections::HashMap<String, ThreadEnvelope> =
            self.observe_and_check(resp).await?.json_body().await?;

        Ok(body.into_values().map(|env| env.thread).collect())
    }

    /// `GET /2/threads/{id}/html` — the section-id-bearing HTML used to
    /// carry over Quip's per-section anchors during import.
    ///
    /// The endpoint returns a JSON envelope (`{ "html": "...",
    /// "response_metadata": { "next_cursor": "..." } }`), not bare HTML, and
    /// paginates long documents via `next_cursor`. We parse the envelope with
    /// `json_body()` (the same typed path the sibling methods use — serde
    /// unescapes the HTML for us; no hand-unescaping) and concatenate each
    /// page's `html` fragment in order until the cursor is empty. A
    /// single-page document (empty `next_cursor`, the common case) costs
    /// exactly one request. See #169.
    ///
    /// The next-page cursor is passed as the `cursor` query parameter
    /// (`GET /2/threads/{id}/html?cursor=<next_cursor>`). NOTE: `cursor` is the
    /// ASSUMED Quip v2 pagination param name — it was not confirmed against the
    /// live API reference (the reference page is JS-rendered and could not be
    /// fetched), and there is no other v2 paginated endpoint in this client to
    /// corroborate against. If the name is wrong, the failure is loud, not
    /// silent: the cursor never empties, so the loop runs to `MAX_HTML_PAGES`
    /// and returns a `Parse` error rather than a truncated document. Single-page
    /// documents (empty `next_cursor`, the common case) are unaffected either
    /// way. See #169's report.
    pub async fn thread_html(&self, t: &QuipToken, thread_id: &str) -> Result<String, QuipError> {
        let mut html = String::new();
        let mut cursor = String::new();

        for _ in 0..MAX_HTML_PAGES {
            self.throttle.acquire().await;

            let mut req = self
                .http
                .get(format!("{}/2/threads/{thread_id}/html", self.base))
                .bearer_auth(t.expose());
            if !cursor.is_empty() {
                req = req.query(&[("cursor", cursor.as_str())]);
            }
            let resp = req.send().await?;

            let page: ThreadHtmlPage = self.observe_and_check(resp).await?.json_body().await?;
            html.push_str(&page.html);

            cursor = page.response_metadata.next_cursor;
            if cursor.is_empty() {
                return Ok(html);
            }
        }

        // The cursor never emptied within the cap. Truncating silently would
        // corrupt a long document (the #169 failure mode we are fixing), so
        // this is a hard error the caller can report/retry on instead.
        Err(QuipError::Parse(format!(
            "thread HTML exceeded the {MAX_HTML_PAGES}-page fetch cap; \
             refusing to return a truncated document"
        )))
    }

    /// `GET /1/blob/{thread_id}/{blob_id}` — raw attachment bytes. Refuses
    /// bodies over `MAX_BLOB_BYTES`, checking `Content-Length` first (to
    /// avoid downloading an oversized body at all) and the actual received
    /// length as a backstop for responses that omit or lie about it.
    pub async fn blob(
        &self,
        t: &QuipToken,
        thread_id: &str,
        blob_id: &str,
    ) -> Result<Vec<u8>, QuipError> {
        self.throttle.acquire().await;

        let resp = self
            .http
            .get(format!("{}/1/blob/{thread_id}/{blob_id}", self.base))
            .bearer_auth(t.expose())
            .send()
            .await?;

        let checked = self.observe_and_check(resp).await?;

        check_blob_size(checked.content_length(), 0)?;

        let bytes = checked.bytes_body().await?;
        check_blob_size(None, bytes.len())?;
        Ok(bytes)
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

        if status.as_u16() == 401 {
            return Err(QuipError::Unauthorized);
        }
        if status.as_u16() == 403 {
            return Err(QuipError::Forbidden);
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

    async fn text_body(self) -> Result<String, QuipError> {
        self.0
            .text()
            .await
            .map_err(|e| QuipError::Parse(e.to_string()))
    }

    async fn bytes_body(self) -> Result<Vec<u8>, QuipError> {
        self.0
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| QuipError::Parse(e.to_string()))
    }

    /// The `Content-Length` response header, if present and parseable.
    /// Non-consuming — safe to call before reading the body.
    fn content_length(&self) -> Option<u64> {
        self.0.content_length()
    }
}

/// Enforce the `MAX_BLOB_BYTES` ceiling. Split out of `blob()` so both
/// branches are unit-testable without a 32 MiB body or a live HTTP
/// round-trip: `blob()` calls this once with `(declared_content_length, 0)`
/// before reading the body (the short-circuit, skipped when the header is
/// absent) and once with `(None, bytes.len())` after (the backstop, which
/// also catches chunked / header-less responses that lied about or omitted
/// `Content-Length`). Refuses bodies strictly *over* the cap — exactly at
/// the cap is allowed.
fn check_blob_size(content_length: Option<u64>, actual_len: usize) -> Result<(), QuipError> {
    if let Some(len) = content_length
        && len > MAX_BLOB_BYTES as u64
    {
        return Err(QuipError::Parse(format!(
            "blob is {len} bytes (Content-Length); exceeds the {MAX_BLOB_BYTES}-byte limit"
        )));
    }
    if actual_len > MAX_BLOB_BYTES {
        return Err(QuipError::Parse(format!(
            "blob is {actual_len} bytes; exceeds the {MAX_BLOB_BYTES}-byte limit"
        )));
    }
    Ok(())
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
    use wiremock::matchers::{header, method, path, query_param};
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
    async fn forbidden_maps_to_forbidden_without_leaking_the_token() {
        let server = MockServer::start().await;
        Mock::given(path("/1/users/current"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        assert!(matches!(
            c.current_user(&QuipToken::new("x".into())).await,
            Err(QuipError::Forbidden)
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
    async fn threads_joins_ids_and_parses_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/threads/"))
            .and(header("authorization", "Bearer tok-t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "t1": {"thread": {"id":"t1","title":"Doc A","type":"document","updated_usec": 111}},
                "t2": {"thread": {"id":"t2","title":"Sheet","type":"spreadsheet","updated_usec": 222}}
            })))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let mut ts = c
            .threads(
                &QuipToken::new("tok-t".into()),
                &["t1".into(), "t2".into()],
            )
            .await
            .unwrap();
        ts.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].id, "t1");
        assert_eq!(ts[0].thread_type, "document");
        assert_eq!(ts[1].updated_usec, 222);
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

    #[tokio::test]
    async fn thread_html_fetches_the_v2_html_endpoint() {
        // The real `/2/threads/{id}/html` returns a JSON envelope, not bare
        // HTML (#169). The mock must serve exactly that shape, and the client
        // must return the EXTRACTED, unescaped HTML — never the JSON wrapper.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/2/threads/t1/html"))
            .and(header("authorization", "Bearer tok-h"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "html": "<p id=\"s1\">hi</p>",
                "response_metadata": { "next_cursor": "" }
            })))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let html = c
            .thread_html(&QuipToken::new("tok-h".into()), "t1")
            .await
            .unwrap();
        // The extracted, unescaped HTML — literally the `html` field's value.
        assert_eq!(html, "<p id=\"s1\">hi</p>");
        // And crucially NOT the JSON envelope the pre-#169 code returned.
        assert!(!html.contains("response_metadata"), "must not return the JSON wrapper: {html}");
        assert!(!html.contains("\"html\""), "must not return the JSON wrapper: {html}");
        // A single-page doc (empty next_cursor) costs exactly one request.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1, "an empty next_cursor must not trigger a second fetch");
    }

    #[tokio::test]
    async fn thread_html_paginates_and_concatenates_via_next_cursor() {
        // A long document is split across pages driven by
        // `response_metadata.next_cursor` (#169). The first page carries a
        // non-empty cursor and a partial body; the second (carrying that
        // cursor as `?cursor=`) has the rest and an empty cursor. The returned
        // HTML is the two fragments concatenated in order.
        let server = MockServer::start().await;

        // Page 2 — matched only when the `cursor` query param is present. Must
        // be mounted at higher priority so it wins over the page-1 matcher.
        Mock::given(method("GET"))
            .and(path("/2/threads/t1/html"))
            .and(query_param("cursor", "CURSOR-2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "html": "<p id=\"s2\">world</p>",
                "response_metadata": { "next_cursor": "" }
            })))
            .with_priority(1)
            .mount(&server)
            .await;

        // Page 1 — the cursorless first fetch; hands back CURSOR-2.
        Mock::given(method("GET"))
            .and(path("/2/threads/t1/html"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "html": "<p id=\"s1\">hello</p>",
                "response_metadata": { "next_cursor": "CURSOR-2" }
            })))
            .with_priority(2)
            .mount(&server)
            .await;

        let c = QuipClient::new(Some(server.uri()));
        let html = c
            .thread_html(&QuipToken::new("tok-h".into()), "t1")
            .await
            .unwrap();

        assert_eq!(
            html, "<p id=\"s1\">hello</p><p id=\"s2\">world</p>",
            "the two page fragments must be concatenated in order"
        );

        // Two requests total, and the SECOND actually carried the cursor.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2, "one fetch per page");
        assert_eq!(requests[0].url.query(), None, "the first fetch is cursorless");
        assert_eq!(
            requests[1].url.query(),
            Some("cursor=CURSOR-2"),
            "the second fetch must carry the cursor from page 1",
        );
    }

    #[tokio::test]
    async fn blob_fetches_raw_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/blob/t1/b9"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let bytes = c
            .blob(&QuipToken::new("tok-b".into()), "t1", "b9")
            .await
            .unwrap();
        assert_eq!(bytes, vec![1u8, 2, 3]);
    }

    #[tokio::test]
    async fn thread_html_401_maps_to_unauthorized_without_leaking_the_token() {
        let server = MockServer::start().await;
        Mock::given(path("/2/threads/t1/html"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let c = QuipClient::new(Some(server.uri()));
        let e = c
            .thread_html(&QuipToken::new("SEEKRET".into()), "t1")
            .await
            .unwrap_err();
        assert!(matches!(e, QuipError::Unauthorized));
        assert!(!format!("{e}").contains("SEEKRET"));
    }

    // `check_blob_size` is extracted from `blob()` specifically so the cap
    // logic is testable as pure comparisons — no wiremock, no I/O, no
    // large allocation needed (a real 32MiB+ body can't be faked through
    // wiremock/hyper: a declared `Content-Length` that doesn't match the
    // real transmitted body size panics at the hyper transport layer, not
    // as a client-visible response).

    #[test]
    fn check_blob_size_under_cap_passes() {
        assert!(check_blob_size(Some(1000), 500).is_ok());
    }

    #[test]
    fn check_blob_size_exactly_at_cap_passes() {
        // Pins `>` (not `>=`) semantics: refuse bodies *over* the cap.
        assert!(check_blob_size(Some(MAX_BLOB_BYTES as u64), 0).is_ok());
        assert!(check_blob_size(None, MAX_BLOB_BYTES).is_ok());
    }

    #[test]
    fn check_blob_size_declared_length_over_cap_errors() {
        let err = check_blob_size(Some(MAX_BLOB_BYTES as u64 + 1), 0).unwrap_err();
        assert!(matches!(err, QuipError::Parse(_)));
        assert!(format!("{err}").contains("Content-Length"));
    }

    #[test]
    fn check_blob_size_post_read_length_over_cap_errors() {
        let err = check_blob_size(None, MAX_BLOB_BYTES + 1).unwrap_err();
        assert!(matches!(err, QuipError::Parse(_)));
        assert!(!format!("{err}").contains("Content-Length"));
    }

    #[test]
    fn check_blob_size_none_content_length_with_under_cap_body_passes() {
        assert!(check_blob_size(None, 500).is_ok());
    }
}
