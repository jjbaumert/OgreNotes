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

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<&'a Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
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
        let tool_refs: Vec<&Tool> = tools.iter().collect();
        let body = MessagesRequest {
            model: &self.model,
            max_tokens,
            system,
            messages,
            tools: tool_refs,
            stream: None,
        };

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
