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
const SECTION_H: f64 = 26.0;
const BOX_H: f64 = 26.0; // task box height
const AXIS_GAP: f64 = 22.0;
const FACE_PLOT_H: f64 = 150.0; // vertical span of the score-positioned faces
const LEFT_MARGIN: f64 = 72.0; // left gutter for the actor legend
const DOT_R: f64 = 13.0;
const MAX_TASKS: usize = 400;
const MAX_SCORE: f64 = 5.0;
/// Per-section pastel palette (Mermaid tints each section and its task boxes).
const SECTION_COLORS: &[&str] =
    &["#8a8ae0", "#e8e85a", "#a8e05a", "#c99be0", "#5ad1e0", "#e0a85a", "#5ae0a8", "#e05ad1"];
const DASH_COLOR: &str = "#bbbbbb";

fn section_color(sec: Option<usize>) -> &'static str {
    match sec {
        Some(s) => SECTION_COLORS[s % SECTION_COLORS.len()],
        None => "#cccccc",
    }
}

/// Per-actor colors, cycled (Mermaid colors journey points by actor).
const PALETTE: &[&str] =
    &["#3b82f6", "#ef4444", "#22c55e", "#a855f7", "#f59e0b", "#14b8a6", "#ec4899", "#64748b"];

/// A satisfaction face inside a score dot: eyes + a mouth that smiles (score
/// 4-5), stays flat (3), or frowns (0-2), like Mermaid's journey emoji.
fn face(cx: f64, cy: f64, r: f64, score: u8) -> String {
    let eye_dx = r * 0.32;
    let eye_dy = r * 0.22;
    let eye_r = r * 0.11;
    let mut s = format!(
        r#"<circle cx="{:.1}" cy="{:.1}" r="{eye_r:.1}" fill="currentColor"/><circle cx="{:.1}" cy="{:.1}" r="{eye_r:.1}" fill="currentColor"/>"#,
        cx - eye_dx, cy - eye_dy, cx + eye_dx, cy - eye_dy,
    );
    let mw = r * 0.45;
    let my = cy + r * 0.28;
    let mouth = if score >= 4 {
        // smile: control point below the endpoints
        format!(r#"<path d="M {:.1} {:.1} Q {cx:.1} {:.1} {:.1} {:.1}" fill="none" stroke="currentColor" stroke-width="1.3"/>"#,
            cx - mw, my - r * 0.12, my + r * 0.28, cx + mw, my - r * 0.12)
    } else if score <= 2 {
        // frown: control point above the endpoints
        format!(r#"<path d="M {:.1} {:.1} Q {cx:.1} {:.1} {:.1} {:.1}" fill="none" stroke="currentColor" stroke-width="1.3"/>"#,
            cx - mw, my + r * 0.18, my - r * 0.22, cx + mw, my + r * 0.18)
    } else {
        // neutral: a flat line
        format!(r#"<line x1="{:.1}" y1="{my:.1}" x2="{:.1}" y2="{my:.1}" stroke="currentColor" stroke-width="1.3"/>"#,
            cx - mw, cx + mw)
    };
    s.push_str(&mouth);
    s
}

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
    let total_w = PAD + LEFT_MARGIN + n.max(1) as f64 * COL_W + PAD;
    // Unique actors in first-seen order, each mapped to a palette color.
    let mut actors: Vec<String> = Vec::new();
    for t in &j.tasks {
        for a in &t.actors {
            if !actors.contains(a) {
                actors.push(a.clone());
            }
        }
    }
    let actor_color = |name: &str| -> &'static str {
        actors.iter().position(|a| a == name).map(|i| PALETTE[i % PALETTE.len()]).unwrap_or("#888")
    };
    let lefts = |i: usize| PAD + LEFT_MARGIN + i as f64 * COL_W;
    let center = |i: usize| lefts(i) + COL_W / 2.0;

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

    // Layout bands: section headers -> task boxes -> axis -> score faces.
    let section_top = y;
    let task_top = section_top + SECTION_H + 6.0;
    let axis_y = task_top + BOX_H + AXIS_GAP;
    let face_top = axis_y + AXIS_GAP;
    let face_y = |score: u8| face_top + (1.0 - score as f64 / MAX_SCORE) * FACE_PLOT_H;
    let drop_bottom = face_top + FACE_PLOT_H + DOT_R + 8.0;

    // Colored section bands spanning contiguous same-section tasks.
    let mut i = 0;
    while i < n {
        let sec = j.tasks[i].section;
        let mut k = i;
        while k + 1 < n && j.tasks[k + 1].section == sec {
            k += 1;
        }
        let (left, right) = (lefts(i), lefts(k) + COL_W);
        body.push_str(&format!(
            r#"<rect x="{:.1}" y="{section_top:.1}" width="{:.1}" height="{SECTION_H:.1}" fill="{}" rx="4"/>"#,
            left + 3.0,
            right - left - 6.0,
            section_color(sec),
        ));
        if let Some(si) = sec {
            body.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
                (left + right) / 2.0,
                section_top + SECTION_H / 2.0 + 5.0,
                escape_xml(&j.sections[si])
            ));
        }
        i = k + 1;
    }

    // Dashed droplines from each task box down through the axis to its face.
    for i in 0..n {
        let c = center(i);
        body.push_str(&format!(
            r#"<line x1="{c:.1}" y1="{:.1}" x2="{c:.1}" y2="{drop_bottom:.1}" stroke="{DASH_COLOR}" stroke-width="1" stroke-dasharray="3 3"/>"#,
            task_top + BOX_H,
        ));
    }
    // Time axis arrow.
    body.push_str(&format!(
        r#"<line x1="{:.1}" y1="{axis_y:.1}" x2="{:.1}" y2="{axis_y:.1}" stroke="currentColor" stroke-width="2.5" marker-end="url(#jr-arrow)"/>"#,
        PAD + LEFT_MARGIN - 10.0,
        total_w - PAD / 2.0,
    ));

    // Task boxes (tinted by section) with the task name + small actor dots.
    for (i, t) in j.tasks.iter().enumerate() {
        let (bx, bw) = (lefts(i) + 6.0, COL_W - 12.0);
        body.push_str(&format!(
            r#"<rect x="{bx:.1}" y="{task_top:.1}" width="{bw:.1}" height="{BOX_H:.1}" rx="4" fill="{}" fill-opacity="0.5" stroke="{}"/>"#,
            section_color(t.section),
            section_color(t.section),
        ));
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="12" fill="currentColor">{}</text>"#,
            bx + bw / 2.0,
            task_top + BOX_H / 2.0 + 4.0,
            escape_xml(&t.name)
        ));
        for (a_i, a) in t.actors.iter().enumerate() {
            body.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="3.5" fill="{}"/>"#,
                bx + 6.0,
                task_top + 6.0 + a_i as f64 * 8.0,
                actor_color(a),
            ));
        }
    }

    // Score faces on the droplines (5 near the axis, 0 at the bottom).
    for (i, t) in j.tasks.iter().enumerate() {
        let (cx, cy) = (center(i), face_y(t.score));
        body.push_str(&format!(
            r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{DOT_R}" fill="var(--surface, #fff)" stroke="{DASH_COLOR}"/>"#
        ));
        body.push_str(&face(cx, cy, DOT_R, t.score));
    }

    // Actor legend in the top-left gutter.
    for (a_i, a) in actors.iter().enumerate() {
        let ly = section_top + 4.0 + a_i as f64 * 16.0;
        body.push_str(&format!(
            r#"<circle cx="{:.1}" cy="{:.1}" r="4" fill="{}"/><text x="{:.1}" y="{:.1}" font-size="12" fill="currentColor">{}</text>"#,
            PAD + 4.0,
            ly - 4.0,
            actor_color(a),
            PAD + 14.0,
            ly,
            escape_xml(a)
        ));
    }

    let total_h = drop_bottom + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px"><defs><marker id="jr-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="9" markerHeight="9" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
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
    fn renders_timeline_layout_with_sections_and_axis() {
        let svg = render_svg(
            &parse("journey\n title T\n section S\n Make tea: 5: Me\n Do work: 1: Me, Cat").unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">Make tea<") && svg.contains(">T<") && svg.contains(">S<"));
        // Timeline-style: a colored section band, an axis arrow, and dashed
        // droplines (replacing the old satisfaction line-curve).
        assert!(!svg.contains("<polyline"), "no more line-chart curve: {svg}");
        assert!(svg.contains(SECTION_COLORS[0]), "colored section: {svg}");
        assert!(svg.contains("marker-end=\"url(#jr-arrow)\""), "axis arrow: {svg}");
        assert!(svg.contains(r#"stroke-dasharray="3 3""#), "droplines: {svg}");
        // Actors in the top-left legend.
        assert!(svg.contains(">Me<") && svg.contains(">Cat<"), "actor legend: {svg}");
    }

    #[test]
    fn scores_render_as_faces_and_actors_are_colored() {
        let svg = render_svg(
            &parse("journey\n title T\n Happy: 5: Me\n Sad: 0: Cat").unwrap(),
        );
        // A smile (score 5) and a frown (score 0) both draw a mouth <path>;
        // the neutral case would be a <line>. Faces replace the bare numbers.
        assert!(svg.matches("<path").count() >= 2, "smile + frown mouths: {svg}");
        // Each actor gets a distinct palette color (not the old gray #888).
        assert!(svg.contains(PALETTE[0]) && svg.contains(PALETTE[1]), "actor colors: {svg}");
    }
}
