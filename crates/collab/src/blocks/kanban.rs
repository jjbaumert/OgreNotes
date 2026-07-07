// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #137 — Kanban board live-app block.
//!
//! Three-NodeType tree:
//!
//!   Kanban → KanbanColumn → KanbanCard
//!
//! ## Attribute schema
//!
//! ### `Kanban`
//! Currently no interesting attributes; a future `wipEnabled`
//! boolean would toggle per-column WIP enforcement.
//!
//! ### `KanbanColumn`
//! - `title` — column header (required, max 60 chars). Empty
//!   titles would produce blank column headers so validation
//!   rejects the empty case.
//! - `wipLimit` — optional non-negative integer. v1 stores but
//!   doesn't enforce.
//!
//! ### `KanbanCard`
//! - `title` — card headline (required, max 120 chars).
//! - `content` — optional short description (max 500 chars,
//!   clamped like `CalendarEvent::content`).
//! - `color` — one of the six-hue palette Calendar uses.

use std::collections::HashMap;

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct KanbanBlock;
pub static KANBAN: KanbanBlock = KanbanBlock;

/// Six-hue palette reused from Calendar so card labels and event
/// chips render with the same visual language.
pub const CARD_COLORS: &[&str] =
    &["red", "orange", "yellow", "green", "blue", "violet"];
pub const DEFAULT_CARD_COLOR: &str = "blue";

pub const MAX_COLUMN_TITLE_LEN: usize = 60;
pub const MAX_CARD_TITLE_LEN: usize = 120;
pub const MAX_CARD_CONTENT_LEN: usize = 500;
// Newer card fields added in Phase 4b/4c. Keep in sync with the
// client-side renderer defenses in
// `frontend/src/editor/blocks/kanban.rs`.
//
// MAX_LABEL_RAW_LEN sized to accommodate the theoretical max
// canonical form: MAX_LABEL_COUNT * (MAX_LABEL_NAME_LEN + "|violet"
// + ";") ≈ 20 * 48 = 960. Round to 1024 for headroom. The
// client-side <input maxlength="400"> is a UX signal, not a
// security boundary — this server cap is what actually protects
// the stored value.
pub const MAX_LABEL_RAW_LEN: usize = 1024;
pub const MAX_LABEL_COUNT: usize = 20;
pub const MAX_LABEL_NAME_LEN: usize = 40;
pub const MAX_ASSIGNEE_NAME_LEN: usize = 120;
pub const MAX_ASSIGNEE_ID_LEN: usize = 64;
pub const DUE_AT_LEN: usize = 10; // Exactly "YYYY-MM-DD"

/// Attribute names each node type carries. Single source of
/// truth for callers that need to iterate attrs — mirrors
/// `blocks::calendar::CALENDAR_ATTR_NAMES` etc.
pub const KANBAN_ATTR_NAMES: &[&str] = &[];
pub const COLUMN_ATTR_NAMES: &[&str] = &["title", "wipLimit"];
pub const CARD_ATTR_NAMES: &[&str] = &[
    "title",
    "content",
    "color",
    "dueAt",
    "labels",
    "assigneeId",
    "assigneeName",
];

impl LiveAppBlock for KanbanBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[
            NodeType::Kanban,
            NodeType::KanbanColumn,
            NodeType::KanbanCard,
        ]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        match node_type {
            NodeType::Kanban => Ok(preserve_block_id(attrs)),
            NodeType::KanbanColumn => validate_column_attrs(attrs),
            NodeType::KanbanCard => validate_card_attrs(attrs),
            other => Err(BlockValidationError {
                node_type: other,
                field: std::borrow::Cow::Borrowed("node_type"),
                reason: format!(
                    "KanbanBlock cannot validate {}",
                    other.tag_name()
                ),
            }),
        }
    }
}

fn preserve_block_id(attrs: &HashMap<String, String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(bid) = attrs.get("blockId") {
        out.insert("blockId".into(), bid.clone());
    }
    out
}

fn validate_column_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    let mut out = preserve_block_id(attrs);
    let title = attrs
        .get("title")
        .cloned()
        .unwrap_or_default();
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(BlockValidationError {
            node_type: NodeType::KanbanColumn,
            field: std::borrow::Cow::Borrowed("title"),
            reason: "column title cannot be empty".into(),
        });
    }
    if title.chars().count() > MAX_COLUMN_TITLE_LEN {
        return Err(BlockValidationError {
            node_type: NodeType::KanbanColumn,
            field: std::borrow::Cow::Borrowed("title"),
            reason: format!(
                "column title exceeds {MAX_COLUMN_TITLE_LEN} chars",
            ),
        });
    }
    out.insert("title".into(), title);
    if let Some(limit) = attrs.get("wipLimit").filter(|v| !v.is_empty()) {
        match limit.parse::<u32>() {
            Ok(_) => {
                out.insert("wipLimit".into(), limit.clone());
            }
            Err(_) => {
                return Err(BlockValidationError {
                    node_type: NodeType::KanbanColumn,
                    field: std::borrow::Cow::Borrowed("wipLimit"),
                    reason: format!("expected non-negative integer, got {limit:?}"),
                });
            }
        }
    }
    Ok(out)
}

fn validate_card_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    let mut out = preserve_block_id(attrs);
    let title = attrs.get("title").cloned().unwrap_or_default();
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(BlockValidationError {
            node_type: NodeType::KanbanCard,
            field: std::borrow::Cow::Borrowed("title"),
            reason: "card title cannot be empty".into(),
        });
    }
    let title_clamped: String = if title.chars().count() > MAX_CARD_TITLE_LEN {
        title.chars().take(MAX_CARD_TITLE_LEN).collect()
    } else {
        title
    };
    out.insert("title".into(), title_clamped);

    let content = attrs.get("content").cloned().unwrap_or_default();
    let content_clamped: String = if content.chars().count() > MAX_CARD_CONTENT_LEN {
        content.chars().take(MAX_CARD_CONTENT_LEN).collect()
    } else {
        content
    };
    out.insert("content".into(), content_clamped);

    let color = attrs
        .get("color")
        .cloned()
        .unwrap_or_else(|| DEFAULT_CARD_COLOR.to_string());
    if !CARD_COLORS.contains(&color.as_str()) {
        return Err(BlockValidationError {
            node_type: NodeType::KanbanCard,
            field: std::borrow::Cow::Borrowed("color"),
            reason: format!("expected one of {CARD_COLORS:?}, got {color:?}"),
        });
    }
    out.insert("color".into(), color);

    // Phase 4b/4c fields — validated here so paste/import and
    // (Phase 2) the pre-apply CRDT gate both hit the same wall.
    // Clamp policy matches title/content: oversized values are
    // truncated silently (friendly to paste); structurally
    // wrong values reject (malformed `dueAt`).
    if let Some(due) = attrs.get("dueAt").filter(|s| !s.is_empty()) {
        let trimmed = due.trim();
        if trimmed.chars().count() != DUE_AT_LEN
            || !is_valid_ymd(trimmed)
        {
            return Err(BlockValidationError {
                node_type: NodeType::KanbanCard,
                field: std::borrow::Cow::Borrowed("dueAt"),
                reason: format!(
                    "expected YYYY-MM-DD, got {due:?}"
                ),
            });
        }
        out.insert("dueAt".into(), trimmed.to_string());
    }

    if let Some(raw) = attrs.get("labels").filter(|s| !s.is_empty()) {
        // Canonicalize by parsing → clamping → re-serializing.
        // Guarantees the stored value fits within both the raw
        // length cap and the per-label count / name cap. Also
        // drops empty segments so the client's `parse_labels`
        // renders exactly what we stored.
        let canonical = clamp_labels(raw);
        if !canonical.is_empty() {
            out.insert("labels".into(), canonical);
        }
    }

    if let Some(id) = attrs.get("assigneeId").filter(|s| !s.is_empty()) {
        let clamped = clamp_chars(id, MAX_ASSIGNEE_ID_LEN);
        out.insert("assigneeId".into(), clamped);
    }
    if let Some(name) = attrs.get("assigneeName").filter(|s| !s.is_empty()) {
        let clamped = clamp_chars(name, MAX_ASSIGNEE_NAME_LEN);
        out.insert("assigneeName".into(), clamped);
    }

    Ok(out)
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// True iff `s` matches `YYYY-MM-DD` component-wise. Does not
/// validate calendar semantics (Feb 31 passes) — callers that
/// need calendar-correct dates must layer chrono on top.
fn is_valid_ymd(s: &str) -> bool {
    let mut parts = s.splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    y.len() == 4
        && m.len() == 2
        && d.len() == 2
        && y.chars().all(|c| c.is_ascii_digit())
        && m.chars().all(|c| c.is_ascii_digit())
        && d.chars().all(|c| c.is_ascii_digit())
}

/// Parse the pipe-and-semicolon label syntax, cap count + name
/// length, canonicalize back to a `name|color;name;...` string.
/// Empty result → caller drops the attr entirely.
fn clamp_labels(raw: &str) -> String {
    // Byte-length gate: refuse to parse anything longer than the
    // canonical raw cap. An adversarial 10 MB labels string
    // shouldn't even walk the parser. Char-based prefix (not
    // byte slicing) so a multi-byte char at the boundary can't
    // panic — MAX_LABEL_RAW_LEN is a soft "reasonable" ceiling,
    // not a byte-precise gate.
    let capped: String = if raw.len() > MAX_LABEL_RAW_LEN {
        raw.chars().take(MAX_LABEL_RAW_LEN).collect()
    } else {
        raw.to_string()
    };
    let mut kept: Vec<String> = Vec::new();
    for pair in capped.split(';') {
        if kept.len() >= MAX_LABEL_COUNT {
            break;
        }
        let mut parts = pair.splitn(2, '|');
        let name = parts.next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let name = clamp_chars(name, MAX_LABEL_NAME_LEN);
        let color = parts.next().unwrap_or("").trim();
        if color.is_empty() || !CARD_COLORS.contains(&color) {
            kept.push(name);
        } else {
            kept.push(format!("{name}|{color}"));
        }
    }
    kept.join(";")
}

/// HTML tag for a Kanban-owned NodeType. Rendered as nested
/// `<div>`s to preserve the container/column/card hierarchy in
/// static HTML export — no interactive controls, just structure.
pub fn html_tag(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Kanban | NodeType::KanbanColumn | NodeType::KanbanCard => "div",
        _ => "div",
    }
}

/// Extra HTML attributes per NodeType. Delegated from
/// `export.rs::render_html_attrs`.
pub fn html_attrs(node_type: NodeType, attrs: &HashMap<String, String>) -> String {
    let mut out = String::new();
    match node_type {
        NodeType::Kanban => {
            out.push_str(" class=\"kanban-block\"");
        }
        NodeType::KanbanColumn => {
            out.push_str(" class=\"kanban-column\"");
            if let Some(title) = attrs.get("title") {
                out.push_str(&format!(" data-title=\"{}\"", escape_attr(title)));
            }
            if let Some(limit) = attrs.get("wipLimit") {
                out.push_str(&format!(" data-wip-limit=\"{}\"", escape_attr(limit)));
            }
        }
        NodeType::KanbanCard => {
            let color = attrs
                .get("color")
                .filter(|c| CARD_COLORS.contains(&c.as_str()))
                .cloned()
                .unwrap_or_else(|| DEFAULT_CARD_COLOR.to_string());
            out.push_str(&format!(
                " class=\"kanban-card kanban-card--{}\"",
                escape_attr(&color)
            ));
            if let Some(title) = attrs.get("title") {
                out.push_str(&format!(" data-title=\"{}\"", escape_attr(title)));
            }
        }
        _ => {}
    }
    out
}

/// Markdown placeholder for kanban nodes.
/// - `Kanban` renders a leading horizontal-rule marker; the
///   trailing marker is emitted by the caller in
///   `export.rs::render_node_markdown` after the children walk,
///   so the board is bracketed once each side.
/// - `KanbanColumn` renders a `## title` heading (with optional
///   WIP limit noted).
/// - `KanbanCard` renders `- title` followed by a content
///   paragraph if content is non-empty.
///
/// The rendering is lossy but preserves the structural shape so
/// a paste-into-markdown workflow at least keeps titles + order.
pub fn markdown_placeholder(
    node_type: NodeType,
    attrs: &HashMap<String, String>,
) -> String {
    match node_type {
        // Empty — the caller in export.rs brackets the board with
        // dividers on either side of the children walk. Duplicating
        // a `---` here would produce back-to-back separators.
        NodeType::Kanban => String::new(),
        NodeType::KanbanColumn => {
            let title = attrs
                .get("title")
                .cloned()
                .unwrap_or_else(|| "(untitled)".to_string());
            let wip = attrs
                .get("wipLimit")
                .map(|v| format!(" (WIP {v})"))
                .unwrap_or_default();
            format!("## {title}{wip}\n\n")
        }
        NodeType::KanbanCard => {
            let title = attrs
                .get("title")
                .cloned()
                .unwrap_or_else(|| "(untitled)".to_string());
            let color = attrs
                .get("color")
                .cloned()
                .unwrap_or_else(|| DEFAULT_CARD_COLOR.to_string());
            let content = attrs.get("content").cloned().unwrap_or_default();
            let content_line = if content.is_empty() {
                String::new()
            } else {
                format!("  \n  {content}\n")
            };
            format!("- ({color}) {title}\n{content_line}")
        }
        _ => String::new(),
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    #[test]
    fn column_requires_non_empty_title() {
        assert!(validate_column_attrs(&HashMap::new()).is_err());
        assert!(validate_column_attrs(&attrs(&[("title", "   ")])).is_err());
    }

    #[test]
    fn column_rejects_overlong_title() {
        let long = "x".repeat(MAX_COLUMN_TITLE_LEN + 1);
        let a = attrs(&[("title", long.as_str())]);
        assert!(validate_column_attrs(&a).is_err());
    }

    #[test]
    fn column_accepts_wip_limit() {
        let a = attrs(&[("title", "In Progress"), ("wipLimit", "5")]);
        let out = validate_column_attrs(&a).unwrap();
        assert_eq!(out.get("wipLimit").map(String::as_str), Some("5"));
    }

    #[test]
    fn column_rejects_non_numeric_wip_limit() {
        let a = attrs(&[("title", "Done"), ("wipLimit", "many")]);
        assert!(validate_column_attrs(&a).is_err());
    }

    #[test]
    fn card_requires_non_empty_title() {
        assert!(validate_card_attrs(&HashMap::new()).is_err());
        assert!(validate_card_attrs(&attrs(&[("title", "  ")])).is_err());
    }

    #[test]
    fn card_clamps_long_title_and_content() {
        let long_title = "x".repeat(MAX_CARD_TITLE_LEN + 50);
        let long_content = "y".repeat(MAX_CARD_CONTENT_LEN + 200);
        let a = attrs(&[
            ("title", long_title.as_str()),
            ("content", long_content.as_str()),
        ]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(out.get("title").unwrap().chars().count(), MAX_CARD_TITLE_LEN);
        assert_eq!(
            out.get("content").unwrap().chars().count(),
            MAX_CARD_CONTENT_LEN
        );
    }

    #[test]
    fn card_rejects_unknown_color() {
        let a = attrs(&[("title", "Ship it"), ("color", "chartreuse")]);
        assert!(validate_card_attrs(&a).is_err());
    }

    #[test]
    fn card_defaults_color_when_absent() {
        let a = attrs(&[("title", "Ship it")]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(out.get("color").map(String::as_str), Some(DEFAULT_CARD_COLOR));
    }

    #[test]
    fn html_attrs_card_uses_color_class() {
        let a = attrs(&[("title", "Draft"), ("color", "green")]);
        let s = html_attrs(NodeType::KanbanCard, &a);
        assert!(s.contains("kanban-card--green"));
        assert!(s.contains("data-title=\"Draft\""));
    }

    #[test]
    fn html_attrs_column_carries_title() {
        let a = attrs(&[("title", "In Progress")]);
        let s = html_attrs(NodeType::KanbanColumn, &a);
        assert!(s.contains("class=\"kanban-column\""));
        assert!(s.contains("data-title=\"In Progress\""));
    }

    #[test]
    fn markdown_column_renders_as_heading() {
        let a = attrs(&[("title", "In Progress"), ("wipLimit", "5")]);
        let s = markdown_placeholder(NodeType::KanbanColumn, &a);
        assert!(s.contains("## In Progress (WIP 5)"));
    }

    #[test]
    fn markdown_card_renders_content_line_when_present() {
        let a = attrs(&[
            ("title", "Fix bug"),
            ("color", "red"),
            ("content", "See #123"),
        ]);
        let s = markdown_placeholder(NodeType::KanbanCard, &a);
        assert!(s.contains("- (red) Fix bug"));
        assert!(s.contains("See #123"));
    }

    // ── Phase 4b/4c newer fields ─────────────────────────

    #[test]
    fn card_accepts_valid_due_at() {
        let a = attrs(&[("title", "Fix"), ("dueAt", "2026-07-04")]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(out.get("dueAt").map(String::as_str), Some("2026-07-04"));
    }

    #[test]
    fn card_rejects_malformed_due_at() {
        let a = attrs(&[("title", "Fix"), ("dueAt", "not-a-date")]);
        assert!(validate_card_attrs(&a).is_err());
        let a = attrs(&[("title", "Fix"), ("dueAt", "2026-7-4")]);
        assert!(validate_card_attrs(&a).is_err());
        let a = attrs(&[("title", "Fix"), ("dueAt", "2026-07-04-extra")]);
        assert!(validate_card_attrs(&a).is_err());
    }

    #[test]
    fn card_omits_empty_due_at() {
        let a = attrs(&[("title", "Fix"), ("dueAt", "")]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(out.get("dueAt"), None);
    }

    #[test]
    fn card_clamps_assignee_fields() {
        let long_id = "i".repeat(MAX_ASSIGNEE_ID_LEN + 20);
        let long_name = "n".repeat(MAX_ASSIGNEE_NAME_LEN + 20);
        let a = attrs(&[
            ("title", "Fix"),
            ("assigneeId", long_id.as_str()),
            ("assigneeName", long_name.as_str()),
        ]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(
            out.get("assigneeId").unwrap().chars().count(),
            MAX_ASSIGNEE_ID_LEN
        );
        assert_eq!(
            out.get("assigneeName").unwrap().chars().count(),
            MAX_ASSIGNEE_NAME_LEN
        );
    }

    #[test]
    fn card_omits_empty_assignee_fields() {
        let a = attrs(&[
            ("title", "Fix"),
            ("assigneeId", ""),
            ("assigneeName", ""),
        ]);
        let out = validate_card_attrs(&a).unwrap();
        assert!(out.get("assigneeId").is_none());
        assert!(out.get("assigneeName").is_none());
    }

    #[test]
    fn clamp_labels_caps_count() {
        // 25 short-named labels — must clamp to MAX_LABEL_COUNT.
        // Names fit under the name cap so this test isolates the
        // count cap from the name cap.
        let src = (0..25)
            .map(|i| format!("a{i}|red"))
            .collect::<Vec<_>>()
            .join(";");
        let out = clamp_labels(&src);
        let parts: Vec<&str> = out.split(';').collect();
        assert_eq!(parts.len(), MAX_LABEL_COUNT);
    }

    #[test]
    fn clamp_labels_caps_name_len() {
        // Single oversized label — must clamp to MAX_LABEL_NAME_LEN.
        let long_name = "x".repeat(MAX_LABEL_NAME_LEN + 20);
        let src = format!("{long_name}|red");
        let out = clamp_labels(&src);
        let name = out.split('|').next().unwrap();
        assert_eq!(name.chars().count(), MAX_LABEL_NAME_LEN);
    }

    #[test]
    fn clamp_labels_drops_unknown_color() {
        let out = clamp_labels("bug|neon");
        assert_eq!(out, "bug");
    }

    #[test]
    fn clamp_labels_preserves_valid_syntax() {
        let out = clamp_labels("bug|red;ux|blue;urgent");
        assert_eq!(out, "bug|red;ux|blue;urgent");
    }

    #[test]
    fn clamp_labels_handles_oversized_raw() {
        // Byte-length of a 10000-char raw string well exceeds
        // MAX_LABEL_RAW_LEN; must not panic and must produce a
        // bounded result.
        let big = "a|red;".repeat(2000);
        let out = clamp_labels(&big);
        // Bounded number of segments.
        assert!(out.split(';').count() <= MAX_LABEL_COUNT);
    }

    #[test]
    fn card_stores_canonicalized_labels() {
        let a = attrs(&[("title", "Fix"), ("labels", "bug|red;;ux|blue;")]);
        let out = validate_card_attrs(&a).unwrap();
        assert_eq!(out.get("labels").map(String::as_str), Some("bug|red;ux|blue"));
    }

    #[test]
    fn is_valid_ymd_accepts_and_rejects() {
        assert!(is_valid_ymd("2026-07-04"));
        assert!(is_valid_ymd("0001-01-01"));
        assert!(!is_valid_ymd("2026-7-04"));
        assert!(!is_valid_ymd("2026-07-4"));
        assert!(!is_valid_ymd("26-07-04"));
        assert!(!is_valid_ymd("2026/07/04"));
        assert!(!is_valid_ymd(""));
    }
}
