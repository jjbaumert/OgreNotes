// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Awareness state for cursor presence and user tracking.
//!
//! Each connected user broadcasts their cursor position, selection range,
//! and identity. Awareness updates are serialized as JSON within binary
//! protocol messages (MessageType::Awareness).

use ogrenotes_common::metrics::{counter, MetricKey};
use serde::{Deserialize, Serialize};

/// Maximum total bytes of a single awareness JSON payload (#37).
///
/// Awareness messages are small by construction — user id, display
/// name, a couple of block ids, a few u32 offsets. A 4 KiB cap is
/// well above any legitimate payload and tight enough that a hostile
/// client can't amplify a fan-out to 100 subscribers into hundreds
/// of MiB per ping. Anything above this is dropped before the JSON
/// parser even runs.
pub const MAX_AWARENESS_PAYLOAD_BYTES: usize = 4 * 1024;

/// Maximum bytes of any single string field inside the decoded
/// awareness state (#37). The struct's string fields are:
///   - `user_id` — overwritten by the WS handler from the
///     authenticated session, but bounded as defense-in-depth.
///   - `name` — display name; UTF-8.
///   - `cursor_block_id`, `sel_anchor_block_id`,
///     `sel_head_block_id` — DOM block ids; the editor's id
///     generator (nanoid, 16 char base62) fits well within this.
///   - `typing_thread_id` — comment thread id; same generator.
///
/// Reject the whole payload if any field exceeds — a partial
/// rebroadcast would leak that a field was trimmed.
pub const MAX_AWARENESS_FIELD_BYTES: usize = 256;

/// Per-user awareness state broadcast to all collaborators.
///
/// The frontend (see `frontend/src/collab/ws_client.rs::AwarenessPayload`)
/// now publishes cursor and selection positions as *block-relative* tuples
/// (`block_id`, `offset_within_block`) for cross-client portability. The
/// server only validates `user_id` (anti-spoofing) and rebroadcasts via
/// decode → mutate → encode; every field the server cares to preserve must
/// be present on this struct or serde will drop it on decode.
///
/// The legacy absolute-position fields (`cursor_pos`, `selection_anchor`,
/// `selection_head`) remain for backwards compatibility with any older
/// client; the frontend sends them as `None`. Both shapes coexist on the
/// wire without conflict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessState {
    /// User ID of the collaborator.
    pub user_id: String,
    /// Display name.
    pub name: String,
    /// Color index (0-11) for cursor and selection highlight.
    pub color: u8,

    // ─── Block-relative positions (current wire format) ────────

    /// Block element ID the cursor is inside, if focused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_block_id: Option<String>,
    /// Character offset within `cursor_block_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_offset: Option<u32>,
    /// Block element ID of the selection anchor (where the selection began).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sel_anchor_block_id: Option<String>,
    /// Character offset within `sel_anchor_block_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sel_anchor_offset: Option<u32>,
    /// Block element ID of the selection head (where the caret currently sits).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sel_head_block_id: Option<String>,
    /// Character offset within `sel_head_block_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sel_head_offset: Option<u32>,

    // ─── Legacy absolute positions (backwards compat) ──────────

    /// Cursor position in the document (absolute model position), if focused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_pos: Option<u32>,
    /// Selection anchor position (absolute), if a range is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_anchor: Option<u32>,
    /// Selection head position (absolute), if a range is selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_head: Option<u32>,

    /// Thread ID where the user is currently typing (for typing indicators).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing_thread_id: Option<String>,
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
    // AwarenessState can't fail to serialize today, but if a future field
    // ever could, an empty vec would make a peer's presence silently
    // vanish (decode returns None) with no trace — log it instead (#8).
    serde_json::to_vec(state).unwrap_or_else(|e| {
        tracing::error!(error = %e, "awareness encode failed");
        Vec::new()
    })
}

/// Decode an awareness state from JSON bytes.
///
/// Returns `None` when:
///   - the payload is too large (above `MAX_AWARENESS_PAYLOAD_BYTES`),
///   - the JSON is malformed,
///   - or any string field exceeds `MAX_AWARENESS_FIELD_BYTES`.
///
/// The caller (the WS handler in `routes::ws`) already silently
/// drops on `None`, so an oversized or hostile payload never
/// reaches the broadcast fan-out. Each rejection bumps a counter
/// so operators can see the rate of dropped frames.
pub fn decode_awareness(data: &[u8]) -> Option<AwarenessState> {
    if data.len() > MAX_AWARENESS_PAYLOAD_BYTES {
        counter::inc(MetricKey::new(
            "ws.awareness_rejected_total",
            &[("reason", "payload_oversize")],
        ));
        return None;
    }
    let state: AwarenessState = match serde_json::from_slice(data) {
        Ok(s) => s,
        Err(_) => {
            counter::inc(MetricKey::new(
                "ws.awareness_rejected_total",
                &[("reason", "json_malformed")],
            ));
            return None;
        }
    };
    if state_has_oversize_field(&state) {
        counter::inc(MetricKey::new(
            "ws.awareness_rejected_total",
            &[("reason", "field_oversize")],
        ));
        return None;
    }
    Some(state)
}

/// True if any single string field on the decoded state exceeds the
/// per-field byte cap. Numeric fields are bounded by their `u32`
/// type, so this only walks the string-shaped fields.
///
/// Destructures `AwarenessState` exhaustively rather than using
/// named-field access. If a future contributor adds a new `String`
/// (or any new field) to the struct, this fn fails to compile
/// instead of silently skipping the new field's bound. Numeric
/// fields are bound to `_` because their `u32` / `u8` types already
/// bound their byte cost; binding them to a name (rather than
/// catching them with `..`) keeps the exhaustiveness check fire-
/// armed if one is ever removed.
fn state_has_oversize_field(s: &AwarenessState) -> bool {
    fn over(s: &str) -> bool {
        s.len() > MAX_AWARENESS_FIELD_BYTES
    }
    fn over_opt(s: &Option<String>) -> bool {
        s.as_deref().map(over).unwrap_or(false)
    }
    let AwarenessState {
        user_id,
        name,
        color: _,
        cursor_block_id,
        cursor_offset: _,
        sel_anchor_block_id,
        sel_anchor_offset: _,
        sel_head_block_id,
        sel_head_offset: _,
        cursor_pos: _,
        selection_anchor: _,
        selection_head: _,
        typing_thread_id,
    } = s;
    over(user_id)
        || over(name)
        || over_opt(cursor_block_id)
        || over_opt(sel_anchor_block_id)
        || over_opt(sel_head_block_id)
        || over_opt(typing_thread_id)
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: construct an `AwarenessState` with all fields defaulted to None
    /// except the ones explicitly set. Keeps the test literals readable as
    /// the struct grows.
    fn empty_state(user_id: &str, name: &str, color: u8) -> AwarenessState {
        AwarenessState {
            user_id: user_id.to_string(),
            name: name.to_string(),
            color,
            cursor_block_id: None,
            cursor_offset: None,
            sel_anchor_block_id: None,
            sel_anchor_offset: None,
            sel_head_block_id: None,
            sel_head_offset: None,
            cursor_pos: None,
            selection_anchor: None,
            selection_head: None,
            typing_thread_id: None,
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let state = AwarenessState {
            cursor_pos: Some(42),
            selection_anchor: Some(10),
            selection_head: Some(20),
            ..empty_state("user-123", "Alice", 3)
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
            cursor_pos: Some(5),
            ..empty_state("u1", "Bob", 0)
        };
        let json = String::from_utf8(encode_awareness(&state)).unwrap();
        assert!(!json.contains("selection_anchor"));
        assert!(!json.contains("selection_head"));

        let decoded = decode_awareness(json.as_bytes()).unwrap();
        assert_eq!(decoded.cursor_pos, Some(5));
        assert!(decoded.selection_anchor.is_none());
    }

    /// Regression: the server broadcasts awareness via decode → mutate
    /// user_id → encode. Prior to adding the block-relative fields to
    /// `AwarenessState`, serde dropped `cursor_block_id` / `cursor_offset` /
    /// `sel_*_block_id` / `sel_*_offset` during decode, so the rebroadcast
    /// arrived at the peer with only null legacy fields and nothing
    /// rendered. This test pins the pass-through shape that the frontend
    /// depends on.
    #[test]
    fn pass_through_preserves_block_relative_fields() {
        // Exactly what the frontend sends: populated new fields, null
        // legacy fields (omitted from JSON by skip_serializing_if).
        let frontend_json = r#"{
            "user_id": "client-claimed-id",
            "name": "Alice",
            "color": 3,
            "cursor_block_id": "block-abc",
            "cursor_offset": 7,
            "sel_anchor_block_id": "block-abc",
            "sel_anchor_offset": 2,
            "sel_head_block_id": "block-def",
            "sel_head_offset": 9
        }"#;

        // Server-side broadcast path: decode → mutate user_id → encode.
        let mut state = decode_awareness(frontend_json.as_bytes())
            .expect("frontend JSON must decode");
        assert_eq!(state.cursor_block_id.as_deref(), Some("block-abc"));
        assert_eq!(state.cursor_offset, Some(7));
        state.user_id = "server-authoritative-id".to_string();

        let rebroadcast = encode_awareness(&state);

        // Peer decode — every new field must still be present.
        let peer = decode_awareness(&rebroadcast).expect("rebroadcast must decode");
        assert_eq!(peer.user_id, "server-authoritative-id");
        assert_eq!(peer.cursor_block_id.as_deref(), Some("block-abc"));
        assert_eq!(peer.cursor_offset, Some(7));
        assert_eq!(peer.sel_anchor_block_id.as_deref(), Some("block-abc"));
        assert_eq!(peer.sel_anchor_offset, Some(2));
        assert_eq!(peer.sel_head_block_id.as_deref(), Some("block-def"));
        assert_eq!(peer.sel_head_offset, Some(9));
        // Legacy fields stay absent.
        assert!(peer.cursor_pos.is_none());
        assert!(peer.selection_anchor.is_none());
        assert!(peer.selection_head.is_none());
    }

    // ─── Golden wire-format fixture tests ───────────────────────
    //
    // The fixtures under `tests/fixtures/protocol/awareness/*.json` are the
    // shared contract with the frontend. Both sides must be able to
    // decode every fixture, re-encode it, and round-trip the populated
    // fields losslessly. A regression where either side silently drops a
    // field on encode (the class of bug that broke cursor rendering)
    // will fail here.
    //
    // `include_str!` embeds the fixtures at compile time — no runtime
    // filesystem access, so the same test strategy works on native and
    // WASM targets.

    const FIXTURE_CURSOR_ONLY: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/cursor-only.json");
    const FIXTURE_SELECTION: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/selection.json");
    const FIXTURE_TYPING_INDICATOR: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/typing-indicator.json");
    const FIXTURE_LEGACY_ABSOLUTE: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/legacy-absolute.json");
    const FIXTURE_NO_PRESENCE: &str =
        include_str!("../../../tests/fixtures/protocol/awareness/no-presence.json");

    /// Asserts a fixture decodes, re-encodes, and decodes again without losing
    /// any populated field. Uses a `serde_json::Value` equality check on the
    /// populated subset so the test is independent of field ordering.
    fn assert_fixture_round_trips(raw: &str, name: &str) {
        use serde_json::Value;

        // Decode the source once to establish the "populated fields" baseline.
        let source: Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{name}: fixture is not valid JSON: {e}"));
        let source_obj = source
            .as_object()
            .unwrap_or_else(|| panic!("{name}: fixture root must be an object"));

        // Decode → re-encode via the wire-format struct.
        let state = decode_awareness(raw.as_bytes())
            .unwrap_or_else(|| panic!("{name}: decode_awareness rejected the fixture"));
        let rebroadcast = encode_awareness(&state);
        let rebroadcast_value: Value = serde_json::from_slice(&rebroadcast)
            .unwrap_or_else(|e| panic!("{name}: rebroadcast bytes are not valid JSON: {e}"));
        let rebroadcast_obj = rebroadcast_value
            .as_object()
            .unwrap_or_else(|| panic!("{name}: rebroadcast root must be an object"));

        // Every key present in the source must also be present and identical
        // in the rebroadcast. (Rebroadcast MAY add keys that were serialized
        // with a default — e.g. an enum tag — but MUST NOT drop anything.)
        for (key, value) in source_obj {
            let rebroadcast_value = rebroadcast_obj.get(key).unwrap_or_else(|| {
                panic!(
                    "{name}: field `{key}` dropped by backend decode/encode — \
                    the wire-format struct is missing this field"
                )
            });
            assert_eq!(
                rebroadcast_value, value,
                "{name}: field `{key}` changed across decode/encode"
            );
        }
    }

    #[test]
    fn fixture_cursor_only_preserved() {
        assert_fixture_round_trips(FIXTURE_CURSOR_ONLY, "cursor-only");
    }

    #[test]
    fn fixture_selection_preserved() {
        assert_fixture_round_trips(FIXTURE_SELECTION, "selection");
    }

    #[test]
    fn fixture_typing_indicator_preserved() {
        assert_fixture_round_trips(FIXTURE_TYPING_INDICATOR, "typing-indicator");
    }

    #[test]
    fn fixture_legacy_absolute_preserved() {
        assert_fixture_round_trips(FIXTURE_LEGACY_ABSOLUTE, "legacy-absolute");
    }

    #[test]
    fn fixture_no_presence_preserved() {
        assert_fixture_round_trips(FIXTURE_NO_PRESENCE, "no-presence");
    }

    #[test]
    fn block_relative_cursor_omits_legacy_fields_on_encode() {
        let state = AwarenessState {
            cursor_block_id: Some("b1".to_string()),
            cursor_offset: Some(4),
            ..empty_state("u", "n", 0)
        };
        let json = String::from_utf8(encode_awareness(&state)).unwrap();
        assert!(json.contains("\"cursor_block_id\":\"b1\""));
        assert!(json.contains("\"cursor_offset\":4"));
        assert!(!json.contains("\"cursor_pos\""));
        assert!(!json.contains("\"selection_anchor\""));
        assert!(!json.contains("\"selection_head\""));
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

    #[test]
    fn cursor_colors_are_valid_hex() {
        assert_eq!(CURSOR_COLORS.len(), 12);
        for (i, color) in CURSOR_COLORS.iter().enumerate() {
            assert!(color.starts_with('#'), "color {i} doesn't start with #: {color}");
            assert_eq!(color.len(), 7, "color {i} is not 7 chars: {color}");
            assert!(
                u32::from_str_radix(&color[1..], 16).is_ok(),
                "color {i} is not valid hex: {color}"
            );
        }
    }

    #[test]
    fn color_wraps_for_large_client_ids() {
        // Should wrap around without panic for any u64 value
        let c = color_for_client(u64::MAX);
        assert!((c as usize) < CURSOR_COLORS.len());

        // Verify wrapping: u64::MAX % 12 should match
        let expected = (u64::MAX % 12) as u8;
        assert_eq!(c, expected);
    }

    #[test]
    fn decode_with_missing_optional_fields() {
        // JSON with only required fields — optional fields absent
        let json = r#"{"user_id":"u1","name":"Bob","color":5}"#;
        let state = decode_awareness(json.as_bytes()).unwrap();
        assert_eq!(state.user_id, "u1");
        assert_eq!(state.name, "Bob");
        assert_eq!(state.color, 5);
        assert!(state.cursor_pos.is_none());
        assert!(state.selection_anchor.is_none());
        assert!(state.selection_head.is_none());
    }

    // ─── #37: payload + per-field bounds ────────────────────────

    #[test]
    fn decode_rejects_payload_larger_than_cap() {
        // A `name` of just under 4 KiB pushes the JSON encoding past
        // the payload cap. Without the size check we'd parse it and
        // then re-broadcast 4 KiB × N subscribers per ping.
        let huge_name = "x".repeat(MAX_AWARENESS_PAYLOAD_BYTES);
        let json = format!(
            r#"{{"user_id":"u1","name":"{huge_name}","color":1}}"#
        );
        assert!(json.len() > MAX_AWARENESS_PAYLOAD_BYTES);
        assert!(decode_awareness(json.as_bytes()).is_none());
    }

    #[test]
    fn decode_rejects_oversize_name_field_under_payload_cap() {
        // `name` alone above the per-field cap but total payload
        // still under the 4 KiB envelope. Without the per-field
        // check this would smuggle large strings through to the
        // broadcast path.
        let long_name = "n".repeat(MAX_AWARENESS_FIELD_BYTES + 1);
        let json = format!(
            r#"{{"user_id":"u1","name":"{long_name}","color":1}}"#
        );
        assert!(json.len() < MAX_AWARENESS_PAYLOAD_BYTES);
        assert!(decode_awareness(json.as_bytes()).is_none());
    }

    #[test]
    fn decode_rejects_oversize_block_id_field() {
        let long_id = "b".repeat(MAX_AWARENESS_FIELD_BYTES + 1);
        let json = format!(
            r#"{{"user_id":"u1","name":"x","color":1,"cursor_block_id":"{long_id}"}}"#
        );
        assert!(decode_awareness(json.as_bytes()).is_none());
    }

    #[test]
    fn decode_accepts_string_exactly_at_field_cap() {
        // Boundary case — equal to the cap is accepted; one byte
        // over is rejected by the test above.
        let at_cap = "x".repeat(MAX_AWARENESS_FIELD_BYTES);
        let json = format!(
            r#"{{"user_id":"u1","name":"{at_cap}","color":1}}"#
        );
        let state = decode_awareness(json.as_bytes())
            .expect("string equal to cap must be accepted");
        assert_eq!(state.name.len(), MAX_AWARENESS_FIELD_BYTES);
    }

    #[test]
    fn decode_rejects_multibyte_string_above_cap_in_bytes() {
        // Pins the byte-vs-char semantic. The cap is bytes-in-
        // transit (the goal is bandwidth, not glyph count). A
        // future regression that switches to `.chars().count()`
        // would silently let 256 multibyte chars (potentially
        // 1 KiB+) through; this test catches that. `é` is two
        // bytes in UTF-8, so 129 of them is 258 bytes — just
        // over the 256-byte cap.
        let multibyte_over = "é".repeat(MAX_AWARENESS_FIELD_BYTES / 2 + 1);
        assert_eq!(multibyte_over.len(), MAX_AWARENESS_FIELD_BYTES + 2);
        let json = format!(
            r#"{{"user_id":"u1","name":"{multibyte_over}","color":1}}"#
        );
        assert!(decode_awareness(json.as_bytes()).is_none());

        // The matching just-under-cap case must be accepted —
        // 128 two-byte chars = 256 bytes, exactly at the cap.
        let multibyte_at = "é".repeat(MAX_AWARENESS_FIELD_BYTES / 2);
        assert_eq!(multibyte_at.len(), MAX_AWARENESS_FIELD_BYTES);
        let json = format!(
            r#"{{"user_id":"u1","name":"{multibyte_at}","color":1}}"#
        );
        decode_awareness(json.as_bytes())
            .expect("multibyte string exactly at byte cap must be accepted");
    }
}
