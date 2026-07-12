//! Mermaid user-journey (`journey`) diagrams: parser + SVG renderer (Tier 2).
//!
//! Syntax: an optional `title`, optional `section` groupings, then one line
//! per task — `Task name : score : Actor[, Actor…]` where `score` is a 0–5
//! satisfaction rating. Rendered as a journey curve: tasks are points placed
//! left-to-right, their height set by score (5 at top, 0 at the bottom),
//! joined by a line; each point carries the task name and its actors, and
//! section bands span the tasks they group.

use crate::{escape_xml, ParseError};

const PAD: f64 = 20.0;
const COL_W: f64 = 120.0;
const SECTION_H: f64 = 28.0;
const PLOT_H: f64 = 200.0;
const DOT_R: f64 = 9.0;
const MAX_TASKS: usize = 400;
const MAX_SCORE: f64 = 5.0;

#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub section: Option<usize>,
    pub name: String,
    pub score: u8, // 0..=5
    pub actors: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Journey {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub tasks: Vec<Task>,
}

pub(crate) fn parse(source: &str) -> Result<Journey, ParseError> {
    let mut title = None;
    let mut sections: Vec<String> = Vec::new();
    let mut tasks: Vec<Task> = Vec::new();
    let mut cur_section: Option<usize> = None;
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "journey" {
                return Err(err("journey diagram must start with `journey`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            title = Some(t.trim().to_string());
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            let name = s.trim();
            if name.is_empty() {
                return Err(err("`section` needs a name", line_no));
            }
            sections.push(name.to_string());
            cur_section = Some(sections.len() - 1);
            continue;
        }
        // `Task name : score : Actor, Actor`.
        let mut parts = line.split(':').map(str::trim);
        let name = parts.next().unwrap_or("").to_string();
        if name.is_empty() {
            return Err(err("journey task needs a name", line_no));
        }
        let score_str = parts.next().unwrap_or("").trim();
        let score: u8 = score_str
            .parse()
            .ok()
            .filter(|s| *s <= 5)
            .ok_or_else(|| err(format!("journey task needs a 0–5 score, found {score_str:?}"), line_no))?;
        let actors = parts
            .next()
            .map(|a| a.split(',').map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        if tasks.len() >= MAX_TASKS {
            return Err(err(format!("journey too large: more than {MAX_TASKS} tasks"), line_no));
        }
        tasks.push(Task { section: cur_section, name, score, actors });
    }

    if !seen_header {
        return Err(ParseError {
            message: "journey diagram must start with `journey`".into(),
            line: Some(1),
        });
    }
    Ok(Journey { title, sections, tasks })
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(j: &Journey) -> String {
    let n = j.tasks.len();
    let total_w = (2.0 * PAD + n.max(1) as f64 * COL_W).max(2.0 * PAD + COL_W);
    let center = |i: usize| PAD + i as f64 * COL_W + COL_W / 2.0;
    // score → y (5 at top of the plot, 0 at the bottom).
    let plot_top_ref = |plot_top: f64, score: u8| plot_top + (1.0 - score as f64 / MAX_SCORE) * PLOT_H;

    let mut y = PAD;
    let mut body = String::new();

    // Title.
    if let Some(title) = &j.title {
        y += 22.0;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{y:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            total_w / 2.0,
            escape_xml(title)
        ));
        y += 14.0;
    }

    // Section bands spanning contiguous same-section tasks.
    if !j.sections.is_empty() {
        let band_top = y;
        let mut i = 0;
        while i < n {
            let sec = j.tasks[i].section;
            let mut k = i;
            while k + 1 < n && j.tasks[k + 1].section == sec {
                k += 1;
            }
            if let Some(si) = sec {
                let left = PAD + i as f64 * COL_W;
                let right = PAD + (k + 1) as f64 * COL_W;
                body.push_str(&format!(
                    r#"<rect x="{left:.1}" y="{band_top:.1}" width="{:.1}" height="{SECTION_H:.1}" fill="var(--mermaid-cluster-fill, #7773)" rx="4"/>"#,
                    right - left
                ));
                body.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
                    (left + right) / 2.0,
                    band_top + SECTION_H / 2.0 + 5.0,
                    escape_xml(&j.sections[si])
                ));
            }
            i = k + 1;
        }
        y += SECTION_H + 10.0;
    }

    // Plot area: the journey curve through the task points.
    let plot_top = y;
    if n > 1 {
        let pts: Vec<String> = j
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{:.1},{:.1}", center(i), plot_top_ref(plot_top, t.score)))
            .collect();
        body.push_str(&format!(
            r#"<polyline points="{}" fill="none" stroke="currentColor" stroke-width="2"/>"#,
            pts.join(" ")
        ));
    }
    for (i, t) in j.tasks.iter().enumerate() {
        let cx = center(i);
        let cy = plot_top_ref(plot_top, t.score);
        body.push_str(&format!(
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>"#
        ));
        // score inside the dot.
        body.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-size="11" fill="currentColor">{}</text>"#,
            cy + 4.0,
            t.score
        ));
        // task name below the plot, actors under that.
        let name_y = plot_top + PLOT_H + 22.0;
        body.push_str(&format!(
            r#"<text x="{cx:.1}" y="{name_y:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
            escape_xml(&t.name)
        ));
        if !t.actors.is_empty() {
            body.push_str(&format!(
                r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-size="12" fill="var(--color-text-secondary, #888)">{}</text>"#,
                name_y + 18.0,
                escape_xml(&t.actors.join(", "))
            ));
        }
    }

    let total_h = plot_top + PLOT_H + 22.0 + 18.0 + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_sections_tasks_and_actors() {
        let j = parse(
            "journey\n title My day\n section Work\n  Make tea: 5: Me\n  Do work: 1: Me, Cat\n section Home\n  Sit down: 5: Me",
        )
        .unwrap();
        assert_eq!(j.title.as_deref(), Some("My day"));
        assert_eq!(j.sections, vec!["Work".to_string(), "Home".to_string()]);
        assert_eq!(j.tasks.len(), 3);
        assert_eq!(j.tasks[0].name, "Make tea");
        assert_eq!(j.tasks[0].score, 5);
        assert_eq!(j.tasks[1].actors, vec!["Me".to_string(), "Cat".to_string()]);
        assert_eq!(j.tasks[2].section, Some(1));
    }

    #[test]
    fn score_must_be_0_to_5() {
        assert!(parse("journey\n T: 7: Me").is_err());
        assert!(parse("journey\n T: x: Me").is_err());
        assert!(parse("journey\n T: 0: Me").is_ok());
    }

    #[test]
    fn header_and_nameless_task_error() {
        assert!(parse("title X\nT: 5: Me").is_err()); // missing `journey`
        assert!(parse("journey\n : 5: Me").is_err()); // no task name
    }

    #[test]
    fn renders_curve_with_dots_and_labels() {
        let svg = render_svg(
            &parse("journey\n title T\n section S\n Make tea: 5: Me\n Do work: 1: Me, Cat").unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("<polyline")); // the journey curve
        assert!(svg.contains("<circle")); // task dots
        assert!(svg.contains(">Make tea<") && svg.contains(">Me, Cat<") && svg.contains(">T<"));
    }
}
