//! Mermaid `gantt` chart: parser + SVG renderer.
//!
//! Dates use the default `YYYY-MM-DD` format (other `dateFormat` values are
//! accepted but not reinterpreted), converted to integer day numbers via
//! the civil-calendar algorithm. Task starts may be a date or `after
//! <id>...`; ends may be a date or a `<n>d|w|h` duration. The time axis is
//! scaled to a fixed chart width so any date range fits a bounded canvas.

use crate::{escape_xml, ParseError};
use std::collections::HashMap;

const MAX_TASKS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Normal,
    Done,
    Active,
    Crit,
}

#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub name: String,
    pub section: usize,
    pub start: f64, // day number
    pub end: f64,
    pub status: Status,
    pub milestone: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Gantt {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub tasks: Vec<Task>,
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`). Valid for any in-range y/m/d.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// `YYYY-MM-DD` -> day number, or `None` if it isn't a valid such date.
fn parse_date(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let y: i64 = parts[0].parse().ok()?;
    let m: i64 = parts[1].parse().ok()?;
    let d: i64 = parts[2].parse().ok()?;
    if parts[0].len() != 4 || !(1..=12).contains(&m) {
        return None;
    }
    // Validate the day against the month's real length, so `2024-02-31`
    // is rejected instead of silently normalising into March.
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max_day = match m {
        2 => if leap { 29 } else { 28 },
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=max_day).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) as f64)
}

/// `<n>d|w|h` (or a bare `<n>`, treated as days) -> length in days.
fn parse_duration(s: &str) -> Option<f64> {
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    match unit {
        "" | "d" => Some(n),
        "w" => Some(n * 7.0),
        "h" => Some(n / 24.0),
        _ => None,
    }
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn parse(source: &str) -> Result<Gantt, ParseError> {
    let mut title = None;
    let mut sections: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut cur_section: Option<usize> = None;
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "gantt" {
                return Err(err("gantt diagram must start with `gantt`", line_no));
            }
            seen_header = true;
            continue;
        }

        let first = line.split_whitespace().next().unwrap_or("");
        match first {
            "title" => {
                title = Some(line["title".len()..].trim().to_string());
                continue;
            }
            "section" => {
                sections.push(line["section".len()..].trim().to_string());
                cur_section = Some(sections.len() - 1);
                continue;
            }
            // Accepted but not interpreted (single date format / no calendar).
            "dateFormat" | "axisFormat" | "excludes" | "includes" | "todayMarker"
            | "tickInterval" | "weekday" => continue,
            _ => {}
        }

        // Task line: `Name : metadata`.
        let Some((name, meta)) = line.split_once(':') else {
            return Err(err(format!("expected `Task : metadata`, found {line:?}"), line_no));
        };
        let name = name.trim().to_string();
        let section = match cur_section {
            Some(s) => s,
            None => {
                sections.push(String::new());
                cur_section = Some(0);
                0
            }
        };

        let toks: Vec<&str> = meta.split(',').map(str::trim).collect();
        let mut i = 0;
        let mut status = Status::Normal;
        let mut milestone = false;
        while i < toks.len() {
            match toks[i] {
                "done" => status = Status::Done,
                "active" => status = Status::Active,
                "crit" => status = Status::Crit,
                "milestone" => milestone = true,
                _ => break,
            }
            i += 1;
        }
        let rest = &toks[i..];
        if rest.is_empty() || rest[0].is_empty() {
            return Err(err("task needs a start date or `after <id>`", line_no));
        }
        // A start spec is a date or `after ...`; anything else in the first
        // slot is the (optional) task id.
        let is_start = |t: &str| t.starts_with("after ") || parse_date(t).is_some();
        let (id, start_spec, end_spec): (Option<&str>, &str, Option<&str>) =
            if is_start(rest[0]) {
                (None, rest[0], rest.get(1).copied())
            } else {
                let start = rest
                    .get(1)
                    .copied()
                    .ok_or_else(|| err("task needs a start date or `after <id>`", line_no))?;
                (Some(rest[0]), start, rest.get(2).copied())
            };

        let start = if let Some(after) = start_spec.strip_prefix("after ") {
            let mut acc: Option<f64> = None;
            for rid in after.split_whitespace() {
                let &ti = ids.get(rid).ok_or_else(|| {
                    err(format!("`after` references unknown task `{rid}`"), line_no)
                })?;
                let e = tasks[ti].end;
                acc = Some(acc.map_or(e, |x| x.max(e)));
            }
            acc.ok_or_else(|| err("`after` needs a task id", line_no))?
        } else {
            parse_date(start_spec)
                .ok_or_else(|| err(format!("invalid start date {start_spec:?}"), line_no))?
        };

        let end = match end_spec {
            None => start, // a point (milestone-like)
            Some(e) => {
                if let Some(date) = parse_date(e) {
                    date
                } else if let Some(dur) = parse_duration(e) {
                    start + dur
                } else {
                    return Err(err(format!("invalid end date or duration {e:?}"), line_no));
                }
            }
        };

        if tasks.len() >= MAX_TASKS {
            return Err(err(format!("gantt too large: more than {MAX_TASKS} tasks"), line_no));
        }
        let ti = tasks.len();
        if let Some(id) = id {
            ids.insert(id.to_string(), ti);
        }
        tasks.push(Task { name, section, start, end: end.max(start), status, milestone });
    }

    if !seen_header {
        return Err(ParseError {
            message: "gantt diagram must start with `gantt`".into(),
            line: None,
        });
    }
    if tasks.is_empty() {
        return Err(ParseError { message: "gantt has no tasks".into(), line: None });
    }
    Ok(Gantt { title, sections, tasks })
}

const LEFT: f64 = 160.0;
const CHART_W: f64 = 620.0;
const ROW_H: f64 = 28.0;
const BAR_H: f64 = 18.0;
const TOP: f64 = 40.0;

fn status_fill(s: Status) -> &'static str {
    match s {
        Status::Normal => "var(--mermaid-gantt-task, #8a90dd)",
        Status::Active => "var(--mermaid-gantt-active, #bfc7ff)",
        Status::Done => "var(--mermaid-gantt-done, #b8b8b8)",
        Status::Crit => "var(--mermaid-gantt-crit, #ff6b6b)",
    }
}

pub(crate) fn render_svg(g: &Gantt) -> String {
    let min = g.tasks.iter().map(|t| t.start).fold(f64::INFINITY, f64::min);
    let max = g.tasks.iter().map(|t| t.end).fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1.0);
    let px = CHART_W / span;
    let x_of = |day: f64| LEFT + (day - min) * px;

    let n = g.tasks.len();
    let w = LEFT + CHART_W + 24.0;
    let h = TOP + n as f64 * ROW_H + 16.0;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:12px">"#
    );

    if let Some(title) = &g.title {
        svg.push_str(&format!(
            r#"<text x="{:.0}" y="24" text-anchor="middle" fill="currentColor" style="font-size:15px;font-weight:600">{}</text>"#,
            w / 2.0,
            escape_xml(title),
        ));
    }

    // Section background bands (alternating) + section name at the band's
    // top-left. A band spans the contiguous run of rows in one section.
    let mut row = 0usize;
    while row < n {
        let sec = g.tasks[row].section;
        let mut end_row = row;
        while end_row + 1 < n && g.tasks[end_row + 1].section == sec {
            end_row += 1;
        }
        let band_y = TOP + row as f64 * ROW_H;
        let band_h = (end_row - row + 1) as f64 * ROW_H;
        if sec % 2 == 1 {
            svg.push_str(&format!(
                r#"<rect x="0" y="{band_y:.1}" width="{w:.0}" height="{band_h:.1}" fill="var(--mermaid-gantt-band, #00000010)"/>"#
            ));
        }
        if let Some(name) = g.sections.get(sec) {
            if !name.is_empty() {
                // Rotated into a thin far-left strip so it never collides
                // with the (right-aligned) task names in the gutter.
                let scy = band_y + band_h / 2.0;
                svg.push_str(&format!(
                    r#"<text x="12" y="{scy:.1}" text-anchor="middle" transform="rotate(-90 12 {scy:.1})" fill="currentColor" font-weight="600">{}</text>"#,
                    escape_xml(name),
                ));
            }
        }
        row = end_row + 1;
    }

    // Task rows: bar (or milestone diamond) + right-aligned name in the gutter.
    for (i, t) in g.tasks.iter().enumerate() {
        let y = TOP + i as f64 * ROW_H;
        let cy = y + ROW_H / 2.0;
        let bx = x_of(t.start);
        let fill = status_fill(t.status);
        if t.milestone {
            let r = BAR_H / 2.0;
            svg.push_str(&format!(
                r#"<path d="M {bx:.1} {:.1} L {:.1} {cy:.1} L {bx:.1} {:.1} L {:.1} {cy:.1} Z" fill="{fill}" stroke="currentColor"/>"#,
                cy - r,
                bx + r,
                cy + r,
                bx - r,
            ));
        } else {
            let bw = ((t.end - t.start) * px).max(2.0);
            svg.push_str(&format!(
                r#"<rect x="{bx:.1}" y="{:.1}" width="{bw:.1}" height="{BAR_H}" rx="3" fill="{fill}" stroke="currentColor" stroke-width="0.5"/>"#,
                y + (ROW_H - BAR_H) / 2.0,
            ));
        }
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="end" fill="currentColor">{}</text>"#,
            LEFT - 8.0,
            cy + 4.0,
            escape_xml(&t.name),
        ));
    }

    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Gantt {
        parse(src).expect("parse ok")
    }





    #[test]
    fn header_required() {
        assert!(parse("A : 2024-01-01, 1d").is_err());
        assert!(parse("gantt\nsection S\nA : 2024-01-01, 1d").is_ok());
    }

    #[test]
    fn date_math() {
        // 2024-01-01 -> 2024-01-08 is 7 days.
        assert_eq!(parse_date("2024-01-08").unwrap() - parse_date("2024-01-01").unwrap(), 7.0);
        // Leap day exists in 2024 but not 2023; impossible days rejected.
        assert!(parse_date("2024-02-29").is_some());
        assert!(parse_date("2023-02-29").is_none());
        assert!(parse_date("2024-02-31").is_none());
        assert!(parse_date("2024-04-31").is_none());
        assert_eq!(parse_duration("2w").unwrap(), 14.0);
        assert_eq!(parse_duration("5").unwrap(), 5.0);
        assert!(parse_date("2024-13-01").is_none());
    }

    #[test]
    fn task_forms() {
        let g = p("gantt\ntitle Plan\ndateFormat YYYY-MM-DD\nsection Dev\n\
                   Design :done, des1, 2024-01-01, 2024-01-05\n\
                   Build :active, b1, after des1, 3d\n\
                   Ship :milestone, m1, after b1, 0d");
        assert_eq!(g.title.as_deref(), Some("Plan"));
        assert_eq!(g.sections, vec!["Dev"]);
        assert_eq!(g.tasks.len(), 3);
        // des1: Jan 1 -> Jan 5
        assert_eq!(g.tasks[0].status, Status::Done);
        assert_eq!(g.tasks[0].end - g.tasks[0].start, 4.0);
        // b1 starts after des1 ends (Jan 5), lasts 3 days.
        assert_eq!(g.tasks[1].status, Status::Active);
        assert_eq!(g.tasks[1].start, g.tasks[0].end);
        assert_eq!(g.tasks[1].end - g.tasks[1].start, 3.0);
        // milestone
        assert!(g.tasks[2].milestone);
        assert_eq!(g.tasks[2].start, g.tasks[1].end);
    }

    #[test]
    fn task_without_id_or_status() {
        let g = p("gantt\nsection S\nA task :2024-03-10, 12d");
        assert_eq!(g.tasks[0].name, "A task");
        assert_eq!(g.tasks[0].end - g.tasks[0].start, 12.0);
        assert_eq!(g.tasks[0].status, Status::Normal);
    }

    #[test]
    fn implicit_section() {
        let g = p("gantt\nOnly :2024-01-01, 1d");
        assert_eq!(g.sections.len(), 1);
        assert_eq!(g.tasks[0].section, 0);
    }

    #[test]
    fn after_unknown_task_errors() {
        let e = parse("gantt\nsection S\nA :after ghost, 1d").unwrap_err();
        assert!(e.message.contains("unknown task"), "got: {}", e.message);
    }

    #[test]
    fn empty_and_bad_lines_error() {
        assert!(parse("gantt").unwrap_err().message.contains("no tasks"));
        assert!(parse("gantt\nsection S\nBad line no colon").is_err());
        assert!(parse("gantt\nsection S\nA :notadate, 1d").is_err());
    }

    #[test]
    fn renders_bars_sections_and_milestone() {
        let g = p("gantt\ntitle T\nsection Phase 1\n\
                   Task A :done, a, 2024-01-01, 3d\n\
                   Task B :active, b, after a, 2d\n\
                   section Phase 2\n\
                   Done :milestone, m, after b, 0d");
        let svg = render_svg(&g);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("Task A") && svg.contains("Task B") && svg.contains("Done"));
        assert!(svg.contains("Phase 1") && svg.contains("Phase 2"));
        assert!(svg.matches("<rect").count() >= 2, "task bars"); // >=2 bars
        assert!(svg.contains("<path"), "milestone diamond");
        assert!(svg.contains("T")); // title
    }

    #[test]
    fn label_markup_escaped() {
        let g = p("gantt\nsection S\n<script> :2024-01-01, 1d");
        let svg = render_svg(&g);
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn task_cap_enforced() {
        let mut src = String::from("gantt\nsection S\n");
        for i in 0..=MAX_TASKS {
            src.push_str(&format!("T{i} :2024-01-01, 1d\n"));
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
