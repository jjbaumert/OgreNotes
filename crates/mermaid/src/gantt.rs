//! Mermaid `gantt` chart: parser + SVG renderer.
//!
//! Dates use the default `YYYY-MM-DD` format. Unsupported date/calendar
//! directives are rejected explicitly. Dates become integer day numbers via
//! the civil-calendar algorithm. Task starts may be a date or `after
//! <id>...`; ends may be a date, `until <id>...`, or a duration. The time axis is
//! scaled to a fixed chart width so any date range fits a bounded canvas.

use crate::{escape_xml, measure, ParseError};
use std::collections::HashMap;

const MAX_TASKS: usize = 200;
/// Most date-axis ticks drawn. Ticks are weekly until the span needs more
/// than this, then every N weeks, so the axis costs O(1) whatever the span.
const MAX_TICKS: f64 = 20.0;

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
    pub critical: bool,
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

/// Inverse of `days_from_civil` (Hinnant's `civil_from_days`): a day number
/// back to `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A full `YYYY-MM-DD` axis label for a day number (Mermaid's default
/// `axisFormat`).
fn fmt_axis_date(day: f64) -> String {
    let (y, m, d) = civil_from_days(day.round() as i64);
    format!("{y:04}-{m:02}-{d:02}")
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
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=max_day).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d) as f64)
}

/// Fixed-length durations in days. Calendar months/years are resolved against
/// the actual task start below. Day/week decimals follow dayjs rounding.
fn parse_duration(s: &str) -> Option<f64> {
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    if num.is_empty() || num.starts_with('.') || num.ends_with('.') {
        return None;
    }
    let n = crate::parse_number(num)?;
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    match unit {
        "" | "d" => Some(n.round()),
        "w" => Some((n * 7.0).round()),
        "h" => Some(n / 24.0),
        "m" => Some(n / 1440.0),
        "s" => Some(n / 86400.0),
        "ms" => Some(n / 86400000.0),
        _ => None,
    }
}

fn duration_end(s: &str, start: f64) -> Option<f64> {
    if let Some(days) = parse_duration(s) {
        return Some(start + days);
    }
    let (unit, multiplier) = if s.ends_with('M') {
        ('M', 1.0)
    } else if s.ends_with('y') {
        ('y', 12.0)
    } else {
        return None;
    };
    let number = s.strip_suffix(unit)?;
    if number.is_empty()
        || number.starts_with('.')
        || number.ends_with('.')
        || !number.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return None;
    }
    let n = crate::parse_number(number)?;
    if !(0.0..=120000.0).contains(&n) {
        return None;
    }
    let (y, m, d) = civil_from_days(start.floor() as i64);
    let months = y * 12 + m - 1 + n.trunc() as i64 * multiplier as i64;
    let (ny, nm) = (months.div_euclid(12), months.rem_euclid(12) + 1);
    if !(0..=9999).contains(&ny) {
        return None;
    }
    let next_month = if nm == 12 {
        days_from_civil(ny + 1, 1, 1)
    } else {
        days_from_civil(ny, nm + 1, 1)
    };
    let max_day = next_month - days_from_civil(ny, nm, 1);
    Some(days_from_civil(ny, nm, d.min(max_day)) as f64 + (start - start.floor()))
}

#[derive(Clone)]
struct PendingTask {
    name: String,
    section: usize,
    start: Option<String>,
    end: Option<String>,
    status: Status,
    critical: bool,
    milestone: bool,
    line: usize,
}

#[derive(Clone, Copy)]
enum DatePart {
    Start = 0,
    End = 1,
}

/// A start and an end are separate dependency values: `until b` needs only
/// b's start, whereas `after b` needs its end. Caching whole tasks invents
/// cycles between otherwise independent dates.
struct DateResolver<'a> {
    tasks: &'a [PendingTask],
    ids: &'a HashMap<String, usize>,
    dates: Vec<[Option<f64>; 2]>,
    visiting: Vec<[bool; 2]>,
}

impl<'a> DateResolver<'a> {
    fn new(tasks: &'a [PendingTask], ids: &'a HashMap<String, usize>) -> Self {
        Self {
            tasks,
            ids,
            dates: vec![[None; 2]; tasks.len()],
            visiting: vec![[false; 2]; tasks.len()],
        }
    }

    fn reference_dates(
        &mut self,
        references: &str,
        part: DatePart,
        line: usize,
    ) -> Result<f64, ParseError> {
        let mut result: Option<f64> = None;
        for id in references.split_whitespace() {
            let &other = self
                .ids
                .get(id)
                .ok_or_else(|| err(format!("unknown task `{id}` in Gantt dependency"), line))?;
            let date = self.resolve(other, part)?;
            result = Some(result.map_or(date, |value| match part {
                DatePart::Start => value.min(date),
                DatePart::End => value.max(date),
            }));
        }
        result.ok_or_else(|| err("Gantt dependency needs a task id", line))
    }

    fn resolve(&mut self, index: usize, part: DatePart) -> Result<f64, ParseError> {
        let slot = part as usize;
        if let Some(date) = self.dates[index][slot] {
            return Ok(date);
        }
        let line = self.tasks[index].line;
        if self.visiting[index][slot] {
            return Err(err("cyclic Gantt task dependency", line));
        }
        self.visiting[index][slot] = true;
        let spec = match part {
            DatePart::Start => self.tasks[index].start.clone(),
            DatePart::End => self.tasks[index].end.clone(),
        };
        let date = match (part, spec.as_deref()) {
            (DatePart::Start, Some(spec)) if spec.starts_with("after ") => {
                self.reference_dates(&spec[6..], DatePart::End, line)?
            }
            (DatePart::Start, Some(spec)) => {
                parse_date(spec).ok_or_else(|| err(format!("invalid start date {spec:?}"), line))?
            }
            (DatePart::Start, None) if index > 0 => self.resolve(index - 1, DatePart::End)?,
            (DatePart::Start, None) => {
                return Err(err(
                    "a task with no start date must follow another task",
                    line,
                ))
            }
            (DatePart::End, None) => self.resolve(index, DatePart::Start)?,
            (DatePart::End, Some(spec)) if spec.starts_with("until ") => {
                self.reference_dates(&spec[6..], DatePart::Start, line)?
            }
            (DatePart::End, Some(spec)) => match parse_date(spec) {
                Some(date) => date,
                None => {
                    let start = self.resolve(index, DatePart::Start)?;
                    duration_end(spec, start).ok_or_else(|| {
                        err(format!("invalid end date or duration {spec:?}"), line)
                    })?
                }
            },
        };
        if !date.is_finite() || date > days_from_civil(9999, 12, 31) as f64 {
            return Err(err("task ends after the year 9999", line));
        }
        self.visiting[index][slot] = false;
        self.dates[index][slot] = Some(date);
        Ok(date)
    }
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError {
        message: message.into(),
        line: Some(line),
    }
}

pub(crate) fn parse(source: &str) -> Result<Gantt, ParseError> {
    let mut title = None;
    let mut sections: Vec<String> = Vec::new();
    let mut pending: Vec<PendingTask> = Vec::new();
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
            "dateFormat" if line["dateFormat".len()..].trim() == "YYYY-MM-DD" => continue,
            "axisFormat" if line["axisFormat".len()..].trim() == "%Y-%m-%d" => continue,
            "todayMarker" if line["todayMarker".len()..].trim() == "off" => continue,
            "dateFormat" | "axisFormat" | "excludes" | "includes" | "todayMarker"
            | "tickInterval" | "weekday" => {
                return Err(err(format!("unsupported Gantt directive: {line}"), line_no));
            }
            _ => {}
        }

        // Task line: `Name : metadata`.
        let Some((name, meta)) = line.split_once(':') else {
            return Err(err(
                format!("expected `Task : metadata`, found {line:?}"),
                line_no,
            ));
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(err("Gantt task needs a name", line_no));
        }
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
        let mut critical = false;
        while i < toks.len() {
            match toks[i] {
                "done" => status = Status::Done,
                "active" => status = Status::Active,
                "crit" => critical = true,
                "milestone" => milestone = true,
                _ => break,
            }
            i += 1;
        }
        if critical && status == Status::Normal {
            status = Status::Crit;
        }
        let rest = &toks[i..];
        if rest.is_empty() || rest[0].is_empty() {
            return Err(err("task needs a start date or `after <id>`", line_no));
        }
        // Classify the metadata as [id?] [start?] [end?]. A start spec is a
        // date or `after ...`; a non-start token that precedes the end is the
        // task id. Mermaid lets the start be omitted, in which case the task
        // begins when the previous task ends.
        let is_start = |t: &str| t.starts_with("after ") || parse_date(t).is_some();
        let (id, start_spec, end_spec): (Option<&str>, Option<&str>, Option<&str>) =
            if is_start(rest[0]) {
                // start [end]
                (None, Some(rest[0]), rest.get(1).copied())
            } else if rest.len() >= 2 && is_start(rest[1]) {
                // id start [end]
                (Some(rest[0]), Some(rest[1]), rest.get(2).copied())
            } else if rest.len() >= 2 {
                // id end — start omitted (implicit: after the previous task)
                (Some(rest[0]), None, Some(rest[1]))
            } else {
                // a lone duration/end — start omitted, no id
                (None, None, Some(rest[0]))
            };

        let expected = usize::from(id.is_some())
            + usize::from(start_spec.is_some())
            + usize::from(end_spec.is_some());
        if rest.len() != expected {
            return Err(err("too many Gantt task metadata fields", line_no));
        }
        if pending.len() >= MAX_TASKS {
            return Err(err(
                format!("gantt too large: more than {MAX_TASKS} tasks"),
                line_no,
            ));
        }
        if let Some(id) = id {
            if ids.insert(id.to_string(), pending.len()).is_some() {
                return Err(err(format!("duplicate Gantt task id `{id}`"), line_no));
            }
        }
        pending.push(PendingTask {
            name,
            section,
            start: start_spec.map(str::to_string),
            end: end_spec.map(str::to_string),
            status,
            critical,
            milestone,
            line: line_no,
        });
    }

    if !seen_header {
        return Err(ParseError {
            message: "gantt diagram must start with `gantt`".into(),
            line: None,
        });
    }
    if pending.is_empty() {
        return Err(ParseError {
            message: "gantt has no tasks".into(),
            line: None,
        });
    }
    let mut dates = DateResolver::new(&pending, &ids);
    let mut tasks = Vec::with_capacity(pending.len());
    for (index, task) in pending.iter().enumerate() {
        let start = dates.resolve(index, DatePart::Start)?;
        let end = dates.resolve(index, DatePart::End)?;
        if end < start {
            return Err(err("task ends before it starts", task.line));
        }
        tasks.push(Task {
            name: task.name.clone(),
            section: task.section,
            start,
            end,
            status: task.status,
            critical: task.critical,
            milestone: task.milestone,
        });
    }
    Ok(Gantt {
        title,
        sections,
        tasks,
    })
}

const LEFT: f64 = 80.0; // minimum left margin; section captions can widen it
const CHART_W: f64 = 620.0;
const ROW_H: f64 = 28.0;
const BAR_H: f64 = 18.0;
const TOP: f64 = 40.0;
const AXIS_H: f64 = 22.0;

fn status_fill(s: Status) -> &'static str {
    match s {
        Status::Normal => "var(--mermaid-gantt-task, #8a90dd)",
        Status::Active => "var(--mermaid-gantt-active, #bfc7ff)",
        Status::Done => "var(--mermaid-gantt-done, #b8b8b8)",
        Status::Crit => "var(--mermaid-gantt-crit, #ff6b6b)",
    }
}

pub(crate) fn render_svg(g: &Gantt) -> String {
    let min = g
        .tasks
        .iter()
        .map(|t| t.start)
        .fold(f64::INFINITY, f64::min);
    let max = g
        .tasks
        .iter()
        .map(|t| t.end)
        .fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1.0);
    let px = CHART_W / span;
    let section_width = g
        .sections
        .iter()
        .filter(|label| !label.is_empty())
        .map(|label| measure::literal_overlap_width(label, 12.0) * 1.04)
        .fold(0.0, f64::max);
    // Section captions occupy their own gutter. Task names can overhang short
    // bars too, so reserve their left overhang whenever a section is present.
    let left = if section_width > 0.0 {
        let overhang = g
            .tasks
            .iter()
            .map(|task| {
                let center = ((task.start + task.end) / 2.0 - min) * px;
                (measure::literal_overlap_width(&task.name, 12.0) / 2.0 + 2.0 - center).max(0.0)
            })
            .fold(0.0, f64::max);
        LEFT.max(8.0 + section_width + 12.0 + overhang)
    } else {
        LEFT
    };
    let x_of = |day: f64| left + (day - min) * px;

    let n = g.tasks.len();
    // Rows start below the title; the date axis sits BELOW the chart (Mermaid).
    let top = TOP;
    let chart_bottom = top + n as f64 * ROW_H;
    let w = left + CHART_W + 24.0;
    // These labels are XML-escaped and rendered as one literal text run.
    // Measuring Mermaid <br> lines would underestimate their actual width.
    let mut min_x = 0.0_f64;
    let mut max_x =
        w.max(x_of(max) + measure::literal_overlap_width(&fmt_axis_date(max), 11.0) / 2.0 + 12.0);
    for section in &g.sections {
        max_x = max_x.max(measure::literal_overlap_width(section, 12.0) * 1.04 + 20.0);
    }
    for task in &g.tasks {
        let center = x_of((task.start + task.end) / 2.0);
        let half = measure::literal_overlap_width(&task.name, 12.0) / 2.0 + 12.0;
        min_x = min_x.min(center - half);
        max_x = max_x.max(center + half);
    }
    if let Some(title) = &g.title {
        let half = measure::literal_overlap_width(title, 15.0) * 1.04 / 2.0 + 12.0;
        let center = (min_x + max_x) / 2.0;
        min_x = min_x.min(center - half);
        max_x = max_x.max(center + half);
    }
    let canvas_w = max_x - min_x;
    let h = chart_bottom + AXIS_H + 12.0;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{min_x:.1} 0 {canvas_w:.1} {h:.0}" width="{canvas_w:.1}" height="{h:.0}" style="font-family:sans-serif;font-size:12px">"#
    );

    if let Some(title) = &g.title {
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="24" text-anchor="middle" fill="currentColor" style="font-size:15px;font-weight:600">{}</text>"#,
            min_x + canvas_w / 2.0,
            escape_xml(title),
        ));
    }

    // Section background bands: Mermaid tints the FIRST section and alternates
    // (sectionBkgColor / white). A band spans the contiguous run of one section.
    let mut row = 0usize;
    while row < n {
        let sec = g.tasks[row].section;
        let mut end_row = row;
        while end_row + 1 < n && g.tasks[end_row + 1].section == sec {
            end_row += 1;
        }
        let band_y = top + row as f64 * ROW_H;
        let band_h = (end_row - row + 1) as f64 * ROW_H;
        if sec.is_multiple_of(2) {
            svg.push_str(&format!(
                r#"<rect x="{min_x:.1}" y="{band_y:.1}" width="{canvas_w:.1}" height="{band_h:.1}" fill="var(--mermaid-gantt-section, rgba(102,102,255,0.14))"/>"#
            ));
        }
        if let Some(name) = g.sections.get(sec) {
            if !name.is_empty() {
                let scy = band_y + band_h / 2.0;
                svg.push_str(&format!(
                    r#"<text x="8" y="{:.1}" fill="currentColor" font-weight="600">{}</text>"#,
                    scy + 4.0,
                    escape_xml(name),
                ));
            }
        }
        row = end_row + 1;
    }

    // Weekly date-axis gridlines through the chart, dates labeled BELOW it.
    // Weekly is Mermaid's default cadence; a longer span widens the step to
    // whole weeks so there are never more than MAX_TICKS.
    let step = 7.0 * ((max - min) / 7.0 / MAX_TICKS).ceil().max(1.0);
    let mut day = min;
    while day <= max + 0.5 {
        let tx = x_of(day);
        svg.push_str(&format!(
            r#"<line x1="{tx:.1}" y1="{top:.1}" x2="{tx:.1}" y2="{chart_bottom:.1}" stroke="currentColor" opacity="0.15"/>"#
        ));
        svg.push_str(&format!(
            r#"<text x="{tx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" style="font-size:11px" opacity="0.75">{}</text>"#,
            chart_bottom + 14.0,
            fmt_axis_date(day),
        ));
        day += step;
    }

    // Task rows: bar (or milestone diamond) with the task name centered on it.
    for (i, t) in g.tasks.iter().enumerate() {
        let y = top + i as f64 * ROW_H;
        let cy = y + ROW_H / 2.0;
        let bx = x_of(if t.milestone {
            (t.start + t.end) / 2.0
        } else {
            t.start
        });
        let stroke = if t.critical {
            "var(--mermaid-gantt-crit, #ff6b6b)"
        } else {
            "currentColor"
        };
        let stroke_width = if t.critical { 2.0 } else { 0.5 };
        let fill = status_fill(t.status);
        let name_x = if t.milestone {
            let r = BAR_H / 2.0;
            svg.push_str(&format!(
                r#"<path d="M {bx:.1} {:.1} L {:.1} {cy:.1} L {bx:.1} {:.1} L {:.1} {cy:.1} Z" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}"/>"#,
                cy - r,
                bx + r,
                cy + r,
                bx - r,
            ));
            bx
        } else {
            let bw = ((t.end - t.start) * px).max(2.0);
            svg.push_str(&format!(
                r#"<rect x="{bx:.1}" y="{:.1}" width="{bw:.1}" height="{BAR_H}" rx="3" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}"/>"#,
                y + (ROW_H - BAR_H) / 2.0,
            ));
            bx + bw / 2.0
        };
        // Name centered on the bar (Mermaid places the label inside the task).
        svg.push_str(&format!(
            r#"<text x="{name_x:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
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
    fn section_captions_do_not_overlap_task_bars_or_overhanging_names() {
        let section = "M".repeat(80);
        for task in ["Task".to_string(), "W".repeat(160)] {
            let chart = p(&format!("gantt\nsection {section}\n{task} :2024-01-01, 1d"));
            let output = render_svg(&chart);
            let (_, drawn) = crate::extent::scan(&output);
            let caption = &drawn.iter().find(|item| item.text == section).unwrap().bbox;
            let task_name = &drawn.iter().find(|item| item.text == task).unwrap().bbox;
            assert_eq!(
                caption.overlap(task_name),
                0.0,
                "section and task captions overlap"
            );
            for bar in drawn.iter().filter(|item| {
                item.tag == "rect" && (item.bbox.y1 - item.bbox.y0 - BAR_H).abs() < 0.1
            }) {
                assert_eq!(
                    caption.overlap(&bar.bbox),
                    0.0,
                    "task bar covers section caption"
                );
            }
        }
    }

    #[test]
    fn duration_units_resolve_to_expected_calendar_dates_and_times() {
        // Expected days/times are independent of the duration parser. Fractions
        // exercise sub-day precision; dates cover leap and calendar clamping.
        for (start, duration, end, fraction) in [
            ("2024-01-01", "1ms", "2024-01-01", 1.0 / 86_400_000.0),
            ("2024-01-01", "500ms", "2024-01-01", 500.0 / 86_400_000.0),
            ("2024-01-01", "30s", "2024-01-01", 30.0 / 86_400.0),
            ("2024-01-01", "1.5s", "2024-01-01", 1.5 / 86_400.0),
            ("2024-01-01", "90m", "2024-01-01", 90.0 / 1_440.0),
            ("2024-01-01", "1.5h", "2024-01-01", 1.5 / 24.0),
            ("2024-02-28", "24h", "2024-02-29", 0.0),
            ("2024-01-01", "1d", "2024-01-02", 0.0),
            ("2024-01-01", "1.4d", "2024-01-02", 0.0),
            ("2024-01-01", "1.5d", "2024-01-03", 0.0),
            ("2024-01-01", "0.49d", "2024-01-01", 0.0),
            ("2024-01-01", "0.5d", "2024-01-02", 0.0),
            ("2024-01-01", "1w", "2024-01-08", 0.0),
            ("2024-01-01", "0.5w", "2024-01-05", 0.0),
            ("2024-01-01", "1.5w", "2024-01-12", 0.0),
            ("2024-01-31", "1M", "2024-02-29", 0.0),
            ("2023-01-31", "1M", "2023-02-28", 0.0),
            ("2023-12-31", "2M", "2024-02-29", 0.0),
            ("2024-01-31", "1.5M", "2024-02-29", 0.0),
            ("2024-02-29", "1y", "2025-02-28", 0.0),
            ("2024-02-29", "4y", "2028-02-29", 0.0),
            ("2024-02-29", "1.5y", "2025-02-28", 0.0),
            ("2024-02-28", "1d", "2024-02-29", 0.0),
            ("2023-02-28", "1d", "2023-03-01", 0.0),
            ("2024-12-31", "1d", "2025-01-01", 0.0),
        ] {
            let chart = p(&format!("gantt\nTask :{start}, {duration}"));
            assert_eq!(
                chart.tasks[0].start,
                parse_date(start).unwrap(),
                "{start} + {duration}"
            );
            let expected = parse_date(end).unwrap() + fraction;
            assert!(
                (chart.tasks[0].end - expected).abs() < 1e-9,
                "{start} + {duration}: expected {expected}, got {}",
                chart.tasks[0].end
            );
        }
    }

    #[test]
    fn until_multiple_references_uses_the_earliest_start_in_any_order() {
        for references in ["late early", "early late"] {
            let chart = p(&format!("gantt\nA :a, 2024-01-01, until {references}\nLate :late, 2024-01-10, 20d\nEarly :early, 2024-01-03, 30d"));
            assert_eq!(chart.tasks[0].end, parse_date("2024-01-03").unwrap());
        }
    }

    #[test]
    fn calendar_durations_preserve_time_before_the_epoch() {
        let chart = p("gantt\nStart :start, 1969-01-01, 12h\nMonth :month, after start, 1M\nZero :zero, after start, 0M");
        let noon = parse_date("1969-01-01").unwrap() + 0.5;
        assert_eq!(chart.tasks[1].start, noon);
        assert_eq!(chart.tasks[1].end, parse_date("1969-02-01").unwrap() + 0.5);
        assert_eq!((chart.tasks[2].start, chart.tasks[2].end), (noon, noon));
    }

    #[test]
    fn until_only_requires_the_referenced_start_date() {
        let chart =
            p("gantt\nA :a, 2024-01-01, until b\nB :b, 2024-01-03, until c\nC :c, after a, 1d");
        let jan_1 = parse_date("2024-01-01").unwrap();
        let jan_3 = parse_date("2024-01-03").unwrap();
        assert_eq!((chart.tasks[0].start, chart.tasks[0].end), (jan_1, jan_3));
        assert_eq!((chart.tasks[1].start, chart.tasks[1].end), (jan_3, jan_3));
        assert_eq!(
            (chart.tasks[2].start, chart.tasks[2].end),
            (jan_3, jan_3 + 1.0)
        );
    }

    #[test]
    fn header_required() {
        assert!(parse("A : 2024-01-01, 1d").is_err());
        assert!(parse("gantt\nsection S\nA : 2024-01-01, 1d").is_ok());
    }

    #[test]
    fn renders_a_date_axis_with_gridlines() {
        let svg = render_svg(&p(
            "gantt\ndateFormat YYYY-MM-DD\nsection S\nA task :a1, 2024-01-01, 30d\nB task :after a1, 20d",
        ));
        // The span start is labeled (full YYYY-MM-DD) with a vertical gridline.
        assert!(svg.contains(">2024-01-01<"), "axis start date label: {svg}");
        assert!(svg.contains("<line"), "axis gridlines drawn: {svg}");
        assert_eq!(
            fmt_axis_date(days_from_civil(2024, 2, 15) as f64),
            "2024-02-15"
        );
    }

    #[test]
    fn date_math() {
        // 2024-01-01 -> 2024-01-08 is 7 days.
        assert_eq!(
            parse_date("2024-01-08").unwrap() - parse_date("2024-01-01").unwrap(),
            7.0
        );
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
    fn duration_only_task_starts_after_previous() {
        // A task with only a duration (no start / no `after`) begins when the
        // previous task ends — the canonical intro gantt uses this on its last
        // task. Regression for the "task needs a start date" parse failure.
        let g = p("gantt\ndateFormat YYYY-MM-DD\nsection S\n\
                   First :2014-01-12, 12d\n\
                   another task :24d");
        assert_eq!(g.tasks.len(), 2);
        assert_eq!(g.tasks[1].start, g.tasks[0].end); // starts at First's end
        assert_eq!(g.tasks[1].end - g.tasks[1].start, 24.0);

        // id + duration, start still implicit; the id is registered.
        let g2 = p("gantt\nsection S\nA :2014-01-01, 5d\nB :b2, 10d\nC :after b2, 2d");
        assert_eq!(g2.tasks[1].start, g2.tasks[0].end);
        assert_eq!(g2.tasks[1].end - g2.tasks[1].start, 10.0);
        assert_eq!(g2.tasks[2].start, g2.tasks[1].end); // `after b2` resolves

        // A leading task with no start has nothing to follow → error.
        assert!(parse("gantt\nsection S\nLonely :5d").is_err());
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
    fn long_spans_draw_a_bounded_number_of_ticks() {
        // Years 1..9999: formerly one tick per week (~520k) — a 280 MB SVG.
        let g = p("gantt\nsection S\nA :a1, 0001-01-01, 9999-12-31\n");
        let svg = render_svg(&g);
        let ticks = svg.matches("opacity=\"0.15\"").count();
        assert!(
            (2..=MAX_TICKS as usize + 1).contains(&ticks),
            "{ticks} ticks"
        );
        // A short chart keeps weekly ticks.
        let g = p("gantt\nsection S\nA :a1, 2024-01-01, 21d\n");
        assert_eq!(render_svg(&g).matches("opacity=\"0.15\"").count(), 4);
    }

    #[test]
    fn durations_past_year_9999_are_rejected() {
        for dur in ["9999999d", "99999999999999999999d", &"9".repeat(400)] {
            let src = format!("gantt\nsection S\nA :a1, 2024-01-01, {dur}\n");
            assert!(parse(&src).unwrap_err().message.contains("9999"), "{dur}");
        }
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
