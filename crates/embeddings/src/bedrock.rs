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
