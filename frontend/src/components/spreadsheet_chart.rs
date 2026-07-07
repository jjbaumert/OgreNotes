// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SVG chart rendering for bar, line, and pie charts.

use crate::spreadsheet::eval::{ChartConfig, ChartType, SpreadsheetEngine};

const CHART_WIDTH: f64 = 400.0;
const CHART_HEIGHT: f64 = 250.0;
const PADDING: f64 = 40.0;
const COLORS: &[&str] = &["#2D5F2D", "#5C3D2E", "#4A90D9", "#D9534F", "#F0AD4E", "#5CB85C", "#9B59B6", "#E67E22"];

/// Render a chart as an SVG string using data from the engine.
pub fn render_chart_svg(chart: &ChartConfig, engine: &SpreadsheetEngine) -> String {
    let values = extract_chart_data(chart, engine);
    if values.is_empty() {
        return String::new();
    }

    match chart.chart_type {
        ChartType::Bar => render_bar(&chart.title, &values),
        ChartType::Line => render_line(&chart.title, &values),
        ChartType::Pie => render_pie(&chart.title, &values),
    }
}

/// Extract (label, value) pairs from the chart's data range.
fn extract_chart_data(chart: &ChartConfig, engine: &SpreadsheetEngine) -> Vec<(String, f64)> {
    let ((c1, r1), (c2, r2)) = chart.data_range;
    let mut data = Vec::new();

    if c2 > c1 {
        // Multiple columns: first column = labels, second column = values
        for r in r1..=r2 {
            let label = engine.get_display((c1, r));
            let value = engine.get_value((c2.min(c1 + 1), r)).as_number().unwrap_or(0.0);
            data.push((label, value));
        }
    } else {
        // Single column: row index = label, cell = value
        for r in r1..=r2 {
            let label = format!("{}", r + 1);
            let value = engine.get_value((c1, r)).as_number().unwrap_or(0.0);
            data.push((label, value));
        }
    }
    data
}

fn render_bar(title: &str, data: &[(String, f64)]) -> String {
    let n = data.len();
    if n == 0 { return String::new(); }

    let max_val = data.iter().map(|(_, v)| *v).fold(0.0f64, f64::max).max(1.0);
    let plot_w = CHART_WIDTH - 2.0 * PADDING;
    let plot_h = CHART_HEIGHT - 2.0 * PADDING - 20.0; // room for title
    let bar_w = (plot_w / n as f64) * 0.7;
    let gap = (plot_w / n as f64) * 0.3;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{CHART_WIDTH}" height="{CHART_HEIGHT}" style="font-family:sans-serif;font-size:11px">"#
    );

    // Title
    svg.push_str(&format!(
        r#"<text x="{}" y="18" text-anchor="middle" font-weight="bold" font-size="13">{}</text>"#,
        CHART_WIDTH / 2.0, escape_xml(title)
    ));

    // Bars
    for (i, (label, value)) in data.iter().enumerate() {
        let x = PADDING + i as f64 * (bar_w + gap) + gap / 2.0;
        let h = (value / max_val) * plot_h;
        let y = PADDING + 20.0 + (plot_h - h);
        let color = COLORS[i % COLORS.len()];

        svg.push_str(&format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{bar_w:.1}" height="{h:.1}" fill="{color}" rx="2"/>"#
        ));

        // Value label
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="10">{}</text>"#,
            x + bar_w / 2.0, y - 4.0, format_val(*value)
        ));

        // X-axis label
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="10">{}</text>"#,
            x + bar_w / 2.0, CHART_HEIGHT - 5.0, escape_xml(&truncate_label(label, 8))
        ));
    }

    // Axes
    let ax_y = PADDING + 20.0 + plot_h;
    svg.push_str(&format!(
        "<line x1=\"{PADDING}\" y1=\"{ax_y}\" x2=\"{}\" y2=\"{ax_y}\" stroke=\"rgb(204,204,204)\"/>",
        CHART_WIDTH - PADDING
    ));

    svg.push_str("</svg>");
    svg
}

fn render_line(title: &str, data: &[(String, f64)]) -> String {
    let n = data.len();
    if n < 2 { return render_bar(title, data); } // fallback for single point

    let max_val = data.iter().map(|(_, v)| *v).fold(0.0f64, f64::max).max(1.0);
    let min_val = data.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
    let range = (max_val - min_val).max(1.0);
    let plot_w = CHART_WIDTH - 2.0 * PADDING;
    let plot_h = CHART_HEIGHT - 2.0 * PADDING - 20.0;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{CHART_WIDTH}" height="{CHART_HEIGHT}" style="font-family:sans-serif;font-size:11px">"#
    );

    // Title
    svg.push_str(&format!(
        r#"<text x="{}" y="18" text-anchor="middle" font-weight="bold" font-size="13">{}</text>"#,
        CHART_WIDTH / 2.0, escape_xml(title)
    ));

    // Grid lines
    for i in 0..5 {
        let y = PADDING + 20.0 + (i as f64 / 4.0) * plot_h;
        svg.push_str(&format!(
            "<line x1=\"{PADDING}\" y1=\"{y:.1}\" x2=\"{}\" y2=\"{y:.1}\" stroke=\"rgb(238,238,238)\"/>",
            CHART_WIDTH - PADDING
        ));
    }

    // Points and line
    let points: Vec<(f64, f64)> = data.iter().enumerate().map(|(i, (_, v))| {
        let x = PADDING + (i as f64 / (n - 1) as f64) * plot_w;
        let y = PADDING + 20.0 + (1.0 - (v - min_val) / range) * plot_h;
        (x, y)
    }).collect();

    let polyline: String = points.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect::<Vec<_>>().join(" ");
    svg.push_str(&format!(
        r#"<polyline points="{polyline}" fill="none" stroke="{}" stroke-width="2"/>"#,
        COLORS[0]
    ));

    for (i, (x, y)) in points.iter().enumerate() {
        svg.push_str(&format!(
            r#"<circle cx="{x:.1}" cy="{y:.1}" r="3.5" fill="{}" stroke="white" stroke-width="1.5"/>"#,
            COLORS[0]
        ));
        // X-axis label
        svg.push_str(&format!(
            r#"<text x="{x:.1}" y="{:.1}" text-anchor="middle" font-size="10">{}</text>"#,
            CHART_HEIGHT - 5.0, escape_xml(&truncate_label(&data[i].0, 8))
        ));
    }

    svg.push_str("</svg>");
    svg
}

fn render_pie(title: &str, data: &[(String, f64)]) -> String {
    let total: f64 = data.iter().map(|(_, v)| v.max(0.0)).sum();
    if total <= 0.0 { return String::new(); }

    let cx = CHART_WIDTH / 2.0;
    let cy = CHART_HEIGHT / 2.0 + 10.0;
    let radius = (CHART_WIDTH.min(CHART_HEIGHT) / 2.0 - PADDING).min(100.0);

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{CHART_WIDTH}" height="{CHART_HEIGHT}" style="font-family:sans-serif;font-size:11px">"#
    );

    // Title
    svg.push_str(&format!(
        r#"<text x="{}" y="18" text-anchor="middle" font-weight="bold" font-size="13">{}</text>"#,
        CHART_WIDTH / 2.0, escape_xml(title)
    ));

    let mut start_angle = -std::f64::consts::FRAC_PI_2; // start at top
    for (i, (label, value)) in data.iter().enumerate() {
        let fraction = value.max(0.0) / total;
        let sweep = fraction * 2.0 * std::f64::consts::PI;
        let end_angle = start_angle + sweep;

        let x1 = cx + radius * start_angle.cos();
        let y1 = cy + radius * start_angle.sin();
        let x2 = cx + radius * end_angle.cos();
        let y2 = cy + radius * end_angle.sin();
        let large_arc = if sweep > std::f64::consts::PI { 1 } else { 0 };
        let color = COLORS[i % COLORS.len()];

        svg.push_str(&format!(
            r#"<path d="M {cx:.1} {cy:.1} L {x1:.1} {y1:.1} A {radius:.1} {radius:.1} 0 {large_arc} 1 {x2:.1} {y2:.1} Z" fill="{color}" stroke="white" stroke-width="1"/>"#
        ));

        // Label
        let mid_angle = start_angle + sweep / 2.0;
        let lx = cx + (radius + 15.0) * mid_angle.cos();
        let ly = cy + (radius + 15.0) * mid_angle.sin();
        let pct = (fraction * 100.0).round() as u32;
        if pct >= 3 {
            svg.push_str(&format!(
                r#"<text x="{lx:.1}" y="{ly:.1}" text-anchor="middle" font-size="10">{} {pct}%</text>"#,
                escape_xml(&truncate_label(label, 6))
            ));
        }

        start_angle = end_angle;
    }

    svg.push_str("</svg>");
    svg
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() }
    else { format!("{}..", s.chars().take(max - 2).collect::<String>()) }
}

fn format_val(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e10 { format!("{}", v as i64) }
    else { format!("{:.1}", v) }
}

#[cfg(test)]
mod tests {
    //! Regression tests for SVG XSS via cell-derived chart labels.
    //! The rendered SVG is injected via `inner_html`, so every user-supplied
    //! string must be XML-escaped before insertion.

    use super::*;
    use crate::spreadsheet::eval::SpreadsheetEngine;

    fn engine_with(cells: &[((usize, usize), &str)]) -> SpreadsheetEngine {
        let mut e = SpreadsheetEngine::new();
        for (addr, val) in cells {
            e.set_cell(*addr, val);
        }
        e
    }

    #[test]
    fn bar_escapes_malicious_label() {
        // Label is truncated to 8 chars for bar axes, so the tail of the
        // payload gets dropped — but whatever survives must be escaped.
        let eng = engine_with(&[
            ((0, 0), "</text><script>alert(1)</script>"),
            ((1, 0), "42"),
        ]);
        let chart = ChartConfig {
            chart_type: ChartType::Bar,
            data_range: ((0, 0), (1, 0)),
            title: "safe".into(),
        };
        let svg = render_chart_svg(&chart, &eng);
        assert!(!svg.contains("<script>"), "unescaped <script> leaked into SVG: {svg}");
        // Raw `</text>` prefix (from the first 7 chars of the label) must be
        // escaped; if truncation cut before the `>` we'd see `&lt;/text`.
        assert!(svg.contains("&lt;/text"), "axis label not XML-escaped: {svg}");
    }

    #[test]
    fn pie_escapes_malicious_label() {
        let eng = engine_with(&[
            ((0, 0), "<img src=x onerror=alert(1)>"),
            ((1, 0), "10"),
            ((0, 1), "ok"),
            ((1, 1), "20"),
        ]);
        let chart = ChartConfig {
            chart_type: ChartType::Pie,
            data_range: ((0, 0), (1, 1)),
            title: "t".into(),
        };
        let svg = render_chart_svg(&chart, &eng);
        assert!(!svg.contains("<img "), "unescaped <img> leaked into SVG: {svg}");
    }

    #[test]
    fn line_escapes_malicious_label() {
        let eng = engine_with(&[
            ((0, 0), "</text>&<x>"),
            ((1, 0), "1"),
            ((0, 1), "</text>"),
            ((1, 1), "2"),
        ]);
        let chart = ChartConfig {
            chart_type: ChartType::Line,
            data_range: ((0, 0), (1, 1)),
            title: "t".into(),
        };
        let svg = render_chart_svg(&chart, &eng);
        // Axis labels are truncated to 8 chars so the full payload may not
        // appear; but any `</text>` that *does* appear must be escaped.
        let after_title = svg
            .split_once("text-anchor=\"middle\" font-weight=\"bold\"")
            .map(|(_, rest)| rest)
            .unwrap_or(&svg);
        assert!(!after_title.contains("</text>&"), "axis label not escaped: {svg}");
    }
}
