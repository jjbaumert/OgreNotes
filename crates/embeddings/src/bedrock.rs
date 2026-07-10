// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::Client as BedrockClient;

use crate::error::EmbeddingError;

/// Amazon Bedrock Titan Embed Text v2 client.
pub struct BedrockEmbedder {
    client: BedrockClient,
    model_id: String,
    dimensions: u32,
}

impl BedrockEmbedder {
    pub fn new(client: BedrockClient, model_id: String, dimensions: u32) -> Self {
        Self {
            client,
            model_id,
            dimensions,
        }
    }

    /// Embed a single text string. Returns a vector of `dimensions` floats.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let body = serde_json::json!({
            "inputText": text,
            "dimensions": self.dimensions,
            "normalize": true,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| EmbeddingError::Serialization(e.to_string()))?;

        let response = self
            .client
            .invoke_model()
            .model_id(&self.model_id)
            .content_type("application/json")
            .accept("application/json")
            .body(Blob::new(body_bytes))
            .send()
            .await
            .map_err(|e| EmbeddingError::Bedrock(e.to_string()))?;

        let response_bytes = response.body.into_inner();
        let response_json: serde_json::Value = serde_json::from_slice(&response_bytes)
            .map_err(|e| EmbeddingError::Serialization(e.to_string()))?;

        let embedding = response_json["embedding"]
            .as_array()
            .ok_or_else(|| {
                EmbeddingError::Serialization("missing 'embedding' field in response".to_string())
            })?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| {
                        EmbeddingError::Serialization("non-numeric embedding value".to_string())
                    })
            })
            .collect::<Result<Vec<f32>, _>>()?;

        Ok(embedding)
    }

    /// Embed multiple texts sequentially. Returns vectors in the same order.
    pub async fn embed_many(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_bedrockruntime::config::{BehaviorVersion, Credentials, Region};
    use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
    use aws_smithy_types::body::SdkBody;

    const MODEL_ID: &str = "amazon.titan-embed-text-v2:0";

    /// Build an embedder whose HTTP transport is a canned replay — no
    /// credentials resolution, no network, no live Bedrock.
    fn replay_embedder(
        events: Vec<ReplayEvent>,
        dimensions: u32,
    ) -> (BedrockEmbedder, StaticReplayClient) {
        let http_client = StaticReplayClient::new(events);
        let config = aws_sdk_bedrockruntime::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("AKID", "SECRET", None, None, "test"))
            .retry_config(aws_sdk_bedrockruntime::config::retry::RetryConfig::disabled())
            .http_client(http_client.clone())
            .build();
        let client = BedrockClient::from_conf(config);
        (
            BedrockEmbedder::new(client, MODEL_ID.to_string(), dimensions),
            http_client,
        )
    }

    /// A replay event returning `status` + `response_body`. The recorded
    /// request side is a placeholder; assertions on what was actually
    /// sent go through `StaticReplayClient::actual_requests`.
    fn event(status: u16, response_body: &str) -> ReplayEvent {
        ReplayEvent::new(
            http::Request::builder()
                .uri("https://bedrock-runtime.us-east-1.amazonaws.com/")
                .body(SdkBody::empty())
                .unwrap(),
            http::Response::builder()
                .status(status)
                .body(SdkBody::from(response_body.to_string()))
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn embed_parses_embedding_and_sends_expected_request_shape() {
        let (embedder, http_client) = replay_embedder(
            vec![event(
                200,
                r#"{"embedding": [0.25, -0.5, 1.0], "inputTextTokenCount": 3}"#,
            )],
            512,
        );

        let vector = embedder.embed("hello world").await.expect("embed");
        assert_eq!(vector, vec![0.25_f32, -0.5, 1.0]);

        // Pin the wire shape of the InvokeModel request.
        let requests: Vec<_> = http_client.actual_requests().collect();
        assert_eq!(requests.len(), 1);
        let uri = requests[0].uri().to_string();
        assert!(
            uri.contains("/model/amazon.titan-embed-text-v2"),
            "unexpected uri: {uri}"
        );
        let body: serde_json::Value =
            serde_json::from_slice(requests[0].body().bytes().expect("in-memory body"))
                .expect("request body is JSON");
        assert_eq!(body["inputText"], "hello world");
        assert_eq!(body["dimensions"], 512);
        assert_eq!(body["normalize"], true);
    }

    #[tokio::test]
    async fn embed_missing_embedding_field_maps_to_serialization_error() {
        let (embedder, _http) =
            replay_embedder(vec![event(200, r#"{"inputTextTokenCount": 3}"#)], 512);

        let err = embedder.embed("text").await.expect_err("should fail");
        match err {
            EmbeddingError::Serialization(msg) => {
                assert!(msg.contains("missing 'embedding' field"), "msg: {msg}")
            }
            other => panic!("expected Serialization error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn embed_non_numeric_embedding_value_maps_to_serialization_error() {
        let (embedder, _http) = replay_embedder(
            vec![event(200, r#"{"embedding": [0.1, "oops", 0.3]}"#)],
            512,
        );

        let err = embedder.embed("text").await.expect_err("should fail");
        match err {
            EmbeddingError::Serialization(msg) => {
                assert!(msg.contains("non-numeric"), "msg: {msg}")
            }
            other => panic!("expected Serialization error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn embed_non_json_response_maps_to_serialization_error() {
        let (embedder, _http) = replay_embedder(vec![event(200, "not json at all")], 512);

        let err = embedder.embed("text").await.expect_err("should fail");
        assert!(
            matches!(err, EmbeddingError::Serialization(_)),
            "expected Serialization error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn embed_service_error_maps_to_bedrock_error() {
        let (embedder, _http) = replay_embedder(
            vec![event(
                400,
                r#"{"message": "validation error: input too long"}"#,
            )],
            512,
        );

        let err = embedder.embed("text").await.expect_err("should fail");
        assert!(
            matches!(err, EmbeddingError::Bedrock(_)),
            "expected Bedrock error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn embed_many_preserves_input_order() {
        let (embedder, http_client) = replay_embedder(
            vec![
                event(200, r#"{"embedding": [1.0, 2.0]}"#),
                event(200, r#"{"embedding": [3.0, 4.0]}"#),
            ],
            2,
        );

        let vectors = embedder
            .embed_many(&["first", "second"])
            .await
            .expect("embed_many");
        assert_eq!(vectors, vec![vec![1.0_f32, 2.0], vec![3.0_f32, 4.0]]);

        // One InvokeModel call per input, in input order.
        let bodies: Vec<serde_json::Value> = http_client
            .actual_requests()
            .map(|r| serde_json::from_slice(r.body().bytes().unwrap()).unwrap())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["inputText"], "first");
        assert_eq!(bodies[1]["inputText"], "second");
    }

    #[tokio::test]
    async fn embed_many_stops_at_first_failure() {
        let (embedder, http_client) = replay_embedder(
            vec![
                event(200, r#"{"embedding": [1.0]}"#),
                event(500, r#"{"message": "internal error"}"#),
                event(200, r#"{"embedding": [2.0]}"#),
            ],
            1,
        );

        let err = embedder
            .embed_many(&["a", "b", "c"])
            .await
            .expect_err("second call fails");
        assert!(matches!(err, EmbeddingError::Bedrock(_)));
        // Sequential fail-fast: the third input is never sent.
        assert_eq!(http_client.actual_requests().count(), 2);
    }

    #[tokio::test]
    async fn embed_many_with_no_inputs_makes_no_calls() {
        let (embedder, http_client) = replay_embedder(Vec::new(), 512);

        let vectors = embedder.embed_many(&[]).await.expect("empty input ok");
        assert!(vectors.is_empty());
        assert_eq!(http_client.actual_requests().count(), 0);
    }
}
