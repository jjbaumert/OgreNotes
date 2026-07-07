// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #136 — frontend Calendar live-app block.
//!
//! Ships a month-grid renderer + an insert entry that drops a
//! Calendar with today's month + empty event list. Interaction
//! surface (add / edit / drag events) is provided by
//! `crate::components::calendar_controls`, which observes the
//! selection on Calendar atoms and manages the modal state.
//!
//! v1 scope-down (see design/live-app-blocks.md):
//! - Month grid only (week/day views scaffolded; render as month
//!   with a "view coming soon" banner).
//! - Click-to-add and event modal are wired through a global
//!   window bridge (`window.__ogreCalendar.*`) that Leptos
//!   observes; see `components/calendar_controls.rs`.
//! - Drag-to-move / drag-to-resize deferred; users can delete
//!   events via backspace on the selected block or re-insert.

use std::collections::HashMap;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, Node as DomNode};

use super::super::model::{Fragment, Node, NodeType};
use super::{LiveAppBlockInsert, LiveAppBlockView};

// tz-aware date-parts extraction reuses the shared
// `crate::i18n::IntlDateTimeFormat` binding — DON'T declare a
// second extern here. Two extern types targeting the same JS
// class (`Intl.DateTimeFormat`) trigger a
// `symbol multiply defined` LTO error at release-build time.
use crate::i18n::IntlDateTimeFormat;

pub struct CalendarView;
pub struct CalendarInsert;

pub const COLORS: &[&str] = &["red", "orange", "yellow", "green", "blue", "violet"];
pub const DEFAULT_COLOR: &str = "blue";
pub const DEFAULT_VIEW: &str = "month";
pub const DEFAULT_TIMEZONE: &str = "UTC";

// Renderer-defense caps. See `blocks/kanban.rs` for the full
// rationale — server-side interactive CRDT writes are not yet
// gated (Phase 2), so an authenticated co-author can still plant
// oversized / adversarial attrs. These caps keep such payloads
// from crashing the renderer for viewers.
//
// IANA timezone names are ~≤32 chars in practice (e.g.
// "America/Argentina/Buenos_Aires" = 30). 64 gives headroom
// without permitting megabyte payloads.
const MAX_TIMEZONE_LEN: usize = 64;
// Matches `crates/collab/src/blocks/calendar.rs`'s paste/import
// clamp on CalendarEvent.content — keep in sync.
const MAX_EVENT_CONTENT_LEN: usize = 200;
const MAX_ISO_DATE_LEN: usize = 40; // YYYY-MM-DDTHH:MM:SS.sssZ + wiggle
const MAX_CURSOR_LEN: usize = 10;   // YYYY-MM-DD
const MAX_DATA_ATTR_LEN: usize = 4096;

impl LiveAppBlockView for CalendarView {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Calendar, NodeType::CalendarEvent]
    }

    fn render(
        &self,
        doc: &Document,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
        content: &Fragment,
    ) -> Option<DomNode> {
        match node_type {
            NodeType::Calendar => render_calendar(doc, attrs, content),
            NodeType::CalendarEvent => render_event_span(doc, attrs),
            _ => None,
        }
    }
}

impl LiveAppBlockInsert for CalendarInsert {
    fn id(&self) -> &'static str {
        "calendar"
    }
    fn label_key(&self) -> &'static str {
        "insert-calendar-label"
    }
    fn description_key(&self) -> &'static str {
        "insert-calendar-description"
    }
    fn icon(&self) -> &'static str {
        "\u{1F4C5}" // 📅
    }
    fn build_default_node(&self) -> Node {
        // Seed the block with today's month as the cursor and the
        // browser's IANA timezone as the display TZ. Storing the
        // author's tz here (rather than hard-coding UTC) means a
        // peer viewing the block later at least sees what tz the
        // block was authored in — v2 will convert display times
        // into each viewer's own tz.
        let (y, m, _) = today_ymd();
        let tz = browser_timezone().unwrap_or_else(|| DEFAULT_TIMEZONE.to_string());
        let mut attrs = HashMap::new();
        attrs.insert("view".into(), DEFAULT_VIEW.into());
        attrs.insert("cursor".into(), format!("{y:04}-{m:02}"));
        attrs.insert("timezone".into(), tz);
        Node::element_with_attrs(NodeType::Calendar, attrs, Fragment::empty())
    }
}

/// Read the browser's IANA timezone name via
/// `Intl.DateTimeFormat().resolvedOptions().timeZone`. Returns
/// `None` if the API is unavailable or returns a non-string.
fn browser_timezone() -> Option<String> {
    let intl = js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("Intl"),
    )
    .ok()?;
    let ctor = js_sys::Reflect::get(&intl, &wasm_bindgen::JsValue::from_str("DateTimeFormat"))
        .ok()?;
    let ctor: js_sys::Function = ctor.dyn_into().ok()?;
    let fmt = js_sys::Reflect::construct(&ctor, &js_sys::Array::new()).ok()?;
    let opts_fn = js_sys::Reflect::get(&fmt, &wasm_bindgen::JsValue::from_str("resolvedOptions"))
        .ok()?;
    let opts_fn: js_sys::Function = opts_fn.dyn_into().ok()?;
    let opts = opts_fn.call0(&fmt).ok()?;
    let tz = js_sys::Reflect::get(&opts, &wasm_bindgen::JsValue::from_str("timeZone")).ok()?;
    tz.as_string()
}

// ─── Renderer-defense accessors ─────────────────────────────────

/// Truncate to `max` chars (matches server-side `chars().take(N)`).
fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// `view` attribute: whitelist to {month, week, day}. Falls back
/// to DEFAULT_VIEW.
pub(super) fn safe_view(attrs: &HashMap<String, String>) -> &'static str {
    match attrs.get("view").map(String::as_str) {
        Some("month") => "month",
        Some("week") => "week",
        Some("day") => "day",
        _ => DEFAULT_VIEW,
    }
}

/// `color` attribute: whitelist to the six-hue palette. Falls back
/// to DEFAULT_COLOR.
pub(super) fn safe_color(attrs: &HashMap<String, String>) -> &'static str {
    match attrs
        .get("color")
        .map(String::as_str)
        .filter(|c| COLORS.contains(c))
    {
        Some("red") => "red",
        Some("orange") => "orange",
        Some("yellow") => "yellow",
        Some("green") => "green",
        Some("blue") => "blue",
        Some("violet") => "violet",
        _ => DEFAULT_COLOR,
    }
}

/// `timezone` attribute: character whitelist + length cap. Bad
/// tz strings fall back to UTC. The semantic check happens later
/// when the string is fed to Intl.DateTimeFormat — if that throws
/// for a well-shaped-but-unknown IANA name, the individual date
/// format call's `.ok()?` handles it. This gate is about
/// preventing the constructor from ever seeing a
/// multi-megabyte / control-character payload.
pub(super) fn safe_timezone(attrs: &HashMap<String, String>) -> String {
    let raw = attrs
        .get("timezone")
        .map(String::as_str)
        .unwrap_or(DEFAULT_TIMEZONE);
    if raw.len() > MAX_TIMEZONE_LEN {
        return DEFAULT_TIMEZONE.to_string();
    }
    let looks_ok = !raw.is_empty()
        && raw.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '+' | '-')
        });
    if looks_ok {
        raw.to_string()
    } else {
        DEFAULT_TIMEZONE.to_string()
    }
}

/// `cursor` attribute: length cap; must be either YYYY-MM or
/// YYYY-MM-DD. Returns empty string if malformed — callers
/// already handle that by falling back to today.
pub(super) fn safe_cursor(attrs: &HashMap<String, String>) -> String {
    let raw = attrs.get("cursor").map(String::as_str).unwrap_or("");
    if raw.is_empty() || raw.len() > MAX_CURSOR_LEN {
        return String::new();
    }
    // "YYYY-MM" (7) or "YYYY-MM-DD" (10). Loose char-set check.
    let ok = raw.chars().all(|c| c.is_ascii_digit() || c == '-');
    if ok { raw.to_string() } else { String::new() }
}

/// Optional text with length cap; None when absent or empty.
pub(super) fn safe_optional_text(
    attrs: &HashMap<String, String>,
    key: &str,
    max: usize,
) -> Option<String> {
    attrs
        .get(key)
        .filter(|s| !s.is_empty())
        .map(|s| clamp_chars(s, max))
}

/// blockId with a length cap. Not exploitable via set_attribute,
/// but a megabyte id still bloats the DOM.
pub(super) fn safe_block_id(attrs: &HashMap<String, String>) -> Option<String> {
    safe_optional_text(attrs, "blockId", MAX_DATA_ATTR_LEN)
}

// ─── Rendering ──────────────────────────────────────────────────

fn render_calendar(
    doc: &Document,
    attrs: &HashMap<String, String>,
    content: &Fragment,
) -> Option<DomNode> {
    let wrapper = doc.create_element("div").ok()?;
    wrapper.set_attribute("class", "calendar-block").ok()?;
    wrapper.set_attribute("contenteditable", "false").ok()?;
    // Tell dom_to_model_walk to treat this wrapper as a single atom
    // with model size 2 + sum(children.node_size()). Without this,
    // the walk recurses into the toolbar/month-grid divs and
    // wildly overcounts positions past the calendar — every click
    // after a Calendar returned a bogus model position and every
    // keystroke failed with `insert_text failed`. The +2 accounts
    // for the atom's own open/close boundaries.
    wrapper.set_attribute(
        "data-atom-size",
        &(content.size() + 2).to_string(),
    ).ok()?;
    // The block-id is what the yrs-bridge uses to align the model
    // against CRDT state — carry it through so scroll-in-view and
    // selection helpers can still find the node.
    if let Some(bid) = safe_block_id(attrs) {
        wrapper.set_attribute("data-block-id", &bid).ok()?;
    }

    let view = safe_view(attrs);
    wrapper.set_attribute("data-view", view).ok()?;

    let cursor = safe_cursor(attrs);
    if !cursor.is_empty() {
        wrapper.set_attribute("data-cursor", &cursor).ok()?;
    }
    let tz = safe_timezone(attrs);
    wrapper.set_attribute("data-timezone", &tz).ok()?;

    // Toolbar (view toggle + prev/next).
    let toolbar = render_toolbar(doc, &cursor, view)?;
    wrapper.append_child(toolbar.as_ref()).ok()?;

    let body = doc.create_element("div").ok()?;
    body.set_attribute("class", "calendar-body").ok()?;
    let grid = match view {
        "week" => render_week_grid(doc, &cursor, content, &tz)?,
        "day" => render_day_grid(doc, &cursor, content, &tz)?,
        _ => render_month_grid(doc, &cursor, content, &tz)?,
    };
    body.append_child(grid.as_ref()).ok()?;
    wrapper.append_child(&body).ok()?;

    Some(wrapper.into())
}

fn render_toolbar(doc: &Document, cursor: &str, view: &str) -> Option<Element> {
    let toolbar = doc.create_element("div").ok()?;
    toolbar.set_attribute("class", "calendar-toolbar").ok()?;

    let title = doc.create_element("span").ok()?;
    title.set_attribute("class", "calendar-title").ok()?;
    let (y, m) = parse_month_cursor(cursor).unwrap_or_else(|| {
        let (y, m, _) = today_ymd();
        (y, m)
    });
    title.set_text_content(Some(&localized_month_year(y, m)));
    toolbar.append_child(&title).ok()?;

    let toggles = doc.create_element("span").ok()?;
    toggles.set_attribute("class", "calendar-view-toggles").ok()?;
    // Localized labels for the three view-toggle buttons. The
    // `data-calendar-view` attribute keeps the machine value
    // (`month`/`week`/`day`) so the click observer's dispatch
    // doesn't have to reverse-map a translation.
    for (v, key) in [
        ("month", "calendar-view-month"),
        ("week",  "calendar-view-week"),
        ("day",   "calendar-view-day"),
    ] {
        let btn = doc.create_element("button").ok()?;
        btn.set_attribute("type", "button").ok()?;
        btn.set_attribute(
            "class",
            if v == view {
                "calendar-view-toggle calendar-view-toggle--active"
            } else {
                "calendar-view-toggle"
            },
        )
        .ok()?;
        btn.set_attribute("data-calendar-action", "set-view").ok()?;
        btn.set_attribute("data-calendar-view", v).ok()?;
        btn.set_text_content(Some(&crate::i18n::translate(key, None)));
        toggles.append_child(&btn).ok()?;
    }
    toolbar.append_child(&toggles).ok()?;

    let nav = doc.create_element("span").ok()?;
    nav.set_attribute("class", "calendar-nav").ok()?;
    // Prev/next stay as their glyphs (‹ ›) — universal enough,
    // no translation key. The "Today" middle button uses the
    // existing calendar-nav-today Fluent key.
    let today_label = crate::i18n::translate("calendar-nav-today", None);
    for (action, label) in [
        ("prev",  "\u{2039}"),
        ("today", today_label.as_str()),
        ("next",  "\u{203A}"),
    ] {
        let btn = doc.create_element("button").ok()?;
        btn.set_attribute("type", "button").ok()?;
        btn.set_attribute("class", "calendar-nav-btn").ok()?;
        btn.set_attribute("data-calendar-action", action).ok()?;
        btn.set_text_content(Some(label));
        nav.append_child(&btn).ok()?;
    }
    toolbar.append_child(&nav).ok()?;

    Some(toolbar)
}

fn render_month_grid(
    doc: &Document,
    cursor: &str,
    content: &Fragment,
    tz: &str,
) -> Option<Element> {
    let (year, month) = parse_month_cursor(cursor).unwrap_or_else(|| {
        let (y, m, _) = today_in_tz(tz);
        (y, m)
    });
    let table = doc.create_element("table").ok()?;
    table.set_attribute("class", "calendar-month-grid").ok()?;

    // Weekday header row.
    let thead = doc.create_element("thead").ok()?;
    let tr = doc.create_element("tr").ok()?;
    for dow in 0u32..7 {
        let day_name = localized_short_weekday(dow);
        let day_name = day_name.as_str();
        let th = doc.create_element("th").ok()?;
        th.set_text_content(Some(day_name));
        tr.append_child(&th).ok()?;
    }
    thead.append_child(&tr).ok()?;
    table.append_child(&thead).ok()?;

    // Bucket events by YYYY-MM-DD (in the block's declared tz).
    // Multi-day events populate every day they overlap.
    let events_by_day = bucket_events_by_day(content, tz);

    // Body — 6 rows × 7 cols, starting from the Monday on/before day 1.
    let tbody = doc.create_element("tbody").ok()?;
    let (start_year, start_month, start_day) =
        first_grid_cell(year, month);
    let today = today_in_tz(tz);
    let mut y = start_year;
    let mut m = start_month;
    let mut d = start_day;
    for _ in 0..6 {
        let row = doc.create_element("tr").ok()?;
        for _ in 0..7 {
            let td = doc.create_element("td").ok()?;
            let is_current_month = m == month;
            let is_today = (y, m, d) == today;
            let mut classes = vec!["calendar-day"];
            if !is_current_month {
                classes.push("calendar-day--other-month");
            }
            if is_today {
                classes.push("calendar-day--today");
            }
            td.set_attribute("class", &classes.join(" ")).ok()?;
            td.set_attribute("data-calendar-action", "add-event").ok()?;
            let iso = format!("{y:04}-{m:02}-{d:02}");
            td.set_attribute("data-calendar-date", &iso).ok()?;

            let day_num = doc.create_element("span").ok()?;
            day_num.set_attribute("class", "calendar-day-num").ok()?;
            day_num.set_text_content(Some(&d.to_string()));
            td.append_child(&day_num).ok()?;

            if let Some(evs) = events_by_day.get(&iso) {
                for ev in evs {
                    if let Some(span) = render_event_span(doc, &ev.attrs) {
                        td.append_child(&span).ok()?;
                    }
                }
            }
            row.append_child(&td).ok()?;
            let next = next_day(y, m, d);
            y = next.0;
            m = next.1;
            d = next.2;
        }
        tbody.append_child(&row).ok()?;
    }
    table.append_child(&tbody).ok()?;
    Some(table)
}

// ─── Week / Day time-grid renderers ─────────────────────────────
//
// Both views share the same time-axis + hourly-row shape; the only
// difference is the number of day columns (7 vs 1). Timed events
// float over their day column, positioned by start/end wall-clock
// time (browser-local). All-day events sit in a horizontal band at
// the top of the grid so they don't collide with the timed area.

const HOUR_HEIGHT_PX: u32 = 40;
const HOURS_IN_DAY: u32 = 24;
const MIN_EVENT_HEIGHT_PX: u32 = 20;

fn render_week_grid(
    doc: &Document,
    cursor: &str,
    content: &Fragment,
    tz: &str,
) -> Option<Element> {
    let anchor = parse_ymd(cursor).unwrap_or_else(|| today_in_tz(tz));
    let start = week_start_of(anchor);
    let mut days = Vec::with_capacity(7);
    let mut cursor_day = start;
    for _ in 0..7 {
        days.push(cursor_day);
        cursor_day = next_day(cursor_day.0, cursor_day.1, cursor_day.2);
    }
    render_time_grid(doc, &days, content, "week", tz)
}

fn render_day_grid(
    doc: &Document,
    cursor: &str,
    content: &Fragment,
    tz: &str,
) -> Option<Element> {
    let day = parse_ymd(cursor).unwrap_or_else(|| today_in_tz(tz));
    render_time_grid(doc, &[day], content, "day", tz)
}

fn render_time_grid(
    doc: &Document,
    days: &[(u32, u32, u32)],
    content: &Fragment,
    view: &str,
    tz: &str,
) -> Option<Element> {
    let root = doc.create_element("div").ok()?;
    root.set_attribute(
        "class",
        &format!("calendar-time-grid calendar-time-grid--{view}"),
    )
    .ok()?;

    // Bucket events once so header + all-day + body all read from
    // the same map. Both all-day and timed events populate every
    // day in their range so the render loops can pull per-day
    // slices without redoing the walk.
    let events_by_day = bucket_events_by_day(content, tz);
    let today = today_in_tz(tz);

    // Header row: empty cell over time axis, then one header per
    // day with name + date.
    let header = doc.create_element("div").ok()?;
    header.set_attribute("class", "calendar-week-headers").ok()?;
    let corner = doc.create_element("div").ok()?;
    corner.set_attribute("class", "calendar-time-axis-corner").ok()?;
    header.append_child(&corner).ok()?;
    for &(y, m, d) in days {
        let cell = doc.create_element("div").ok()?;
        let is_today = (y, m, d) == today;
        let mut classes = vec!["calendar-week-day-header"];
        if is_today {
            classes.push("calendar-week-day-header--today");
        }
        cell.set_attribute("class", &classes.join(" ")).ok()?;
        let name = doc.create_element("span").ok()?;
        name.set_attribute("class", "day-name").ok()?;
        name.set_text_content(Some(&localized_short_weekday(day_of_week(y, m, d))));
        let num = doc.create_element("span").ok()?;
        num.set_attribute("class", "day-num").ok()?;
        num.set_text_content(Some(&d.to_string()));
        cell.append_child(&name).ok()?;
        cell.append_child(&num).ok()?;
        header.append_child(&cell).ok()?;
    }
    root.append_child(&header).ok()?;

    // All-day row: one cell per day; timed events skip this row.
    let allday = doc.create_element("div").ok()?;
    allday.set_attribute("class", "calendar-week-allday").ok()?;
    let allday_label = doc.create_element("div").ok()?;
    allday_label
        .set_attribute("class", "calendar-time-axis-header")
        .ok()?;
    allday_label.set_text_content(Some(&crate::i18n::translate(
        "calendar-all-day-strip",
        None,
    )));
    allday.append_child(&allday_label).ok()?;
    for &(y, m, d) in days {
        let iso = format!("{y:04}-{m:02}-{d:02}");
        let cell = doc.create_element("div").ok()?;
        cell.set_attribute("class", "calendar-week-allday-cell").ok()?;
        cell.set_attribute("data-calendar-action", "add-event").ok()?;
        cell.set_attribute("data-calendar-date", &iso).ok()?;
        if let Some(evs) = events_by_day.get(&iso) {
            for ev in evs {
                let all_day =
                    ev.attrs.get("allDay").map(String::as_str) == Some("true");
                if !all_day {
                    continue;
                }
                if let Some(span) = render_event_span(doc, ev.attrs) {
                    cell.append_child(&span).ok()?;
                }
            }
        }
        allday.append_child(&cell).ok()?;
    }
    root.append_child(&allday).ok()?;

    // Body: time axis + one day column per day. Each day column
    // contains an hour-cell grid (for click-to-add) and, layered
    // on top, positioned event blocks for that day.
    let body = doc.create_element("div").ok()?;
    body.set_attribute("class", "calendar-week-body").ok()?;
    let axis = render_time_axis(doc)?;
    body.append_child(axis.as_ref()).ok()?;
    for &(y, m, d) in days {
        let iso = format!("{y:04}-{m:02}-{d:02}");
        let col = doc.create_element("div").ok()?;
        col.set_attribute("class", "calendar-week-day-column").ok()?;
        col.set_attribute("data-calendar-date", &iso).ok()?;
        // Hourly click surfaces (24 rows, 40px each). Each row is
        // a click target that pre-fills the modal with HH:00.
        for hour in 0..HOURS_IN_DAY {
            let hr_cell = doc.create_element("div").ok()?;
            hr_cell.set_attribute("class", "calendar-hour-cell").ok()?;
            hr_cell.set_attribute("data-calendar-action", "add-event").ok()?;
            hr_cell.set_attribute("data-calendar-date", &iso).ok()?;
            hr_cell.set_attribute("data-calendar-time", &format!("{hour:02}:00")).ok()?;
            col.append_child(&hr_cell).ok()?;
        }
        // Positioned event blocks for timed events on this day.
        // Pass the day iso down so multi-day events can slice
        // themselves at midnight (start day is startTime→24:00,
        // mid days are 00:00→24:00, end day is 00:00→endTime).
        if let Some(evs) = events_by_day.get(&iso) {
            for ev in evs {
                let all_day =
                    ev.attrs.get("allDay").map(String::as_str) == Some("true");
                if all_day {
                    continue;
                }
                if let Some(block) = render_timed_event_block(doc, ev.attrs, &iso, tz)
                {
                    col.append_child(&block).ok()?;
                }
            }
        }
        body.append_child(&col).ok()?;
    }
    root.append_child(&body).ok()?;

    Some(root)
}

fn render_time_axis(doc: &Document) -> Option<Element> {
    let axis = doc.create_element("div").ok()?;
    axis.set_attribute("class", "calendar-time-axis").ok()?;
    // 24 hour labels at fixed HOUR_HEIGHT_PX spacing. Emit the
    // 00:00 label too so the top-of-day is not implicit.
    for hour in 0..HOURS_IN_DAY {
        let row = doc.create_element("div").ok()?;
        row.set_attribute("class", "calendar-time-label").ok()?;
        row.set_text_content(Some(&localized_hour_label(hour)));
        axis.append_child(&row).ok()?;
    }
    Some(axis)
}

/// Position a timed event as an absolutely-positioned block within
/// its day column. When the event spans multiple days, the day
/// column receives only its slice of the event:
///   - Start day: from event's start-time down to 24:00.
///   - Middle days: full 00:00 → 24:00 band.
///   - End day: from 00:00 down to event's end-time.
/// The slice's role is stamped as `data-calendar-day-role` so the
/// drag / edit paths can special-case (a future refinement).
fn render_timed_event_block(
    doc: &Document,
    attrs: &HashMap<String, String>,
    day_iso: &str,
    tz: &str,
) -> Option<Element> {
    let start_iso = attrs.get("startAt")?;
    let end_iso = attrs.get("endAt").cloned().unwrap_or_else(|| start_iso.clone());
    let start_local_date = utc_iso_to_tz_date(start_iso, tz)?;
    let end_local_date =
        utc_iso_to_tz_date(&end_iso, tz).unwrap_or_else(|| start_local_date.clone());
    let (start_h, start_m) = utc_iso_to_tz_hm(start_iso, tz)?;
    let (end_h, end_m) =
        utc_iso_to_tz_hm(&end_iso, tz).unwrap_or((start_h, start_m));

    let is_start_day = day_iso == start_local_date;
    let is_end_day = day_iso == end_local_date;
    // Determine which slice of the event's timeline this column
    // gets. `start_mins` is offset from midnight (0..1440);
    // `end_mins` follows the same convention.
    let (start_mins, end_mins) = match (is_start_day, is_end_day) {
        (true, true) => {
            let s = start_h * 60 + start_m;
            let mut e = end_h * 60 + end_m;
            if e <= s {
                e = HOURS_IN_DAY * 60;
            }
            (s, e)
        }
        (true, false) => {
            // Start day of a multi-day event — from start-time to
            // midnight.
            (start_h * 60 + start_m, HOURS_IN_DAY * 60)
        }
        (false, true) => {
            // End day of a multi-day event — from midnight to
            // end-time.
            (0, end_h * 60 + end_m)
        }
        (false, false) => {
            // Middle day of a multi-day event — full column.
            (0, HOURS_IN_DAY * 60)
        }
    };
    let top_px = start_mins * HOUR_HEIGHT_PX / 60;
    let mut height_px = (end_mins - start_mins) * HOUR_HEIGHT_PX / 60;
    if height_px < MIN_EVENT_HEIGHT_PX {
        height_px = MIN_EVENT_HEIGHT_PX;
    }

    let color = safe_color(attrs);
    let block = doc.create_element("div").ok()?;
    block
        .set_attribute(
            "class",
            &format!("calendar-event calendar-event--timed calendar-event--{color}"),
        )
        .ok()?;
    block
        .set_attribute(
            "style",
            &format!("top: {top_px}px; height: {height_px}px;"),
        )
        .ok()?;
    block.set_attribute("data-calendar-action", "edit-event").ok()?;
    // Non-start slices of a multi-day event are not draggable /
    // resizable — dragging their "top" would violate the "starts
    // at midnight" invariant. Only the start-day slice carries
    // the interactive attributes.
    if is_start_day {
        block.set_attribute("data-calendar-draggable", "timed").ok()?;
    }
    if let Some(bid) = safe_block_id(attrs) {
        block.set_attribute("data-event-id", &bid).ok()?;
    }
    // Mirror the allDay/data-start-at/data-end-at fields the click
    // observer reads to pre-fill the edit modal. Cap iso lengths;
    // the parse already rejects malformed input upstream, but a
    // hostile 10 MB string bypasses parsing when it hits data-*.
    block.set_attribute("data-all-day", "false").ok()?;
    block
        .set_attribute("data-start-at", &clamp_chars(start_iso, MAX_ISO_DATE_LEN))
        .ok()?;
    block
        .set_attribute("data-end-at", &clamp_chars(&end_iso, MAX_ISO_DATE_LEN))
        .ok()?;
    block
        .set_attribute(
            "data-calendar-day-role",
            match (is_start_day, is_end_day) {
                (true, true) => "single",
                (true, false) => "start",
                (false, true) => "end",
                (false, false) => "middle",
            },
        )
        .ok()?;

    let content = safe_optional_text(attrs, "content", MAX_EVENT_CONTENT_LEN)
        .unwrap_or_default();
    let content = content.as_str();
    // Label the block with the tz-adjusted start-of-slice time so
    // multi-day events aren't confusingly all labeled with the
    // original start hour.
    let (label_h, label_m) = if is_start_day {
        (start_h, start_m)
    } else {
        (0, 0)
    };
    let label = if content.is_empty() {
        format!("(no title)  {label_h:02}:{label_m:02}")
    } else {
        format!("{content}  {label_h:02}:{label_m:02}")
    };
    let label_span = doc.create_element("span").ok()?;
    label_span.set_attribute("class", "calendar-event-label").ok()?;
    label_span.set_text_content(Some(&label));
    block.append_child(&label_span).ok()?;

    // Bottom-edge resize handle: extends endAt on drag. Only
    // rendered on the end-day slice (or single-day event) so
    // dragging a middle-day slice doesn't try to reshape a slice
    // that has no meaningful bottom.
    if !is_end_day {
        // No resize handle on start / middle slices — but we still
        // need SOMETHING to satisfy the tuple return in the loop
        // below. Leave the block as-is; the trailing return picks
        // up the block without appending a handle. Fall through to
        // the trailing `Some(block)`.
        return Some(block);
    }
    let handle = doc.create_element("span").ok()?;
    handle
        .set_attribute("class", "calendar-event-resize calendar-event-resize--y")
        .ok()?;
    handle.set_attribute("data-calendar-resize", "timed").ok()?;
    block.append_child(&handle).ok()?;

    Some(block)
}

/// The Monday on/before the given (y, m, d).
fn week_start_of((y, m, d): (u32, u32, u32)) -> (u32, u32, u32) {
    let dow = day_of_week(y, m, d);
    let mut result = (y, m, d);
    for _ in 0..dow {
        // Walk backward; handle month/year rollover via a scratch
        // impl (`next_day` only steps forward).
        result = prev_day(result);
    }
    result
}

fn prev_day((y, m, d): (u32, u32, u32)) -> (u32, u32, u32) {
    if d > 1 {
        (y, m, d - 1)
    } else if m > 1 {
        let pm = m - 1;
        (y, pm, days_in_month(y, pm))
    } else {
        // Year underflow shouldn't hit in normal usage (cursor
        // comes from `today_ymd` or a user-driven date input);
        // saturate rather than panic.
        (y.saturating_sub(1), 12, 31)
    }
}

fn short_weekday(dow: u32) -> &'static str {
    match dow {
        0 => "Mon",
        1 => "Tue",
        2 => "Wed",
        3 => "Thu",
        4 => "Fri",
        5 => "Sat",
        6 => "Sun",
        _ => "?",
    }
}

fn render_event_span(doc: &Document, attrs: &HashMap<String, String>) -> Option<DomNode> {
    let span = doc.create_element("span").ok()?;
    let color = safe_color(attrs);
    span.set_attribute(
        "class",
        &format!("calendar-event calendar-event--{color}"),
    )
    .ok()?;
    span.set_attribute("data-calendar-action", "edit-event").ok()?;
    // #136 drag support: mark month-grid all-day spans as
    // draggable (move by day) and as containing a resize handle
    // (extend end-date). The click observer starts a drag on
    // pointerdown and only opens the edit modal on a stationary
    // pointerup.
    span.set_attribute("data-calendar-draggable", "allday").ok()?;
    if let Some(bid) = safe_block_id(attrs) {
        span.set_attribute("data-event-id", &bid).ok()?;
    }
    stamp_event_time_attrs(&span, attrs);
    let content = safe_optional_text(attrs, "content", MAX_EVENT_CONTENT_LEN);
    let display = match content.as_deref() {
        Some(c) if !c.is_empty() => c,
        _ => "(no title)",
    };
    span.set_text_content(Some(display));
    // Right-edge resize handle: extends endDate on drag.
    let handle = doc.create_element("span").ok()?;
    handle.set_attribute("class", "calendar-event-resize calendar-event-resize--x").ok()?;
    handle.set_attribute("data-calendar-resize", "allday").ok()?;
    span.append_child(&handle).ok()?;
    Some(span.into())
}

/// Copy the event's date/time attrs onto the rendered element as
/// `data-*` so the drag observer can read them without a re-lookup
/// through the model. All values pass through a length cap first
/// to keep an oversized attribute from ballooning the DOM.
fn stamp_event_time_attrs(el: &Element, attrs: &HashMap<String, String>) {
    let all_day = attrs.get("allDay").map(String::as_str) == Some("true");
    let _ = el.set_attribute("data-all-day", if all_day { "true" } else { "false" });
    if all_day {
        if let Some(v) = safe_optional_text(attrs, "startDate", MAX_ISO_DATE_LEN) {
            let _ = el.set_attribute("data-start-date", &v);
        }
        if let Some(v) = safe_optional_text(attrs, "endDate", MAX_ISO_DATE_LEN) {
            let _ = el.set_attribute("data-end-date", &v);
        }
    } else {
        if let Some(v) = safe_optional_text(attrs, "startAt", MAX_ISO_DATE_LEN) {
            let _ = el.set_attribute("data-start-at", &v);
        }
        if let Some(v) = safe_optional_text(attrs, "endAt", MAX_ISO_DATE_LEN) {
            let _ = el.set_attribute("data-end-at", &v);
        }
    }
}

// ─── Event bucketing ───────────────────────────────────────────

struct EventRef<'a> {
    attrs: &'a HashMap<String, String>,
}

fn bucket_events_by_day<'a>(
    content: &'a Fragment,
    tz: &str,
) -> HashMap<String, Vec<EventRef<'a>>> {
    let mut by_day: HashMap<String, Vec<EventRef<'a>>> = HashMap::new();
    for child in &content.children {
        if let Node::Element {
            node_type,
            attrs,
            ..
        } = child
        {
            if *node_type != NodeType::CalendarEvent {
                continue;
            }
            let all_day = attrs.get("allDay").map(String::as_str) == Some("true");
            if all_day {
                let start = attrs.get("startDate").cloned().unwrap_or_default();
                let end = attrs
                    .get("endDate")
                    .cloned()
                    .unwrap_or_else(|| start.clone());
                for iso in iter_dates(&start, &end) {
                    by_day
                        .entry(iso)
                        .or_default()
                        .push(EventRef { attrs });
                }
            } else if let Some(start) = attrs.get("startAt") {
                // Timed events bucket by their LOCAL start-to-end
                // date range in the block's declared timezone. A
                // 22:00 Monday → 06:00 Tuesday event lands under
                // BOTH Monday and Tuesday so each day column can
                // render its own slice of the span.
                let end = attrs.get("endAt").cloned().unwrap_or_else(|| start.clone());
                let start_local = utc_iso_to_tz_date(start, tz);
                let end_local = utc_iso_to_tz_date(&end, tz);
                if let (Some(sd), Some(ed)) = (start_local, end_local) {
                    for iso in iter_dates(&sd, &ed) {
                        by_day
                            .entry(iso)
                            .or_default()
                            .push(EventRef { attrs });
                    }
                }
            }
        }
    }
    by_day
}

// ─── Date helpers ──────────────────────────────────────────────

/// Return the `(year, month, day, hour, minute)` of an RFC 3339
/// UTC instant AS SEEN IN `tz` (IANA name). Empty `tz` or a bad
/// name falls back to the browser's local timezone so we always
/// return something sensible.
///
/// Uses `Intl.DateTimeFormat({timeZone: tz}).formatToParts()` —
/// the only reliable cross-browser way to project a UTC instant
/// into an arbitrary IANA zone without shipping our own tzdata.
pub fn parts_in_tz(utc_iso: &str, tz: &str) -> Option<(u32, u32, u32, u32, u32)> {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(utc_iso));
    if d.get_time().is_nan() {
        return None;
    }
    // Fast path: no tz specified — read browser-local values off
    // the Date directly.
    if tz.is_empty() {
        return Some((
            d.get_full_year(),
            d.get_month() + 1,
            d.get_date(),
            d.get_hours(),
            d.get_minutes(),
        ));
    }
    let opts = js_sys::Object::new();
    let set = |k: &str, v: &str| {
        let _ = js_sys::Reflect::set(
            &opts,
            &wasm_bindgen::JsValue::from_str(k),
            &wasm_bindgen::JsValue::from_str(v),
        );
    };
    set("timeZone", tz);
    set("year", "numeric");
    set("month", "2-digit");
    set("day", "2-digit");
    set("hour", "2-digit");
    set("minute", "2-digit");
    set("hourCycle", "h23");
    let fmt = match IntlDateTimeFormat::new("en-US", &opts) {
        Ok(f) => f,
        Err(_) => {
            // Bad tz name → browser-local fallback.
            return Some((
                d.get_full_year(),
                d.get_month() + 1,
                d.get_date(),
                d.get_hours(),
                d.get_minutes(),
            ));
        }
    };
    let parts = fmt.format_to_parts(&d);
    let mut y = None;
    let mut mo = None;
    let mut da = None;
    let mut h = None;
    let mut mi = None;
    for i in 0..parts.length() {
        let p = parts.get(i);
        let t = js_sys::Reflect::get(&p, &wasm_bindgen::JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string());
        let v = js_sys::Reflect::get(&p, &wasm_bindgen::JsValue::from_str("value"))
            .ok()
            .and_then(|v| v.as_string());
        if let (Some(t), Some(v)) = (t, v) {
            match t.as_str() {
                "year" => y = v.parse().ok(),
                "month" => mo = v.parse().ok(),
                "day" => da = v.parse().ok(),
                "hour" => h = v.parse().ok(),
                "minute" => mi = v.parse().ok(),
                _ => {}
            }
        }
    }
    // Intl sometimes emits `hour = "24"` at the day boundary in
    // `h23` cycle; normalize to 0 so parsers downstream don't
    // choke.
    let h = h.map(|v| if v == 24 { 0 } else { v });
    Some((y?, mo?, da?, h?, mi?))
}

/// The `YYYY-MM-DD` of a UTC ISO string as seen in `tz`.
pub fn utc_iso_to_tz_date(utc_iso: &str, tz: &str) -> Option<String> {
    let (y, m, d, _, _) = parts_in_tz(utc_iso, tz)?;
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// The `(hour, minute)` of a UTC ISO string as seen in `tz`.
pub fn utc_iso_to_tz_hm(utc_iso: &str, tz: &str) -> Option<(u32, u32)> {
    let (_, _, _, h, m) = parts_in_tz(utc_iso, tz)?;
    Some((h, m))
}

/// Today's `(year, month, day)` in `tz`. Falls back to
/// browser-local on an unusable tz.
pub fn today_in_tz(tz: &str) -> (u32, u32, u32) {
    let now = js_sys::Date::new_0();
    let iso = String::from(now.to_iso_string());
    if let Some((y, m, d, _, _)) = parts_in_tz(&iso, tz) {
        (y, m, d)
    } else {
        today_ymd()
    }
}

/// Compose a wall-clock `(y, m, d, h, mi)` in `tz` and return the
/// UTC instant it corresponds to as an RFC 3339 string. Uses a
/// one-pass delta-math trick so we never need a bundled tzdata:
/// treat the desired wall clock as if it were UTC, ask Intl what
/// the resulting scratch instant looks like in `tz`, then shift
/// by the difference between what we WANTED and what Intl SHOWED.
/// Not exact on DST boundaries — spring-forward hours don't
/// exist and fall-back hours have two candidates — but the
/// one-pass result is within 1 hour of correct in every case,
/// good enough for a user typing a meeting time.
pub fn local_wall_to_utc_iso(
    y: u32,
    m: u32,
    d: u32,
    h: u32,
    mi: u32,
    tz: &str,
) -> Option<String> {
    if tz.is_empty() {
        let dt = js_sys::Date::new_with_year_month_day_hr_min_sec(
            y,
            (m - 1) as i32,
            d as i32,
            h as i32,
            mi as i32,
            0,
        );
        return Some(String::from(dt.to_iso_string()));
    }
    let desired = wall_as_scratch_ms(y, m, d, h, mi);
    let scratch_iso = ms_to_iso(desired);
    let (gy, gm, gd, gh, gmi) = parts_in_tz(&scratch_iso, tz)?;
    let seen = wall_as_scratch_ms(gy, gm, gd, gh, gmi);
    let offset = desired - seen;
    let actual = desired + offset;
    Some(ms_to_iso(actual))
}

/// Format `(y, m, d, h, mi)` as `YYYY-MM-DDTHH:MM:00Z` and parse
/// it via `Date`; used as a scratch anchor for the delta-math in
/// `local_wall_to_utc_iso`. The resulting ms is NOT a real UTC
/// timestamp for these wall parts in any real timezone — it's a
/// stable numeric handle we can subtract against.
fn wall_as_scratch_ms(y: u32, m: u32, d: u32, h: u32, mi: u32) -> f64 {
    let s = format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:00Z");
    js_sys::Date::new(&wasm_bindgen::JsValue::from_str(&s)).get_time()
}

fn ms_to_iso(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    String::from(d.to_iso_string())
}

/// Today in the runtime's local TZ. Extracted so tests can mock it.
fn today_ymd() -> (u32, u32, u32) {
    let now = js_sys::Date::new_0();
    let year = now.get_full_year();
    let month = now.get_month() + 1; // JS is 0-indexed.
    let day = now.get_date();
    (year, month, day)
}

fn parse_month_cursor(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) {
        return None;
    }
    Some((y, m))
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "?",
    }
}

/// Locale-aware "Month YYYY" using the shared `IntlDateTimeFormat`
/// binding. Falls back to English `month_name(m) + " " + y` if
/// the Intl construct returns Err. Reads the empty-locales array
/// so Intl picks up `navigator.language` — matches other browser-
/// visible date UI.
fn localized_month_year(y: u32, m: u32) -> String {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"month".into(), &"long".into());
    let _ = js_sys::Reflect::set(&opts, &"year".into(), &"numeric".into());
    let fmt = match crate::i18n::IntlDateTimeFormat::new("", &opts) {
        Ok(f) => f,
        Err(_) => return format!("{} {y}", month_name(m)),
    };
    let date = js_sys::Date::new_with_year_month_day(y, (m - 1) as i32, 1);
    fmt.format(&date)
}

/// Locale-aware hour label for the time-grid views. Passes
/// `hour: "numeric"` to Intl so 12-hour locales (en-US) render
/// `1 AM`, `2 PM`, and 24-hour locales (de-DE, fr-FR) render
/// `01`, `13`. Falls back to `HH:00` (24-hour) on Intl failure.
fn localized_hour_label(hour: u32) -> String {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"hour".into(), &"numeric".into());
    let fmt = match crate::i18n::IntlDateTimeFormat::new("", &opts) {
        Ok(f) => f,
        Err(_) => return format!("{hour:02}:00"),
    };
    // Any date works — we only need the hour component.
    let date = js_sys::Date::new_with_year_month_day_hr_min_sec(
        2024, 0, 1, hour as i32, 0, 0,
    );
    fmt.format(&date)
}

/// Locale-aware short weekday name (e.g. "Mon", "Lun", "月")
/// via `IntlDateTimeFormat`. `dow` is 0=Monday..6=Sunday to
/// match the rest of this module. Falls back to English
/// `short_weekday` on failure.
fn localized_short_weekday(dow: u32) -> String {
    let opts = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&opts, &"weekday".into(), &"short".into());
    let fmt = match crate::i18n::IntlDateTimeFormat::new("", &opts) {
        Ok(f) => f,
        Err(_) => return short_weekday(dow).to_string(),
    };
    // 2024-01-01 was a Monday; ymd(2024, 1, 1 + dow) walks
    // Mon..Sun cleanly within January.
    let date = js_sys::Date::new_with_year_month_day(2024, 0, 1 + dow as i32);
    fmt.format(&date)
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn is_leap_year(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Day-of-week for a given YMD, using Zeller's congruence.
/// Returns 0 = Monday, 6 = Sunday to match the ISO weekday
/// convention used by the grid header.
fn day_of_week(y: u32, m: u32, d: u32) -> u32 {
    let (mut y, mut m) = (y as i32, m as i32);
    if m < 3 {
        m += 12;
        y -= 1;
    }
    let k = y % 100;
    let j = y / 100;
    // Zeller: h = 0 Saturday, 1 Sunday, 2 Monday, ...
    let h = (d as i32 + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Convert to Monday=0.
    ((h + 5) % 7) as u32
}

/// First cell (top-left) of the month grid. The grid starts on the
/// Monday on/before the 1st of the month; we walk backward from
/// that Monday.
fn first_grid_cell(year: u32, month: u32) -> (u32, u32, u32) {
    let dow_of_first = day_of_week(year, month, 1);
    if dow_of_first == 0 {
        return (year, month, 1);
    }
    let mut prev_year = year;
    let mut prev_month = month;
    if prev_month == 1 {
        prev_month = 12;
        prev_year -= 1;
    } else {
        prev_month -= 1;
    }
    let prev_last = days_in_month(prev_year, prev_month);
    (prev_year, prev_month, prev_last - dow_of_first + 1)
}

fn next_day(y: u32, m: u32, d: u32) -> (u32, u32, u32) {
    let last = days_in_month(y, m);
    if d < last {
        (y, m, d + 1)
    } else if m < 12 {
        (y, m + 1, 1)
    } else {
        (y + 1, 1, 1)
    }
}

/// Runaway-loop cap for `iter_dates`. Sized to comfortably cover
/// realistic all-day events (~2 years) so a legitimate long-range
/// planning event doesn't lose its tail; still a hard ceiling
/// against a malformed `YEAR 9999` end date that would otherwise
/// spin the render loop forever.
const ITER_DATES_MAX: usize = 800;

/// Iterate inclusive [start, end] as YYYY-MM-DD strings. `start`
/// and `end` are expected to be well-formed; malformed input
/// yields an empty iteration. If the range exceeds
/// [`ITER_DATES_MAX`] days the tail is truncated and a `web_sys`
/// console warning fires so the missing dates aren't silently
/// lost — same posture as the picker's "stale row" warnings.
fn iter_dates(start: &str, end: &str) -> Vec<String> {
    let Some((mut y, mut m, mut d)) = parse_ymd(start) else {
        return Vec::new();
    };
    let Some((ey, em, ed)) = parse_ymd(end) else {
        return Vec::new();
    };
    if (y, m, d) > (ey, em, ed) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut truncated = true;
    for _ in 0..ITER_DATES_MAX {
        out.push(format!("{y:04}-{m:02}-{d:02}"));
        if (y, m, d) == (ey, em, ed) {
            truncated = false;
            break;
        }
        let n = next_day(y, m, d);
        y = n.0;
        m = n.1;
        d = n.2;
    }
    if truncated {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::warn_1(
            &format!(
                "calendar: event range {start}..{end} exceeds {ITER_DATES_MAX} days; \
                 tail truncated (bump ITER_DATES_MAX if this hits a real workflow)"
            )
            .into(),
        );
    }
    out
}

fn parse_ymd(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.splitn(3, '-');
    let y: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    // Reject impossible calendar dates. See the equivalent guard
    // in editor_component.rs::parse_ymd and the backend is_ymd.
    if !(1..=12).contains(&m) || !(1..=days_in_month(y, m)).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_of_week_matches_known_dates() {
        // 2026-07-04 is a Saturday.
        assert_eq!(day_of_week(2026, 7, 4), 5);
        // 2026-01-01 is a Thursday.
        assert_eq!(day_of_week(2026, 1, 1), 3);
        // 2000-02-29 is a Tuesday (leap year sanity check).
        assert_eq!(day_of_week(2000, 2, 29), 1);
    }

    #[test]
    fn days_in_month_covers_leap_year() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2025, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn first_grid_cell_snaps_to_monday() {
        // July 2026 — day 1 is a Wednesday, so grid starts the
        // Monday before: June 29.
        assert_eq!(first_grid_cell(2026, 7), (2026, 6, 29));
        // February 2027 — day 1 is a Monday, grid starts at Feb 1.
        assert_eq!(first_grid_cell(2027, 2), (2027, 2, 1));
    }

    #[test]
    fn next_day_rolls_month_and_year() {
        assert_eq!(next_day(2026, 7, 30), (2026, 7, 31));
        assert_eq!(next_day(2026, 7, 31), (2026, 8, 1));
        assert_eq!(next_day(2026, 12, 31), (2027, 1, 1));
        assert_eq!(next_day(2024, 2, 28), (2024, 2, 29));
        assert_eq!(next_day(2024, 2, 29), (2024, 3, 1));
    }

    #[test]
    fn iter_dates_produces_inclusive_range() {
        let out = iter_dates("2026-07-14", "2026-07-16");
        assert_eq!(
            out,
            vec![
                "2026-07-14".to_string(),
                "2026-07-15".to_string(),
                "2026-07-16".to_string(),
            ]
        );
    }

    #[test]
    fn iter_dates_bounded_on_malformed_end() {
        assert!(iter_dates("2026-07-14", "not-a-date").is_empty());
    }

    #[test]
    fn iter_dates_empty_when_end_before_start() {
        assert!(iter_dates("2026-07-15", "2026-07-14").is_empty());
    }

    #[test]
    fn parse_month_cursor_accepts_and_rejects() {
        assert_eq!(parse_month_cursor("2026-07"), Some((2026, 7)));
        assert_eq!(parse_month_cursor("2026-13"), None);
        assert_eq!(parse_month_cursor(""), None);
        assert_eq!(parse_month_cursor("2026"), None);
    }

    // ── renderer-defense caps ──

    fn attrs_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        m
    }

    #[test]
    fn safe_view_defaults_and_whitelists() {
        assert_eq!(safe_view(&HashMap::new()), DEFAULT_VIEW);
        assert_eq!(safe_view(&attrs_with(&[("view", "month")])), "month");
        assert_eq!(safe_view(&attrs_with(&[("view", "week")])), "week");
        assert_eq!(safe_view(&attrs_with(&[("view", "day")])), "day");
        assert_eq!(safe_view(&attrs_with(&[("view", "year")])), DEFAULT_VIEW);
        assert_eq!(safe_view(&attrs_with(&[("view", "'--DROP")])), DEFAULT_VIEW);
    }

    #[test]
    fn safe_color_defaults_and_whitelists() {
        assert_eq!(safe_color(&HashMap::new()), DEFAULT_COLOR);
        assert_eq!(safe_color(&attrs_with(&[("color", "red")])), "red");
        assert_eq!(safe_color(&attrs_with(&[("color", "PURPLE")])), DEFAULT_COLOR);
        assert_eq!(safe_color(&attrs_with(&[("color", "javascript:")])), DEFAULT_COLOR);
    }

    #[test]
    fn safe_timezone_falls_back_on_oversized_input() {
        let long = "A".repeat(MAX_TIMEZONE_LEN + 1);
        let m = attrs_with(&[("timezone", long.as_str())]);
        assert_eq!(safe_timezone(&m), DEFAULT_TIMEZONE);
    }

    #[test]
    fn safe_timezone_falls_back_on_bad_chars() {
        // Space, control char, quote — none valid in an IANA name.
        assert_eq!(
            safe_timezone(&attrs_with(&[("timezone", "America/Los Angeles")])),
            DEFAULT_TIMEZONE
        );
        assert_eq!(
            safe_timezone(&attrs_with(&[("timezone", "\";DROP")])),
            DEFAULT_TIMEZONE
        );
    }

    #[test]
    fn safe_timezone_accepts_well_shaped_iana() {
        assert_eq!(
            safe_timezone(&attrs_with(&[("timezone", "America/Los_Angeles")])),
            "America/Los_Angeles"
        );
        assert_eq!(
            safe_timezone(&attrs_with(&[("timezone", "Etc/GMT+7")])),
            "Etc/GMT+7"
        );
        assert_eq!(safe_timezone(&HashMap::new()), DEFAULT_TIMEZONE);
    }

    #[test]
    fn safe_cursor_bounds_and_charset() {
        let long = "1".repeat(MAX_CURSOR_LEN + 1);
        assert_eq!(safe_cursor(&attrs_with(&[("cursor", long.as_str())])), "");
        assert_eq!(safe_cursor(&attrs_with(&[("cursor", "2026-07-04")])), "2026-07-04");
        assert_eq!(safe_cursor(&attrs_with(&[("cursor", "2026-07")])), "2026-07");
        assert_eq!(safe_cursor(&attrs_with(&[("cursor", "bad!chr")])), "");
    }

    #[test]
    fn safe_optional_text_clamps() {
        let m = attrs_with(&[("content", "x".repeat(MAX_EVENT_CONTENT_LEN + 50).as_str())]);
        let out = safe_optional_text(&m, "content", MAX_EVENT_CONTENT_LEN).unwrap();
        assert_eq!(out.chars().count(), MAX_EVENT_CONTENT_LEN);
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod prop {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn safe_timezone_bounded(s in ".*") {
                let m = attrs_with(&[("timezone", s.as_str())]);
                let out = safe_timezone(&m);
                prop_assert!(out.len() <= MAX_TIMEZONE_LEN.max(DEFAULT_TIMEZONE.len()));
            }

            #[test]
            fn safe_optional_text_bounded(s in ".*", max in 1usize..500) {
                let m = attrs_with(&[("content", s.as_str())]);
                if let Some(v) = safe_optional_text(&m, "content", max) {
                    prop_assert!(v.chars().count() <= max);
                }
            }

            #[test]
            fn safe_view_always_returns_valid(s in ".*") {
                let m = attrs_with(&[("view", s.as_str())]);
                let v = safe_view(&m);
                prop_assert!(matches!(v, "month" | "week" | "day"));
            }

            #[test]
            fn safe_cursor_bounded(s in ".*") {
                let m = attrs_with(&[("cursor", s.as_str())]);
                let out = safe_cursor(&m);
                prop_assert!(out.len() <= MAX_CURSOR_LEN);
            }
        }
    }
}
