// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Cross-user leak coverage for `/api/v1/ask`.
//!
//! The agentic Q&A endpoint runs an LLM loop that calls
//! `keyword_search`, `semantic_search`, `get_document`, and
//! `get_related` tools. Each tool gates on `check_doc_access(...
//! AccessLevel::View)` — the same ACL the rest of the API uses. This
//! file pins that integration end-to-end with a stubbed Claude that
//! deterministically tries to access another user's document, then
//! asserts (a) the tool results returned to Claude never contain the
//! foreign content, and (b) the SSE response stream never echoes a
//! unique phrase from that content.
//!
//! Without these tests the agent loop is the most plausible
//! exfiltration path: a bug in `check_doc_access` could leak phrases
//! the model summarized rather than echoed verbatim, slipping past
//! coarser "did the API return user B's title to user A" checks.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use http_body_util::BodyExt;
use hyper::{Method, Request};
use tower::ServiceExt;

use async_trait::async_trait;
use ogrenotes_api::claude::{
    ClaudeError, ClaudeMessages, Message, MessageContent, MessagesResponse, ResponseBlock, Tool,
};
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{ReadTxn, Text, Transact, WriteTxn};

/// Build Y.Doc state bytes containing a single paragraph with the
/// given text. Mirrors the helper in `test_search.rs` so the search
/// indexer sees a real document the way the route would render one.
fn make_doc_bytes(text: &str) -> Vec<u8> {
    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        let p = frag.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
        let t = p.insert(&mut txn, 0, XmlTextPrelim::new(""));
        t.push(&mut txn, text);
    }
    let txn = doc.transact();
    txn.encode_state_as_update_v1(&yrs::StateVector::default())
}

/// A scripted ClaudeMessages impl. Returns canned responses in order
/// and records every (system, messages) pair Claude was called with so
/// the test can assert what tool_result content the agent returned to
/// the model.
///
/// **Why the stub does NOT panic when the script is exhausted:** the
/// agent loop runs the Claude call inside `tokio::spawn` (see
/// `routes/ask.rs::ask`). A panic in a spawned task is caught by the
/// runtime and dropped silently — the SSE response has already
/// returned its 200 by the time the spawn fires, so the test's
/// `assert_eq!(status, 200)` does not surface the panic. We instead
/// return a recognizable `ClaudeError::Api` and bump an atomic
/// over-call counter; `assert_fully_consumed()` reads both the
/// counter and the unconsumed-script length so a script-length mismatch
/// produces a clear error message instead of a panic the runtime ate.
struct ScriptedClaude {
    responses: Mutex<Vec<MessagesResponse>>,
    /// All `messages` lists Claude saw, snapshotted in call order.
    /// `recorded_calls[N]` is the conversation as of call N+1.
    recorded_calls: Mutex<Vec<Vec<Message>>>,
    /// Bumped when `messages()` is called after the script has been
    /// exhausted. The test asserts this is zero after `ask_collect`.
    over_calls: AtomicUsize,
}

impl ScriptedClaude {
    fn new(responses: Vec<MessagesResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            recorded_calls: Mutex::new(Vec::new()),
            over_calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> Vec<Vec<Message>> {
        self.recorded_calls.lock().unwrap().clone()
    }

    /// Verify the agent consumed exactly the scripted number of
    /// responses — neither too many nor too few. Call after the SSE
    /// stream has fully drained. Panics inside the test thread (not the
    /// agent's spawned task), so the failure message reaches the
    /// reporter.
    fn assert_fully_consumed(&self) {
        let over = self.over_calls.load(Ordering::SeqCst);
        let remaining = self.responses.lock().unwrap().len();
        assert_eq!(
            over, 0,
            "ScriptedClaude was called {over} time(s) after the script was exhausted; \
             agent loop ran more rounds than expected",
        );
        assert_eq!(
            remaining, 0,
            "ScriptedClaude has {remaining} canned response(s) left over; \
             agent loop ran fewer rounds than expected",
        );
    }
}

#[async_trait]
impl ClaudeMessages for ScriptedClaude {
    async fn messages(
        &self,
        _system: &str,
        messages: &[Message],
        _tools: &[Tool],
        _max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError> {
        self.recorded_calls
            .lock()
            .unwrap()
            .push(messages.to_vec());
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            // Don't panic — the agent loop runs in tokio::spawn and the
            // runtime swallows panics there. Bump the over-call counter
            // and return an Err so the loop terminates cleanly; the
            // test's assert_fully_consumed() will surface the mismatch.
            self.over_calls.fetch_add(1, Ordering::SeqCst);
            return Err(ClaudeError::Api {
                status: 0,
                message: "ScriptedClaude: ran out of canned responses".to_string(),
            });
        }
        Ok(q.remove(0))
    }
}

fn tool_use(id: &str, name: &str, input: serde_json::Value) -> ResponseBlock {
    ResponseBlock::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input,
    }
}

fn text_block(text: &str) -> ResponseBlock {
    ResponseBlock::Text {
        text: text.to_string(),
    }
}

fn response(blocks: Vec<ResponseBlock>) -> MessagesResponse {
    MessagesResponse {
        content: blocks,
        stop_reason: None,
        usage: None,
    }
}

/// Send POST /api/v1/ask and read the entire SSE body as one byte
/// vector — small responses fit in memory; the agent script keeps
/// them short.
async fn ask_collect(
    app: &common::TestApp,
    token: &str,
    question: &str,
) -> (u16, Vec<u8>) {
    let body = serde_json::json!({ "question": question });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ask")
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// Pull the tool_result content string out of the assistant/user
/// conversation that Claude saw on its `index`-th turn. Walks the
/// last user message in `messages[index]` and returns the content of
/// its first `ToolResult` block.
fn last_tool_result(messages: &[Message]) -> Option<String> {
    let last = messages.last()?;
    if last.role != "user" {
        return None;
    }
    if let MessageContent::Blocks(blocks) = &last.content {
        for b in blocks {
            if let ogrenotes_api::claude::ContentBlock::ToolResult { content, .. } = b {
                return Some(content.clone());
            }
        }
    }
    None
}

const SECRET: &str = "OWNER_B_SECRET_TOKEN_XYZ";

/// User A asks; the stubbed Claude tries to read user B's private
/// doc via `keyword_search` and `get_document`. Both tool results
/// returned to the model must show denial (empty list / canned error
/// string) — the SSE stream must not contain the secret phrase.
#[tokio::test]
async fn test_ask_does_not_leak_unshared_doc_content() {
    common::require_infra!();

    // We bake the doc_id into the stub *after* doc creation. Keep it
    // behind an Arc<Mutex<Option<String>>> so the stub closure can
    // read it at run time without us having to know the id at script
    // build time.
    let target_doc_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Round 1: ask Claude — Claude requests keyword_search.
    // Round 2: keyword_search came back (filtered to []) — Claude tries
    //          get_document on the secret doc anyway.
    // Round 3: get_document came back (access denied) — Claude gives
    //          up and writes a final text answer.
    let script = vec![
        response(vec![tool_use(
            "tu-1",
            "keyword_search",
            serde_json::json!({ "query": SECRET }),
        )]),
        response(vec![tool_use(
            "tu-2",
            "get_document",
            // Filled in below after we know the doc id.
            serde_json::json!({ "doc_id": "PLACEHOLDER" }),
        )]),
        response(vec![text_block("I could not find that information.")]),
    ];
    let stub = Arc::new(ScriptedClaude::new(script));
    let stub_for_state: Arc<dyn ClaudeMessages> = stub.clone();

    let app = common::TestApp::new_with_claude(Some(stub_for_state)).await;

    // User A is the asker; user B owns the secret doc.
    let (_, token_a) = app.create_user("alice@test.com").await;
    let (_, token_b) = app.create_user("bob@test.com").await;

    let secret_doc_id = app.create_doc(&token_b, "Bob Secrets", None).await;
    *target_doc_id.lock().unwrap() = Some(secret_doc_id.clone());

    // PUT some content into B's doc that contains the unique phrase.
    // The route auto-indexes so keyword_search would find it absent the
    // ACL filter — making this a meaningful leak test.
    let bytes = make_doc_bytes(&format!("preamble {SECRET} trailing"));
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{secret_doc_id}/content"),
            Some(&token_b),
            bytes,
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204, "B should be able to PUT content on its own doc");
    // Search indexing is fire-and-forget after PUT /content; give it a
    // moment to land before keyword_search is invoked.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // Patch the script's get_document doc_id to the real id we just
    // learned. Replace the response queue's second element directly.
    {
        let mut q = stub.responses.lock().unwrap();
        q[1] = response(vec![tool_use(
            "tu-2",
            "get_document",
            serde_json::json!({ "doc_id": secret_doc_id }),
        )]);
    }

    // A calls /api/v1/ask. Stub drives the agent through the scripted
    // sequence above.
    let (status, sse_bytes) = ask_collect(&app, &token_a, "Tell me about the secret").await;
    assert_eq!(status, 200, "ask should return 200 OK with SSE body");

    // Surface a script-length mismatch *before* the per-round
    // assertions so the failure message is "ran out / surplus" rather
    // than a misleading "calls[1] was None" downstream.
    stub.assert_fully_consumed();

    let calls = stub.calls();
    assert_eq!(
        calls.len(),
        3,
        "stub should have been called exactly 3 times (round 1 + tool feedback × 2)",
    );

    // Round 2 input: the keyword_search tool result Claude saw.
    // It must be the empty array — B's doc must NOT appear in the
    // results because A has no access.
    let kw_result =
        last_tool_result(&calls[1]).expect("expected keyword_search tool result on round 2");
    assert_eq!(
        kw_result, "[]",
        "keyword_search must return [] to the model when only B owns the matching doc; got: {kw_result}",
    );

    // Round 3 input: the get_document tool result Claude saw.
    // It must be the canned access-denied error, NOT the doc content.
    let gd_result =
        last_tool_result(&calls[2]).expect("expected get_document tool result on round 3");
    assert!(
        gd_result.contains("not found or access denied"),
        "get_document tool result must be access-denied error, got: {gd_result}",
    );
    assert!(
        !gd_result.contains(SECRET),
        "get_document tool result must NOT contain the secret phrase; got: {gd_result}",
    );

    // SSE response must never carry the secret. This is the strongest
    // leak guarantee: even if Claude hallucinated B's content (it can't,
    // because the model never received it), the bytes a client would
    // see don't include it.
    let sse_str = String::from_utf8_lossy(&sse_bytes);
    assert!(
        !sse_str.contains(SECRET),
        "SSE response must not echo the secret; got body:\n{sse_str}",
    );
    // No source event for the inaccessible doc.
    let source_marker = format!("\"docId\":\"{secret_doc_id}\"");
    assert!(
        !sse_str.contains(&source_marker),
        "SSE response must not emit a source event for the inaccessible doc; got body:\n{sse_str}",
    );

    app.cleanup().await;
}

/// Sister test: same scripted sequence, but B *shares* the doc to A
/// at View. Now the agent SHOULD see B's content — assertions flip.
/// This catches a regression where the ACL is too tight (denies docs
/// the user actually has access to).
#[tokio::test]
async fn test_ask_returns_shared_doc_content() {
    common::require_infra!();

    let target_doc_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let script = vec![
        response(vec![tool_use(
            "tu-1",
            "keyword_search",
            serde_json::json!({ "query": SECRET }),
        )]),
        response(vec![tool_use(
            "tu-2",
            "get_document",
            serde_json::json!({ "doc_id": "PLACEHOLDER" }),
        )]),
        response(vec![text_block("Done.")]),
    ];
    let stub = Arc::new(ScriptedClaude::new(script));
    let stub_for_state: Arc<dyn ClaudeMessages> = stub.clone();

    let app = common::TestApp::new_with_claude(Some(stub_for_state)).await;

    let (alice_id, token_a) = app.create_user("alice@test.com").await;
    let (_, token_b) = app.create_user("bob@test.com").await;

    let shared_doc_id = app.create_doc(&token_b, "Shared", None).await;
    *target_doc_id.lock().unwrap() = Some(shared_doc_id.clone());

    // B fills D with the secret phrase and shares to A as a viewer.
    let bytes = make_doc_bytes(&format!("preamble {SECRET} trailing"));
    app.bytes_request(
        Method::PUT,
        &format!("/api/v1/documents/{shared_doc_id}/content"),
        Some(&token_b),
        bytes,
        "application/octet-stream",
    )
    .await;

    let share_body = serde_json::json!({ "userId": alice_id, "accessLevel": "VIEW" });
    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{shared_doc_id}/members"),
            Some(&token_b),
            Some(share_body),
        )
        .await;
    assert_eq!(status, 204, "B should be able to share its doc to A");

    {
        let mut q = stub.responses.lock().unwrap();
        q[1] = response(vec![tool_use(
            "tu-2",
            "get_document",
            serde_json::json!({ "doc_id": shared_doc_id }),
        )]);
    }

    let (status, sse_bytes) = ask_collect(&app, &token_a, "Tell me about the secret").await;
    assert_eq!(status, 200);

    stub.assert_fully_consumed();

    let calls = stub.calls();
    assert_eq!(calls.len(), 3);

    // keyword_search result this time SHOULD include the shared doc.
    let kw_result =
        last_tool_result(&calls[1]).expect("expected keyword_search tool result on round 2");
    assert!(
        kw_result.contains(&shared_doc_id),
        "keyword_search must return the shared doc to A; got: {kw_result}",
    );

    // get_document result this time SHOULD include the secret content.
    let gd_result =
        last_tool_result(&calls[2]).expect("expected get_document tool result on round 3");
    assert!(
        gd_result.contains(SECRET),
        "get_document must return the shared doc's content (including the secret) to A; got: {gd_result}",
    );

    // The SSE stream emits a source event for the shared doc.
    let sse_str = String::from_utf8_lossy(&sse_bytes);
    let source_marker = format!("\"docId\":\"{shared_doc_id}\"");
    assert!(
        sse_str.contains(&source_marker),
        "SSE response should emit a source event for the shared doc; got body:\n{sse_str}",
    );

    app.cleanup().await;
}

/// #59 T-11: the newly-registered `list_documents` agent tool enumerates
/// the caller's own documents end to end. Scripted Claude requests it
/// once, then answers. The tool result the model sees must list the docs
/// the asker owns — and must NOT include another user's document (the
/// listing is scoped to owned docs, the access-safe subset).
#[tokio::test]
async fn test_ask_list_documents_lists_only_owned_docs() {
    common::require_infra!();

    let script = vec![
        response(vec![tool_use("tu-1", "list_documents", serde_json::json!({}))]),
        response(vec![text_block("Here are your documents.")]),
    ];
    let stub = Arc::new(ScriptedClaude::new(script));
    let stub_for_state: Arc<dyn ClaudeMessages> = stub.clone();
    let app = common::TestApp::new_with_claude(Some(stub_for_state)).await;

    let (_, token_a) = app.create_user("lister@test.com").await;
    let (_, token_b) = app.create_user("other-owner@test.com").await;

    app.create_doc(&token_a, "Alpha Notes", None).await;
    app.create_doc(&token_a, "Beta Plan", None).await;
    // Owned by B — must never appear in A's listing.
    app.create_doc(&token_b, "Carol Private", None).await;

    let (status, _sse) = ask_collect(&app, &token_a, "What documents do I have?").await;
    assert_eq!(status, 200, "ask should return 200 OK");

    stub.assert_fully_consumed();
    let calls = stub.calls();
    assert_eq!(calls.len(), 2, "round 1 + one tool-feedback round");

    let result =
        last_tool_result(&calls[1]).expect("expected list_documents tool result on round 2");
    assert!(result.contains("Alpha Notes"), "A's doc must be listed; got: {result}");
    assert!(result.contains("Beta Plan"), "A's doc must be listed; got: {result}");
    assert!(
        !result.contains("Carol Private"),
        "another owner's doc must NOT be listed; got: {result}",
    );

    app.cleanup().await;
}

// ─── Stub self-check ────────────────────────────────────────────
//
// Pure unit tests that prove `assert_fully_consumed` actually catches
// the failure modes it claims to. Without these the guard is just a
// claim — these are the meta-test.

#[tokio::test]
#[should_panic(expected = "canned response(s) left over")]
async fn assert_fully_consumed_catches_under_call() {
    let stub = ScriptedClaude::new(vec![response(vec![text_block("unused")])]);
    // Caller never invokes messages() — script left at length 1.
    stub.assert_fully_consumed();
}

#[tokio::test]
#[should_panic(expected = "after the script was exhausted")]
async fn assert_fully_consumed_catches_over_call() {
    let stub = ScriptedClaude::new(vec![response(vec![text_block("only response")])]);
    // First call consumes the only response; second call hits the
    // exhausted-script branch and bumps over_calls.
    let _ = ClaudeMessages::messages(&stub, "", &[], &[], 1).await;
    let _ = ClaudeMessages::messages(&stub, "", &[], &[], 1).await;
    stub.assert_fully_consumed();
}

#[tokio::test]
async fn assert_fully_consumed_passes_when_script_balanced() {
    let stub = ScriptedClaude::new(vec![response(vec![text_block("only response")])]);
    let _ = ClaudeMessages::messages(&stub, "", &[], &[], 1).await;
    stub.assert_fully_consumed(); // must not panic
}
