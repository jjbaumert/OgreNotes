// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Redis integration for collaboration: pubsub fanout and single-use token store.

use fred::clients::SubscriberClient;
use fred::prelude::*;
use fred::types::Message;
use std::sync::Arc;
use tokio::task::JoinHandle;

use super::room::RoomRegistry;

/// Length of the instance-ID prefix on published update payloads, in bytes.
/// Every message published to `doc:{id}` starts with 8 big-endian bytes of
/// the publisher's instance ID so subscribers skip messages that originated
/// on their own instance (otherwise every update would be applied twice).
const INSTANCE_ID_BYTES: usize = 8;

/// Write authority a minted WS token grants (#111). The level is baked
/// into the single-use token value (server-authored, stored in Redis,
/// GETDEL-consumed) — never read from the client — so a captured
/// read-only token can't be replayed to gain write access. The room reads
/// this off the validated token and drops inbound CRDT update frames from
/// a `ReadOnly` session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsAccess {
    /// May send CRDT updates — minted for Edit/Own access at request time.
    ReadWrite,
    /// Receives server→client updates + awareness, but the room rejects any
    /// inbound CRDT update frame — minted for View access at request time.
    ReadOnly,
}

impl WsAccess {
    /// Single-char tag stored as the token's 4th field.
    fn tag(self) -> char {
        match self {
            WsAccess::ReadWrite => 'w',
            WsAccess::ReadOnly => 'r',
        }
    }

    /// Decode the token's access tag. `None` means an old-format (3-field)
    /// token still in flight across a deploy — those were minted under the
    /// historical Edit-only gate, so they're legitimately read-write. Any
    /// present but unrecognized tag fails closed to `ReadOnly` (the value is
    /// server-authored so this can't be attacker-induced, but failing closed
    /// is the safe default). The encoder always appends a non-empty tag, so
    /// `Some("")` is unreachable in practice; it fails closed regardless.
    fn from_tag(tag: Option<&str>) -> WsAccess {
        match tag {
            None | Some("w") => WsAccess::ReadWrite,
            _ => WsAccess::ReadOnly,
        }
    }
}

/// Redis-backed pubsub and token store for collaboration.
pub struct RedisPubSub {
    client: Arc<RedisClient>,
    /// Unique ID for this server instance. Prepended to every published
    /// update so the subscriber on the same instance can drop its own
    /// messages off the pubsub fanout.
    instance_id: u64,
}

impl RedisPubSub {
    /// Create a new RedisPubSub from a connected Redis client.
    /// Generates a random `instance_id` used to tag published messages.
    pub fn new(client: Arc<RedisClient>) -> Self {
        Self {
            client,
            instance_id: generate_instance_id(),
        }
    }

    /// Create with an explicit `instance_id` — for tests that need two
    /// RedisPubSub instances to pretend to be different servers.
    pub fn with_instance_id(client: Arc<RedisClient>, instance_id: u64) -> Self {
        Self { client, instance_id }
    }

    /// This instance's ID. Visible mostly for tests and diagnostics.
    pub fn instance_id(&self) -> u64 {
        self.instance_id
    }

    /// Store a single-use WebSocket authentication token.
    /// The token maps to `user_id\0doc_id\0client_version\0access` and
    /// expires after `ttl_secs`.
    pub async fn store_ws_token(
        &self,
        token: &str,
        user_id: &str,
        doc_id: &str,
        client_version: Option<&str>,
        access: WsAccess,
        ttl_secs: u64,
    ) -> Result<(), RedisError> {
        let value = encode_token_value(user_id, doc_id, client_version, access);
        let key = token_key(token);
        self.client
            .set::<(), _, _>(&key, value.as_str(), Some(Expiration::EX(ttl_secs as i64)), None, false)
            .await?;
        Ok(())
    }

    /// Validate and consume a single-use WebSocket token.
    /// Returns (user_id, doc_id, client_version, access) if valid, None if
    /// expired or already used.
    pub async fn validate_ws_token(
        &self,
        token: &str,
    ) -> Result<Option<(String, String, Option<String>, WsAccess)>, RedisError> {
        let key = token_key(token);
        let value: Option<String> = self.client.getdel(&key).await?;
        Ok(value.and_then(|v| parse_token_value(&v)))
    }

    /// Publish a document update to the Redis channel for multi-instance fanout.
    /// The payload is prefixed with 8 bytes of this instance's ID so the
    /// subscriber on the originating instance can filter out the loopback.
    pub async fn publish_update(
        &self,
        doc_id: &str,
        data: &[u8],
    ) -> Result<(), RedisError> {
        let channel = doc_channel(doc_id);
        let mut framed = Vec::with_capacity(INSTANCE_ID_BYTES + data.len());
        framed.extend_from_slice(&self.instance_id.to_be_bytes());
        framed.extend_from_slice(data);
        self.client.publish::<(), _, _>(&channel, framed.as_slice()).await?;
        Ok(())
    }

    /// Spawn a background task that subscribes to `doc:*` on `subscriber`
    /// and fans received updates into the matching local rooms via
    /// [`RoomRegistry::apply_remote_update`]. Messages published by this
    /// same instance are filtered out by the `instance_id` prefix.
    ///
    /// `subscriber` must already be `init()`-ed by the caller; this method
    /// takes ownership so the client (and therefore its pubsub connection
    /// and message broadcast channel) stays alive for the lifetime of the
    /// task. Prefer `SubscriberClient` over a plain `RedisClient` so
    /// `manage_subscriptions` can re-subscribe after a reconnect — call
    /// `subscriber.manage_subscriptions()` before handing it in.
    ///
    /// Returns `Err` if the initial `PSUBSCRIBE` fails.
    pub async fn spawn_subscriber(
        &self,
        subscriber: SubscriberClient,
        registry: Arc<RoomRegistry>,
    ) -> Result<JoinHandle<()>, RedisError> {
        // Get the message receiver *before* subscribing so no messages can
        // slip past us during the subscribe roundtrip.
        let mut rx = subscriber.message_rx();
        subscriber.psubscribe("doc:*").await?;

        let instance_id = self.instance_id;
        let handle = tokio::spawn(async move {
            // Move `subscriber` into the task so the TCP connection and
            // the message broadcast sender outlive this function call.
            // (The 86c5c89 revision took an Arc and dropped it here; in
            // practice delivery still worked in isolation but was
            // unreliable in production under load — the forensic test in
            // this module documents the experiment.)
            let _keep_alive = subscriber;

            tracing::info!(instance_id, "redis pubsub subscriber started");
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if let Some((doc_id, payload)) = parse_pubsub_message(&msg, instance_id) {
                            registry.apply_remote_update(&doc_id, &payload).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Older messages were dropped from the broadcast
                        // buffer because we fell behind. Those specific
                        // updates are unrecoverable on this channel, so
                        // we self-heal (#10): ask every connected client
                        // of every locally-hosted room to re-handshake
                        // (SyncStep1). yrs sync is idempotent — a client
                        // that missed nothing no-ops; one that diverged
                        // gets the gap backfilled. This bounds divergence
                        // to a single lag event instead of "until the
                        // client disconnects and reconnects" (which for
                        // an idle-but-connected client could be hours).
                        //
                        // `collab_log_events_total{event=...}` piggybacks
                        // on the existing event-counting tracing layer in
                        // crates/common.
                        tracing::warn!(
                            event_type = "collab_subscriber_lagged",
                            instance_id,
                            lost = n,
                            "redis subscriber lagged, dropped messages"
                        );
                        let resynced = registry.resync_all_rooms().await;
                        tracing::info!(
                            event_type = "collab_subscriber_resync",
                            instance_id,
                            rooms = resynced,
                            "re-synced local rooms after subscriber lag"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::warn!(
                            instance_id,
                            "redis subscriber channel closed, exiting"
                        );
                        break;
                    }
                }
            }
        });
        Ok(handle)
    }
}

/// Parse a pub/sub message into `(doc_id, update_payload)`. Returns `None`
/// when the message originated on our own instance, when the channel isn't
/// in the expected `doc:*` format, or when the value is malformed.
fn parse_pubsub_message(msg: &Message, our_instance: u64) -> Option<(String, Vec<u8>)> {
    let channel: &str = &msg.channel;
    let bytes = msg.value.as_bytes()?;
    parse_pubsub_frame(channel, bytes, our_instance)
}

/// Channel-and-bytes variant of [`parse_pubsub_message`] factored out so
/// tests can exercise the framing logic without constructing a fred Message.
fn parse_pubsub_frame(channel: &str, bytes: &[u8], our_instance: u64) -> Option<(String, Vec<u8>)> {
    let doc_id = channel.strip_prefix("doc:")?.to_string();
    if bytes.len() < INSTANCE_ID_BYTES {
        return None;
    }
    let (prefix, rest) = bytes.split_at(INSTANCE_ID_BYTES);
    let prefix_arr: [u8; 8] = prefix.try_into().ok()?;
    let sender = u64::from_be_bytes(prefix_arr);
    if sender == our_instance {
        return None;
    }
    Some((doc_id, rest.to_vec()))
}

/// Generate a per-process instance ID using a CSPRNG.
///
/// Uniqueness matters a lot here: if two instances ever collide on an
/// instance_id, every update published by one is treated as a peer
/// update by the other *and* by itself, so every edit applies and
/// re-broadcasts twice in a tight loop, saturating the local mpsc
/// senders. That is a plausible explanation for the production
/// failure that led to 86c5c89 being reverted. A previous version of
/// this function XOR-combined low-entropy sources (SystemTime nanos,
/// pid, a process-local counter) — non-uniform, and vulnerable to
/// collision in a container fleet where pids reset from 1 and
/// instances start simultaneously. Using `rand::random::<u64>()`
/// (CSPRNG-backed) makes collision probability ~N²/2⁶⁴ for real.
fn generate_instance_id() -> u64 {
    rand::random::<u64>()
}

/// Build the Redis key for a WebSocket token.
fn token_key(token: &str) -> String {
    format!("ws_token:{token}")
}

/// Build the Redis channel name for a document.
fn doc_channel(doc_id: &str) -> String {
    format!("doc:{doc_id}")
}

/// Encode a token value as `user_id\0doc_id\0client_version\0access`.
///
/// # Panics
/// Panics if `user_id`, `doc_id`, or `client_version` contain null bytes.
/// These are the field separators, so a null byte would shift the decoded
/// fields and let one field bleed into another (#8). The checks are
/// `assert!` (not `debug_assert!`) so a release build fails loudly rather
/// than minting a silently-corrupt token. The `access` tag is a fixed
/// single char, so it needs no null check.
fn encode_token_value(
    user_id: &str,
    doc_id: &str,
    client_version: Option<&str>,
    access: WsAccess,
) -> String {
    assert!(!user_id.contains('\0'), "user_id must not contain null bytes");
    assert!(!doc_id.contains('\0'), "doc_id must not contain null bytes");
    assert!(
        !client_version.is_some_and(|v| v.contains('\0')),
        "client_version must not contain null bytes"
    );
    let version = client_version.unwrap_or("");
    format!("{user_id}\0{doc_id}\0{version}\0{}", access.tag())
}

/// Parse a token value back into (user_id, doc_id, client_version, access).
/// An absent 4th field (old-format token in flight across a deploy) decodes
/// to `WsAccess::ReadWrite` — see `WsAccess::from_tag`.
fn parse_token_value(value: &str) -> Option<(String, String, Option<String>, WsAccess)> {
    let mut parts = value.splitn(4, '\0');
    let user_id = parts.next()?.to_string();
    let doc_id = parts.next()?.to_string();
    let client_version = parts.next()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let access = WsAccess::from_tag(parts.next());
    Some((user_id, doc_id, client_version, access))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Key/channel formatting ─────────────────────────────────────

    #[test]
    fn token_key_format() {
        assert_eq!(token_key("abc123"), "ws_token:abc123");
    }

    #[test]
    fn doc_channel_format() {
        assert_eq!(doc_channel("doc-42"), "doc:doc-42");
    }

    // ── Token value encoding/parsing ───────────────────────────────

    #[test]
    fn encode_parse_roundtrip_with_version() {
        let encoded = encode_token_value("user-1", "doc-42", Some("v2.1"), WsAccess::ReadWrite);
        let (user_id, doc_id, version, access) = parse_token_value(&encoded).unwrap();
        assert_eq!(user_id, "user-1");
        assert_eq!(doc_id, "doc-42");
        assert_eq!(version.as_deref(), Some("v2.1"));
        assert_eq!(access, WsAccess::ReadWrite);
    }

    #[test]
    fn encode_parse_roundtrip_without_version() {
        let encoded = encode_token_value("user-1", "doc-42", None, WsAccess::ReadWrite);
        let (user_id, doc_id, version, access) = parse_token_value(&encoded).unwrap();
        assert_eq!(user_id, "user-1");
        assert_eq!(doc_id, "doc-42");
        assert!(version.is_none());
        assert_eq!(access, WsAccess::ReadWrite);
    }

    #[test]
    fn encode_parse_roundtrip_read_only() {
        // #111: a ReadOnly token round-trips its access tag, with and
        // without a client version.
        let with_ver = encode_token_value("u", "d", Some("v1"), WsAccess::ReadOnly);
        let (_, _, ver, access) = parse_token_value(&with_ver).unwrap();
        assert_eq!(ver.as_deref(), Some("v1"));
        assert_eq!(access, WsAccess::ReadOnly);

        let no_ver = encode_token_value("u", "d", None, WsAccess::ReadOnly);
        let (_, _, ver, access) = parse_token_value(&no_ver).unwrap();
        assert!(ver.is_none());
        assert_eq!(access, WsAccess::ReadOnly);
    }

    #[test]
    fn parse_legacy_three_field_token_defaults_to_read_write() {
        // #111: an old-format token (no access field) still in flight across
        // a deploy was minted under the historical Edit-only gate, so it
        // must decode to ReadWrite — never silently downgraded or rejected.
        let (_, _, ver, access) = parse_token_value("user-1\0doc-42\0v2").unwrap();
        assert_eq!(ver.as_deref(), Some("v2"));
        assert_eq!(access, WsAccess::ReadWrite);

        // Legacy with empty version too.
        let (_, _, ver, access) = parse_token_value("user-1\0doc-42\0").unwrap();
        assert!(ver.is_none());
        assert_eq!(access, WsAccess::ReadWrite);
    }

    #[test]
    fn parse_unrecognized_access_tag_fails_closed() {
        // A present-but-unknown tag (can't be attacker-induced — the value
        // is server-authored — but fail closed regardless).
        let (_, _, _, access) = parse_token_value("u\0d\0v\0x").unwrap();
        assert_eq!(access, WsAccess::ReadOnly);
    }

    #[test]
    fn parse_missing_fields_returns_none() {
        // Only user_id, no null separator
        assert!(parse_token_value("just-user").is_none());
    }

    #[test]
    fn parse_empty_string() {
        // Empty string has user_id="" but no doc_id
        assert!(parse_token_value("").is_none());
    }

    #[test]
    fn encode_preserves_special_chars() {
        let encoded =
            encode_token_value("user@example.com", "doc/with/slashes", Some("1.0"), WsAccess::ReadWrite);
        let (user_id, doc_id, version, _) = parse_token_value(&encoded).unwrap();
        assert_eq!(user_id, "user@example.com");
        assert_eq!(doc_id, "doc/with/slashes");
        assert_eq!(version.as_deref(), Some("1.0"));
    }

    #[test]
    fn encode_empty_version_string_treated_as_none() {
        // Explicitly passing Some("") should behave like None on parse
        let encoded = encode_token_value("u", "d", Some(""), WsAccess::ReadWrite);
        let (_, _, version, _) = parse_token_value(&encoded).unwrap();
        assert!(version.is_none());
    }

    #[test]
    fn encode_format_uses_null_separator() {
        let encoded = encode_token_value("alice", "doc-1", Some("v1"), WsAccess::ReadWrite);
        assert_eq!(encoded, "alice\0doc-1\0v1\0w");
    }

    #[test]
    #[should_panic(expected = "user_id must not contain null bytes")]
    fn encode_rejects_null_in_user_id() {
        encode_token_value("user\0inject", "doc-1", None, WsAccess::ReadWrite);
    }

    #[test]
    #[should_panic(expected = "doc_id must not contain null bytes")]
    fn encode_rejects_null_in_doc_id() {
        encode_token_value("user-1", "doc\0inject", None, WsAccess::ReadWrite);
    }

    #[test]
    #[should_panic(expected = "client_version must not contain null bytes")]
    fn encode_rejects_null_in_version() {
        encode_token_value("user-1", "doc-1", Some("v\0bad"), WsAccess::ReadWrite);
    }

    // ── Struct construction ─────────────────────────────────────────

    /// Connect to local Redis (docker-compose) for integration tests.
    async fn make_connected_pubsub() -> RedisPubSub {
        let config = RedisConfig::default(); // localhost:6379
        let client = Arc::new(RedisClient::new(config, None, None, None));
        client.init().await.expect("Redis must be running (docker-compose up)");
        RedisPubSub::new(client)
    }

    // ── Integration tests against local Redis ──────────────────────

    #[tokio::test]
    async fn store_and_validate_token_roundtrip() {
        let pubsub = make_connected_pubsub().await;
        let token = format!("test-token-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());

        pubsub.store_ws_token(&token, "user-1", "doc-42", Some("v2"), WsAccess::ReadOnly, 60)
            .await.unwrap();

        let result = pubsub.validate_ws_token(&token).await.unwrap();
        let (user_id, doc_id, version, access) = result.expect("token should be valid");
        assert_eq!(user_id, "user-1");
        assert_eq!(doc_id, "doc-42");
        assert_eq!(version.as_deref(), Some("v2"));
        // #111: the access level survives the Redis round-trip.
        assert_eq!(access, WsAccess::ReadOnly);
    }

    #[tokio::test]
    async fn validate_token_is_single_use() {
        let pubsub = make_connected_pubsub().await;
        let token = format!("test-single-use-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());

        pubsub.store_ws_token(&token, "user-1", "doc-1", None, WsAccess::ReadWrite, 60)
            .await.unwrap();

        // First validate consumes
        let first = pubsub.validate_ws_token(&token).await.unwrap();
        assert!(first.is_some());

        // Second validate returns None (token consumed)
        let second = pubsub.validate_ws_token(&token).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn validate_nonexistent_token_returns_none() {
        let pubsub = make_connected_pubsub().await;
        let result = pubsub.validate_ws_token("nonexistent-token-xyz").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn store_token_without_version() {
        let pubsub = make_connected_pubsub().await;
        let token = format!("test-no-ver-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());

        pubsub.store_ws_token(&token, "user-1", "doc-1", None, WsAccess::ReadWrite, 60)
            .await.unwrap();

        let (user_id, doc_id, version, _access) = pubsub.validate_ws_token(&token)
            .await.unwrap().unwrap();
        assert_eq!(user_id, "user-1");
        assert_eq!(doc_id, "doc-1");
        assert!(version.is_none());
    }

    #[tokio::test]
    async fn publish_update_succeeds() {
        let pubsub = make_connected_pubsub().await;
        // publish_update should succeed even with no subscribers
        pubsub.publish_update("doc-test", b"some update bytes")
            .await.unwrap();
    }

    // ── Cross-instance fanout end-to-end ───────────────────────────
    //
    // These tests model a two-server deployment: each "instance" has its
    // own RedisPubSub, its own RoomRegistry, its own subscriber task.
    // Shared: one Redis. Each test verifies the full loop — a publisher
    // on instance A's publish_update shows up on instance B's local
    // client (via subscribe → apply_remote_update → broadcast), and the
    // publisher itself does NOT see its own message loop back.

    use super::super::document::OgreDoc;
    use super::super::protocol::{encode_message, MessageType};
    use super::super::room::RoomRegistry;

    /// Build a subscriber client connected to localhost:6379. Used in
    /// integration tests to drive the subscribe side of RedisPubSub.
    async fn make_subscriber_client() -> SubscriberClient {
        use fred::types::Builder;
        let config = RedisConfig::default();
        let builder = Builder::from_config(config);
        let sub = builder
            .build_subscriber_client()
            .expect("build_subscriber_client");
        sub.init().await.expect("subscriber init");
        sub
    }

    /// Build a valid yrs incremental update against an empty base. The
    /// bytes are a wire-framed `MessageType::Update` payload — the exact
    /// thing `publish_update` carries across instances.
    fn sample_update_wire() -> Vec<u8> {
        let base = OgreDoc::new();
        let base_sv = base.state_vector();

        let ext = OgreDoc::new();
        {
            use yrs::{Transact, WriteTxn, types::xml::{XmlFragment, XmlOut, XmlTextPrelim}};
            let mut txn = ext.inner().transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            if let Some(XmlOut::Element(p)) = frag.get(&txn, 0) {
                p.insert(&mut txn, 0, XmlTextPrelim::new("remote edit"));
            }
        }
        let diff = ext.encode_diff(&base_sv).expect("encode_diff");
        encode_message(MessageType::Update, &diff)
    }

    #[tokio::test]
    async fn cross_instance_fanout_reaches_peer_and_skips_self() {
        // ── wire up two "server instances" ────────────────────────
        let pub_a = make_connected_pubsub().await;
        let pub_b = make_connected_pubsub().await;
        assert_ne!(
            pub_a.instance_id(),
            pub_b.instance_id(),
            "two fresh RedisPubSub instances must have distinct instance_ids",
        );

        let registry_a = Arc::new(RoomRegistry::new());
        let registry_b = Arc::new(RoomRegistry::new());

        let doc_id = format!("fanout-doc-{}", nanoid::nanoid!(8));
        let room_a = registry_a.get_or_insert(&doc_id, OgreDoc::new());
        let room_b = registry_b.get_or_insert(&doc_id, OgreDoc::new());

        // One "WS client" per instance, modelled as an mpsc channel
        // just like the real WS handler does.
        let (tx_a, mut rx_a) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        room_a.add_client(1, "alice".to_string(), tx_a).await;
        room_b.add_client(1, "bob".to_string(), tx_b).await;

        // Subscribers on both sides.
        let sub_a_handle = pub_a
            .spawn_subscriber(make_subscriber_client().await, registry_a.clone())
            .await
            .expect("spawn A subscriber");
        let sub_b_handle = pub_b
            .spawn_subscriber(make_subscriber_client().await, registry_b.clone())
            .await
            .expect("spawn B subscriber");

        // Give both PSUBSCRIBE roundtrips time to settle before we
        // publish — otherwise the message can outrun the subscription.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // ── simulate alice typing on instance A ───────────────────
        let wire = sample_update_wire();
        // WS handler sequence: apply locally, local broadcast (skip
        // sender), publish to Redis.
        let (_, payload) = super::super::protocol::decode_message(&wire).unwrap();
        room_a.apply_update(payload).await.expect("apply on A");
        room_a.broadcast(1, wire.clone()).await;
        pub_a.publish_update(&doc_id, &wire).await.expect("publish");

        // ── bob (on B) must see it via the Redis fanout path ──────
        let bob_got = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx_b.recv(),
        )
        .await
        .expect("timed out waiting for peer update on instance B")
        .expect("B's broadcast channel closed unexpectedly");

        assert_eq!(
            bob_got, wire,
            "B should receive the exact wire frame A published"
        );

        // ── alice (on A) must not see her own update back ─────────
        // We assert absence *after* verifying bob received the message:
        // by the time bob has it, the round-trip through Redis is
        // complete, so A's subscriber has had a chance to see (and
        // filter out) the self-published message. A plain timeout on
        // rx_a.recv() would also pass if A's subscriber had silently
        // died, hence the additional `is_finished` check — we only
        // trust the self-filter result if the task is still running.
        assert!(
            !sub_a_handle.is_finished(),
            "A's subscriber task must still be running; if it died we can't trust the self-filter assertion"
        );
        assert!(
            !sub_b_handle.is_finished(),
            "B's subscriber task must still be running"
        );
        let alice_echo = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            rx_a.recv(),
        )
        .await;
        assert!(
            alice_echo.is_err(),
            "self-published message must not loop back to the sender's own room (got {alice_echo:?})"
        );
    }

    #[tokio::test]
    async fn subscriber_stays_alive_after_spawn_returns() {
        // Regression guard for the original 86c5c89 defect: the subscriber
        // client was passed by Arc and dropped at the end of the spawn_*
        // function, severing the broadcast channel before the receive
        // loop could run. Here we spawn the subscriber, then publish
        // *after* spawn_subscriber has returned — the subscriber task
        // must still be alive to pick up the message.
        let pub_a = make_connected_pubsub().await;
        let pub_b = make_connected_pubsub().await;

        let registry_b = Arc::new(RoomRegistry::new());
        let doc_id = format!("keep-alive-doc-{}", nanoid::nanoid!(8));
        let room_b = registry_b.get_or_insert(&doc_id, OgreDoc::new());
        let (tx_b, mut rx_b) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        room_b.add_client(1, "bob".to_string(), tx_b).await;

        let _sub_handle = pub_b
            .spawn_subscriber(make_subscriber_client().await, registry_b.clone())
            .await
            .expect("spawn subscriber");

        // Wait substantially longer than any "quick init" window to make
        // sure the task is past its setup and sitting in `rx.recv()`.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let wire = sample_update_wire();
        pub_a.publish_update(&doc_id, &wire).await.expect("publish");

        let bob_got = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            rx_b.recv(),
        )
        .await
        .expect("subscriber task exited after spawn — dropped client?")
        .expect("B's broadcast channel closed");

        assert_eq!(bob_got, wire);
    }

    #[tokio::test]
    async fn unknown_room_is_silently_dropped() {
        // If a peer publishes for a doc this instance doesn't host, the
        // subscriber should ignore it (the peer owns that room; we'll
        // replay from DocRepo on next join).
        let pub_a = make_connected_pubsub().await;
        let pub_b = make_connected_pubsub().await;

        let registry_b = Arc::new(RoomRegistry::new()); // empty — no rooms
        let _sub_handle = pub_b
            .spawn_subscriber(make_subscriber_client().await, registry_b.clone())
            .await
            .expect("spawn subscriber");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // Publish for a doc B doesn't know about. Should not panic the
        // subscriber task; should not create a room on B.
        let wire = sample_update_wire();
        pub_a
            .publish_update("unknown-doc-xyz", &wire)
            .await
            .expect("publish");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(registry_b.room_count(), 0);
    }

}
