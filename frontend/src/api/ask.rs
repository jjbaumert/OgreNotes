// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.2 piece A — frontend client for `POST /api/v1/ask`.
//!
//! The endpoint streams Server-Sent Events. EventSource won't work
//! here (the spec only supports GET, and /ask takes a JSON body),
//! so we drop down to `fetch` + `ReadableStream` and decode SSE
//! frames by hand.
//!
//! The wire shape from `crates/api/src/routes/ask.rs`:
//!
//! ```text
//! event: status
//! data: Thinking (round 1)...
//!
//! event: text
//! data: <agent text>
//!
//! event: source
//! data: {"docId": "...", "title": "..."}
//!
//! event: done
//! data: done
//!
//! event: error
//! data: <error message>
//! ```
//!
//! Frames are separated by a blank line (`\n\n`). Axum's
//! `Sse::keep_alive` may interleave SSE comment lines starting with
//! `:` — those carry no event and are dropped by the parser.

use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Headers, ReadableStreamDefaultReader, Request, RequestInit,
    Response, TextDecoder,
};

use crate::api::client;

/// Decoded SSE payload variants the /ask endpoint emits.
#[derive(Debug, Clone)]
pub enum AskEvent {
    /// Status update ("Thinking…", "Using tool: keyword_search…")
    /// — informational, safe to render or ignore.
    Status(String),
    /// Final-answer text chunk. The endpoint emits the agent's
    /// completed response as one or more `text` events in order;
    /// the consumer concatenates them.
    Text(String),
    /// A document the agent cited. Render as a clickable link.
    /// `doc_type` is the lowercase string from `DocType::as_str`:
    /// "document" / "spreadsheet" / "chat". Drives the provider
    /// icon next to the citation in the UI.
    Source {
        doc_id: String,
        title: String,
        doc_type: String,
    },
    /// Stream complete. The reader closes after this event.
    Done,
    /// Server-side error during the agent loop. Renders as an
    /// alert in the UI.
    Error(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourcePayload {
    doc_id: String,
    title: String,
    /// Optional for backward compat with stacks that haven't
    /// redeployed the M-6.2 piece C backend yet. Frontend
    /// defaults to "document" (the most common case) when
    /// absent so the renderer doesn't blank out a citation.
    #[serde(default)]
    doc_type: Option<String>,
}

/// Open a streaming /ask request and pump events into `on_event` as
/// they arrive. Returns when the server emits `Done`, when the
/// stream closes, or on the first network error.
///
/// `on_event` is `Fn(AskEvent) + 'static` — Rc-cloneable, no Send
/// bounds (we're single-threaded wasm). Use a Leptos signal +
/// closure to update the UI, e.g.:
///
/// ```ignore
/// let (events, set_events) = signal::<Vec<AskEvent>>(Vec::new());
/// spawn_local(async move {
///     ask_stream("How does auth work?", move |ev| {
///         set_events.update(|v| v.push(ev));
///     }).await;
/// });
/// ```
/// #148 v2 — mirrors the backend `AskMode`. `Agent` is the
/// default (RAG + tools loop); `Direct` disables tools for the
/// @-menu directive wrappers whose prompt already carries the
/// source content.
#[derive(Debug, Clone, Copy, Default)]
pub enum AskMode {
    #[default]
    Agent,
    Direct,
}

impl AskMode {
    fn as_wire(&self) -> &'static str {
        match self {
            AskMode::Agent => "agent",
            AskMode::Direct => "direct",
        }
    }
}

pub async fn ask_stream<F>(question: &str, on_event: F) -> Result<(), AskError>
where
    F: Fn(AskEvent) + 'static,
{
    ask_stream_with_mode(question, AskMode::Agent, on_event).await
}

pub async fn ask_stream_with_mode<F>(
    question: &str,
    mode: AskMode,
    on_event: F,
) -> Result<(), AskError>
where
    F: Fn(AskEvent) + 'static,
{
    let body = serde_json::json!({
        "question": question,
        "mode": mode.as_wire(),
    })
    .to_string();

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_body(&JsValue::from_str(&body));

    let headers = Headers::new()
        .map_err(|e| AskError::Network(format!("Headers::new: {e:?}")))?;
    headers
        .set("Content-Type", "application/json")
        .map_err(|e| AskError::Network(format!("set content-type: {e:?}")))?;
    headers
        .set("Accept", "text/event-stream")
        .map_err(|e| AskError::Network(format!("set accept: {e:?}")))?;
    if let Some(token) = client::get_token() {
        headers
            .set("Authorization", &format!("Bearer {token}"))
            .map_err(|e| AskError::Network(format!("set auth: {e:?}")))?;
    }
    // #29 BYOK: if the user has registered their own Anthropic key in this
    // browser, forward it so the request runs under their key (cost is
    // theirs, operator caps don't apply). Browser-only — never persisted
    // server-side.
    if let Some(key) = get_byok_key() {
        headers
            .set("x-anthropic-key", &key)
            .map_err(|e| AskError::Network(format!("set byok: {e:?}")))?;
    }
    opts.set_headers(&headers);

    let request = Request::new_with_str_and_init("/api/v1/ask", &opts)
        .map_err(|e| AskError::Network(format!("Request::new: {e:?}")))?;

    let window = web_sys::window()
        .ok_or_else(|| AskError::Network("no window".to_string()))?;
    let resp_val = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| AskError::Network(format!("fetch: {e:?}")))?;
    let response: Response = resp_val
        .dyn_into()
        .map_err(|e| AskError::Network(format!("response cast: {e:?}")))?;

    let status = response.status();
    if !response.ok() {
        // Drain the body for the message — non-2xx responses are
        // small JSON ApiError envelopes, not SSE streams.
        let text_promise = response
            .text()
            .map_err(|e| AskError::Network(format!("text promise: {e:?}")))?;
        let body_val = JsFuture::from(text_promise)
            .await
            .unwrap_or_else(|_| JsValue::from_str(""));
        let body_str = body_val.as_string().unwrap_or_default();
        return Err(AskError::Http(status, body_str));
    }

    let stream = response
        .body()
        .ok_or_else(|| AskError::Network("no body stream".to_string()))?;
    let reader: ReadableStreamDefaultReader = stream
        .get_reader()
        .dyn_into()
        .map_err(|e| AskError::Network(format!("reader cast: {e:?}")))?;

    let decoder = TextDecoder::new()
        .map_err(|e| AskError::Network(format!("TextDecoder::new: {e:?}")))?;

    // Buffer for partial SSE frames straddling chunk boundaries.
    let buffer = Rc::new(RefCell::new(String::new()));
    let on_event_rc: Rc<dyn Fn(AskEvent)> = Rc::new(on_event);

    loop {
        let chunk_promise = reader.read();
        let chunk_val = JsFuture::from(chunk_promise)
            .await
            .map_err(|e| AskError::Network(format!("read: {e:?}")))?;

        // {value: Uint8Array, done: bool} — destructure via reflection.
        let done = js_sys::Reflect::get(&chunk_val, &JsValue::from_str("done"))
            .map(|v| v.as_bool().unwrap_or(false))
            .unwrap_or(false);
        if done {
            break;
        }

        let value = js_sys::Reflect::get(&chunk_val, &JsValue::from_str("value"))
            .map_err(|e| AskError::Network(format!("chunk value: {e:?}")))?;
        // The Uint8Array → string decode keeps multi-byte sequences
        // intact across chunk boundaries via the streaming option
        // ({stream: true}); the final flush after the loop catches
        // any trailing bytes.
        let uint8 = value
            .dyn_into::<js_sys::Uint8Array>()
            .map_err(|e| AskError::Network(format!("uint8 cast: {e:?}")))?;
        let opts = web_sys::TextDecodeOptions::new();
        opts.set_stream(true);
        let text = decoder
            .decode_with_buffer_source_and_options(&uint8, &opts)
            .map_err(|e| AskError::Network(format!("decode: {e:?}")))?;

        buffer.borrow_mut().push_str(&text);
        drain_frames(&buffer, &on_event_rc);
    }

    // Final flush — pass an empty input + stream=false to drain
    // any pending bytes from the decoder's internal state.
    let opts = web_sys::TextDecodeOptions::new();
    let empty: [u8; 0] = [];
    let tail = decoder
        .decode_with_u8_array_and_options(&empty, &opts)
        .unwrap_or_default();
    if !tail.is_empty() {
        buffer.borrow_mut().push_str(&tail);
    }
    drain_frames(&buffer, &on_event_rc);

    Ok(())
}

/// Walk the buffer looking for `\n\n`-separated SSE frames; dispatch
/// each complete frame and retain the partial tail for the next read.
fn drain_frames(buffer: &Rc<RefCell<String>>, on_event: &Rc<dyn Fn(AskEvent)>) {
    loop {
        let split_idx = buffer.borrow().find("\n\n");
        let Some(idx) = split_idx else { break };
        let mut buf = buffer.borrow_mut();
        let frame = buf[..idx].to_string();
        buf.replace_range(..idx + 2, "");
        drop(buf);

        if let Some(ev) = parse_frame(&frame) {
            on_event(ev);
        }
    }
}

/// Parse a single SSE frame (one or more lines, no separator).
/// Returns `None` for keepalive comments and malformed frames.
fn parse_frame(frame: &str) -> Option<AskEvent> {
    let mut event_name: Option<&str> = None;
    let mut data_lines: Vec<&str> = Vec::new();

    for line in frame.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            // SSE comment / keepalive — ignore.
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event_name = Some(rest.trim());
        } else if let Some(rest) = line.strip_prefix("data:") {
            // SSE spec strips at most one leading space; the backend
            // always emits "data: <value>" with one space, so trim
            // the leading space and join multi-line data with '\n'.
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // Other field types (id, retry) are not used by the backend.
    }

    let data = data_lines.join("\n");
    match event_name? {
        "status" => Some(AskEvent::Status(data)),
        "text" => Some(AskEvent::Text(data)),
        "source" => serde_json::from_str::<SourcePayload>(&data)
            .ok()
            .map(|p| AskEvent::Source {
                doc_id: p.doc_id,
                title: p.title,
                doc_type: p.doc_type.unwrap_or_else(|| "document".to_string()),
            }),
        "done" => Some(AskEvent::Done),
        "error" => Some(AskEvent::Error(data)),
        _ => None,
    }
}

/// Error variants surfaced to the caller. `Http(status, body)`
/// keeps the body string so the UI can render the server's reason
/// (rate-limit / quota / admin-disabled).
#[derive(Debug, Clone)]
pub enum AskError {
    Network(String),
    Http(u16, String),
}

impl std::fmt::Display for AskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AskError::Network(e) => write!(f, "network: {e}"),
            AskError::Http(s, b) => write!(f, "HTTP {s}: {b}"),
        }
    }
}

impl std::error::Error for AskError {}

// ─── #29: browser-only BYOK key storage ────────────────────────────────
//
// The user's personal Anthropic key lives ONLY in this browser's
// localStorage and is forwarded per-request via `x-anthropic-key`. It is
// never sent to or persisted by the OgreNotes server beyond the transient
// pass-through on `/ask`, and never logged.

const BYOK_STORAGE_KEY: &str = "anthropic_byok_key";

fn byok_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// The browser-stored BYOK key, trimmed; `None` if unset/blank or storage
/// is unavailable.
pub fn get_byok_key() -> Option<String> {
    let v = byok_storage()?.get_item(BYOK_STORAGE_KEY).ok()??;
    let v = v.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Persist (or, with a blank value, clear) the BYOK key in this browser.
pub fn set_byok_key(key: &str) {
    let Some(storage) = byok_storage() else { return };
    let trimmed = key.trim();
    if trimmed.is_empty() {
        let _ = storage.remove_item(BYOK_STORAGE_KEY);
    } else {
        let _ = storage.set_item(BYOK_STORAGE_KEY, trimmed);
    }
}

/// Remove the stored BYOK key.
pub fn clear_byok_key() {
    if let Some(storage) = byok_storage() {
        let _ = storage.remove_item(BYOK_STORAGE_KEY);
    }
}

/// A masked fingerprint of the stored key for display — last 4 chars only,
/// never the full key. `None` when no key is stored.
pub fn byok_fingerprint() -> Option<String> {
    get_byok_key().as_deref().map(mask_byok_key)
}

/// Mask a key to `…XXXX` (last 4 chars). Pure so the "never reveal the full
/// key" guarantee is unit-tested.
fn mask_byok_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let start = chars.len().saturating_sub(4);
    let last4: String = chars[start..].iter().collect();
    format!("…{last4}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_byok_key_reveals_only_last_four() {
        // #29: the fingerprint must never expose the full key.
        let masked = mask_byok_key("sk-ant-api03-SECRETSECRETwxyz");
        assert_eq!(masked, "…wxyz");
        assert!(!masked.contains("SECRET"));
    }

    #[test]
    fn mask_byok_key_handles_short_keys() {
        assert_eq!(mask_byok_key("ab"), "…ab");
        assert_eq!(mask_byok_key(""), "…");
    }

    fn collect_event(frame: &str) -> Option<String> {
        match parse_frame(frame)? {
            AskEvent::Status(s) | AskEvent::Text(s) | AskEvent::Error(s) => Some(s),
            AskEvent::Source { doc_id, title, doc_type } => {
                Some(format!("{doc_id}|{title}|{doc_type}"))
            }
            AskEvent::Done => Some("done".to_string()),
        }
    }

    #[test]
    fn parse_status_frame() {
        let frame = "event: status\ndata: Thinking (round 1)...";
        assert_eq!(
            collect_event(frame).as_deref(),
            Some("Thinking (round 1)...")
        );
    }

    #[test]
    fn parse_text_frame() {
        let frame = "event: text\ndata: The answer is 42.";
        assert_eq!(collect_event(frame).as_deref(), Some("The answer is 42."));
    }

    #[test]
    fn parse_source_frame() {
        let frame = r#"event: source
data: {"docId":"abc","title":"My Doc","docType":"spreadsheet"}"#;
        assert_eq!(
            collect_event(frame).as_deref(),
            Some("abc|My Doc|spreadsheet"),
        );
    }

    #[test]
    fn parse_source_frame_defaults_doc_type_when_missing() {
        // Backward-compat with stacks that haven't redeployed the
        // M-6.2 piece C backend (no docType field in the payload).
        // Frontend falls back to "document" rather than dropping
        // the citation.
        let frame = r#"event: source
data: {"docId":"abc","title":"Legacy Doc"}"#;
        assert_eq!(
            collect_event(frame).as_deref(),
            Some("abc|Legacy Doc|document"),
        );
    }

    #[test]
    fn parse_done_frame() {
        let frame = "event: done\ndata: done";
        assert_eq!(collect_event(frame).as_deref(), Some("done"));
    }

    #[test]
    fn parse_error_frame() {
        let frame = "event: error\ndata: claude api 429";
        assert_eq!(collect_event(frame).as_deref(), Some("claude api 429"));
    }

    #[test]
    fn parse_skips_keepalive_comment() {
        let frame = ": keep-alive";
        assert!(parse_frame(frame).is_none());
    }

    #[test]
    fn parse_skips_unknown_event_type() {
        let frame = "event: retry\ndata: 5000";
        assert!(parse_frame(frame).is_none());
    }

    #[test]
    fn parse_handles_multi_line_data() {
        // SSE spec allows multiple data: lines on one frame;
        // they're joined with '\n' when the consumer reads them.
        let frame = "event: text\ndata: line one\ndata: line two";
        assert_eq!(collect_event(frame).as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn parse_tolerates_crlf_line_endings() {
        let frame = "event: status\r\ndata: hello";
        assert_eq!(collect_event(frame).as_deref(), Some("hello"));
    }
}
