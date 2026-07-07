// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #137 — frontend Kanban board block.
//!
//! Renders the three-NodeType tree
//! (`Kanban → KanbanColumn → KanbanCard`) as a horizontal-scrolling
//! column strip. Each column has a header (title + card count) and
//! a vertical card list. Interaction wiring (add/edit/delete card,
//! rename column, drag card between columns) is Phase 2 — this
//! Phase 1 landing renders the model and stamps the data-attrs
//! future observers will hook onto.
//!
//! Reuses the six-hue palette Calendar established.

use std::collections::HashMap;

use web_sys::{Document, Element, Node as DomNode};

use super::super::model::{Fragment, Node, NodeType};
use super::{LiveAppBlockInsert, LiveAppBlockView};

pub struct KanbanView;
pub struct KanbanInsert;

pub const CARD_COLORS: &[&str] =
    &["red", "orange", "yellow", "green", "blue", "violet"];
pub const DEFAULT_CARD_COLOR: &str = "blue";

// Renderer-defense caps. Server-side paste/import validation
// enforces the same numbers in `crates/collab/src/blocks/kanban.rs`
// — keep in sync when either side changes. The CRDT interactive
// write path is NOT yet gated (Phase 2), so an authenticated
// co-author with write permission can plant oversized attrs;
// these caps keep such payloads from crashing / hanging the
// renderer for downstream viewers.
const MAX_COLUMN_TITLE_LEN: usize = 60;
const MAX_CARD_TITLE_LEN: usize = 120;
const MAX_CARD_CONTENT_LEN: usize = 500;
// Not yet enforced server-side (tracked as a Phase 2 gap):
const MAX_LABEL_COUNT: usize = 20;
const MAX_LABEL_NAME_LEN: usize = 40;
const MAX_ASSIGNEE_NAME_LEN: usize = 120;
const MAX_ASSIGNEE_ID_LEN: usize = 64;
const MAX_DATA_ATTR_LEN: usize = 4096;

/// Default columns seeded by the Insert command. Matches the
/// conventional "To Do / In Progress / Done" workflow so a fresh
/// board is immediately usable without configuration.
const DEFAULT_COLUMNS: &[&str] = &["To Do", "In Progress", "Done"];

impl LiveAppBlockView for KanbanView {
    fn node_types(&self) -> &'static [NodeType] {
        &[
            NodeType::Kanban,
            NodeType::KanbanColumn,
            NodeType::KanbanCard,
        ]
    }

    fn render(
        &self,
        doc: &Document,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
        content: &Fragment,
    ) -> Option<DomNode> {
        match node_type {
            NodeType::Kanban => render_board(doc, attrs, content),
            // KanbanColumn / KanbanCard are rendered as part of
            // the board (see `render_board`); the plugin fallback
            // never calls into them directly because they're
            // never top-level.
            _ => None,
        }
    }
}

impl LiveAppBlockInsert for KanbanInsert {
    fn id(&self) -> &'static str {
        "kanban"
    }
    fn label_key(&self) -> &'static str {
        "insert-kanban-label"
    }
    fn description_key(&self) -> &'static str {
        "insert-kanban-description"
    }
    fn icon(&self) -> &'static str {
        "\u{1F5C2}" // 🗂 (card index dividers — reads as "board").
        // Was 📋 in Phase 1; changed to avoid a visual clash with
        // the toolbar's Insert-Table button, which also uses 📋.
    }
    fn build_default_node(&self) -> Node {
        // Seed the board with the conventional three columns so
        // users have somewhere to drop cards immediately.
        let columns: Vec<Node> = DEFAULT_COLUMNS
            .iter()
            .map(|title| {
                let mut attrs = HashMap::new();
                attrs.insert("title".into(), (*title).into());
                Node::element_with_attrs(
                    NodeType::KanbanColumn,
                    attrs,
                    Fragment::empty(),
                )
            })
            .collect();
        Node::element_with_attrs(
            NodeType::Kanban,
            HashMap::new(),
            Fragment::from(columns),
        )
    }
}

fn render_board(
    doc: &Document,
    attrs: &HashMap<String, String>,
    content: &Fragment,
) -> Option<DomNode> {
    let wrapper = doc.create_element("div").ok()?;
    wrapper.set_attribute("class", "kanban-block").ok()?;
    wrapper.set_attribute("contenteditable", "false").ok()?;
    // See the identical data-atom-size comment in blocks/calendar.rs.
    // Without this, dom_to_model_walk descends into every
    // .kanban-column / .kanban-card / button and overcounts model
    // positions past the board. `insert_text failed` on every
    // keystroke after the block was the visible symptom.
    wrapper.set_attribute(
        "data-atom-size",
        &(content.size() + 2).to_string(),
    ).ok()?;
    if let Some(bid) = safe_block_id(attrs) {
        wrapper.set_attribute("data-block-id", &bid).ok()?;
    }

    // Column strip. Each column carries its own click surfaces
    // (title, add-card button, card list) but the pointer wiring
    // lives on the container so the click observer only needs one
    // listener per Kanban.
    let strip = doc.create_element("div").ok()?;
    strip.set_attribute("class", "kanban-strip").ok()?;

    for child in &content.children {
        if let Some(el) = render_column(doc, child) {
            strip.append_child(el.as_ref()).ok()?;
        }
    }

    // "+ Add column" tail button. The `+ ` prefix is presentation;
    // the label itself is localized.
    let add_col = doc.create_element("button").ok()?;
    add_col.set_attribute("type", "button").ok()?;
    add_col.set_attribute("class", "kanban-add-column").ok()?;
    add_col.set_attribute("data-kanban-action", "add-column").ok()?;
    add_col.set_text_content(Some(&format!(
        "+ {}",
        crate::i18n::translate("kanban-add-column", None),
    )));
    strip.append_child(&add_col).ok()?;

    wrapper.append_child(&strip).ok()?;
    Some(wrapper.into())
}

fn render_column(doc: &Document, node: &Node) -> Option<Element> {
    let Node::Element {
        node_type: NodeType::KanbanColumn,
        attrs,
        content,
        ..
    } = node
    else {
        return None;
    };
    let col = doc.create_element("div").ok()?;
    col.set_attribute("class", "kanban-column").ok()?;
    // Uses `data-block-id` (not `data-column-id`) so the shared
    // block-id observers (comment highlights, block-menu, etc.)
    // find the column consistently with every other editor node.
    // Also stamps `data-column-id` as an alias so kanban-specific
    // click observers can query by role without ambiguity when
    // the same element carries both roles.
    if let Some(bid) = safe_block_id(attrs) {
        col.set_attribute("data-block-id", &bid).ok()?;
        col.set_attribute("data-column-id", &bid).ok()?;
    }
    let title = safe_required_text(
        attrs,
        "title",
        MAX_COLUMN_TITLE_LEN,
        "(untitled)",
    );
    col.set_attribute("data-title", &title).ok()?;

    // Header: title + card count. Phase 4a — the header is the
    // drag handle for reordering the column within the Kanban
    // strip. Uses `data-kanban-draggable="column"` so the pointer
    // handler distinguishes it from card drags.
    let header = doc.create_element("div").ok()?;
    header.set_attribute("class", "kanban-column-header").ok()?;
    header.set_attribute("data-kanban-draggable", "column").ok()?;
    let title_el = doc.create_element("span").ok()?;
    title_el.set_attribute("class", "kanban-column-title").ok()?;
    title_el.set_attribute("data-kanban-action", "rename-column").ok()?;
    title_el.set_text_content(Some(&title));
    header.append_child(&title_el).ok()?;

    let count = doc.create_element("span").ok()?;
    count.set_attribute("class", "kanban-column-count").ok()?;
    // Phase 4a — clicking the count pill opens a prompt to set
    // the WIP limit. If already set, prompt pre-fills with the
    // current value; empty submission clears it.
    count.set_attribute("data-kanban-action", "set-wip-limit").ok()?;
    let wip_limit = safe_wip_limit(attrs);
    if let Some(limit) = wip_limit {
        count.set_attribute("data-wip-limit", &limit.to_string()).ok()?;
    }
    let card_count = content
        .children
        .iter()
        .filter(|c| {
            matches!(
                c,
                Node::Element {
                    node_type: NodeType::KanbanCard,
                    ..
                }
            )
        })
        .count();
    // "N" alone when no limit; "N/L" when a limit is set.
    let count_text = match wip_limit {
        Some(limit) => format!("{card_count}/{limit}"),
        None => card_count.to_string(),
    };
    count.set_text_content(Some(&count_text));
    if let Some(limit) = wip_limit {
        if card_count as u32 >= limit {
            count
                .set_attribute(
                    "class",
                    "kanban-column-count kanban-column-count--full",
                )
                .ok()?;
        }
    }
    header.append_child(&count).ok()?;

    // Column menu affordance (delete etc.). Phase 2 will wire a
    // popup; Phase 1 stamps the button so it's visible + hit-
    // testable.
    let menu = doc.create_element("button").ok()?;
    menu.set_attribute("type", "button").ok()?;
    menu.set_attribute("class", "kanban-column-menu").ok()?;
    menu.set_attribute("data-kanban-action", "remove-column").ok()?;
    menu.set_attribute(
        "aria-label",
        &crate::i18n::translate("kanban-delete-column", None),
    ).ok()?;
    menu.set_text_content(Some("\u{00D7}")); // ×
    header.append_child(&menu).ok()?;
    col.append_child(&header).ok()?;

    // Card list.
    let list = doc.create_element("div").ok()?;
    list.set_attribute("class", "kanban-card-list").ok()?;
    list.set_attribute("data-kanban-drop-column", "true").ok()?;
    for card in &content.children {
        if let Some(el) = render_card(doc, card) {
            list.append_child(el.as_ref()).ok()?;
        }
    }
    col.append_child(&list).ok()?;

    // "+ Add card" tail button.
    let add_card = doc.create_element("button").ok()?;
    add_card.set_attribute("type", "button").ok()?;
    add_card.set_attribute("class", "kanban-add-card").ok()?;
    add_card.set_attribute("data-kanban-action", "add-card").ok()?;
    add_card.set_text_content(Some(&format!(
        "+ {}",
        crate::i18n::translate("kanban-add-card", None),
    )));
    col.append_child(&add_card).ok()?;

    Some(col)
}

fn render_card(doc: &Document, node: &Node) -> Option<Element> {
    let Node::Element {
        node_type: NodeType::KanbanCard,
        attrs,
        ..
    } = node
    else {
        return None;
    };
    let color = attrs
        .get("color")
        .filter(|c| CARD_COLORS.contains(&c.as_str()))
        .map(String::as_str)
        .unwrap_or(DEFAULT_CARD_COLOR);
    let card = doc.create_element("div").ok()?;
    card.set_attribute(
        "class",
        &format!("kanban-card kanban-card--{color}"),
    )
    .ok()?;
    card.set_attribute("data-kanban-action", "edit-card").ok()?;
    // Cards are draggable — phase 3 wires the pointer pipeline.
    card.set_attribute("data-kanban-draggable", "card").ok()?;
    if let Some(bid) = safe_block_id(attrs) {
        // Same convention as every other addressable editor node
        // — see the equivalent comment in `render_column`.
        card.set_attribute("data-block-id", &bid).ok()?;
        card.set_attribute("data-card-id", &bid).ok()?;
    }
    // Sanitized reads once — reused by the modal-pre-fill data-*
    // stamps and the visible render below. Length caps prevent an
    // adversarial co-author from planting an oversized attr that
    // would DOM-blow up viewers with read-only permission on the
    // doc (server-side CRDT write validation is Phase 2 —
    // authenticated writers can still plant hostile bytes today).
    let card_title = safe_required_text(
        attrs,
        "title",
        MAX_CARD_TITLE_LEN,
        "(untitled)",
    );
    let card_content = safe_optional_text(attrs, "content", MAX_CARD_CONTENT_LEN);
    let due_at = safe_due_at(attrs);
    let labels_raw = safe_optional_text(attrs, "labels", MAX_DATA_ATTR_LEN);
    let assignee_id = safe_optional_text(attrs, "assigneeId", MAX_ASSIGNEE_ID_LEN);
    let assignee_name = safe_optional_text(attrs, "assigneeName", MAX_ASSIGNEE_NAME_LEN);

    // Stamp title + content on the DOM so the click observer's
    // Edit-mode branch can pre-fill the modal without a model
    // round-trip.
    card.set_attribute("data-title", &card_title).ok()?;
    if let Some(c) = &card_content {
        card.set_attribute("data-content", c).ok()?;
    }
    // Phase 4b/4c — mirror the modal-owned fields as data-*
    // stamps so the Edit modal's pre-fill doesn't need a model
    // round-trip. All are optional; empty attrs skip the stamp.
    if let Some(d) = &due_at {
        card.set_attribute("data-due-at", d).ok()?;
    }
    if let Some(l) = &labels_raw {
        card.set_attribute("data-labels", l).ok()?;
    }
    if let Some(a) = &assignee_id {
        card.set_attribute("data-assignee-id", a).ok()?;
    }
    if let Some(a) = &assignee_name {
        card.set_attribute("data-assignee-name", a).ok()?;
    }

    // Labels row (chips at the top of the card).
    if let Some(labels) = &labels_raw {
        let parsed = parse_labels(labels);
        if !parsed.is_empty() {
            let labels_row = doc.create_element("div").ok()?;
            labels_row.set_attribute("class", "kanban-card-labels").ok()?;
            for (name, color_hint) in &parsed {
                let chip = doc.create_element("span").ok()?;
                let class = match color_hint {
                    Some(c) => format!("kanban-card-label kanban-card-label--{c}"),
                    None => "kanban-card-label".to_string(),
                };
                chip.set_attribute("class", &class).ok()?;
                chip.set_text_content(Some(name));
                labels_row.append_child(&chip).ok()?;
            }
            card.append_child(&labels_row).ok()?;
        }
    }

    let title_span = doc.create_element("div").ok()?;
    title_span.set_attribute("class", "kanban-card-title").ok()?;
    title_span.set_text_content(Some(&card_title));
    card.append_child(&title_span).ok()?;
    if let Some(content) = &card_content {
        let body = doc.create_element("div").ok()?;
        body.set_attribute("class", "kanban-card-body").ok()?;
        body.set_text_content(Some(content));
        card.append_child(&body).ok()?;
    }

    // Footer: due-date pill only. Assignee chip moved to the
    // top-right corner (see below) so it reads as an
    // ownership marker independent of scheduling metadata.
    if let Some(due) = &due_at {
        let footer = doc.create_element("div").ok()?;
        footer.set_attribute("class", "kanban-card-footer").ok()?;
        let pill = doc.create_element("span").ok()?;
        let class = if is_overdue(due) {
            "kanban-card-due kanban-card-due--overdue"
        } else {
            "kanban-card-due"
        };
        pill.set_attribute("class", class).ok()?;
        pill.set_text_content(Some(due));
        footer.append_child(&pill).ok()?;
        card.append_child(&footer).ok()?;
    }

    // Assignee chip: top-right corner via absolute positioning
    // (CSS in main.css). Positioned as a direct child of the
    // card so `top/right` anchor to the card's own bounding box.
    if let Some(name) = &assignee_name {
        let chip = doc.create_element("span").ok()?;
        chip.set_attribute("class", "kanban-card-assignee").ok()?;
        // Two-letter initials for the avatar-chip look. Full
        // name kept in the title attr for accessibility.
        let initials: String = name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect::<String>()
            .to_uppercase();
        chip.set_text_content(Some(&initials));
        chip.set_attribute("title", name).ok()?;
        card.append_child(&chip).ok()?;
    }

    Some(card)
}

/// Parse the card's stringly `labels` attribute into
/// `(name, Option<color>)` pairs. Format is
/// `name|color;name|color;…`. Whitespace around `name` and
/// `color` is trimmed. Empty names, empty segments, and
/// non-palette colors are handled:
///   - empty segments (`bug|red;;ux|blue`) are skipped
///   - empty color (`bug|;ux|blue`) resolves to `None`
///   - name-only (`bug`) also resolves to `None`
///   - color not in the six-hue palette resolves to `None`
///     (chip renders in the neutral style)
///
/// Kept as a pure fn (no JS / DOM types) so the parse rules
/// can be unit-tested without a WASM harness.
pub(super) fn parse_labels(raw: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for pair in raw.split(';') {
        if out.len() >= MAX_LABEL_COUNT {
            break;
        }
        let mut parts = pair.splitn(2, '|');
        let name_raw = parts.next().unwrap_or("").trim();
        if name_raw.is_empty() { continue; }
        let name = clamp_chars(name_raw, MAX_LABEL_NAME_LEN);
        let color_hint = parts.next().unwrap_or("").trim();
        let color = if color_hint.is_empty() {
            None
        } else if CARD_COLORS.contains(&color_hint) {
            Some(color_hint.to_string())
        } else {
            None
        };
        out.push((name, color));
    }
    out
}

/// Truncate to `max` chars, keeping grapheme boundaries best-effort
/// via char-count (matches server-side `chars().take(N)` shape).
/// Adversarial multi-codepoint graphemes may still split — the goal
/// here is only to bound the size, not preserve legibility of
/// hostile input.
fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Read a required-text attr, apply length cap, fall back to
/// `placeholder` if absent or empty after trim.
fn safe_required_text(
    attrs: &HashMap<String, String>,
    key: &str,
    max: usize,
    placeholder: &str,
) -> String {
    match attrs.get(key).map(|s| s.trim()) {
        Some(v) if !v.is_empty() => clamp_chars(v, max),
        _ => placeholder.to_string(),
    }
}

/// Read an optional-text attr, apply length cap. Returns `None`
/// when absent or empty; the render skips its element in that case.
fn safe_optional_text(
    attrs: &HashMap<String, String>,
    key: &str,
    max: usize,
) -> Option<String> {
    attrs
        .get(key)
        .filter(|s| !s.is_empty())
        .map(|s| clamp_chars(s, max))
}

/// Column WIP limit: parse to u32, drop non-numeric / zero.
fn safe_wip_limit(attrs: &HashMap<String, String>) -> Option<u32> {
    attrs
        .get("wipLimit")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|l| *l > 0)
}

/// dueAt: YYYY-MM-DD — valid iff the (y,m,d) tuple parses. Returns
/// the canonical stored string (trimmed) if parseable, else None.
fn safe_due_at(attrs: &HashMap<String, String>) -> Option<String> {
    let raw = attrs.get("dueAt")?.trim();
    parse_ymd(raw).map(|_| raw.to_string())
}

/// blockId is stamped into data-* attrs. Not exploitable via
/// set_attribute, but a multi-megabyte id would still bloat the DOM.
fn safe_block_id(attrs: &HashMap<String, String>) -> Option<String> {
    safe_optional_text(attrs, "blockId", MAX_DATA_ATTR_LEN)
}

/// True when the YYYY-MM-DD `dueAt` string is a valid date and
/// strictly earlier than today (viewer's local time). Malformed
/// strings return false. The DOM-facing wrapper — reads `today`
/// from JS. Comparison logic lives in `is_date_before` so it
/// can be unit-tested in pure Rust.
fn is_overdue(due_at: &str) -> bool {
    let due = match parse_ymd(due_at) {
        Some(t) => t,
        None => return false,
    };
    let today = js_sys::Date::new_0();
    let now = (
        today.get_full_year(),
        today.get_month() + 1,
        today.get_date(),
    );
    is_date_before(due, now)
}

/// Parse a `YYYY-MM-DD` string into `(y, m, d)`. Returns `None`
/// on any parse failure or missing part.
pub(super) fn parse_ymd(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    Some((y, m, d))
}

/// True when date `a` is strictly earlier than date `b`.
/// Component-wise tuple comparison — deliberately does no
/// month/day-range validation, so callers should pass values
/// returned by `parse_ymd` (which allows any u32).
pub(super) fn is_date_before(a: (u32, u32, u32), b: (u32, u32, u32)) -> bool {
    a < b
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_labels ──

    #[test]
    fn parse_labels_basic_pair() {
        let out = parse_labels("bug|red;ux|blue");
        assert_eq!(out, vec![
            ("bug".to_string(), Some("red".to_string())),
            ("ux".to_string(), Some("blue".to_string())),
        ]);
    }

    #[test]
    fn parse_labels_skips_empty_segments() {
        let out = parse_labels("bug|red;;ux|blue;");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "bug");
        assert_eq!(out[1].0, "ux");
    }

    #[test]
    fn parse_labels_name_only_no_color() {
        let out = parse_labels("urgent");
        assert_eq!(out, vec![("urgent".to_string(), None)]);
    }

    #[test]
    fn parse_labels_unknown_color_treated_as_none() {
        let out = parse_labels("urgent|purple");
        assert_eq!(out, vec![("urgent".to_string(), None)]);
    }

    #[test]
    fn parse_labels_trims_whitespace() {
        let out = parse_labels(" bug | red ; ux |blue");
        assert_eq!(out, vec![
            ("bug".to_string(), Some("red".to_string())),
            ("ux".to_string(), Some("blue".to_string())),
        ]);
    }

    #[test]
    fn parse_labels_empty_input_returns_empty() {
        assert!(parse_labels("").is_empty());
    }

    #[test]
    fn parse_labels_only_delimiters_returns_empty() {
        assert!(parse_labels(";;;").is_empty());
    }

    #[test]
    fn parse_labels_pipe_but_no_name_is_skipped() {
        // "|red" — empty name before the pipe.
        assert!(parse_labels("|red").is_empty());
    }

    // ── parse_ymd + is_date_before ──

    #[test]
    fn parse_ymd_accepts_standard_format() {
        assert_eq!(parse_ymd("2026-07-04"), Some((2026, 7, 4)));
    }

    #[test]
    fn parse_ymd_rejects_missing_parts() {
        assert_eq!(parse_ymd("2026-07"), None);
        assert_eq!(parse_ymd("2026"), None);
        assert_eq!(parse_ymd(""), None);
    }

    #[test]
    fn parse_ymd_rejects_non_numeric() {
        assert_eq!(parse_ymd("2026-XX-04"), None);
        assert_eq!(parse_ymd("abc-def-ghi"), None);
    }

    #[test]
    fn is_date_before_year_ordering() {
        assert!(is_date_before((2025, 12, 31), (2026, 1, 1)));
        assert!(!is_date_before((2026, 1, 1), (2025, 12, 31)));
    }

    #[test]
    fn is_date_before_same_day_returns_false() {
        // Overdue is STRICT less-than — same-day is not overdue.
        assert!(!is_date_before((2026, 7, 4), (2026, 7, 4)));
    }

    #[test]
    fn is_date_before_month_and_day_ordering() {
        assert!(is_date_before((2026, 6, 30), (2026, 7, 1)));
        assert!(is_date_before((2026, 7, 3), (2026, 7, 4)));
        assert!(!is_date_before((2026, 7, 5), (2026, 7, 4)));
    }

    // ── renderer-defense caps ──

    fn attrs_with(key: &str, value: String) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(key.to_string(), value);
        m
    }

    #[test]
    fn safe_required_text_returns_placeholder_when_absent() {
        let m = HashMap::new();
        assert_eq!(
            safe_required_text(&m, "title", 10, "(none)"),
            "(none)"
        );
    }

    #[test]
    fn safe_required_text_returns_placeholder_when_blank() {
        let m = attrs_with("title", "   ".to_string());
        assert_eq!(
            safe_required_text(&m, "title", 10, "(none)"),
            "(none)"
        );
    }

    #[test]
    fn safe_required_text_clamps_to_max() {
        let long = "x".repeat(1_000);
        let m = attrs_with("title", long);
        assert_eq!(
            safe_required_text(&m, "title", MAX_CARD_TITLE_LEN, "(x)")
                .chars()
                .count(),
            MAX_CARD_TITLE_LEN
        );
    }

    #[test]
    fn safe_optional_text_none_when_empty() {
        let m = attrs_with("content", "".to_string());
        assert_eq!(safe_optional_text(&m, "content", 10), None);
    }

    #[test]
    fn safe_optional_text_clamps() {
        let m = attrs_with("content", "y".repeat(999));
        assert_eq!(
            safe_optional_text(&m, "content", MAX_CARD_CONTENT_LEN)
                .unwrap()
                .chars()
                .count(),
            MAX_CARD_CONTENT_LEN
        );
    }

    #[test]
    fn safe_wip_limit_rejects_zero_and_non_numeric() {
        assert_eq!(safe_wip_limit(&attrs_with("wipLimit", "0".into())), None);
        assert_eq!(safe_wip_limit(&attrs_with("wipLimit", "abc".into())), None);
        assert_eq!(safe_wip_limit(&attrs_with("wipLimit", "-3".into())), None);
        assert_eq!(safe_wip_limit(&attrs_with("wipLimit", "7".into())), Some(7));
    }

    #[test]
    fn safe_due_at_rejects_malformed() {
        assert_eq!(safe_due_at(&attrs_with("dueAt", "not-a-date".into())), None);
        assert_eq!(safe_due_at(&attrs_with("dueAt", "2026-07-04".into())), Some("2026-07-04".into()));
        // Trims incoming whitespace.
        assert_eq!(
            safe_due_at(&attrs_with("dueAt", "  2026-07-04  ".into())),
            Some("2026-07-04".into())
        );
    }

    #[test]
    fn parse_labels_caps_at_max_label_count() {
        // MAX_LABEL_COUNT + 5 candidates; only MAX_LABEL_COUNT kept.
        let src = (0..MAX_LABEL_COUNT + 5)
            .map(|i| format!("n{i}|red"))
            .collect::<Vec<_>>()
            .join(";");
        let out = parse_labels(&src);
        assert_eq!(out.len(), MAX_LABEL_COUNT);
    }

    #[test]
    fn parse_labels_truncates_name_at_max_len() {
        let long = "x".repeat(MAX_LABEL_NAME_LEN + 20);
        let out = parse_labels(&format!("{long}|red"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0.chars().count(), MAX_LABEL_NAME_LEN);
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            // Adversarial fuzz — for any attr value up to 10 KiB,
            // the safe_* accessors must not panic, must return a
            // string / value within its documented cap, and must
            // clean-trim placeholder fallbacks. Panics from utf8
            // char-boundary math are exactly what the accessor
            // exists to prevent.
            #[test]
            fn safe_required_text_never_panics(s in ".*", max in 1usize..500) {
                let m = attrs_with("title", s);
                let out = safe_required_text(&m, "title", max, "(x)");
                prop_assert!(out.chars().count() <= max);
            }

            #[test]
            fn safe_optional_text_bounded(s in ".*", max in 1usize..500) {
                let m = attrs_with("content", s.clone());
                let out = safe_optional_text(&m, "content", max);
                if let Some(v) = out {
                    prop_assert!(v.chars().count() <= max);
                } else {
                    prop_assert!(s.is_empty());
                }
            }

            #[test]
            fn parse_labels_never_exceeds_cap(s in ".*") {
                let out = parse_labels(&s);
                prop_assert!(out.len() <= MAX_LABEL_COUNT);
                for (name, _) in &out {
                    prop_assert!(name.chars().count() <= MAX_LABEL_NAME_LEN);
                }
            }
        }
    }
}
