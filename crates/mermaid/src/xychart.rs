//! Mermaid `xychart-beta` diagrams: parser + SVG renderer (Tier 2).
//!
//! A bar/line chart: optional quoted `title`, an `x-axis [cat, cat…]`
//! (categorical), a `y-axis "label" min --> max` (numeric range; derived
//! from the data when omitted), and one or more `bar [v, v…]` / `line
//! [v, v…]` series. Rendered with a left y-scale, bottom category axis,
//! grouped bars, and line polylines.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const PLOT_W: f64 = 460.0;
const PLOT_H: f64 = 300.0;
const Y_AXIS_W: f64 = 56.0; // room for y ticks + label
const X_AXIS_H: f64 = 28.0;
const MAX_SERIES: usize = 16;
const MAX_POINTS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeriesKind {
    Bar,
    Line,
}

#[derive(Debug, Clone)]
pub(crate) struct Series {
    pub kind: SeriesKind,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct XYChart {
    pub title: Option<String>,
    pub x_categories: Vec<String>,
    pub y_label: Option<String>,
    pub y_range: Option<(f64, f64)>,
    pub series: Vec<Series>,
}

pub(crate) fn parse(source: &str) -> Result<XYChart, ParseError> {
    let mut c = XYChart {
        title: None,
        x_categories: Vec::new(),
        y_label: None,
        y_range: None,
        series: Vec::new(),
    };
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "xychart-beta" {
                return Err(err("xy chart must start with `xychart-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            c.title = Some(unquote(t.trim()));
        } else if let Some(a) = line.strip_prefix("x-axis ") {
            c.x_categories = parse_list_str(a);
        } else if let Some(a) = line.strip_prefix("y-axis ") {
            let (label, range) = parse_y_axis(a);
            c.y_label = label;
            c.y_range = range;
        } else if let Some(v) = line.strip_prefix("bar ") {
            push_series(&mut c, SeriesKind::Bar, v, line_no)?;
        } else if let Some(v) = line.strip_prefix("line ") {
            push_series(&mut c, SeriesKind::Line, v, line_no)?;
        } else {
            return Err(err(format!("unrecognized xychart line {line:?}"), line_no));
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "xy chart must start with `xychart-beta`".into(),
            line: Some(1),
        });
    }
    if c.series.is_empty() {
        return Err(err("xy chart needs at least one `bar` or `line` series", 1));
    }
    Ok(c)
}

fn push_series(c: &mut XYChart, kind: SeriesKind, list: &str, line_no: usize) -> Result<(), ParseError> {
    let values = parse_list_num(list)
        .ok_or_else(|| err("series must be a `[num, num…]` list", line_no))?;
    if c.series.len() >= MAX_SERIES || values.len() > MAX_POINTS {
        return Err(err("xy chart too large", line_no));
    }
    c.series.push(Series { kind, values });
    Ok(())
}

fn unquote(s: &str) -> String {
    s.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(s).to_string()
}

/// `[a, b, c]` → string items (quotes stripped).
fn parse_list_str(s: &str) -> Vec<String> {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    inner.split(',').map(|x| unquote(x.trim())).filter(|x| !x.is_empty()).collect()
}

/// `[1, 2, 3]` → floats, or `None` if any item isn't numeric.
fn parse_list_num(s: &str) -> Option<Vec<f64>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    inner
        .split(',')
        // Non-finite values ("1e999", "inf", "NaN") turn the axis scale
        // into NaN coordinates — reject them like non-numerics.
        .map(|x| x.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

/// `"label" min --> max`, `"label"`, or `min --> max` — any part optional.
fn parse_y_axis(s: &str) -> (Option<String>, Option<(f64, f64)>) {
    let s = s.trim();
    let (label, rest) = if let Some(r) = s.strip_prefix('"') {
        match r.split_once('"') {
            Some((lbl, after)) => (Some(lbl.to_string()), after.trim()),
            None => (Some(r.to_string()), ""),
        }
    } else {
        (None, s)
    };
    let range = rest.split_once("-->").and_then(|(a, b)| {
        Some((
            a.trim().parse::<f64>().ok().filter(|v| v.is_finite())?,
            b.trim().parse::<f64>().ok().filter(|v| v.is_finite())?,
        ))
    });
    (label, range)
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(c: &XYChart) -> String {
    let n = c
        .x_categories
        .len()
        .max(c.series.iter().map(|s| s.values.len()).max().unwrap_or(0))
        .max(1);
    // y-range: explicit, else [min(0, data_min), data_max].
    let (ymin_raw, ymax_raw) = c.y_range.unwrap_or_else(|| {
        let mut lo = 0.0_f64;
        let mut hi = 1.0_f64;
        for s in &c.series {
            for &v in &s.values {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        (lo, hi)
    });
    // Normalize an inverted explicit range (`10 --> -10`): sorting keeps
    // `span` honest instead of collapsing it to the 1e-9 floor, and makes
    // the value clamp below well-defined.
    let (ymin, ymax) = if ymin_raw <= ymax_raw { (ymin_raw, ymax_raw) } else { (ymax_raw, ymin_raw) };
    let span = (ymax - ymin).max(1e-9);
    // Pin data to the axis range. Values far outside an explicit range
    // (e.g. 1e308 against `0 --> 100`) would otherwise overflow the pixel
    // scale to ±inf; clamped, they draw at the plot edge like any other
    // out-of-range point.
    let cv = move |v: f64| v.clamp(ymin, ymax);

    let plot_left = PAD + Y_AXIS_W;
    let mut plot_top = PAD;
    let total_w = plot_left + PLOT_W + PAD;
    let mut body = String::new();

    if let Some(title) = &c.title {
        plot_top += 24.0;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            plot_left + PLOT_W / 2.0,
            PAD + 18.0,
            escape_xml(title)
        ));
    }
    let (left, top) = (plot_left, plot_top);
    let bottom = top + PLOT_H;
    let vx = |i: f64| left + (i + 0.5) / n as f64 * PLOT_W; // category center
    let vy = |val: f64| bottom - (val - ymin) / span * PLOT_H;

    // axes.
    body.push_str(&format!(
        r#"<line x1="{left:.1}" y1="{top:.1}" x2="{left:.1}" y2="{bottom:.1}" stroke="currentColor"/><line x1="{left:.1}" y1="{bottom:.1}" x2="{:.1}" y2="{bottom:.1}" stroke="currentColor"/>"#,
        left + PLOT_W
    ));
    // y ticks: min, mid, max.
    for frac in [0.0, 0.5, 1.0] {
        let val = ymin + frac * span;
        let gy = bottom - frac * PLOT_H;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="end" font-size="11" fill="currentColor">{}</text>"#,
            left - 6.0,
            gy + 4.0,
            fmt_num(val)
        ));
        if frac > 0.0 {
            body.push_str(&format!(
                r#"<line x1="{left:.1}" y1="{gy:.1}" x2="{:.1}" y2="{gy:.1}" stroke="currentColor" opacity="0.2"/>"#,
                left + PLOT_W
            ));
        }
    }
    // y label (rotated).
    if let Some(lbl) = &c.y_label {
        let lx = PAD + 12.0;
        let ly = top + PLOT_H / 2.0;
        body.push_str(&format!(
            r#"<text x="{lx:.1}" y="{ly:.1}" text-anchor="middle" font-size="12" fill="currentColor" transform="rotate(-90 {lx:.1} {ly:.1})">{}</text>"#,
            escape_xml(lbl)
        ));
    }
    // x category labels.
    for (i, cat) in c.x_categories.iter().enumerate() {
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="11" fill="currentColor">{}</text>"#,
            vx(i as f64),
            bottom + 16.0,
            escape_xml(cat)
        ));
    }

    // series: bars first (grouped), then lines on top.
    let bar_series: Vec<&Series> = c.series.iter().filter(|s| s.kind == SeriesKind::Bar).collect();
    let group_w = PLOT_W / n as f64 * 0.7;
    let bw = if bar_series.is_empty() { 0.0 } else { group_w / bar_series.len() as f64 };
    for (bi, s) in bar_series.iter().enumerate() {
        for (i, &v) in s.values.iter().enumerate() {
            let cx = vx(i as f64) - group_w / 2.0 + bi as f64 * bw;
            let v = cv(v);
            let y = vy(v).min(vy(ymin));
            let h = (vy(ymin) - vy(v)).abs();
            body.push_str(&format!(
                r#"<rect x="{cx:.1}" y="{y:.1}" width="{bw:.1}" height="{h:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>"#
            ));
        }
    }
    for s in c.series.iter().filter(|s| s.kind == SeriesKind::Line) {
        let pts: Vec<String> =
            s.values.iter().enumerate().map(|(i, &v)| format!("{:.1},{:.1}", vx(i as f64), vy(cv(v)))).collect();
        if !pts.is_empty() {
            body.push_str(&format!(
                r#"<polyline points="{}" fill="none" stroke="currentColor" stroke-width="2"/>"#,
                pts.join(" ")
            ));
        }
    }

    let total_h = bottom + X_AXIS_H + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    // touch measure so the import is always used (keeps parity with siblings)
    let _ = measure::LINE_H;
    out
}

/// Compact number: integers without a decimal, else one decimal place.
fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-6 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_axes_and_series() {
        let c = parse(
            "xychart-beta\n title \"Revenue\"\n x-axis [jan, feb, mar]\n y-axis \"$\" 0 --> 100\n bar [10, 40, 90]\n line [10, 40, 90]",
        )
        .unwrap();
        assert_eq!(c.title.as_deref(), Some("Revenue"));
        assert_eq!(c.x_categories, vec!["jan", "feb", "mar"]);
        assert_eq!(c.y_label.as_deref(), Some("$"));
        assert_eq!(c.y_range, Some((0.0, 100.0)));
        assert_eq!(c.series.len(), 2);
        assert_eq!(c.series[0].kind, SeriesKind::Bar);
        assert_eq!(c.series[1].values, vec![10.0, 40.0, 90.0]);
    }

    #[test]
    fn y_range_optional_and_derived() {
        let c = parse("xychart-beta\n bar [3, 7, 5]").unwrap();
        assert_eq!(c.y_range, None); // no explicit range
        // renders (range derived from data)
        assert!(render_svg(&c).contains("<rect"));
    }

    #[test]
    fn errors() {
        assert!(parse("title X\nbar [1]").is_err()); // missing header
        assert!(parse("xychart-beta\n bar [1, x, 3]").is_err()); // non-numeric
        assert!(parse("xychart-beta\n x-axis [a, b]").is_err()); // no series
    }

    #[test]
    fn renders_bars_and_line() {
        let svg = render_svg(
            &parse("xychart-beta\n title \"T\"\n x-axis [a, b, c]\n y-axis \"Y\" 0 --> 10\n bar [2, 5, 8]\n line [3, 6, 9]")
                .unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">a<") && svg.contains(">Y<"));
        assert!(svg.contains("<rect") && svg.contains("<polyline"));
    }
}
