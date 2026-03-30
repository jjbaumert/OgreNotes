//! Awareness state for cursor presence and user tracking.
//!
//! Each connected user broadcasts their cursor position, selection range,
//! and identity. Awareness updates are serialized as JSON within binary
//! protocol messages (MessageType::Awareness).

use serde::{Deserialize, Serialize};

/// Per-user awareness state broadcast to all collaborators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessState {
    /// User ID of the collaborator.
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Color index (0-11) for cursor and selection highlight.
    pub color: u8,
    /// Cursor position in the document (model position), if focused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_pos: Option<u32>,
    /// Selection anchor position, if a range is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_anchor: Option<u32>,
    /// Selection head position, if a range is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_head: Option<u32>,
}

/// Color palette for collaborator cursors (12 distinct colors).
pub const CURSOR_COLORS: [&str; 12] = [
    "#E57373", // red
    "#64B5F6", // blue
    "#81C784", // green
    "#FFB74D", // orange
    "#BA68C8", // purple
    "#4DD0E1", // cyan
    "#F06292", // pink
    "#AED581", // lime
    "#FFD54F", // amber
    "#7986CB", // indigo
    "#4DB6AC", // teal
    "#A1887F", // brown
];

/// Assign a color index based on client ID (deterministic).
pub fn color_for_client(client_id: u64) -> u8 {
    (client_id % CURSOR_COLORS.len() as u64) as u8
}

/// Encode an awareness state to JSON bytes.
pub fn encode_awareness(state: &AwarenessState) -> Vec<u8> {
    serde_json::to_vec(state).unwrap_or_default()
}

/// Decode an awareness state from JSON bytes.
pub fn decode_awareness(data: &[u8]) -> Option<AwarenessState> {
    serde_json::from_slice(data).ok()
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let state = AwarenessState {
            user_id: "user-123".to_string(),
            name: "Alice".to_string(),
            color: 3,
            cursor_pos: Some(42),
            selection_anchor: Some(10),
            selection_head: Some(20),
        };
        let bytes = encode_awareness(&state);
        let decoded = decode_awareness(&bytes).unwrap();
        assert_eq!(decoded.user_id, "user-123");
        assert_eq!(decoded.name, "Alice");
        assert_eq!(decoded.color, 3);
        assert_eq!(decoded.cursor_pos, Some(42));
        assert_eq!(decoded.selection_anchor, Some(10));
        assert_eq!(decoded.selection_head, Some(20));
    }

    #[test]
    fn cursor_only_no_selection() {
        let state = AwarenessState {
            user_id: "u1".to_string(),
            name: "Bob".to_string(),
            color: 0,
            cursor_pos: Some(5),
            selection_anchor: None,
            selection_head: None,
        };
        let json = String::from_utf8(encode_awareness(&state)).unwrap();
        assert!(!json.contains("selection_anchor"));
        assert!(!json.contains("selection_head"));

        let decoded = decode_awareness(json.as_bytes()).unwrap();
        assert_eq!(decoded.cursor_pos, Some(5));
        assert!(decoded.selection_anchor.is_none());
    }

    #[test]
    fn color_assignment_deterministic() {
        assert_eq!(color_for_client(0), 0);
        assert_eq!(color_for_client(12), 0);
        assert_eq!(color_for_client(1), 1);
        assert_eq!(color_for_client(11), 11);
    }

    #[test]
    fn decode_invalid_returns_none() {
        assert!(decode_awareness(b"not json").is_none());
        assert!(decode_awareness(b"").is_none());
    }
}
