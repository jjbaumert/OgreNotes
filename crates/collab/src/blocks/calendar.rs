// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #136 — Calendar live-app block.
//!
//! `NodeType::Calendar` is a container node holding zero or more
//! `NodeType::CalendarEvent` leaf children.
//!
//! ## Attribute schema
//!
//! ### `Calendar`
//! - `view` (`"month"` | `"week"` | `"day"`) — active view. Defaults
//!   to `"month"`.
//! - `cursor` — `YYYY-MM` when view is `month`; `YYYY-MM-DD` when
//!   view is `week` or `day`. Defaults to today in the block's TZ.
//! - `timezone` — IANA name (e.g. `America/Los_Angeles`). Defaults
//!   to `"UTC"` on insert; the client may seed from the user profile.
//!
//! ### `CalendarEvent`
//! - `color` — one of `red`/`orange`/`yellow`/`green`/`blue`/`violet`.
//! - `allDay` — `"true"` | `"false"`. Defaults to `"false"`.
//! - When `allDay="true"`:
//!   - `startDate` / `endDate` — `YYYY-MM-DD`.
//! - When `allDay="false"`:
//!   - `startAt` / `endAt` — RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
//! - `content` — short display string (rendered inside the day cell).

use std::collections::HashMap;

use super::{BlockValidationError, LiveAppBlock};
use crate::schema::NodeType;

pub struct CalendarBlock;
pub static CALENDAR: CalendarBlock = CalendarBlock;

pub const COLORS: &[&str] = &["red", "orange", "yellow", "green", "blue", "violet"];
pub const VIEWS: &[&str] = &["month", "week", "day"];
pub const DEFAULT_VIEW: &str = "month";
pub const DEFAULT_COLOR: &str = "blue";
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// Every attribute name the `Calendar` container node carries.
/// Single source of truth for callers that need to iterate attrs
/// (e.g. `export.rs::render_html_attrs` reading them out of the
/// yrs `XmlElementRef`) so adding a new attr means one edit here,
/// not two.
pub const CALENDAR_ATTR_NAMES: &[&str] = &["view", "cursor", "timezone"];

/// Every attribute name a `CalendarEvent` leaf carries. Same
/// single-source-of-truth guarantee as `CALENDAR_ATTR_NAMES`.
pub const EVENT_ATTR_NAMES: &[&str] = &[
    "color",
    "allDay",
    "startDate",
    "endDate",
    "startAt",
    "endAt",
    "content",
];

impl LiveAppBlock for CalendarBlock {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Calendar, NodeType::CalendarEvent]
    }

    fn validate_attrs(
        &self,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>, BlockValidationError> {
        match node_type {
            NodeType::Calendar => validate_calendar_attrs(attrs),
            NodeType::CalendarEvent => validate_event_attrs(attrs),
            other => Err(BlockValidationError {
                node_type: other,
                field: std::borrow::Cow::Borrowed("node_type"),
                reason: format!(
                    "CalendarBlock cannot validate {}",
                    other.tag_name()
                ),
            }),
        }
    }
}

fn validate_calendar_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    let mut out = HashMap::new();

    let view = attrs
        .get("view")
        .cloned()
        .unwrap_or_else(|| DEFAULT_VIEW.to_string());
    if !VIEWS.contains(&view.as_str()) {
        return Err(BlockValidationError {
            node_type: NodeType::Calendar,
            field: std::borrow::Cow::Borrowed("view"),
            reason: format!("expected one of {VIEWS:?}, got {view:?}"),
        });
    }
    out.insert("view".into(), view.clone());

    // Cursor shape depends on view — month = YYYY-MM, week/day = YYYY-MM-DD.
    // Empty cursor is allowed; the client fills in today at render time.
    if let Some(cursor) = attrs.get("cursor").filter(|s| !s.is_empty()) {
        let ok = if view == "month" {
            parse_ymd_prefix(cursor).is_some_and(|(y, m, d)| d.is_none() && (1..=12).contains(&m) && (1970..=9999).contains(&y))
        } else {
            parse_ymd_prefix(cursor).is_some_and(|(y, m, d)| d.is_some() && (1..=12).contains(&m) && (1970..=9999).contains(&y))
        };
        if !ok {
            return Err(BlockValidationError {
                node_type: NodeType::Calendar,
                field: std::borrow::Cow::Borrowed("cursor"),
                reason: format!(
                    "expected {} for view={view}, got {cursor:?}",
                    if view == "month" { "YYYY-MM" } else { "YYYY-MM-DD" }
                ),
            });
        }
        out.insert("cursor".into(), cursor.clone());
    }

    let timezone = attrs
        .get("timezone")
        .cloned()
        .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
    if !looks_like_iana_tz(&timezone) {
        return Err(BlockValidationError {
            node_type: NodeType::Calendar,
            field: std::borrow::Cow::Borrowed("timezone"),
            reason: format!("not an IANA tz name: {timezone:?}"),
        });
    }
    out.insert("timezone".into(), timezone);

    // Preserve blockId — it's the CRDT anchor; validators must not
    // strip it.
    if let Some(bid) = attrs.get("blockId") {
        out.insert("blockId".into(), bid.clone());
    }

    Ok(out)
}

fn validate_event_attrs(
    attrs: &HashMap<String, String>,
) -> Result<HashMap<String, String>, BlockValidationError> {
    let mut out = HashMap::new();

    let color = attrs
        .get("color")
        .cloned()
        .unwrap_or_else(|| DEFAULT_COLOR.to_string());
    if !COLORS.contains(&color.as_str()) {
        return Err(BlockValidationError {
            node_type: NodeType::CalendarEvent,
            field: std::borrow::Cow::Borrowed("color"),
            reason: format!("expected one of {COLORS:?}, got {color:?}"),
        });
    }
    out.insert("color".into(), color);

    let all_day_raw = attrs
        .get("allDay")
        .cloned()
        .unwrap_or_else(|| "false".into());
    let all_day = match all_day_raw.as_str() {
        "true" => true,
        "false" => false,
        _ => {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("allDay"),
                reason: format!("expected \"true\" or \"false\", got {all_day_raw:?}"),
            });
        }
    };
    out.insert("allDay".into(), all_day_raw);

    if all_day {
        let start = attrs.get("startDate").cloned().ok_or_else(|| {
            BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("startDate"),
                reason: "required when allDay=true".into(),
            }
        })?;
        let end = attrs.get("endDate").cloned().unwrap_or_else(|| start.clone());
        if !is_ymd(&start) {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("startDate"),
                reason: format!("expected YYYY-MM-DD, got {start:?}"),
            });
        }
        if !is_ymd(&end) {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("endDate"),
                reason: format!("expected YYYY-MM-DD, got {end:?}"),
            });
        }
        if end < start {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("endDate"),
                reason: format!("endDate {end:?} precedes startDate {start:?}"),
            });
        }
        out.insert("startDate".into(), start);
        out.insert("endDate".into(), end);
    } else {
        let start = attrs.get("startAt").cloned().ok_or_else(|| {
            BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("startAt"),
                reason: "required when allDay=false".into(),
            }
        })?;
        let end = attrs.get("endAt").cloned().unwrap_or_else(|| start.clone());
        if !is_rfc3339_utc(&start) {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("startAt"),
                reason: format!("expected RFC 3339 UTC, got {start:?}"),
            });
        }
        if !is_rfc3339_utc(&end) {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("endAt"),
                reason: format!("expected RFC 3339 UTC, got {end:?}"),
            });
        }
        if end < start {
            return Err(BlockValidationError {
                node_type: NodeType::CalendarEvent,
                field: std::borrow::Cow::Borrowed("endAt"),
                reason: format!("endAt {end:?} precedes startAt {start:?}"),
            });
        }
        out.insert("startAt".into(), start);
        out.insert("endAt".into(), end);
    }

    // Content is free-text; clamp to 200 chars so a pasted novel
    // doesn't collapse the layout. Empty content is allowed
    // (renders as a color chip).
    let content = attrs.get("content").cloned().unwrap_or_default();
    let content = if content.chars().count() > 200 {
        content.chars().take(200).collect()
    } else {
        content
    };
    out.insert("content".into(), content);

    if let Some(bid) = attrs.get("blockId") {
        out.insert("blockId".into(), bid.clone());
    }

    Ok(out)
}

/// HTML tag for a Calendar-owned NodeType. `resolve_html_tag` in
/// `export.rs` delegates here for the two variants.
pub fn html_tag(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::Calendar => "div",
        NodeType::CalendarEvent => "span",
        _ => "div",
    }
}

/// Extra HTML attributes for a Calendar-owned NodeType. `render_html_attrs`
/// in `export.rs` delegates here. Returns a pre-escaped attribute
/// string (leading space per existing convention).
pub fn html_attrs(node_type: NodeType, attrs: &HashMap<String, String>) -> String {
    let mut out = String::new();
    match node_type {
        NodeType::Calendar => {
            let view = attrs
                .get("view")
                .filter(|v| VIEWS.contains(&v.as_str()))
                .cloned()
                .unwrap_or_else(|| DEFAULT_VIEW.to_string());
            out.push_str(" class=\"calendar-block\"");
            out.push_str(&format!(" data-view=\"{}\"", escape_attr(&view)));
            if let Some(cursor) = attrs.get("cursor").filter(|s| !s.is_empty()) {
                out.push_str(&format!(" data-cursor=\"{}\"", escape_attr(cursor)));
            }
            let tz = attrs
                .get("timezone")
                .cloned()
                .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
            out.push_str(&format!(" data-timezone=\"{}\"", escape_attr(&tz)));
        }
        NodeType::CalendarEvent => {
            let color = attrs
                .get("color")
                .filter(|c| COLORS.contains(&c.as_str()))
                .cloned()
                .unwrap_or_else(|| DEFAULT_COLOR.to_string());
            out.push_str(&format!(
                " class=\"calendar-event calendar-event--{}\"",
                escape_attr(&color)
            ));
            let all_day = attrs.get("allDay").map(String::as_str) == Some("true");
            out.push_str(&format!(" data-all-day=\"{all_day}\""));
            if all_day {
                if let Some(s) = attrs.get("startDate") {
                    out.push_str(&format!(" data-start-date=\"{}\"", escape_attr(s)));
                }
                if let Some(e) = attrs.get("endDate") {
                    out.push_str(&format!(" data-end-date=\"{}\"", escape_attr(e)));
                }
            } else {
                if let Some(s) = attrs.get("startAt") {
                    out.push_str(&format!(" data-start-at=\"{}\"", escape_attr(s)));
                }
                if let Some(e) = attrs.get("endAt") {
                    out.push_str(&format!(" data-end-at=\"{}\"", escape_attr(e)));
                }
            }
        }
        _ => {}
    }
    out
}

/// Markdown placeholder for a Calendar or CalendarEvent. Markdown
/// can't carry a rendered grid; export emits a labelled
/// placeholder so the block's presence is preserved on
/// round-trip and the user knows content was elided.
pub fn markdown_placeholder(node_type: NodeType, attrs: &HashMap<String, String>) -> String {
    match node_type {
        NodeType::Calendar => {
            let view = attrs
                .get("view")
                .cloned()
                .unwrap_or_else(|| DEFAULT_VIEW.to_string());
            let cursor = attrs.get("cursor").cloned().unwrap_or_default();
            let tz = attrs
                .get("timezone")
                .cloned()
                .unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
            format!("[Calendar view={view} cursor={cursor} tz={tz}]\n\n")
        }
        NodeType::CalendarEvent => {
            let content = attrs.get("content").cloned().unwrap_or_default();
            let color = attrs
                .get("color")
                .cloned()
                .unwrap_or_else(|| DEFAULT_COLOR.to_string());
            let when = if attrs.get("allDay").map(String::as_str) == Some("true") {
                let start = attrs.get("startDate").cloned().unwrap_or_default();
                let end = attrs.get("endDate").cloned().unwrap_or_default();
                if start == end {
                    start
                } else {
                    format!("{start}..{end}")
                }
            } else {
                let start = attrs.get("startAt").cloned().unwrap_or_default();
                let end = attrs.get("endAt").cloned().unwrap_or_default();
                if start == end {
                    start
                } else {
                    format!("{start}..{end}")
                }
            };
            format!("- ({color}) {when} — {content}\n")
        }
        _ => String::new(),
    }
}

// ─── helpers ───────────────────────────────────────────────────

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

fn is_ymd(s: &str) -> bool {
    // Reject impossible calendar dates like Feb 31 / Apr 31; the
    // day-of-month range depends on the month (and, for Feb, on
    // whether the year is a leap year).
    parse_ymd_prefix(s).is_some_and(|(y, m, d)| {
        let Some(d) = d else { return false };
        (1..=12).contains(&m) && (1..=days_in_month(y, m)).contains(&d)
    })
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn is_rfc3339_utc(s: &str) -> bool {
    // Compact RFC 3339 shape check: `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
    // Not a full parser — rejects the obviously malformed while
    // trusting the client on the details. Storage is CRDT-string,
    // so a bad timestamp corrupts only the one event.
    if s.len() < 20 {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes[10] != b'T' || *bytes.last().unwrap() != b'Z' {
        return false;
    }
    let (date, rest) = s.split_at(10);
    if !is_ymd(date) {
        return false;
    }
    let time = &rest[1..rest.len() - 1];
    let mut parts = time.splitn(3, ':');
    let (Some(h), Some(m), Some(s2)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let h_ok = h.len() == 2 && h.chars().all(|c| c.is_ascii_digit());
    let m_ok = m.len() == 2 && m.chars().all(|c| c.is_ascii_digit());
    // s2 is `SS` or `SS.fff` — split on '.' and check the seconds
    // prefix AND (when present) the fractional-seconds suffix are
    // both numeric. Without the suffix check a value like
    // `"2026-07-15T14:30:45.abcZ"` slipped past validation.
    let mut sec_parts = s2.splitn(2, '.');
    let sec_prefix = sec_parts.next().unwrap_or("");
    let s_ok = sec_prefix.len() == 2 && sec_prefix.chars().all(|c| c.is_ascii_digit());
    let frac_ok = match sec_parts.next() {
        None => true,
        Some(frac) => !frac.is_empty() && frac.chars().all(|c| c.is_ascii_digit()),
    };
    h_ok && m_ok && s_ok && frac_ok
}

/// Parse a `YYYY-MM` or `YYYY-MM-DD` prefix. Returns (y, m, Some(d))
/// for the longer form. None on malformed input.
fn parse_ymd_prefix(s: &str) -> Option<(u32, u32, Option<u32>)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d = match parts.next() {
        Some(d_str) => Some(d_str.parse::<u32>().ok()?),
        None => None,
    };
    Some((y, m, d))
}

fn looks_like_iana_tz(s: &str) -> bool {
    // IANA tz names are ASCII, optionally with slashes and
    // alphanumeric/`_`/`-`/`+` segments (e.g. Etc/GMT+3, America/
    // Argentina/Buenos_Aires, UTC). Reject anything else. This is
    // a shape check, not a live validation — full IANA lookup
    // requires a tzdata dep we don't want in the collab crate.
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+'))
        && !s.starts_with('/')
        && !s.ends_with('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }

    #[test]
    fn calendar_defaults_apply_on_empty_input() {
        let out = validate_calendar_attrs(&HashMap::new()).unwrap();
        assert_eq!(out.get("view").map(String::as_str), Some("month"));
        assert_eq!(out.get("timezone").map(String::as_str), Some("UTC"));
        assert!(out.get("cursor").is_none(), "cursor is optional");
    }

    #[test]
    fn calendar_rejects_unknown_view() {
        let a = attrs(&[("view", "gantt")]);
        assert!(validate_calendar_attrs(&a).is_err());
    }

    #[test]
    fn calendar_cursor_shape_matches_view() {
        // Month cursor must be YYYY-MM.
        assert!(
            validate_calendar_attrs(&attrs(&[("view", "month"), ("cursor", "2026-07")])).is_ok()
        );
        assert!(
            validate_calendar_attrs(&attrs(&[("view", "month"), ("cursor", "2026-07-01")]))
                .is_err()
        );
        // Week/day cursor must be YYYY-MM-DD.
        assert!(
            validate_calendar_attrs(&attrs(&[("view", "week"), ("cursor", "2026-07-01")]))
                .is_ok()
        );
        assert!(
            validate_calendar_attrs(&attrs(&[("view", "week"), ("cursor", "2026-07")])).is_err()
        );
    }

    #[test]
    fn calendar_rejects_bad_timezone() {
        let a = attrs(&[("timezone", "not a tz!")]);
        assert!(validate_calendar_attrs(&a).is_err());
    }

    #[test]
    fn calendar_accepts_slash_iana_tz() {
        let a = attrs(&[("timezone", "America/Los_Angeles")]);
        assert!(validate_calendar_attrs(&a).is_ok());
    }

    #[test]
    fn event_defaults_all_day_false_but_requires_start_at() {
        let out = validate_event_attrs(&HashMap::new());
        assert!(out.is_err(), "startAt is required when allDay=false");
    }

    #[test]
    fn event_all_day_requires_start_date() {
        let a = attrs(&[("allDay", "true")]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_all_day_defaults_end_to_start_when_absent() {
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
            ("color", "green"),
            ("content", "Team offsite"),
        ]);
        let out = validate_event_attrs(&a).unwrap();
        assert_eq!(out.get("endDate").map(String::as_str), Some("2026-07-15"));
    }

    #[test]
    fn event_timed_defaults_end_to_start_when_absent() {
        let a = attrs(&[
            ("allDay", "false"),
            ("startAt", "2026-07-15T14:30:00Z"),
        ]);
        let out = validate_event_attrs(&a).unwrap();
        assert_eq!(
            out.get("endAt").map(String::as_str),
            Some("2026-07-15T14:30:00Z")
        );
    }

    #[test]
    fn event_rejects_bad_color() {
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
            ("color", "puce"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_rejects_end_before_start() {
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
            ("endDate", "2026-07-14"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_clamps_long_content() {
        let long = "x".repeat(500);
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
            ("content", long.as_str()),
        ]);
        let out = validate_event_attrs(&a).unwrap();
        assert_eq!(out.get("content").unwrap().chars().count(), 200);
    }

    #[test]
    fn event_accepts_rfc3339_with_fractional_seconds() {
        let a = attrs(&[
            ("allDay", "false"),
            ("startAt", "2026-07-15T14:30:45.123Z"),
        ]);
        assert!(validate_event_attrs(&a).is_ok());
    }

    #[test]
    fn event_rejects_impossible_calendar_dates() {
        // Regression: is_ymd used to accept any day 1..=31 regardless
        // of month. Feb 31 / Apr 31 / Feb 30 must all be rejected.
        for bad in ["2026-02-30", "2026-02-31", "2026-04-31", "2026-06-31", "2026-11-31"] {
            let a = attrs(&[
                ("allDay", "true"),
                ("startDate", bad),
            ]);
            assert!(
                validate_event_attrs(&a).is_err(),
                "validator must reject {bad}"
            );
        }
    }

    #[test]
    fn event_rejects_feb_29_in_non_leap_year() {
        // 2025 is NOT a leap year.
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2025-02-29"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_accepts_feb_29_in_leap_year() {
        // 2024 IS a leap year.
        let a = attrs(&[
            ("allDay", "true"),
            ("startDate", "2024-02-29"),
        ]);
        assert!(validate_event_attrs(&a).is_ok());
    }

    #[test]
    fn event_rejects_impossible_date_in_rfc3339() {
        // is_rfc3339_utc calls is_ymd on the date prefix — the
        // tightening must flow through to timed events too.
        let a = attrs(&[
            ("allDay", "false"),
            ("startAt", "2026-02-31T14:30:00Z"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_rejects_non_numeric_fractional_seconds() {
        // Regression: before the tightening, `.abc` slipped past
        // because we only checked the `SS` prefix.
        let a = attrs(&[
            ("allDay", "false"),
            ("startAt", "2026-07-15T14:30:45.abcZ"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn event_rejects_empty_fractional_seconds() {
        // A trailing dot with no digits after it should also fail.
        let a = attrs(&[
            ("allDay", "false"),
            ("startAt", "2026-07-15T14:30:45.Z"),
        ]);
        assert!(validate_event_attrs(&a).is_err());
    }

    #[test]
    fn html_tag_returns_expected_shapes() {
        assert_eq!(html_tag(NodeType::Calendar), "div");
        assert_eq!(html_tag(NodeType::CalendarEvent), "span");
    }

    #[test]
    fn html_attrs_calendar_carries_view_and_tz() {
        let a = attrs(&[
            ("view", "week"),
            ("cursor", "2026-07-15"),
            ("timezone", "America/Los_Angeles"),
        ]);
        let out = html_attrs(NodeType::Calendar, &a);
        assert!(out.contains("data-view=\"week\""));
        assert!(out.contains("data-cursor=\"2026-07-15\""));
        assert!(out.contains("data-timezone=\"America/Los_Angeles\""));
        assert!(out.contains("class=\"calendar-block\""));
    }

    #[test]
    fn html_attrs_event_carries_color_class_and_dates() {
        let a = attrs(&[
            ("color", "green"),
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
            ("endDate", "2026-07-16"),
        ]);
        let out = html_attrs(NodeType::CalendarEvent, &a);
        assert!(out.contains("calendar-event--green"));
        assert!(out.contains("data-all-day=\"true\""));
        assert!(out.contains("data-start-date=\"2026-07-15\""));
        assert!(out.contains("data-end-date=\"2026-07-16\""));
    }

    #[test]
    fn html_attrs_event_falls_back_to_default_color_when_unknown() {
        let a = attrs(&[
            ("color", "chartreuse"),
            ("allDay", "true"),
            ("startDate", "2026-07-15"),
        ]);
        let out = html_attrs(NodeType::CalendarEvent, &a);
        assert!(out.contains("calendar-event--blue"), "got: {out}");
    }
}
