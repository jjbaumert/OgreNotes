//! Mermaid `pie` diagram: parser + SVG renderer.

use crate::{escape_xml, ParseError};

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
            // Header line: `pie` optionally followed by `showData` and/or
            // an inline `title <text>`, in either order:
            //   pie
            //   pie showData
            //   pie title Pets
            //   pie showData title Pets
            let mut toks = line.splitn(2, char::is_whitespace);
            if toks.next() != Some("pie") {
                return Err(ParseError {
                    message: "pie diagram must start with `pie`".to_string(),
                    line: Some(line_no),
                });
            }
            let mut rest = toks.next().unwrap_or("").trim();
            if rest == "showData" {
                show_data = true;
                rest = "";
            } else if let Some(after) = rest.strip_prefix("showData ") {
                show_data = true;
                rest = after.trim();
            }
            if let Some(t) = rest.strip_prefix("title ") {
                title = Some(t.trim().to_string());
            } else if rest == "title" {
                title = Some(String::new());
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
    // Mermaid parity: a wedge is only drawn when its value is at least 1%
    // of the total (`(value / total) * 100 >= 1`); the kept wedges are
    // then re-normalized to fill the full circle (d3.pie recomputes angles
    // from the *kept* sum). The legend still lists every slice, so `shown`
    // gates the wedge only, never the legend row. `total > 0.0` guards the
    // all-zero case (where the ratio would be `0/0`).
    let shown = |v: f64| total > 0.0 && v >= total * 0.01;
    let shown_total: f64 = pie.slices.iter().map(|&(_, v)| v).filter(|&v| shown(v)).sum();
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

    // No visible wedges (all-zero total, or every slice below the 1%
    // threshold): draw an empty outlined circle so the chart area isn't
    // blank.
    if shown_total == 0.0 {
        svg.push_str(&format!(
            r#"<circle cx="{CX}" cy="{CY}" r="{R}" fill="none" stroke="currentColor" stroke-width="1"/>"#
        ));
    }

    let mut angle = -std::f64::consts::FRAC_PI_2; // start at 12 o'clock
    for (i, (label, value)) in pie.slices.iter().enumerate() {
        let frac = if shown(*value) && shown_total > 0.0 { value / shown_total } else { 0.0 };
        let fill = PALETTE[i % PALETTE.len()];
        if frac >= 1.0 {
            // A full-sweep arc's start/end points coincide (especially
            // after `{:.2}` rounding), so the `A` command degenerates to
            // an invisible zero-length path. Emit a full disc instead.
            svg.push_str(&format!(
                r#"<circle cx="{CX}" cy="{CY}" r="{R}" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1"/>"#
            ));
        } else if frac > 0.0 {
            let sweep = frac * std::f64::consts::TAU;
            let (x0, y0) = (CX + R * angle.cos(), CY + R * angle.sin());
            let end = angle + sweep;
            let (x1, y1) = (CX + R * end.cos(), CY + R * end.sin());
            let large_arc = if sweep > std::f64::consts::PI { 1 } else { 0 };
            svg.push_str(&format!(
                r#"<path d="M {CX} {CY} L {x0:.2} {y0:.2} A {R} {R} 0 {large_arc} 1 {x1:.2} {y1:.2} Z" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1"/>"#
            ));
            angle = end;
        }

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
    // Take the integer path only inside i64's exactly-representable
    // range: beyond it the `as` cast saturates and would display
    // i64::MAX instead of the real value (issue #12). f64's Display
    // renders huge whole values correctly on its own.
    if v.fract() == 0.0 && v >= i64::MIN as f64 && v < i64::MAX as f64 {
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

    #[test]
    fn parses_show_data_and_inline_title() {
        let p = parse("pie showData title Pets\n\"A\" : 1").unwrap();
        assert!(p.show_data);
        assert_eq!(p.title.as_deref(), Some("Pets"));
    }

    #[test]
    fn single_slice_pie_renders_full_disc() {
        let p = parse("pie\n\"A\" : 5").unwrap();
        let svg = render_svg(&p);
        // A single 100% slice must render as a visible full disc, not a
        // degenerate zero-length arc.
        assert!(svg.contains("<circle"), "expected a full-disc <circle>, got: {svg}");
    }

    #[test]
    fn full_slice_among_zeros_renders_disc() {
        let p = parse("pie\n\"A\" : 5\n\"B\" : 0\n\"C\" : 0").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains("<circle"), "expected a full-disc <circle>, got: {svg}");
        // Legend still lists all three slices even though only one has a
        // visible wedge.
        assert_eq!(svg.matches("<text x=\"320\"").count(), 3);
    }

    #[test]
    fn all_zero_pie_renders_outlined_circle() {
        let p = parse("pie\n\"A\" : 0\n\"B\" : 0").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains("<circle"), "expected an outlined empty circle, got: {svg}");
        assert!(svg.contains("fill=\"none\""), "zero-total circle must be unfilled, got: {svg}");
    }

    #[test]
    fn sub_one_percent_slice_dropped_from_pie_but_kept_in_legend() {
        // Mermaid parity: a wedge below 1% of the total is not drawn, but
        // it still gets a legend row.
        let p = parse("pie\n\"A\" : 60\n\"B\" : 39.5\n\"C\" : 0.5").unwrap();
        let svg = render_svg(&p);
        assert_eq!(svg.matches("<path").count(), 2, "sub-1% slice C gets no wedge");
        assert_eq!(svg.matches("<text x=\"320\"").count(), 3, "all three slices in the legend");
    }

    #[test]
    fn exactly_one_percent_slice_is_kept() {
        // `(value / total) * 100 >= 1` keeps a slice at exactly 1%.
        let p = parse("pie\n\"A\" : 99\n\"B\" : 1").unwrap();
        let svg = render_svg(&p);
        assert_eq!(svg.matches("<path").count(), 2, "exactly-1% slice B is kept as a wedge");
    }

    #[test]
    fn kept_wedges_renormalize_to_fill_the_circle() {
        // After a sub-1% slice is dropped, the remaining share is computed
        // against the KEPT total — so a lone dominant slice becomes a full
        // disc (frac == 1.0), rendered as a <circle>, not a partial arc.
        let p = parse("pie\n\"Big\" : 199\n\"Tiny\" : 1").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains("<circle"), "Big re-normalizes to a full disc: {svg}");
        assert_eq!(svg.matches("<path").count(), 0, "no partial arc when one wedge fills the circle");
        assert_eq!(svg.matches("<text x=\"320\"").count(), 2, "both slices still in the legend");
    }

    #[test]
    fn parses_standalone_title_line_after_header() {
        let p = parse("pie\ntitle Pets\n\"A\" : 1").unwrap();
        assert_eq!(p.title.as_deref(), Some("Pets"));
        assert_eq!(p.slices.len(), 1);
    }

    #[test]
    fn label_containing_colon_splits_on_last_colon() {
        let p = parse("pie\n\"10:30 standup\" : 2").unwrap();
        assert_eq!(p.slices, vec![("10:30 standup".to_string(), 2.0)]);
    }

    #[test]
    fn duplicate_labels_are_kept_as_separate_slices() {
        let p = parse("pie\n\"A\" : 1\n\"A\" : 2").unwrap();
        assert_eq!(
            p.slices,
            vec![("A".to_string(), 1.0), ("A".to_string(), 2.0)]
        );
        let svg = render_svg(&p);
        assert_eq!(svg.matches("<text x=\"320\"").count(), 2, "both duplicates get a legend row");
    }

    #[test]
    fn unicode_and_emoji_labels_survive_to_svg() {
        let p = parse("pie title Tiere 🐾\n\"🐶 Hünde\" : 3\n\"猫\" : 1").unwrap();
        assert_eq!(p.slices[0].0, "🐶 Hünde");
        let svg = render_svg(&p);
        assert!(svg.contains("🐶 Hünde"), "unicode label must pass through unmangled");
        assert!(svg.contains("猫"));
        assert!(svg.contains("Tiere 🐾"));
    }

    #[test]
    fn infinite_and_nan_values_are_errors() {
        // `"inf".parse::<f64>()` succeeds, so the finiteness guard is the
        // only thing standing between these and the renderer.
        for src in ["pie\n\"A\" : inf", "pie\n\"A\" : -inf", "pie\n\"A\" : NaN"] {
            let err = parse(src).unwrap_err();
            assert_eq!(err.line, Some(2), "source: {src:?}");
        }
    }

    #[test]
    fn error_line_numbers_count_skipped_blank_and_comment_lines() {
        let err = parse("pie\n\n%% a comment\n\"A\" : notanumber").unwrap_err();
        assert_eq!(err.line, Some(4), "line numbers must be physical, not logical");
    }

    #[test]
    fn missing_value_after_colon_is_error_with_line() {
        let err = parse("pie\n\"A\" :").unwrap_err();
        assert_eq!(err.line, Some(2));
    }

    #[test]
    fn svg_escapes_title_markup() {
        let p = parse("pie title <img src=x onerror=alert(1)>\n\"A\" : 1").unwrap();
        let svg = render_svg(&p);
        assert!(!svg.contains("<img"), "raw title markup must not reach the SVG: {svg}");
        assert!(svg.contains("&lt;img"));
    }

    #[test]
    fn escape_xml_escapes_all_specials_without_double_escaping() {
        // `&` must be replaced first; otherwise the `&` produced by the
        // other replacements would itself get re-escaped.
        assert_eq!(escape_xml(r#"<&>""#), "&lt;&amp;&gt;&quot;");
        assert_eq!(escape_xml("a&lt;b"), "a&amp;lt;b");
        assert_eq!(escape_xml("plain"), "plain");
    }

    #[test]
    fn trim_num_formats_whole_and_fractional_values() {
        assert_eq!(trim_num(7.0), "7");
        assert_eq!(trim_num(0.0), "0");
        assert_eq!(trim_num(1.5), "1.5");
    }

    /// Regression: issue #12 — whole values at or beyond 2^63 used to
    /// saturate through the `as i64` cast and display i64::MAX instead
    /// of the actual value.
    #[test]
    fn trim_num_huge_whole_values_do_not_saturate() {
        // 2^63 and 10^19, both exactly representable in f64, both
        // outside i64's range. f64 Display prints the shortest string
        // that round-trips (so 2^63 shows as ...6000, the same f64) —
        // the contract is "the real magnitude, not i64::MAX".
        assert_eq!(trim_num(9223372036854775808.0), "9223372036854776000");
        assert_eq!(trim_num(1e19), "10000000000000000000");
    }

    #[test]
    fn show_data_legend_shows_label_with_value() {
        let p = parse("pie showData\n\"A\" : 1.5\n\"B\" : 2").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains("A (1.5)"), "fractional value kept as-is: {svg}");
        assert!(svg.contains("B (2)"), "whole value loses trailing .0: {svg}");
    }

    #[test]
    fn more_slices_than_palette_entries_render_with_cycled_fills() {
        let src = std::iter::once("pie".to_string())
            .chain((0..10).map(|i| format!("\"S{i}\" : 1")))
            .collect::<Vec<_>>()
            .join("\n");
        let p = parse(&src).unwrap();
        let svg = render_svg(&p);
        assert_eq!(svg.matches("<text x=\"320\"").count(), 10, "one legend row per slice");
        // Slice 8 wraps around to the first palette color.
        assert_eq!(
            svg.matches(PALETTE[0]).count(),
            4,
            "slices 0 and 8 each use palette[0] for wedge fill + legend swatch"
        );
    }
}
