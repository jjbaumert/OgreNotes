//! Redis integration for collaboration: pubsub fanout and single-use token store.

use fred::prelude::*;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Redis-backed pubsub and token store for collaboration.
pub struct RedisPubSub {
    client: Arc<RedisClient>,
}

impl RedisPubSub {
    /// Create a new RedisPubSub from a connected Redis client.
    pub fn new(client: Arc<RedisClient>) -> Self {
        Self { client }
    }

    /// Store a single-use WebSocket authentication token.
    /// The token maps to `user_id:doc_id` and expires after `ttl_secs`.
    pub async fn store_ws_token(
        &self,
        token: &str,
        user_id: &str,
        doc_id: &str,
        ttl_secs: u64,
    ) -> Result<(), RedisError> {
        // Use \0 as delimiter to avoid issues with user_id/doc_id containing ":"
        let value = format!("{user_id}\0{doc_id}");
        let key = format!("ws_token:{token}");
        self.client
            .set::<(), _, _>(&key, value.as_str(), Some(Expiration::EX(ttl_secs as i64)), None, false)
            .await?;
        Ok(())
    }

    /// Validate and consume a single-use WebSocket token.
    /// Returns (user_id, doc_id) if valid, None if expired or already used.
    pub async fn validate_ws_token(
        &self,
        token: &str,
    ) -> Result<Option<(String, String)>, RedisError> {
        let key = format!("ws_token:{token}");
        // GET + DEL atomically (single-use)
        let value: Option<String> = self.client.getdel(&key).await?;
        Ok(value.and_then(|v| {
            let mut parts = v.splitn(2, '\0');
            let user_id = parts.next()?.to_string();
            let doc_id = parts.next()?.to_string();
            Some((user_id, doc_id))
        }))
    }

    /// Publish a document update to the Redis channel for multi-instance fanout.
    pub async fn publish_update(
        &self,
        doc_id: &str,
        data: &[u8],
    ) -> Result<(), RedisError> {
        let channel = format!("doc:{doc_id}");
        self.client.publish::<(), _, _>(&channel, data).await?;
        Ok(())
    }

    /// Get a reference to the underlying Redis client.
    pub fn client(&self) -> &Arc<RedisClient> {
        &self.client
    }
}
