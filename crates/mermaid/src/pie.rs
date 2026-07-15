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

/// Mermaid's default-theme pie section palette (`pie1`..`pie12`), in order.
/// SVG accepts `hsl(...)` fills directly, so the theme values are used verbatim.
const PALETTE: &[&str] = &[
    "#ECECFF",
    "#ffffde",
    "hsl(80, 100%, 56.2745098039%)",
    "hsl(240, 100%, 86.2745098039%)",
    "hsl(60, 100%, 86.2745098039%)",
    "hsl(80, 100%, 86.2745098039%)",
    "hsl(120, 100%, 86.2745098039%)",
    "hsl(300, 100%, 86.2745098039%)",
    "hsl(0, 100%, 86.2745098039%)",
    "hsl(150, 100%, 86.2745098039%)",
    "hsl(0, 100%, 86.2745098039%)",
    "hsl(210, 100%, 86.2745098039%)",
];
const MARGIN: f64 = 40.0;
const SIZE: f64 = 450.0; // square viewport (mermaid: height = pieWidth = 450)
const CX: f64 = SIZE / 2.0; // pie group translated to (pieWidth/2, height/2)
const CY: f64 = SIZE / 2.0;
const R: f64 = SIZE / 2.0 - MARGIN; // 185
const TEXT_POS: f64 = 0.75; // pie.textPosition: label radius = R * 0.75
const LEGEND_H: f64 = 22.0; // legendRectSize(18) + legendSpacing(4)
const LEGEND_X: f64 = CX + 12.0 * 18.0; // horizontal = 12 * legendRectSize = 216

/// Render the pie as a self-contained SVG string, matching mermaid.js@11's
/// `pieRenderer` constants: 450×450 viewport, radius 185, 2px black slice +
/// outer strokes, `pieOpacity` 0.7, labels at 0.75·R, center-anchored legend.
pub(crate) fn render_svg(pie: &Pie) -> String {
    let total: f64 = pie.slices.iter().map(|(_, v)| *v).sum();
    // A wedge is only drawn when it's at least 1% of the total; kept wedges
    // re-normalize to fill the circle. The legend still lists every slice.
    let shown = |v: f64| total > 0.0 && v >= total * 0.01;
    let shown_total: f64 = pie.slices.iter().map(|&(_, v)| v).filter(|&v| shown(v)).sum();
    let n = pie.slices.len();
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE:.0} {SIZE:.0}" width="{SIZE:.0}" height="{SIZE:.0}" style="max-width:{SIZE:.0}px;width:100%;font-family:sans-serif">"#
    );

    // Title: centered over the pie, 25px, normal weight; y = -(height-50)/2.
    if let Some(title) = &pie.title {
        svg.push_str(&format!(
            r#"<text x="{CX}" y="{ty:.0}" text-anchor="middle" fill="currentColor" style="font-size:25px">{t}</text>"#,
            ty = CY - (SIZE - 50.0) / 2.0,
            t = escape_xml(title),
        ));
    }

    // All-zero (or all-sub-1%): an empty outlined circle so the area isn't blank.
    if shown_total == 0.0 {
        svg.push_str(&format!(
            r#"<circle cx="{CX}" cy="{CY}" r="{R}" fill="none" stroke="black" stroke-width="2"/>"#
        ));
    }

    // Slices are laid out in DESCENDING value order (d3.pie default sort), but
    // each keeps its SOURCE-order palette color. The legend (below) stays in
    // source order — the two can legitimately disagree.
    let mut order: Vec<usize> = (0..n).filter(|&i| shown(pie.slices[i].1)).collect();
    order.sort_by(|&a, &b| {
        pie.slices[b].1.partial_cmp(&pie.slices[a].1).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut angle = -std::f64::consts::FRAC_PI_2; // 12 o'clock
    for &i in &order {
        let value = pie.slices[i].1;
        let frac = value / shown_total;
        let fill = PALETTE[i % PALETTE.len()];
        let pct = (value / total * 100.0).round() as i64;
        let slice_attrs = format!(r#"fill="{fill}" fill-opacity="0.7" stroke="black" stroke-width="2""#);
        if frac >= 1.0 {
            // A lone full wedge: a zero-length arc is invisible, so draw a disc.
            svg.push_str(&format!(
                r#"<circle cx="{CX}" cy="{CY}" r="{R}" {slice_attrs}/>"#
            ));
            svg.push_str(&slice_label(CX, CY, pct));
        } else {
            let sweep = frac * std::f64::consts::TAU;
            let (x0, y0) = (CX + R * angle.cos(), CY + R * angle.sin());
            let end = angle + sweep;
            let (x1, y1) = (CX + R * end.cos(), CY + R * end.sin());
            let large_arc = if sweep > std::f64::consts::PI { 1 } else { 0 };
            svg.push_str(&format!(
                r#"<path d="M {CX} {CY} L {x0:.2} {y0:.2} A {R} {R} 0 {large_arc} 1 {x1:.2} {y1:.2} Z" {slice_attrs}/>"#
            ));
            // Label on a dedicated arc of radius R·textPosition (both radii
            // equal), so labels sit further out and thin wedges overflow.
            let mid = angle + sweep / 2.0;
            let (lx, ly) = (CX + TEXT_POS * R * mid.cos(), CY + TEXT_POS * R * mid.sin());
            svg.push_str(&slice_label(lx, ly, pct));
            angle = end;
        }
    }

    // Distinct outer rim on top of the slices.
    if shown_total > 0.0 {
        svg.push_str(&format!(
            r#"<circle cx="{CX}" cy="{CY}" r="{R}" fill="none" stroke="black" stroke-width="2"/>"#
        ));
    }

    // Legend, source order, anchored to the pie center: swatch 18×18 at
    // (CX+216, CY + i·22 - 11n); text offset by (22, 14).
    let offset = LEGEND_H * n as f64 / 2.0;
    for (i, (label, value)) in pie.slices.iter().enumerate() {
        let fill = PALETTE[i % PALETTE.len()];
        let sy = CY + i as f64 * LEGEND_H - offset;
        svg.push_str(&format!(
            r#"<rect x="{LEGEND_X}" y="{sy:.1}" width="18" height="18" fill="{fill}"/>"#
        ));
        let legend = if pie.show_data {
            format!("{label} ({})", trim_num(*value))
        } else {
            label.clone()
        };
        svg.push_str(&format!(
            r#"<text x="{lx}" y="{ly:.1}" fill="currentColor" style="font-size:17px">{}</text>"#,
            escape_xml(&legend),
            lx = LEGEND_X + 22.0,
            ly = sy + 14.0,
        ));
    }

    svg.push_str("</svg>");
    svg
}

/// A centered percentage label for a wedge (17px, theme text color).
fn slice_label(x: f64, y: f64, pct: i64) -> String {
    format!(
        r#"<text x="{x:.1}" y="{y:.1}" text-anchor="middle" dominant-baseline="middle" fill="currentColor" style="font-size:17px">{pct}%</text>"#
    )
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
    fn wedges_get_percentage_labels() {
        // Three slices of the total 4 → 25% / 25% / 50%. Every drawn wedge
        // carries a percentage label (Mermaid parity).
        let p = parse("pie\n\"A\" : 1\n\"B\" : 1\n\"C\" : 2").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains(">25%<"), "quarter slices labeled 25%: {svg}");
        assert!(svg.contains(">50%<"), "half slice labeled 50%: {svg}");
        // Count label endings (`hsl(...)` fills also contain `%`).
        assert_eq!(svg.matches("%</text>").count(), 3, "one percentage label per drawn wedge");
    }

    #[test]
    fn slices_descending_but_legend_source_order() {
        // Defect 7: d3.pie lays slices out in descending value order, while the
        // legend follows source order — these can disagree.
        let p = parse("pie title Ordering check\n\"Small\" : 15\n\"Large\" : 386\n\"Medium\" : 85")
            .unwrap();
        let svg = render_svg(&p);
        // Slices drawn largest→smallest: 79% (Large) then 17% (Medium) then 3% (Small).
        let (p79, p17, p3) =
            (svg.find(">79%<").unwrap(), svg.find(">17%<").unwrap(), svg.find(">3%<").unwrap());
        assert!(p79 < p17 && p17 < p3, "slices not in descending value order");
        // Legend in SOURCE order: Small, Large, Medium.
        let (sm, lg, md) =
            (svg.find(">Small<").unwrap(), svg.find(">Large<").unwrap(), svg.find(">Medium<").unwrap());
        assert!(sm < lg && lg < md, "legend not in source order");
    }

    #[test]
    fn slice_has_black_stroke_and_outer_circle() {
        // Defect 2: 2px black slice strokes + a distinct fill:none outer rim.
        let svg = render_svg(&parse("pie\n\"A\" : 3\n\"B\" : 1").unwrap());
        assert!(svg.contains(r#"stroke="black" stroke-width="2""#), "slice stroke: {svg}");
        assert!(svg.contains(r#"fill-opacity="0.7""#), "pie opacity: {svg}");
        assert!(
            svg.contains(r#"r="185" fill="none" stroke="black" stroke-width="2""#),
            "outer rim circle: {svg}"
        );
    }

    #[test]
    fn full_disc_slice_gets_100_percent_label() {
        let p = parse("pie\n\"A\" : 5").unwrap();
        let svg = render_svg(&p);
        assert!(svg.contains(">100%<"), "single slice labeled 100%: {svg}");
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
        assert_eq!(svg.matches("<text x=\"463\"").count(), 3);
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
        assert_eq!(svg.matches("<text x=\"463\"").count(), 3, "all three slices in the legend");
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
        assert_eq!(svg.matches("<text x=\"463\"").count(), 2, "both slices still in the legend");
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
        assert_eq!(svg.matches("<text x=\"463\"").count(), 2, "both duplicates get a legend row");
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
        // Use more slices than the palette has entries so cycling wraps.
        let n = PALETTE.len() + 2;
        let src = std::iter::once("pie".to_string())
            .chain((0..n).map(|i| format!("\"S{i}\" : 1")))
            .collect::<Vec<_>>()
            .join("\n");
        let p = parse(&src).unwrap();
        let svg = render_svg(&p);
        assert_eq!(svg.matches("<text x=\"463\"").count(), n, "one legend row per slice");
        // Slice `PALETTE.len()` wraps around to the first palette color, so
        // palette[0] is used by two slices (wedge fill + legend swatch each).
        assert_eq!(
            svg.matches(PALETTE[0]).count(),
            4,
            "slices 0 and PALETTE.len() each use palette[0] for wedge fill + legend swatch"
        );
    }
}
