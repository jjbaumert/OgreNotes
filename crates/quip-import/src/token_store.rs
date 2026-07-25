//! `TokenStore` — where a Quip personal access token lives during an
//! import. Prod (`SsmTokenStore`) writes the token to SSM Parameter Store
//! as a KMS-backed `SecureString`; local dev has no SSM, so
//! `InMemoryTokenStore` keeps it in-process instead. In both impls the
//! token value must never appear in a log, an error, or a `Debug` output
//! — see `secret::QuipToken`, which this module never formats directly.

use async_trait::async_trait;
use dashmap::DashMap;
use secrecy::{ExposeSecret, Secret};

use crate::secret::QuipToken;

// ─── Error ─────────────────────────────────────────────────────

/// Errors carry only the `import_id` and the SDK error's own `Display`
/// (status/kind/request-id metadata) — never a parameter *value*. The SSM
/// SDK's error types never include the SecureString value in their
/// Display/Debug output (AWS redacts it service-side), so this is safe by
/// construction, not by our own scrubbing.
#[derive(Debug, thiserror::Error)]
pub enum TokenStoreError {
    #[error("SSM error for import {import_id}: {kind}")]
    Ssm { import_id: String, kind: String },
}

fn ssm_err(import_id: &str, err: impl std::fmt::Display) -> TokenStoreError {
    TokenStoreError::Ssm {
        import_id: import_id.to_string(),
        kind: err.to_string(),
    }
}

// ─── Trait ─────────────────────────────────────────────────────

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn put(&self, import_id: &str, token: &QuipToken) -> Result<(), TokenStoreError>;
    async fn get(&self, import_id: &str) -> Result<Option<QuipToken>, TokenStoreError>;
    async fn delete(&self, import_id: &str) -> Result<(), TokenStoreError>;
}

// ─── In-memory (dev) ───────────────────────────────────────────

/// Single-process, in-memory token store for local dev — the local stack
/// has no SSM. Values are wrapped in `secrecy::Secret` so they zeroize on
/// drop; nothing here ever formats a value into a log or error.
#[derive(Default)]
pub struct InMemoryTokenStore {
    tokens: DashMap<String, Secret<String>>,
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl TokenStore for InMemoryTokenStore {
    async fn put(&self, import_id: &str, token: &QuipToken) -> Result<(), TokenStoreError> {
        self.tokens
            .insert(import_id.to_string(), Secret::new(token.expose().to_string()));
        Ok(())
    }

    async fn get(&self, import_id: &str) -> Result<Option<QuipToken>, TokenStoreError> {
        Ok(self
            .tokens
            .get(import_id)
            .map(|entry| QuipToken::new(entry.expose_secret().clone())))
    }

    async fn delete(&self, import_id: &str) -> Result<(), TokenStoreError> {
        self.tokens.remove(import_id);
        Ok(())
    }
}

// ─── SSM (prod) ────────────────────────────────────────────────

/// SSM Parameter Store-backed token store — prod. Each import's token
/// lives at `<prefix>import/<import_id>/quip-token` as a KMS-backed
/// `SecureString` parameter. `put` overwrites (a re-connect replaces the
/// prior token); `get`/`delete` treat a missing parameter as `None`/`Ok`
/// rather than an error, matching the trait contract.
pub struct SsmTokenStore {
    client: aws_sdk_ssm::Client,
    prefix: String,
}

impl SsmTokenStore {
    pub fn new(client: aws_sdk_ssm::Client, prefix: String) -> Self {
        Self { client, prefix }
    }

    fn param_name(&self, import_id: &str) -> String {
        format!("{}import/{}/quip-token", self.prefix, import_id)
    }
}

#[async_trait]
impl TokenStore for SsmTokenStore {
    async fn put(&self, import_id: &str, token: &QuipToken) -> Result<(), TokenStoreError> {
        self.client
            .put_parameter()
            .name(self.param_name(import_id))
            .value(token.expose())
            .r#type(aws_sdk_ssm::types::ParameterType::SecureString)
            .overwrite(true)
            .send()
            .await
            .map_err(|e| ssm_err(import_id, e.into_service_error()))?;
        Ok(())
    }

    async fn get(&self, import_id: &str) -> Result<Option<QuipToken>, TokenStoreError> {
        match self
            .client
            .get_parameter()
            .name(self.param_name(import_id))
            .with_decryption(true)
            .send()
            .await
        {
            Ok(output) => Ok(output
                .parameter()
                .and_then(|p| p.value())
                .map(|v| QuipToken::new(v.to_string()))),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_parameter_not_found() {
                    Ok(None)
                } else {
                    Err(ssm_err(import_id, service_err))
                }
            }
        }
    }

    async fn delete(&self, import_id: &str) -> Result<(), TokenStoreError> {
        match self
            .client
            .delete_parameter()
            .name(self.param_name(import_id))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_parameter_not_found() {
                    Ok(())
                } else {
                    Err(ssm_err(import_id, service_err))
                }
            }
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_round_trip_and_delete() {
        let s = InMemoryTokenStore::new();
        assert!(s.get("i1").await.unwrap().is_none());
        s.put("i1", &QuipToken::new("tok".into())).await.unwrap();
        assert_eq!(s.get("i1").await.unwrap().unwrap().expose(), "tok");
        s.delete("i1").await.unwrap();
        assert!(s.get("i1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn in_memory_delete_of_missing_key_is_ok() {
        let s = InMemoryTokenStore::new();
        s.delete("nope").await.unwrap();
    }

    #[tokio::test]
    async fn in_memory_put_overwrites() {
        let s = InMemoryTokenStore::new();
        s.put("i1", &QuipToken::new("first".into())).await.unwrap();
        s.put("i1", &QuipToken::new("second".into())).await.unwrap();
        assert_eq!(s.get("i1").await.unwrap().unwrap().expose(), "second");
    }
}
