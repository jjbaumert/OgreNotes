// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Redis-backed short-lived auth-flow state.
//!
//! Holds the identity-flow Redis operations that used to live on
//! `ogrenotes_collab::redis_pubsub::RedisPubSub`: MFA pending-challenge
//! handles and the two SAML surfaces (assertion-replay dedup, AuthnRequest
//! tracking). These are auth-domain concerns, not collaboration concerns, so
//! they belong in the edge crate that orchestrates the auth flow rather than
//! in the collaboration capability. See issue #97.
//!
//! All keys are single-purpose with distinct prefixes so an ops sweep can
//! grep one surface without the others.

use std::sync::Arc;

use fred::prelude::*;

/// Redis operations backing the auth flow's short-lived state.
///
/// Wraps the same command `RedisClient` the rest of the stack shares
/// (see `AppState::redis`); it does not own a connection of its own.
pub struct RedisSessionStore {
    client: Arc<RedisClient>,
}

impl RedisSessionStore {
    /// Create a store over an already-connected command client.
    pub fn new(client: Arc<RedisClient>) -> Self {
        Self { client }
    }

    /// Phase 4 M-E3: hold the user_id of a partly-authenticated user
    /// between OAuth/dev-login success and the MFA challenge step.
    /// `handle` is an opaque random token issued by the auth handler;
    /// the frontend echoes it to `POST /auth/mfa/challenge` along with
    /// the TOTP code. 60s TTL is long enough for a user to switch
    /// windows to their authenticator and tight enough that an
    /// abandoned challenge expires before a casual replay window.
    pub async fn store_mfa_pending(
        &self,
        handle: &str,
        user_id: &str,
        ttl_secs: u64,
    ) -> Result<(), RedisError> {
        let key = mfa_pending_key(handle);
        self.client
            .set::<(), _, _>(
                &key,
                user_id,
                Some(Expiration::EX(ttl_secs as i64)),
                None,
                false,
            )
            .await?;
        Ok(())
    }

    /// Consume a pending-MFA handle. GETDEL is atomic: a successful
    /// take returns the user_id and erases the row, so two parallel
    /// challenge submissions can't both succeed. `None` means the
    /// handle expired, was never minted, or was already used.
    pub async fn take_mfa_pending(
        &self,
        handle: &str,
    ) -> Result<Option<String>, RedisError> {
        let key = mfa_pending_key(handle);
        let value: Option<String> = self.client.getdel(&key).await?;
        Ok(value)
    }

    /// Peek (without consuming) so the challenge endpoint can verify
    /// the TOTP code BEFORE consuming the handle. A wrong code must
    /// leave the handle valid for retry within the TTL — otherwise a
    /// typo forces the user back through the OAuth flow.
    pub async fn peek_mfa_pending(
        &self,
        handle: &str,
    ) -> Result<Option<String>, RedisError> {
        let key = mfa_pending_key(handle);
        let value: Option<String> = self.client.get(&key).await?;
        Ok(value)
    }

    /// Phase 4 M-E4 piece D: assertion-replay protection for the
    /// SAML ACS handler. Stores `saml_assertion:<id>` with a TTL
    /// equal to the configured `max_issue_delay`. Returns `Ok(true)`
    /// when this is the first time we've seen the assertion ID
    /// (key was set), `Ok(false)` when the key already exists
    /// (replay). SET NX EX is atomic at the Redis level — two
    /// concurrent ACS hits with the same assertion ID resolve to
    /// exactly one true and one false.
    pub async fn try_mark_assertion_seen(
        &self,
        assertion_id: &str,
        ttl_secs: u64,
    ) -> Result<bool, RedisError> {
        use fred::types::SetOptions;
        let key = saml_assertion_key(assertion_id);
        // Return type Option<String>: Some("OK") on first set,
        // None when NX rejected because the key exists.
        let result: Option<String> = self
            .client
            .set(
                &key,
                "1",
                Some(Expiration::EX(ttl_secs as i64)),
                Some(SetOptions::NX),
                false,
            )
            .await?;
        Ok(result.is_some())
    }

    /// #82: store a pending SAML AuthnRequest ID with its
    /// workspace binding. Called from `/auth/saml/login` right
    /// before redirecting to the IdP. `ttl_secs` should be the
    /// maximum reasonable round-trip duration (5 min covers the
    /// slowest realistic IdP MFA flow).
    ///
    /// The value stored is the workspace_id the request was minted
    /// for. The ACS handler reads it back to confirm the assertion
    /// answers a request *we* made for *this* workspace — an
    /// attacker who steals an assertion can't post it without also
    /// possessing the ID we never returned to them.
    ///
    /// `SET NX EX` — returns `Ok(true)` when the ID was novel
    /// (expected — we just generated it), `Ok(false)` on the
    /// vanishingly unlikely collision with a still-pending request.
    pub async fn try_store_saml_authn_request(
        &self,
        request_id: &str,
        workspace_id: &str,
        ttl_secs: u64,
    ) -> Result<bool, RedisError> {
        use fred::types::SetOptions;
        let key = saml_authn_request_key(request_id);
        let result: Option<String> = self
            .client
            .set(
                &key,
                workspace_id,
                Some(Expiration::EX(ttl_secs as i64)),
                Some(SetOptions::NX),
                false,
            )
            .await?;
        Ok(result.is_some())
    }

    /// #48: persist a pending OAuth flow (JSON-encoded PKCE verifier +
    /// provider) keyed by its random `state` token. Replaces the former
    /// process-local `PENDING_FLOWS` map so the provider callback can be
    /// served by any instance — multi-task deploys previously failed the
    /// state lookup when login started on A and the callback hit B. The
    /// TTL bounds the login round-trip (10 min covers a slow IdP/consent
    /// screen) and replaces the old background sweeper.
    pub async fn store_oauth_flow(
        &self,
        state: &str,
        flow_json: &str,
        ttl_secs: u64,
    ) -> Result<(), RedisError> {
        let key = oauth_flow_key(state);
        self.client
            .set::<(), _, _>(
                &key,
                flow_json,
                Some(Expiration::EX(ttl_secs as i64)),
                None,
                false,
            )
            .await?;
        Ok(())
    }

    /// #48: peek (without consuming) a pending OAuth flow, so the
    /// provider-scoped callback can reject a cross-provider state replay
    /// before the single-use take below. `None` = unknown or expired
    /// state.
    pub async fn peek_oauth_flow(&self, state: &str) -> Result<Option<String>, RedisError> {
        let key = oauth_flow_key(state);
        Ok(self.client.get(&key).await?)
    }

    /// #48: atomically consume a pending OAuth flow. GETDEL erases the
    /// row as it reads, so two parallel callbacks for one `state` can't
    /// both proceed. `None` = the state was never minted, has expired,
    /// or was already used.
    pub async fn take_oauth_flow(&self, state: &str) -> Result<Option<String>, RedisError> {
        let key = oauth_flow_key(state);
        Ok(self.client.getdel(&key).await?)
    }

    /// #82: atomically consume a pending SAML AuthnRequest ID.
    /// Called from `/auth/saml/acs` to verify the assertion's
    /// `InResponseTo` matches an outstanding request and to
    /// retrieve the workspace it was minted for. Single-use:
    /// `GETDEL` atomically returns the stored workspace_id and
    /// erases the row, so two parallel POSTs of the same captured
    /// assertion can't both succeed even before the dedicated
    /// assertion-replay dedup runs.
    ///
    /// `None` means the request was never minted, has expired, or
    /// was already consumed.
    pub async fn take_saml_authn_request(
        &self,
        request_id: &str,
    ) -> Result<Option<String>, RedisError> {
        let key = saml_authn_request_key(request_id);
        let value: Option<String> = self.client.getdel(&key).await?;
        Ok(value)
    }
}

/// Phase 4 M-E3: Redis key for a pending-MFA handle.
fn mfa_pending_key(handle: &str) -> String {
    format!("mfa_pending:{handle}")
}

/// Phase 4 M-E4: Redis key for "we've seen this SAML assertion."
/// Distinct prefix from the MFA handle keys so a future ops sweep
/// can grep one without the other.
fn saml_assertion_key(assertion_id: &str) -> String {
    format!("saml_assertion:{assertion_id}")
}

/// #82: Redis key for an outstanding SAML AuthnRequest awaiting
/// its IdP response. Separate prefix from the assertion-seen keys
/// — the two surfaces have different TTLs (5 min vs 90 s) and
/// different semantics (request tracking vs replay protection).
fn saml_authn_request_key(request_id: &str) -> String {
    format!("saml_authn_req:{request_id}")
}

/// #48: Redis key for a pending OAuth flow awaiting its provider
/// callback. Distinct prefix so an ops sweep can grep the OAuth surface
/// without the SAML/MFA ones.
fn oauth_flow_key(state: &str) -> String {
    format!("oauth_flow:{state}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Key formatting ─────────────────────────────────────────────

    #[test]
    fn mfa_pending_key_format() {
        assert_eq!(mfa_pending_key("h1"), "mfa_pending:h1");
    }

    #[test]
    fn saml_assertion_key_format() {
        assert_eq!(saml_assertion_key("a1"), "saml_assertion:a1");
    }

    #[test]
    fn saml_authn_request_key_format() {
        assert_eq!(saml_authn_request_key("r1"), "saml_authn_req:r1");
    }

    #[test]
    fn oauth_flow_key_format() {
        assert_eq!(oauth_flow_key("s1"), "oauth_flow:s1");
    }

    // ── Integration tests against local Redis ──────────────────────

    /// Connect to local Redis (docker-compose) for integration tests.
    async fn make_connected_store() -> RedisSessionStore {
        let config = RedisConfig::default(); // localhost:6379
        let client = Arc::new(RedisClient::new(config, None, None, None));
        client.init().await.expect("Redis must be running (docker-compose up)");
        RedisSessionStore::new(client)
    }

    // ── #82: SAML AuthnRequest tracking ───────────────────────

    /// Per-test unique request id so parallel test runs don't
    /// collide on the same Redis key.
    fn unique_authn_id(label: &str) -> String {
        format!(
            "_id-test-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[tokio::test]
    async fn saml_authn_request_store_then_take_returns_workspace() {
        let store = make_connected_store().await;
        let id = unique_authn_id("roundtrip");
        let stored = store
            .try_store_saml_authn_request(&id, "ws-roundtrip", 60)
            .await
            .unwrap();
        assert!(stored, "first store must succeed (NX on fresh key)");

        let taken = store
            .take_saml_authn_request(&id)
            .await
            .unwrap();
        assert_eq!(
            taken.as_deref(),
            Some("ws-roundtrip"),
            "take must return the stored workspace_id"
        );
    }

    #[tokio::test]
    async fn saml_authn_request_take_is_single_use() {
        let store = make_connected_store().await;
        let id = unique_authn_id("single-use");
        store
            .try_store_saml_authn_request(&id, "ws-single", 60)
            .await
            .unwrap();

        let first = store.take_saml_authn_request(&id).await.unwrap();
        assert!(first.is_some(), "first take must succeed");

        let second = store.take_saml_authn_request(&id).await.unwrap();
        assert!(
            second.is_none(),
            "second take must return None — the row is consumed"
        );
    }

    #[tokio::test]
    async fn saml_authn_request_take_on_unknown_returns_none() {
        let store = make_connected_store().await;
        let id = unique_authn_id("never-stored");
        let taken = store.take_saml_authn_request(&id).await.unwrap();
        assert!(
            taken.is_none(),
            "taking a never-stored id must return None"
        );
    }

    // ── #48: OAuth pending-flow lifecycle ──────────────────────

    #[tokio::test]
    async fn oauth_flow_store_peek_then_take_is_single_use() {
        let store = make_connected_store().await;
        let state = unique_authn_id("oauth-roundtrip");
        store
            .store_oauth_flow(&state, r#"{"code_verifier":"v1","provider":"github"}"#, 60)
            .await
            .unwrap();

        // Peek leaves the row intact so the provider-match check doesn't
        // consume it.
        let peeked = store.peek_oauth_flow(&state).await.unwrap();
        assert!(peeked.is_some(), "peek must see the stored flow");
        assert!(
            store.peek_oauth_flow(&state).await.unwrap().is_some(),
            "peek must not consume the flow"
        );

        // Take is single-use.
        let first = store.take_oauth_flow(&state).await.unwrap();
        assert_eq!(
            first.as_deref(),
            Some(r#"{"code_verifier":"v1","provider":"github"}"#)
        );
        let second = store.take_oauth_flow(&state).await.unwrap();
        assert!(second.is_none(), "second take must return None — row consumed");
    }

    #[tokio::test]
    async fn oauth_flow_take_on_unknown_returns_none() {
        let store = make_connected_store().await;
        let state = unique_authn_id("oauth-never-stored");
        assert!(store.take_oauth_flow(&state).await.unwrap().is_none());
        assert!(store.peek_oauth_flow(&state).await.unwrap().is_none());
    }

    // ── M-E3: MFA pending-handle lifecycle ─────────────────────

    /// Per-test unique handle so parallel test runs don't collide on
    /// the same Redis key.
    fn unique_handle(label: &str) -> String {
        format!(
            "_mfa-test-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[tokio::test]
    async fn mfa_pending_store_then_take_returns_user() {
        let store = make_connected_store().await;
        let handle = unique_handle("roundtrip");
        store
            .store_mfa_pending(&handle, "user-roundtrip", 60)
            .await
            .unwrap();

        let taken = store.take_mfa_pending(&handle).await.unwrap();
        assert_eq!(
            taken.as_deref(),
            Some("user-roundtrip"),
            "take must return the stored user_id"
        );
    }

    #[tokio::test]
    async fn mfa_pending_take_is_single_use() {
        // The security-critical GETDEL property: two concurrent
        // challenge submissions can't both consume the same handle,
        // so a partly-authenticated user can't be issued two sessions.
        let store = make_connected_store().await;
        let handle = unique_handle("single-use");
        store
            .store_mfa_pending(&handle, "user-single", 60)
            .await
            .unwrap();

        let first = store.take_mfa_pending(&handle).await.unwrap();
        assert_eq!(
            first.as_deref(),
            Some("user-single"),
            "first take must return the user_id"
        );

        let second = store.take_mfa_pending(&handle).await.unwrap();
        assert!(
            second.is_none(),
            "second take must return None — the handle is consumed"
        );
    }

    #[tokio::test]
    async fn mfa_pending_peek_does_not_consume() {
        // The challenge endpoint peeks to verify the TOTP code before
        // consuming, so a wrong code leaves the handle valid for retry
        // within the TTL rather than forcing the user back through OAuth.
        let store = make_connected_store().await;
        let handle = unique_handle("peek");
        store
            .store_mfa_pending(&handle, "user-peek", 60)
            .await
            .unwrap();

        let peek1 = store.peek_mfa_pending(&handle).await.unwrap();
        assert_eq!(peek1.as_deref(), Some("user-peek"), "first peek sees the user_id");

        let peek2 = store.peek_mfa_pending(&handle).await.unwrap();
        assert_eq!(
            peek2.as_deref(),
            Some("user-peek"),
            "second peek still sees it — peek must not consume the handle"
        );

        // And a take afterwards still succeeds (the handle survived both peeks).
        let taken = store.take_mfa_pending(&handle).await.unwrap();
        assert_eq!(taken.as_deref(), Some("user-peek"), "take after peeks still works");
    }

    #[tokio::test]
    async fn mfa_pending_take_on_unknown_returns_none() {
        let store = make_connected_store().await;
        let handle = unique_handle("never-stored");
        let taken = store.take_mfa_pending(&handle).await.unwrap();
        assert!(
            taken.is_none(),
            "taking a never-stored handle must return None"
        );
    }
}
