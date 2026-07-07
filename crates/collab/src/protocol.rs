// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Binary WebSocket protocol for real-time document collaboration.
//!
//! Each message is a binary frame with a 1-byte type prefix followed by payload bytes.

use ogrenotes_common::metrics::{counter, MetricKey};

/// Message types for the collaboration WebSocket protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    /// Client sends auth token (UTF-8 string).
    Auth = 0x00,
    /// Sync step 1: sender's state vector (binary).
    SyncStep1 = 0x01,
    /// Sync step 2: diff computed from the received state vector (binary).
    SyncStep2 = 0x02,
    /// Incremental yrs update (binary).
    Update = 0x03,
    /// Awareness update (JSON-encoded AwarenessState).
    Awareness = 0x04,
    /// Application-level keepalive. Payload is empty and ignored. The
    /// client sends these on a ~25s cadence while the user is active so the
    /// ALB (default idle_timeout = 60s) doesn't drop the long-lived
    /// WebSocket out from under a quietly-idle document.
    Ping = 0x05,
    /// Comment-thread change notification (server → client). Payload is a
    /// UTF-8 JSON object describing the change, e.g.
    /// `{"kind":"thread_created","threadId":"…"}`. Comments live outside
    /// the CRDT (they're stored in the thread DB, not in the yrs doc), so
    /// this frame is the side channel that lets peers refresh their thread
    /// list / inline highlights without a manual page reload.
    CommentEvent = 0x06,
    /// Client → server: subscribe this connection to live updates from
    /// another document referenced via `=REFERENCERANGE` /
    /// `=REFERENCESHEET`. Payload is the foreign doc id (UTF-8). Server
    /// gates the subscribe behind a per-id read-access check; on
    /// success, every Update originating in that doc is forwarded to
    /// the client as a `ForeignDocUpdate` frame.
    SubscribeForeignDoc = 0x07,
    /// Client → server: stop receiving updates for the given foreign
    /// doc id (payload is the id, UTF-8).
    UnsubscribeForeignDoc = 0x08,
    /// Server → client: a Yjs update for a *foreign* doc the client
    /// has subscribed to. Payload encodes the doc id then the update
    /// bytes — see `encode_foreign_doc_update` /
    /// `decode_foreign_doc_update`. The client routes the update to
    /// its per-foreign-doc yrs::Doc sidecar to refresh the
    /// REFERENCE* cache.
    ForeignDocUpdate = 0x09,
    /// Awareness departure (server → client only). Payload is the leaving
    /// user's id (UTF-8). Sent when a client disconnects and that user has
    /// no other live connection in the room, so peers can drop the user's
    /// cursor immediately instead of leaving it frozen until refresh (#9).
    /// Like `Awareness`, this is local-broadcast only — never redis-fanned.
    AwarenessLeave = 0x0A,
    /// Error message (UTF-8 string). Server → client only.
    Error = 0xFF,
}

impl MessageType {
    /// Parse a message type from a byte.
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(Self::Auth),
            0x01 => Some(Self::SyncStep1),
            0x02 => Some(Self::SyncStep2),
            0x03 => Some(Self::Update),
            0x04 => Some(Self::Awareness),
            0x05 => Some(Self::Ping),
            0x06 => Some(Self::CommentEvent),
            0x07 => Some(Self::SubscribeForeignDoc),
            0x08 => Some(Self::UnsubscribeForeignDoc),
            0x09 => Some(Self::ForeignDocUpdate),
            0x0A => Some(Self::AwarenessLeave),
            0xFF => Some(Self::Error),
            _ => None,
        }
    }
}

/// Encode a message: 1-byte type prefix + payload.
pub fn encode_message(msg_type: MessageType, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1 + payload.len());
    buf.push(msg_type as u8);
    buf.extend_from_slice(payload);
    buf
}

/// Decode a message: returns (type, payload slice).
/// Returns None if the buffer is empty or the type byte is unknown.
///
/// An unknown type byte emits `ws.unknown_msg_type_total{direction=recv}`
/// and a tracing::warn with the byte value before returning None.
/// The silent-drop hole at the historical `_ => None` arm of
/// `MessageType::from_byte` is the primary skew-detector for "new
/// client / old server" version mismatch (see design/observability.md
/// §Version-skew tolerance). The byte value goes in the log line, not
/// the metric dimension, because CloudWatch dislikes 256-cardinality
/// dimensions and the typical operator question is "are we seeing
/// any unknown bytes at all" rather than "which specific byte" —
/// forensics live in the log. An empty buffer is a transport-layer
/// anomaly, not skew, so it's not counted here.
pub fn decode_message(data: &[u8]) -> Option<(MessageType, &[u8])> {
    if data.is_empty() {
        return None;
    }
    match MessageType::from_byte(data[0]) {
        Some(msg_type) => Some((msg_type, &data[1..])),
        None => {
            counter::inc(MetricKey::new(
                "ws.unknown_msg_type_total",
                &[("direction", "recv")],
            ));
            tracing::warn!(
                event_type = "ws_unknown_msg_type",
                direction = "recv",
                byte = format!("0x{:02x}", data[0]),
                payload_len = data.len(),
                "received WS frame with unknown MessageType byte",
            );
            None
        }
    }
}

/// Encode a `ForeignDocUpdate` payload: 1-byte id length followed by
/// `id` bytes and the raw `update` bytes. The 1-byte length prefix is
/// sufficient because doc ids are UUIDs (~36 bytes); larger ids are
/// rejected by the server's auth path before reaching this point.
pub fn encode_foreign_doc_update(id: &str, update: &[u8]) -> Vec<u8> {
    let id_bytes = id.as_bytes();
    debug_assert!(id_bytes.len() <= u8::MAX as usize, "doc id too long for protocol");
    let id_len = id_bytes.len().min(u8::MAX as usize) as u8;
    let mut buf = Vec::with_capacity(1 + id_bytes.len() + update.len());
    buf.push(id_len);
    buf.extend_from_slice(&id_bytes[..id_len as usize]);
    buf.extend_from_slice(update);
    buf
}

/// Inverse of `encode_foreign_doc_update`. Returns `None` if the
/// payload is truncated or the id isn't valid UTF-8.
pub fn decode_foreign_doc_update(payload: &[u8]) -> Option<(&str, &[u8])> {
    let id_len = *payload.first()? as usize;
    if payload.len() < 1 + id_len { return None; }
    let id = std::str::from_utf8(&payload[1..1 + id_len]).ok()?;
    let update = &payload[1 + id_len..];
    Some((id, update))
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let payload = b"hello world";
        let encoded = encode_message(MessageType::Update, payload);
        let (msg_type, decoded_payload) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::Update);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn encode_decode_empty_payload() {
        let encoded = encode_message(MessageType::SyncStep1, &[]);
        let (msg_type, payload) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::SyncStep1);
        assert!(payload.is_empty());
    }

    #[test]
    fn decode_empty_returns_none() {
        assert!(decode_message(&[]).is_none());
    }

    #[test]
    fn decode_invalid_type_returns_none() {
        assert!(decode_message(&[0x42, 1, 2, 3]).is_none());
    }

    #[test]
    fn all_message_types_roundtrip() {
        for (byte, expected) in [
            (0x00, MessageType::Auth),
            (0x01, MessageType::SyncStep1),
            (0x02, MessageType::SyncStep2),
            (0x03, MessageType::Update),
            (0x04, MessageType::Awareness),
            (0x05, MessageType::Ping),
            (0x06, MessageType::CommentEvent),
            (0x07, MessageType::SubscribeForeignDoc),
            (0x08, MessageType::UnsubscribeForeignDoc),
            (0x09, MessageType::ForeignDocUpdate),
            (0x0A, MessageType::AwarenessLeave),
            (0xFF, MessageType::Error),
        ] {
            let msg_type = MessageType::from_byte(byte).unwrap();
            assert_eq!(msg_type, expected);
            let encoded = encode_message(msg_type, b"test");
            let (decoded_type, payload) = decode_message(&encoded).unwrap();
            assert_eq!(decoded_type, expected);
            assert_eq!(payload, b"test");
        }
    }

    #[test]
    fn foreign_doc_update_roundtrip() {
        let id = "a1b2c3d4-e5f6-7890-1234-567890abcdef";
        let update = b"\x01\x02\x03 some update bytes";
        let encoded = encode_foreign_doc_update(id, update);
        let (decoded_id, decoded_update) = decode_foreign_doc_update(&encoded).unwrap();
        assert_eq!(decoded_id, id);
        assert_eq!(decoded_update, update);
    }

    #[test]
    fn foreign_doc_update_truncated_returns_none() {
        // id length claims 10 bytes but only 3 follow.
        assert!(decode_foreign_doc_update(&[10, b'a', b'b', b'c']).is_none());
    }

    #[test]
    fn foreign_doc_update_empty_returns_none() {
        assert!(decode_foreign_doc_update(&[]).is_none());
    }

    #[test]
    fn foreign_doc_update_zero_length_id_returns_empty_id_and_full_update() {
        // A zero-length id is structurally legal; the rest is update bytes.
        let encoded = vec![0u8, 0xAA, 0xBB];
        let (id, update) = decode_foreign_doc_update(&encoded).unwrap();
        assert_eq!(id, "");
        assert_eq!(update, &[0xAA, 0xBB]);
    }

    #[test]
    fn auth_message_utf8() {
        let token = "eyJhbGci...token";
        let encoded = encode_message(MessageType::Auth, token.as_bytes());
        let (msg_type, payload) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::Auth);
        assert_eq!(std::str::from_utf8(payload).unwrap(), token);
    }

    #[test]
    fn comment_event_payload_utf8() {
        let payload = r#"{"kind":"thread_created","threadId":"abcd"}"#;
        let encoded = encode_message(MessageType::CommentEvent, payload.as_bytes());
        let (msg_type, decoded) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::CommentEvent);
        assert_eq!(std::str::from_utf8(decoded).unwrap(), payload);
    }

    #[test]
    fn error_message_utf8() {
        let error = "unauthorized";
        let encoded = encode_message(MessageType::Error, error.as_bytes());
        let (msg_type, payload) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::Error);
        assert_eq!(std::str::from_utf8(payload).unwrap(), error);
    }
}
