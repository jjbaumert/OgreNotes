//! Binary WebSocket protocol for real-time document collaboration.
//!
//! Each message is a binary frame with a 1-byte type prefix followed by payload bytes.

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
pub fn decode_message(data: &[u8]) -> Option<(MessageType, &[u8])> {
    if data.is_empty() {
        return None;
    }
    let msg_type = MessageType::from_byte(data[0])?;
    Some((msg_type, &data[1..]))
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
    fn auth_message_utf8() {
        let token = "eyJhbGci...token";
        let encoded = encode_message(MessageType::Auth, token.as_bytes());
        let (msg_type, payload) = decode_message(&encoded).unwrap();
        assert_eq!(msg_type, MessageType::Auth);
        assert_eq!(std::str::from_utf8(payload).unwrap(), token);
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
