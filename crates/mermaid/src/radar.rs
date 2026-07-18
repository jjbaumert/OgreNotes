//! Mermaid `radar-beta` diagrams: parser + SVG renderer (Tier 4).
//!
//! A radar/spider chart: optional `title`, an `axis` list (bare ids or
//! `id["Label"]`), one or more `curve name["Label"]{v, v, …}` series, and an
//! optional `max`. Rendered polar: N equal-angle axes from a center, a few
//! concentric grid rings, and one polygon per curve.

use crate::{escape_xml, measure, ParseError};
use std::f64::consts::PI;

const PAD: f64 = 20.0;
const RADIUS: f64 = 170.0;
const LABEL_GAP: f64 = 12.0;
const TITLE_H: f64 = 30.0;
const LEGEND_ROW: f64 = 18.0;
const RINGS: usize = 4;
const MAX_AXES: usize = 64;
const MAX_CURVES: usize = 16;

/// Per-curve colors, cycled. Mirrors Mermaid's categorical series palette so
/// overlaid curves stay distinguishable.
const PALETTE: &[&str] =
    &["#3b82f6", "#ef4444", "#22c55e", "#a855f7", "#f59e0b", "#14b8a6", "#ec4899", "#64748b"];

#[derive(Debug, Clone)]
pub(crate) struct Curve {
    pub label: String,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct Radar {
    pub title: Option<String>,
    pub axes: Vec<String>,
    pub curves: Vec<Curve>,
    pub max: f64,
}

pub(crate) fn parse(source: &str) -> Result<Radar, ParseError> {
    let mut title = None;
    let mut axes: Vec<String> = Vec::new();
    let mut curves: Vec<Curve> = Vec::new();
    let mut max: Option<f64> = None;
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "radar-beta" {
                return Err(err("radar diagram must start with `radar-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            title = Some(t.trim().to_string());
        } else if let Some(a) = line.strip_prefix("axis ") {
            axes = a.split(',').map(|t| label_of(t.trim())).filter(|s| !s.is_empty()).collect();
            if axes.len() > MAX_AXES {
                return Err(err("radar has too many axes", line_no));
            }
        } else if let Some(m) = line.strip_prefix("max ") {
            // `"1e999"`/`"inf"` parse as f64 infinity — treat non-finite
            // like non-numeric (ignored) so scale math stays finite.
            max = m.trim().parse().ok().filter(|v: &f64| v.is_finite());
        } else if let Some(c) = line.strip_prefix("curve ") {
            let (label, values) = parse_curve(c)
                .ok_or_else(|| err("curve needs `name{v, v, …}`", line_no))?;
            if curves.len() >= MAX_CURVES {
                return Err(err("radar has too many curves", line_no));
            }
            curves.push(Curve { label, values });
        } else {
            return Err(err(format!("unrecognized radar line {line:?}"), line_no));
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "radar diagram must start with `radar-beta`".into(),
            line: Some(1),
        });
    }
    if axes.is_empty() {
        return Err(err("radar needs an `axis` list", 1));
    }
    // Default max: the largest value across curves (or 1).
    let max = max.unwrap_or_else(|| {
        curves.iter().flat_map(|c| &c.values).cloned().fold(1.0_f64, f64::max)
    });
    Ok(Radar { title, axes, curves, max: max.max(1e-9) })
}

/// `id["Label"]` → `Label`; bare token → itself.
fn label_of(s: &str) -> String {
    let s = s.trim();
    if let Some(o) = s.find('[') {
        if let Some(c) = s.rfind(']') {
            if c > o {
                return s[o + 1..c].trim_matches('"').trim().to_string();
            }
        }
    }
    s.to_string()
}

/// `name{v, v, …}` or `name["Label"]{v, v, …}`.
fn parse_curve(s: &str) -> Option<(String, Vec<f64>)> {
    let brace = s.find('{')?;
    let end = s.rfind('}')?;
    let label = label_of(s[..brace].trim());
    let values: Vec<f64> = s[brace + 1..end]
        .split(',')
        // Non-finite values ("1e999", "inf", "NaN") corrupt the v/max
        // scale into NaN polygon points — reject them like non-numerics.
        .map(|v| v.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()?;
    Some((label, values))
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(r: &Radar) -> String {
    let n = r.axes.len();
    // Horizontal room for the widest axis label sitting outside the ring, so
    // side labels ("Physics", "CompSci") aren't clipped at the canvas edge.
    let label_w = r.axes.iter().map(|a| measure::text_size(a).0).fold(0.0_f64, f64::max);
    let side = RADIUS + LABEL_GAP + label_w;
    let title_h = if r.title.is_some() { TITLE_H } else { 0.0 };
    let top_label = LABEL_GAP + measure::LINE_H; // the top axis label above the ring

    let cx = PAD + side;
    let cy = PAD + title_h + top_label + RADIUS;
    let total_w = 2.0 * (PAD + side);
    let mut body = String::new();

    if let Some(title) = &r.title {
        body.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            PAD + 18.0,
            escape_xml(title)
        ));
    }
    // angle of axis i: start at the top, go clockwise.
    let angle = |i: usize| -PI / 2.0 + i as f64 * 2.0 * PI / n as f64;
    let point = |i: usize, frac: f64| {
        let a = angle(i);
        (cx + frac * RADIUS * a.cos(), cy + frac * RADIUS * a.sin())
    };

    // concentric grid rings (as polygons through the axis directions).
    for ring in 1..=RINGS {
        let frac = ring as f64 / RINGS as f64;
        let pts: Vec<String> =
            (0..n).map(|i| { let (x, y) = point(i, frac); format!("{x:.1},{y:.1}") }).collect();
        body.push_str(&format!(
            r#"<polygon points="{}" fill="none" stroke="currentColor" opacity="0.2"/>"#,
            pts.join(" ")
        ));
    }
    // Ring value ticks: mermaid labels each ring with its axis value up the
    // first (top) spoke, so the concentric grid has a readable scale.
    for ring in 1..=RINGS {
        let frac = ring as f64 / RINGS as f64;
        let v = r.max * frac;
        let txt = if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.1}") };
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" font-size="10" fill="currentColor" opacity="0.55">{txt}</text>"#,
            cx + 4.0,
            cy - frac * RADIUS + 4.0,
        ));
    }
    // axis spokes + labels.
    for i in 0..n {
        let (ex, ey) = point(i, 1.0);
        body.push_str(&format!(
            r#"<line x1="{cx:.1}" y1="{cy:.1}" x2="{ex:.1}" y2="{ey:.1}" stroke="currentColor" opacity="0.3"/>"#
        ));
        let (lx, ly) = point(i, 1.0 + LABEL_GAP / RADIUS);
        let anchor = if (lx - cx).abs() < 1.0 { "middle" } else if lx > cx { "start" } else { "end" };
        body.push_str(&format!(
            r#"<text x="{lx:.1}" y="{:.1}" text-anchor="{anchor}" font-size="12" fill="currentColor">{}</text>"#,
            ly + 4.0,
            escape_xml(&r.axes[i])
        ));
    }
    // curves: one polygon per series (filled translucent), legend below.
    for (ci, c) in r.curves.iter().enumerate() {
        let color = PALETTE[ci % PALETTE.len()];
        let pts: Vec<String> = (0..n)
            .map(|i| {
                let v = c.values.get(i).copied().unwrap_or(0.0);
                let (x, y) = point(i, (v / r.max).clamp(0.0, 1.0));
                format!("{x:.1},{y:.1}")
            })
            .collect();
        body.push_str(&format!(
            r#"<polygon points="{}" fill="{color}" fill-opacity="0.12" stroke="{color}" stroke-width="2"/>"#,
            pts.join(" ")
        ));
        // legend entry, stacked below the bottom axis label.
        let ly = cy + RADIUS + top_label + LEGEND_ROW + ci as f64 * LEGEND_ROW;
        body.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="11" height="11" fill="{color}"/><text x="{:.1}" y="{ly:.1}" font-size="12" fill="currentColor">{}</text>"#,
            PAD,
            ly - 10.0,
            PAD + 16.0,
            escape_xml(&c.label)
        ));
    }

    let legend_h =
        if r.curves.is_empty() { 0.0 } else { LEGEND_ROW + r.curves.len() as f64 * LEGEND_ROW };
    let total_h = cy + RADIUS + top_label + legend_h + PAD;
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
    fn parses_axes_curves_and_max() {
        let r = parse(
            "radar-beta\n title Skills\n axis m[\"Math\"], p[\"Physics\"], c[\"Chem\"]\n curve a[\"Alice\"]{85, 90, 80}\n curve b[\"Bob\"]{70, 85, 95}\n max 100",
        )
        .unwrap();
        assert_eq!(r.title.as_deref(), Some("Skills"));
        assert_eq!(r.axes, vec!["Math", "Physics", "Chem"]);
        assert_eq!(r.curves.len(), 2);
        assert_eq!(r.curves[0].label, "Alice");
        assert_eq!(r.curves[0].values, vec![85.0, 90.0, 80.0]);
        assert_eq!(r.max, 100.0);
    }

    #[test]
    fn bare_axes_and_derived_max() {
        let r = parse("radar-beta\n axis A, B, C\n curve x{1, 5, 3}").unwrap();
        assert_eq!(r.axes, vec!["A", "B", "C"]);
        assert_eq!(r.max, 5.0); // derived from data
    }

    #[test]
    fn errors() {
        assert!(parse("axis A, B\n curve x{1,2}").is_err()); // no header
        assert!(parse("radar-beta\n curve x{1,2}").is_err()); // no axes
        assert!(parse("radar-beta\n axis A\n curve x{bad}").is_err()); // non-numeric
    }

    #[test]
    fn renders_grid_axes_and_curve() {
        let svg = render_svg(&parse("radar-beta\n title T\n axis A, B, C\n curve x[\"X\"]{1, 2, 3}").unwrap());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">A<"));
        assert!(svg.matches("<polygon").count() > RINGS); // rings + curve
    }

    #[test]
    fn rings_carry_value_tick_labels() {
        // With max 100 and 4 rings, the top spoke is labeled 25/50/75/100.
        let svg =
            render_svg(&parse("radar-beta\n axis A, B, C\n curve x{10, 20, 30}\n max 100").unwrap());
        for v in ["25", "50", "75", "100"] {
            assert!(svg.contains(&format!(">{v}</text>")), "missing ring tick {v}: {svg}");
        }
    }
}
