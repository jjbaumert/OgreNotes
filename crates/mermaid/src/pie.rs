//! Mermaid `pie` diagram: parser + SVG renderer.

use crate::ParseError;

#[derive(Debug)]
pub(crate) struct Pie {
    pub title: Option<String>,
    pub show_data: bool,
    pub slices: Vec<(String, f64)>,
}

/// Parse a `pie` diagram. Line numbers in errors are 1-based.
pub(crate) fn parse(source: &str) -> Result<Pie, ParseError> {
    let mut title = None;
    let mut show_data = false;
    let mut slices = Vec::new();
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            // Header line: `pie` optionally followed by `showData` or an
            // inline `title <text>`.
            let mut toks = line.splitn(2, char::is_whitespace);
            if toks.next() != Some("pie") {
                return Err(ParseError {
                    message: "pie diagram must start with `pie`".to_string(),
                    line: Some(line_no),
                });
            }
            let rest = toks.next().unwrap_or("").trim();
            if rest == "showData" {
                show_data = true;
            } else if let Some(t) = rest.strip_prefix("title ") {
                title = Some(t.trim().to_string());
            }
            seen_header = true;
            continue;
        }
        // `title <text>` (only before/among data lines).
        if let Some(rest) = line.strip_prefix("title ") {
            title = Some(rest.trim().to_string());
            continue;
        }
        // Data line: `"Label" : value` or `Label : value`.
        let Some((label_part, value_part)) = line.rsplit_once(':') else {
            return Err(ParseError {
                message: format!("expected `label : value`, got {line:?}"),
                line: Some(line_no),
            });
        };
        let label = label_part.trim().trim_matches('"').to_string();
        let value: f64 = value_part.trim().parse().map_err(|_| ParseError {
            message: format!("`{}` is not a number", value_part.trim()),
            line: Some(line_no),
        })?;
        if value < 0.0 || !value.is_finite() {
            return Err(ParseError {
                message: format!("value must be a non-negative number, got {value}"),
                line: Some(line_no),
            });
        }
        slices.push((label, value));
    }

    if !seen_header {
        return Err(ParseError {
            message: "pie diagram must start with `pie`".to_string(),
            line: None,
        });
    }
    if slices.is_empty() {
        return Err(ParseError {
            message: "pie diagram has no data slices".to_string(),
            line: None,
        });
    }
    Ok(Pie { title, show_data, slices })
}

/// XML-escape a user-supplied string before interpolating into SVG.
pub(crate) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PALETTE: &[&str] = &[
    "#2D5F2D", "#5C3D2E", "#4A90D9", "#D9534F",
    "#F0AD4E", "#5CB85C", "#9B59B6", "#E67E22",
];
const W: f64 = 420.0;
const H: f64 = 300.0;
const CX: f64 = 150.0;
const CY: f64 = 160.0;
const R: f64 = 110.0;

/// Render the pie as a self-contained SVG string. Slice fills come from
/// `PALETTE`; all text uses `currentColor` so it tracks the app theme.
pub(crate) fn render_svg(pie: &Pie) -> String {
    let total: f64 = pie.slices.iter().map(|(_, v)| *v).sum();
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" style="font-family:sans-serif;font-size:12px">"#
    );

    if let Some(title) = &pie.title {
        svg.push_str(&format!(
            r#"<text x="{cx}" y="24" text-anchor="middle" fill="currentColor" style="font-size:15px;font-weight:600">{t}</text>"#,
            cx = CX,
            t = escape_xml(title),
        ));
    }

    // Zero-total: draw an empty outlined circle, still emit one <path> per
    // slice (as zero-length arcs) so callers/tests see slice count.
    let mut angle = -std::f64::consts::FRAC_PI_2; // start at 12 o'clock
    for (i, (label, value)) in pie.slices.iter().enumerate() {
        let frac = if total > 0.0 { value / total } else { 0.0 };
        let sweep = frac * std::f64::consts::TAU;
        let (x0, y0) = (CX + R * angle.cos(), CY + R * angle.sin());
        let end = angle + sweep;
        let (x1, y1) = (CX + R * end.cos(), CY + R * end.sin());
        let large_arc = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let fill = PALETTE[i % PALETTE.len()];
        svg.push_str(&format!(
            r#"<path d="M {CX} {CY} L {x0:.2} {y0:.2} A {R} {R} 0 {large_arc} 1 {x1:.2} {y1:.2} Z" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1"/>"#
        ));
        angle = end;

        // Legend row.
        let ly = 60.0 + i as f64 * 22.0;
        svg.push_str(&format!(
            r#"<rect x="300" y="{ry:.0}" width="14" height="14" fill="{fill}"/>"#,
            ry = ly - 11.0,
        ));
        let legend = if pie.show_data {
            format!("{label} ({})", trim_num(*value))
        } else {
            label.clone()
        };
        svg.push_str(&format!(
            r#"<text x="320" y="{ly:.0}" fill="currentColor">{}</text>"#,
            escape_xml(&legend),
        ));
    }

    svg.push_str("</svg>");
    svg
}

/// Format a value without a trailing `.0` for whole numbers.
fn trim_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_slices() {
        let p = parse("pie title Pets\n\"Dogs\" : 50\n\"Cats\" : 30").unwrap();
        assert_eq!(p.title.as_deref(), Some("Pets"));
        assert!(!p.show_data);
        assert_eq!(p.slices, vec![("Dogs".to_string(), 50.0), ("Cats".to_string(), 30.0)]);
    }

    #[test]
    fn parses_show_data_flag() {
        let p = parse("pie showData\n\"A\" : 1").unwrap();
        assert!(p.show_data);
    }

    #[test]
    fn parses_bare_unquoted_labels() {
        let p = parse("pie\nDogs : 2\nCats : 3").unwrap();
        assert_eq!(p.slices, vec![("Dogs".to_string(), 2.0), ("Cats".to_string(), 3.0)]);
    }

    #[test]
    fn allows_zero_value() {
        let p = parse("pie\n\"A\" : 0\n\"B\" : 5").unwrap();
        assert_eq!(p.slices[0].1, 0.0);
    }

    #[test]
    fn empty_data_is_error() {
        let err = parse("pie").unwrap_err();
        assert!(err.message.to_lowercase().contains("no data") || err.message.to_lowercase().contains("empty"));
    }

    #[test]
    fn missing_header_is_error() {
        assert!(parse("\"A\" : 1").is_err());
    }

    #[test]
    fn negative_value_is_error() {
        let err = parse("pie\n\"A\" : -3").unwrap_err();
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn non_numeric_value_is_error() {
        assert!(parse("pie\n\"A\" : abc").is_err());
    }

    #[test]
    fn svg_has_header_and_one_path_per_slice() {
        let p = parse("pie\n\"A\" : 1\n\"B\" : 1\n\"C\" : 2").unwrap();
        let svg = render_svg(&p);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert_eq!(svg.matches("<path").count(), 3, "one wedge per slice");
        // theme-aware text
        assert!(svg.contains("currentColor"));
    }

    #[test]
    fn svg_escapes_label_markup() {
        let p = parse("pie\n\"<script>\" : 1").unwrap();
        let svg = render_svg(&p);
        assert!(!svg.contains("<script>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn show_data_appends_values_to_legend() {
        let p = parse("pie showData\n\"A\" : 7").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains('7'));
    }
}
