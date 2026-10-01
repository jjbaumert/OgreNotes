//! Mermaid `quadrantChart` diagrams: parser + SVG renderer (Tier 2).
//!
//! A 2×2 quadrant plot: an optional title, `x-axis`/`y-axis` labels (each a
//! `left --> right` / `bottom --> top` pair), up to four `quadrant-N` labels,
//! and `Name: [x, y]` data points with `x`/`y` in 0..1. Rendered as a square
//! split by a center cross, quadrant labels in each cell, axis labels on the
//! edges, and a labeled dot per point.

use crate::{ParseError, escape_xml};

const PAD: f64 = 20.0;
const PLOT: f64 = 380.0; // square plot side
const AXIS_GAP: f64 = 22.0; // room for axis labels
const DOT_R: f64 = 5.0;
const MAX_POINTS: usize = 400;

/// Subtle per-quadrant background tints (TR, TL, BL, BR), Mermaid-style.
const QUAD_TINT: [&str; 4] = [
    "var(--mermaid-quadrant-1, #eef4ff)",
    "var(--mermaid-quadrant-2, #fff7e8)",
    "var(--mermaid-quadrant-3, #fdeef6)",
    "var(--mermaid-quadrant-4, #eefaf1)",
];
/// Categorical fill palette for the plotted points, cycled.
const POINT_PALETTE: &[&str] =
    &["#3b82f6", "#ef4444", "#22c55e", "#a855f7", "#f59e0b", "#14b8a6", "#ec4899", "#64748b"];

#[derive(Debug, Clone)]
pub(crate) struct QuadrantChart {
    pub title: Option<String>,
    pub x_axis: Option<(String, String)>, // (left, right)
    pub y_axis: Option<(String, String)>, // (bottom, top)
    pub quadrants: [Option<String>; 4],   // quadrant-1..4
    pub points: Vec<(String, f64, f64)>,  // (name, x, y) in 0..1
}

pub(crate) fn parse(source: &str) -> Result<QuadrantChart, ParseError> {
    let mut c = QuadrantChart {
        title: None,
        x_axis: None,
        y_axis: None,
        quadrants: [None, None, None, None],
        points: Vec::new(),
    };
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "quadrantChart" {
                return Err(err("quadrant chart must start with `quadrantChart`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            c.title = Some(t.trim().to_string());
        } else if let Some(a) = line.strip_prefix("x-axis ") {
            c.x_axis = Some(split_axis(a));
        } else if let Some(a) = line.strip_prefix("y-axis ") {
            c.y_axis = Some(split_axis(a));
        } else if let Some(rest) = line.strip_prefix("quadrant-") {
            let (num, label) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let n: usize = num
                .parse()
                .ok()
                .filter(|n| (1..=4).contains(n))
                .ok_or_else(|| err("quadrant number must be 1–4", line_no))?;
            c.quadrants[n - 1] = Some(label.trim().to_string());
        } else if let Some((name, coords)) = line.split_once(':') {
            let (x, y) = parse_point(coords).ok_or_else(|| {
                err("data point needs `[x, y]` with x,y in 0..1", line_no)
            })?;
            if c.points.len() >= MAX_POINTS {
                return Err(err(format!("quadrant chart too large: >{MAX_POINTS} points"), line_no));
            }
            c.points.push((name.trim().to_string(), x, y));
        } else {
            return Err(err(format!("unrecognized quadrantChart line {line:?}"), line_no));
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "quadrant chart must start with `quadrantChart`".into(),
            line: Some(1),
        });
    }
    Ok(c)
}

/// `left --> right` (arrow optional); returns `(left, right)`. With no arrow
/// the whole string is the left label and the right is empty.
fn split_axis(s: &str) -> (String, String) {
    match s.split_once("-->") {
        Some((l, r)) => (l.trim().to_string(), r.trim().to_string()),
        None => (s.trim().to_string(), String::new()),
    }
}

/// `[x, y]` with x,y parseable floats clamped to 0..1.
fn parse_point(s: &str) -> Option<(f64, f64)> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (xs, ys) = inner.split_once(',')?;
    let x: f64 = xs.trim().parse().ok()?;
    let y: f64 = ys.trim().parse().ok()?;
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(c: &QuadrantChart) -> String {
    let plot_left = PAD + AXIS_GAP; // room for the y-axis label on the left
    let mut plot_top = PAD;
    let total_w = plot_left + PLOT + PAD;

    let mut body = String::new();
    if let Some(title) = &c.title {
        plot_top += 24.0;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            plot_left + PLOT / 2.0,
            PAD + 18.0,
            escape_xml(title)
        ));
    }
    let (l, top, side) = (plot_left, plot_top, PLOT);
    let (cx0, cy0) = (l + side / 2.0, top + side / 2.0);
    // map data (0..1, 0..1) → svg; y flips (1 = top).
    let px = |x: f64| l + x * side;
    let py = |y: f64| top + (1.0 - y) * side;

    // Per-quadrant pastel tints (Mermaid shades each cell distinctly), drawn
    // behind the border/cross. Order matches quadrant-1..4: TR, TL, BL, BR.
    let half = side / 2.0;
    let cells = [
        (cx0, top, QUAD_TINT[0]),  // 1 top-right
        (l, top, QUAD_TINT[1]),    // 2 top-left
        (l, cy0, QUAD_TINT[2]),    // 3 bottom-left
        (cx0, cy0, QUAD_TINT[3]),  // 4 bottom-right
    ];
    for (qx, qy, tint) in cells {
        body.push_str(&format!(
            r#"<rect x="{qx:.1}" y="{qy:.1}" width="{half:.1}" height="{half:.1}" fill="{tint}"/>"#
        ));
    }
    // Knock out plot strokes beneath labels instead of painting rectangles
    // over the whole scene, which can erase neighboring point markers.
    let label_bounds: Vec<_> = c
        .points
        .iter()
        .map(|(name, x, y)| {
            let (lx, ly) = (px(*x) + DOT_R + 3.0, py(*y) + 4.0);
            let width = crate::measure::literal_text_width(name, 12.0, false) * 1.1 + 8.0;
            (lx - 3.0, ly - 14.0, width, 19.0)
        })
        .collect();
    for (horizontal, fixed, start, end) in [
        (true, top, l, l + side),
        (true, top + side, l, l + side),
        (false, l, top, top + side),
        (false, l + side, top, top + side),
        (false, cx0, top, top + side),
        (true, cy0, l, l + side),
    ] {
        draw_plot_line(&mut body, horizontal, fixed, start, end, &label_bounds);
    }

    // quadrant labels: 1=top-right, 2=top-left, 3=bottom-left, 4=bottom-right.
    let quad_centers = [
        (l + side * 0.75, top + side * 0.25), // 1
        (l + side * 0.25, top + side * 0.25), // 2
        (l + side * 0.25, top + side * 0.75), // 3
        (l + side * 0.75, top + side * 0.75), // 4
    ];
    for (i, label) in c.quadrants.iter().enumerate() {
        if let Some(text) = label {
            if !text.is_empty() {
                let (qx, qy) = quad_centers[i];
                body.push_str(&format!(
                    r#"<text x="{qx:.1}" y="{qy:.1}" text-anchor="middle" fill="currentColor" opacity="0.75">{}</text>"#,
                    escape_xml(text)
                ));
            }
        }
    }

    // axis labels: x below (left/right ends), y on the left (bottom/top ends).
    if let Some((left, right)) = &c.x_axis {
        let ty = top + side + 16.0;
        body.push_str(&format!(
            r#"<text x="{l:.1}" y="{ty:.1}" text-anchor="start" font-size="12" fill="currentColor">{}</text>"#,
            escape_xml(left)
        ));
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{ty:.1}" text-anchor="end" font-size="12" fill="currentColor">{}</text>"#,
            l + side,
            escape_xml(right)
        ));
    }
    if let Some((bottom, top_lbl)) = &c.y_axis {
        let tx = l - 8.0;
        // Rotated to read up the left edge, anchored so each label extends
        // INWARD (toward the plot's vertical center) rather than off-canvas.
        body.push_str(&format!(
            r#"<text x="{tx:.1}" y="{:.1}" text-anchor="start" font-size="12" fill="currentColor" transform="rotate(-90 {tx:.1} {:.1})">{}</text>"#,
            top + side,
            top + side,
            escape_xml(bottom)
        ));
        body.push_str(&format!(
            r#"<text x="{tx:.1}" y="{top:.1}" text-anchor="end" font-size="12" fill="currentColor" transform="rotate(-90 {tx:.1} {top:.1})">{}</text>"#,
            escape_xml(top_lbl)
        ));
    }

    // data points: filled, cycled through a categorical palette (Mermaid
    // colors each point) with a thin light outline for contrast on the tints.
    for (i, (name, x, y)) in c.points.iter().enumerate() {
        let (dx, dy) = (px(*x), py(*y));
        let fill = POINT_PALETTE[i % POINT_PALETTE.len()];
        body.push_str(&format!(
            r#"<circle cx="{dx:.1}" cy="{dy:.1}" r="{DOT_R}" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1"/>"#
        ));
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" font-size="12" fill="currentColor">{}</text>"#,
            dx + DOT_R + 3.0,
            dy + 4.0,
            escape_xml(name)
        ));
    }

    let total_h = top + side + AXIS_GAP + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

/// Draw a horizontal/vertical stroke with gaps through label bounds. Sorting
/// and merging intervals keeps output linear and avoids global SVG mask IDs.
fn draw_plot_line(
    out: &mut String,
    horizontal: bool,
    fixed: f64,
    start: f64,
    end: f64,
    labels: &[(f64, f64, f64, f64)],
) {
    let mut gaps: Vec<_> = labels
        .iter()
        .filter_map(|&(x, y, w, h)| {
            let (across, length, along, span) = if horizontal {
                (y, h, x, w)
            } else {
                (x, w, y, h)
            };
            if fixed < across || fixed > across + length {
                return None;
            }
            let (a, b) = (along.max(start), (along + span).min(end));
            (b > a).then_some((a, b))
        })
        .collect();
    gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut cursor = start;
    for (a, b) in gaps.into_iter().chain(std::iter::once((end, end))) {
        if a > cursor {
            let (x1, y1, x2, y2) = if horizontal {
                (cursor, fixed, a, fixed)
            } else {
                (fixed, cursor, fixed, a)
            };
            out.push_str(&format!(r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="currentColor"/>"#));
        }
        cursor = cursor.max(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_axes_quadrants_and_points() {
        let c = parse(
            "quadrantChart\n title Campaigns\n x-axis Low Reach --> High Reach\n y-axis Low Eng --> High Eng\n quadrant-1 Expand\n quadrant-3 Re-evaluate\n Campaign A: [0.3, 0.6]\n Campaign B: [0.45, 0.23]",
        )
        .unwrap();
        assert_eq!(c.title.as_deref(), Some("Campaigns"));
        assert_eq!(c.x_axis, Some(("Low Reach".into(), "High Reach".into())));
        assert_eq!(c.y_axis, Some(("Low Eng".into(), "High Eng".into())));
        assert_eq!(c.quadrants[0].as_deref(), Some("Expand"));
        assert_eq!(c.quadrants[2].as_deref(), Some("Re-evaluate"));
        assert_eq!(c.points.len(), 2);
        assert_eq!(c.points[0], ("Campaign A".into(), 0.3, 0.6));
    }

    #[test]
    fn point_coords_validated_and_clamped() {
        assert!(parse("quadrantChart\n A: [x, 0.5]").is_err());
        assert!(parse("quadrantChart\n A: 0.3, 0.5").is_err()); // no brackets
        // out-of-range clamps rather than errors
        let c = parse("quadrantChart\n A: [1.5, -0.2]").unwrap();
        assert_eq!(c.points[0].1, 1.0);
        assert_eq!(c.points[0].2, 0.0);
    }

    #[test]
    fn header_required() {
        assert!(parse("title X\nA: [0.1, 0.1]").is_err());
    }

    #[test]
    fn renders_grid_quadrants_and_points() {
        let svg = render_svg(
            &parse("quadrantChart\n title T\n x-axis Lo --> Hi\n quadrant-1 Q1\n A: [0.8, 0.8]").unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">Q1<") && svg.contains(">A<"));
        assert!(svg.matches("<line").count() >= 2); // the center cross
        assert!(svg.contains("<circle")); // the point
    }

    #[test]
    fn quadrants_are_tinted_and_points_filled() {
        let svg = render_svg(
            &parse("quadrantChart\n A: [0.8, 0.8]\n B: [0.2, 0.2]").unwrap(),
        );
        // Each of the four quadrant tints appears as a cell fill.
        for tint in QUAD_TINT {
            assert!(svg.contains(tint), "missing quadrant tint {tint}: {svg}");
        }
        // Points are filled with the palette (not the old hollow node-fill).
        assert!(svg.contains(POINT_PALETTE[0]), "first point uses palette fill: {svg}");
        assert!(!svg.contains("var(--mermaid-node-fill"), "points no longer hollow: {svg}");
    }
}
