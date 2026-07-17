// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Claude Messages API client using reqwest.
//!
//! Non-streaming requests only — the agent loop in `routes/ask.rs`
//! needs the complete response of each tool-calling round before it
//! can act on it.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

// ─── Error ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("unexpected response format: {0}")]
    Parse(String),
}

// ─── Request types ─────────────────────────────────────────────

/// An ephemeral prompt-cache breakpoint. Anthropic bills a cached prefix at
/// ~0.1x on a read and ~1.25x on the write, so caching pays off after the
/// second request that shares the prefix. The `/ask` agent loop always
/// qualifies: it makes up to `MAX_TOOL_ROUNDS + 1` calls per question, each
/// re-sending the same tools + system prompt and a growing message history
/// (which accumulates up to `MAX_DOCUMENT_CHARS` of document content).
#[derive(Serialize, Clone, Copy)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl CacheControl {
    const EPHEMERAL: CacheControl = CacheControl { kind: "ephemeral" };
}

/// The system prompt serialized as a one-element text-block array so it can
/// carry a cache breakpoint. Render order is tools → system → messages, so a
/// breakpoint here caches the whole tools+system prefix — shared by every
/// `/ask` call, giving cross-request reuse within the cache TTL. A prefix
/// below the model's minimum cacheable size simply isn't cached (no write,
/// no premium), so this is never a cost regression.
#[derive(Serialize)]
struct SystemBlock<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
    cache_control: CacheControl,
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: [SystemBlock<'a>; 1],
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<&'a Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    /// Top-level auto-cache: the API places a breakpoint on the last cacheable
    /// block — the tail of the growing message history — so each agent-loop
    /// round reads the prior round's accumulated tool results and document
    /// content from cache instead of re-billing them. Only set for the
    /// tool-calling loop (`tools` non-empty). A single-shot Direct call
    /// (`@summarize`/`@translate`/…) passes no tools and is never followed by
    /// a request that shares its history, so caching it would only pay the
    /// write premium with no later read. (The rare max-rounds synthesis
    /// fallback also passes no tools and thus forgoes the history-read hit.)
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// Build the wire request, wiring in the prompt-cache breakpoints. Extracted
/// from `messages` so the caching behavior is unit-testable without an HTTP
/// round trip.
fn build_messages_request<'a>(
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [Message],
    tools: &'a [Tool],
) -> MessagesRequest<'a> {
    MessagesRequest {
        model,
        max_tokens,
        system: [SystemBlock {
            kind: "text",
            text: system,
            cache_control: CacheControl::EPHEMERAL,
        }],
        messages,
        tools: tools.iter().collect(),
        stream: None,
        cache_control: (!tools.is_empty()).then_some(CacheControl::EPHEMERAL),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },

    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// ─── Response types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub content: Vec<ResponseBlock>,
    pub stop_reason: Option<String>,
    /// Token usage for this call. Anthropic returns this on every
    /// non-streaming response. Optional in the struct so test
    /// fixtures that pre-date the field still deserialize cleanly.
    /// The agent loop sums these across rounds to surface a
    /// per-question total via the Usage SSE event.
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

// ─── Client ────────────────────────────────────────────────────

/// Abstraction over the Claude messages endpoint. Production code uses
/// `ClaudeClient`, which talks to api.anthropic.com over HTTP. Tests
/// substitute their own implementation to script tool-call sequences
/// and assert on the conversation Claude saw — see
/// crates/api/tests/test_ask_acl.rs.
///
/// Only the non-streaming `messages` path exists; that's what the agent
/// loop in `routes/ask.rs` calls.
#[async_trait]
pub trait ClaudeMessages: Send + Sync {
    async fn messages(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError>;
}

pub struct ClaudeClient {
    http: Client,
    api_key: String,
    model: String,
}

impl ClaudeClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            http: Client::new(),
            api_key,
            model,
        }
    }

    /// Send a non-streaming messages request.
    /// Used for tool-calling rounds where we need the complete response.
    pub async fn messages(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError> {
        let body = build_messages_request(&self.model, max_tokens, system, messages, tools);

        let resp = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClaudeError::Api {
                status,
                message: text,
            });
        }

        resp.json::<MessagesResponse>()
            .await
            .map_err(|e| ClaudeError::Parse(e.to_string()))
    }

}

#[async_trait]
impl ClaudeMessages for ClaudeClient {
    async fn messages(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[Tool],
        max_tokens: u32,
    ) -> Result<MessagesResponse, ClaudeError> {
        // Delegate to the inherent method so callers that already hold
        // a concrete &ClaudeClient keep working unchanged.
        ClaudeClient::messages(self, system, messages, tools, max_tokens).await
    }
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_use_response_parsing() {
        let json = r#"{
            "content": [
                {"type": "tool_use", "id": "toolu_01", "name": "keyword_search", "input": {"query": "auth"}}
            ],
            "stop_reason": "tool_use"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.content.len(), 1);
        match &resp.content[0] {
            ResponseBlock::ToolUse { name, input, .. } => {
                assert_eq!(name, "keyword_search");
                assert_eq!(input["query"], "auth");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn test_text_response_parsing() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Here is the answer based on the documents I found."}
            ],
            "stop_reason": "end_turn"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason.as_deref(), Some("end_turn"));
        match &resp.content[0] {
            ResponseBlock::Text { text } => {
                assert!(text.starts_with("Here is"));
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_mixed_response_parsing() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Let me search for that."},
                {"type": "tool_use", "id": "toolu_02", "name": "semantic_search", "input": {"query": "token refresh"}}
            ],
            "stop_reason": "tool_use"
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.content.len(), 2);
    }

    #[test]
    fn test_message_serialization() {
        let msg = Message {
            role: "user".to_string(),
            content: MessageContent::Text("Hello".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"Hello\""));
    }

    #[test]
    fn test_tool_result_serialization() {
        let msg = Message {
            role: "user".to_string(),
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01".to_string(),
                content: "[{\"doc_id\":\"abc\"}]".to_string(),
                is_error: None,
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("tool_result"));
        assert!(json.contains("toolu_01"));
    }

    #[test]
    fn agent_loop_request_sets_cache_breakpoints() {
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text("hello".to_string()),
        }];
        let tools = vec![Tool {
            name: "keyword_search".to_string(),
            description: "Search".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
        }];
        let body =
            build_messages_request("claude-haiku-4-5", 4096, "system prompt", &messages, &tools);
        let v = serde_json::to_value(&body).unwrap();

        // System is a text-block array whose block carries an ephemeral
        // breakpoint — caches the tools+system prefix.
        assert_eq!(v["system"][0]["type"], "text");
        assert_eq!(v["system"][0]["text"], "system prompt");
        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");

        // Top-level breakpoint present for the tool loop → the API caches the
        // growing message-history prefix round to round.
        assert_eq!(v["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn direct_call_caches_system_but_not_history() {
        // Direct mode (@summarize etc.) passes no tools and makes a single
        // call, so caching the message tail would only pay the write premium
        // with no later read. The system prefix is still cached.
        let messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text("summarize this".to_string()),
        }];
        let body = build_messages_request("claude-haiku-4-5", 4096, "system", &messages, &[]);
        let v = serde_json::to_value(&body).unwrap();

        assert_eq!(v["system"][0]["cache_control"]["type"], "ephemeral");
        assert!(
            v.get("cache_control").is_none(),
            "no top-level cache breakpoint for single-shot Direct calls"
        );
    }

    #[test]
    fn test_tool_definition_serialization() {
        let tool = Tool {
            name: "keyword_search".to_string(),
            description: "Search documents".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("keyword_search"));
        assert!(json.contains("input_schema"));
    }
}
