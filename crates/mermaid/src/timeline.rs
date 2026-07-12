//! Mermaid `timeline` diagrams: parser + SVG renderer (Tier 2).
//!
//! Syntax: an optional `title`, optional `section` groupings, then one line
//! per time period — `Period : Event [: Event…]`. Rendering is a columnar
//! layout: each period is a column with its label on a horizontal timeline,
//! its events stacked as boxes beneath it, and (when present) section bands
//! spanning the columns they group.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const GAP_X: f64 = 18.0;
const INNER_X: f64 = 14.0;
const BOX_H: f64 = 34.0;
const ROW_GAP: f64 = 14.0;
const SECTION_H: f64 = 28.0;
const MIN_COL_W: f64 = 90.0;
const MAX_PERIODS: usize = 400;

#[derive(Debug, Clone)]
pub(crate) struct Period {
    pub section: Option<usize>,
    pub time: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Timeline {
    pub title: Option<String>,
    pub sections: Vec<String>,
    pub periods: Vec<Period>,
}

pub(crate) fn parse(source: &str) -> Result<Timeline, ParseError> {
    let mut title = None;
    let mut sections: Vec<String> = Vec::new();
    let mut periods: Vec<Period> = Vec::new();
    let mut cur_section: Option<usize> = None;
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "timeline" {
                return Err(err("timeline diagram must start with `timeline`", line_no));
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
        // `Period : Event : Event…` — the first `:`-field is the time, the
        // rest are events. A bare `Period` (no colon) is a period with no
        // events.
        let mut parts = line.split(':').map(str::trim);
        let time = parts.next().unwrap_or("").to_string();
        if time.is_empty() {
            return Err(err("timeline entry needs a time period", line_no));
        }
        let events: Vec<String> = parts.filter(|e| !e.is_empty()).map(str::to_string).collect();
        if periods.len() >= MAX_PERIODS {
            return Err(err(format!("timeline too large: more than {MAX_PERIODS} periods"), line_no));
        }
        periods.push(Period { section: cur_section, time, events });
    }

    if !seen_header {
        return Err(ParseError {
            message: "timeline diagram must start with `timeline`".into(),
            line: Some(1),
        });
    }
    Ok(Timeline { title, sections, periods })
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(t: &Timeline) -> String {
    // Column widths: the wider of the period label and its widest event.
    let col_w: Vec<f64> = t
        .periods
        .iter()
        .map(|p| {
            let time_w = measure::text_size(&p.time).0;
            let ev_w = p.events.iter().map(|e| measure::text_size(e).0).fold(0.0_f64, f64::max);
            (time_w.max(ev_w) + 2.0 * INNER_X).max(MIN_COL_W)
        })
        .collect();

    // Column left edges + centers.
    let mut lefts = Vec::with_capacity(col_w.len());
    let mut x = PAD;
    for w in &col_w {
        lefts.push(x);
        x += w + GAP_X;
    }
    let total_w = (x - GAP_X + PAD).max(2.0 * PAD + MIN_COL_W);
    let center = |i: usize| lefts[i] + col_w[i] / 2.0;

    let mut y = PAD;
    let mut body = String::new();

    // Title.
    if let Some(title) = &t.title {
        y += 22.0;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{y:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            total_w / 2.0,
            escape_xml(title)
        ));
        y += 14.0;
    }

    // Section bands: one rect per contiguous run of same-section periods.
    if !t.sections.is_empty() {
        let band_top = y;
        let mut i = 0;
        while i < t.periods.len() {
            let sec = t.periods[i].section;
            let mut j = i;
            while j + 1 < t.periods.len() && t.periods[j + 1].section == sec {
                j += 1;
            }
            if let Some(si) = sec {
                let left = lefts[i];
                let right = lefts[j] + col_w[j];
                body.push_str(&format!(
                    r#"<rect x="{left:.1}" y="{band_top:.1}" width="{:.1}" height="{SECTION_H:.1}" fill="var(--mermaid-cluster-fill, #7773)" rx="4"/>"#,
                    right - left
                ));
                body.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
                    (left + right) / 2.0,
                    band_top + SECTION_H / 2.0 + 5.0,
                    escape_xml(&t.sections[si])
                ));
            }
            i = j + 1;
        }
        y += SECTION_H + ROW_GAP;
    }

    // Period row: a horizontal timeline line + a labeled box per period.
    let period_top = y;
    let line_y = period_top + BOX_H / 2.0;
    if col_w.len() > 1 {
        body.push_str(&format!(
            r#"<line x1="{:.1}" y1="{line_y:.1}" x2="{:.1}" y2="{line_y:.1}" stroke="currentColor" stroke-width="2"/>"#,
            center(0),
            center(col_w.len() - 1)
        ));
    }
    for (i, p) in t.periods.iter().enumerate() {
        box_with_label(&mut body, lefts[i], period_top, col_w[i], BOX_H, &p.time, true);
    }
    y += BOX_H;

    // Events: stacked below each period, connected top-to-bottom.
    let mut max_bottom = y;
    for (i, p) in t.periods.iter().enumerate() {
        let mut ey = period_top + BOX_H;
        for ev in &p.events {
            let top = ey + ROW_GAP;
            body.push_str(&format!(
                r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{top:.1}" stroke="currentColor"/>"#,
                center(i),
                ey,
                center(i)
            ));
            box_with_label(&mut body, lefts[i], top, col_w[i], BOX_H, ev, false);
            ey = top + BOX_H;
        }
        max_bottom = max_bottom.max(ey);
    }

    let total_h = max_bottom + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

/// A rounded box filling `[left, left+w]` at `top`, with centered text.
/// `strong` boxes (period labels) use the node fill + bold text; event
/// boxes use the lighter surface fill.
fn box_with_label(out: &mut String, left: f64, top: f64, w: f64, h: f64, text: &str, strong: bool) {
    let fill = if strong { "var(--mermaid-node-fill, #ececff)" } else { "var(--surface, #fff)" };
    let weight = if strong { r#" font-weight="600""# } else { "" };
    out.push_str(&format!(
        r#"<rect x="{left:.1}" y="{top:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}" stroke="currentColor" rx="4"/>"#
    ));
    out.push_str(&format!(
        r#"<text x="{:.1}" y="{:.1}" text-anchor="middle"{weight} fill="currentColor">{}</text>"#,
        left + w / 2.0,
        top + h / 2.0 + 5.0,
        escape_xml(text)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_sections_and_events() {
        let t = parse(
            "timeline\n title History\n section 2000s\n  2002 : LinkedIn\n  2004 : Facebook : Google\n section 2010s\n  2010 : Instagram",
        )
        .unwrap();
        assert_eq!(t.title.as_deref(), Some("History"));
        assert_eq!(t.sections, vec!["2000s".to_string(), "2010s".to_string()]);
        assert_eq!(t.periods.len(), 3);
        assert_eq!(t.periods[0].time, "2002");
        assert_eq!(t.periods[0].events, vec!["LinkedIn".to_string()]);
        assert_eq!(t.periods[1].events, vec!["Facebook".to_string(), "Google".to_string()]);
        assert_eq!(t.periods[0].section, Some(0));
        assert_eq!(t.periods[2].section, Some(1));
    }

    #[test]
    fn sectionless_and_bare_period() {
        let t = parse("timeline\n 2002 : LinkedIn\n 2003").unwrap();
        assert!(t.sections.is_empty());
        assert_eq!(t.periods.len(), 2);
        assert_eq!(t.periods[0].section, None);
        assert!(t.periods[1].events.is_empty()); // bare period, no events
    }

    #[test]
    fn header_and_empty_period_error() {
        assert!(parse("title X\n2002 : e").is_err()); // missing `timeline`
        assert!(parse("timeline\n : orphan event").is_err()); // no time period
    }

    #[test]
    fn renders_svg_with_title_section_and_events() {
        let svg = render_svg(
            &parse("timeline\n title T\n section S\n 2002 : LinkedIn : Twitter\n 2004 : Facebook")
                .unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<")); // title
        assert!(svg.contains(">S<")); // section band label
        assert!(svg.contains(">2002<") && svg.contains(">LinkedIn<") && svg.contains(">Twitter<"));
    }

    #[test]
    fn render_never_panics_on_no_periods() {
        let svg = render_svg(&parse("timeline\n title only").unwrap());
        assert!(svg.starts_with("<svg"));
    }
}
