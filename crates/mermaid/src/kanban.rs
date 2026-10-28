//! Mermaid `kanban` diagrams: parser + SVG renderer (Tier 3).
//!
//! Indentation-structured: top-level lines are columns, more-indented lines
//! are the cards in the column above them. Both accept an optional
//! `id[Display text]` form (the display text is used; a bare word is its own
//! label). Rendered as columns of stacked card boxes under a column header.

use crate::{escape_xml, measure, ParseError};
use unicode_segmentation::UnicodeSegmentation;

const PAD: f64 = 20.0;
const COL_GAP: f64 = 16.0;
const CARD_GAP: f64 = 10.0;
const INNER: f64 = 12.0;
const HEADER_H: f64 = 34.0;
const MIN_COL_W: f64 = 130.0;
const MAX_CARD_W: f64 = 240.0;
const MAX_ITEMS: usize = 1000;
/// Per-column pastel palette (Mermaid tints each column a distinct hue).
const COLUMN_COLORS: &[&str] = &[
    "#a8e05a", "#c99be0", "#e05ad1", "#5ad1e0", "#e0a85a", "#8a8ae0", "#5ae0a8", "#e05a5a",
];

#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub title: String,
    pub cards: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Kanban {
    pub columns: Vec<Column>,
}

pub(crate) fn parse(source: &str) -> Result<Kanban, ParseError> {
    let mut columns: Vec<Column> = Vec::new();
    let mut base_indent: Option<usize> = None; // indent of the column level
    let mut seen_header = false;
    let mut items = 0usize;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        if raw.trim().is_empty() || raw.trim_start().starts_with("%%") {
            continue;
        }
        if !seen_header {
            if raw
                .trim()
                .strip_suffix(';')
                .unwrap_or(raw.trim())
                .trim_end()
                != "kanban"
            {
                return Err(err("kanban diagram must start with `kanban`", line_no));
            }
            seen_header = true;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let text = label_of(raw.trim(), line_no)?;
        if text.is_empty() {
            continue;
        }
        items += 1;
        if items > MAX_ITEMS {
            return Err(err(
                format!("kanban too large: more than {MAX_ITEMS} items"),
                line_no,
            ));
        }
        match base_indent {
            // First content line sets the column-indent level.
            None => {
                base_indent = Some(indent);
                columns.push(Column {
                    title: text,
                    cards: Vec::new(),
                });
            }
            Some(base) if indent <= base => {
                columns.push(Column {
                    title: text,
                    cards: Vec::new(),
                });
            }
            Some(_) => match columns.last_mut() {
                Some(col) => col.cards.push(text),
                None => columns.push(Column {
                    title: text,
                    cards: Vec::new(),
                }),
            },
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "kanban diagram must start with `kanban`".into(),
            line: Some(1),
        });
    }
    Ok(Kanban { columns })
}

/// An apostrophe is a quote boundary only when it closes the outer label.
/// Internal apostrophes and a leading apostrophe without a closing pair remain
/// literal text. Backslash escaping retains the scanner's existing meaning.
fn outer_single_quote_end(source: &str, close: Option<char>) -> Option<usize> {
    let mut escaped = false;
    for (at, ch) in source.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch != '\'' {
            continue;
        }
        let tail = source[at + 1..].trim_start();
        if close.map_or(tail.is_empty() || tail.starts_with("@{"), |end| {
            tail.starts_with(end)
        }) {
            return Some(at);
        }
    }
    None
}

/// A card has one optional outer label. Quotes protect literal delimiters;
/// every token following the matching close must be handled or rejected.
fn label_of(source: &str, line: usize) -> Result<String, ParseError> {
    let source = source.trim();
    let (mut quote, mut escaped) = (None, false);
    let mut single_end = None;
    let mut opening = None;
    for (index, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote.is_some() && ch == '\\' {
            escaped = true;
            continue;
        }
        if quote == Some(ch) && (ch != '\'' || single_end == Some(index)) {
            quote = None;
            continue;
        }
        if quote.is_none() {
            if ch == '\'' && source[..index].trim().is_empty() {
                single_end = outer_single_quote_end(&source[index..], None).map(|end| index + end);
            }
            if ch == '"' || (ch == '\'' && single_end.is_some_and(|end| end > index)) {
                quote = Some(ch);
                continue;
            }
        }
        if quote.is_some() {
            continue;
        }
        if ch == '@' && source[index..].starts_with("@{") {
            return Err(err("kanban card metadata is not supported", line));
        }
        if matches!(ch, '[' | '(') {
            opening = Some((index, ch));
            break;
        }
    }
    let Some((start, open)) = opening else {
        if quote.is_some() {
            return Err(err("unclosed quoted kanban label", line));
        }
        return Ok(crate::unquote_label(source).to_string());
    };
    let id = &source[..start];
    if !id
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-'))
    {
        // Shape labels require an adjacent identifier. Spaced prose such as
        // `Review [PR 12]` is a complete literal card/column caption.
        return Ok(crate::unquote_label(source).to_string());
    }
    let close = if open == '[' { ']' } else { ')' };
    let mut depth = 1;
    let (mut quote, mut escaped) = (None, false);
    let mut single_end = None;
    for (offset, ch) in source[start + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote.is_some() && ch == '\\' {
            escaped = true;
            continue;
        }
        if quote == Some(ch) && (ch != '\'' || single_end == Some(start + 1 + offset)) {
            quote = None;
            continue;
        }
        if quote.is_none() {
            let at = start + 1 + offset;
            if ch == '\'' && source[start + 1..at].trim().is_empty() {
                single_end = outer_single_quote_end(&source[at..], Some(close)).map(|end| at + end);
            }
            if ch == '"' || (ch == '\'' && single_end.is_some_and(|end| end > at)) {
                quote = Some(ch);
                continue;
            }
        }
        if quote.is_some() {
            continue;
        }
        if ch == open {
            depth += 1;
        }
        if ch != close {
            continue;
        }
        depth -= 1;
        if depth != 0 {
            continue;
        }
        let end = start + 1 + offset;
        let remaining = source[end + 1..].trim();
        if remaining.starts_with("@{") {
            return Err(err("kanban card metadata is not supported", line));
        }
        if !remaining.is_empty() {
            return Err(err("unsupported text after kanban label", line));
        }
        return Ok(crate::unquote_label(source[start + 1..end].trim()).to_string());
    }
    Err(err("unclosed kanban label", line))
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError {
        message: message.into(),
        line: Some(line),
    }
}

pub(crate) fn render_svg(k: &Kanban) -> String {
    // Ordinary labels wrap within the cap. An indivisible grapheme can have
    // many advancing letters, so its column must reserve that complete width.
    let inset = 8.0;
    let widest_grapheme = |text: &str, bold: bool| {
        text.graphemes(true)
            .map(|g| measure::literal_overlap_width(g, 14.0) * if bold { 1.04 } else { 1.0 })
            .fold(0.0_f64, f64::max)
    };
    let col_w: Vec<f64> = k
        .columns
        .iter()
        .map(|c| {
            let hw = measure::literal_overlap_width(&c.title, 14.0) * 1.04;
            let cw = c
                .cards
                .iter()
                .map(|t| measure::literal_overlap_width(t, 14.0))
                .fold(0.0_f64, f64::max);
            let normal = (hw.max(cw) + 2.0 * INNER + 14.0).clamp(MIN_COL_W, MAX_CARD_W);
            let header_min = widest_grapheme(&c.title, true) + 2.0 * INNER;
            let card_min = c
                .cards
                .iter()
                .map(|text| widest_grapheme(text, false) + 2.0 * inset + 20.0)
                .fold(0.0_f64, f64::max);
            normal.max(header_min).max(card_min)
        })
        .collect();

    let mut lefts = Vec::with_capacity(col_w.len());
    let mut x = PAD;
    for w in &col_w {
        lefts.push(x);
        x += w + COL_GAP;
    }
    let total_w = (x - COL_GAP + PAD).max(2.0 * PAD + MIN_COL_W);

    let mut body = String::new();
    let mut max_bottom = PAD + HEADER_H;

    for (i, c) in k.columns.iter().enumerate() {
        let (l, w) = (lefts[i], col_w[i]);
        let color = COLUMN_COLORS[i % COLUMN_COLORS.len()];
        let titles = measure::wrap_literal_to_width(&c.title, w - 2.0 * INNER, 14.0, true);
        let header_h = HEADER_H.max(titles.len() as f64 * measure::LINE_H + 14.0);
        let cards: Vec<Vec<String>> = c
            .cards
            .iter()
            .map(|s| measure::wrap_literal_to_width(s, w - 2.0 * inset - 20.0, 14.0, false))
            .collect();
        let card_heights: Vec<f64> = cards
            .iter()
            .map(|s| s.len() as f64 * measure::LINE_H + 14.0)
            .collect();
        let n = cards.len();
        // Column body height: header + inset-padded cards.
        let body_h = if n == 0 {
            header_h + inset
        } else {
            header_h + inset + card_heights.iter().sum::<f64>() + (n - 1) as f64 * CARD_GAP + inset
        };
        // Colored column container (behind), then a tinted header band.
        body.push_str(&format!(
            r#"<rect x="{l:.1}" y="{:.1}" width="{w:.1}" height="{body_h:.1}" fill="{color}" fill-opacity="0.3" stroke="{color}" rx="6"/>"#,
            PAD
        ));
        body.push_str(&format!(
            r#"<rect x="{l:.1}" y="{:.1}" width="{w:.1}" height="{header_h:.1}" fill="{color}" fill-opacity="0.85" rx="6"/>"#,
            PAD
        ));
        emit_lines(&mut body, &titles, l + w / 2.0, PAD + header_h / 2.0, true);
        // White cards inside the column, left-aligned text with a color accent.
        let (cx, cw) = (l + inset, w - 2.0 * inset);
        let mut cy = PAD + header_h + inset;
        for (card, &card_h) in cards.iter().zip(&card_heights) {
            body.push_str(&format!(
                r#"<rect x="{cx:.1}" y="{cy:.1}" width="{cw:.1}" height="{card_h:.1}" fill="var(--surface, #fff)" stroke="{color}" rx="3"/>"#
            ));
            body.push_str(&format!(
                r#"<rect x="{cx:.1}" y="{cy:.1}" width="3" height="{card_h:.1}" fill="{color}"/>"#
            ));
            emit_lines(&mut body, card, cx + 10.0, cy + card_h / 2.0, false);
            cy += card_h + CARD_GAP;
        }
        max_bottom = max_bottom.max(PAD + body_h);
    }

    let total_h = max_bottom + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

fn emit_lines(out: &mut String, lines: &[String], x: f64, cy: f64, header: bool) {
    let first = cy - (lines.len().saturating_sub(1) as f64 * measure::LINE_H) / 2.0 + 5.0;
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&format!(
            r#"<text x="{x:.1}" y="{:.1}" {} fill="currentColor">{}</text>"#,
            first + i as f64 * measure::LINE_H,
            if header {
                "text-anchor=\"middle\" font-weight=\"600\""
            } else {
                ""
            },
            escape_xml(line)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_cards_by_indent() {
        let k = parse(
            "kanban\n  Todo\n    Create docs\n    task[Write blog]\n  In Progress\n    id6[Build renderer]\n  Done",
        )
        .unwrap();
        assert_eq!(k.columns.len(), 3);
        assert_eq!(k.columns[0].title, "Todo");
        assert_eq!(
            k.columns[0].cards,
            vec!["Create docs".to_string(), "Write blog".to_string()]
        );
        assert_eq!(k.columns[1].cards, vec!["Build renderer".to_string()]);
        assert!(k.columns[2].cards.is_empty()); // empty column
    }

    #[test]
    fn id_bracket_form_uses_display_text() {
        let k = parse("kanban\n col[My Column]\n  card1[Do the thing]").unwrap();
        assert_eq!(k.columns[0].title, "My Column");
        assert_eq!(k.columns[0].cards, vec!["Do the thing".to_string()]);
    }

    #[test]
    fn header_required() {
        assert!(parse("Todo\n  card").is_err());
    }

    #[test]
    fn renders_columns_and_cards() {
        let svg = render_svg(&parse("kanban\n Todo\n  A\n  B\n Done\n  C").unwrap());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">Todo<") && svg.contains(">Done<"));
        assert!(svg.contains(">A<") && svg.contains(">B<") && svg.contains(">C<"));
    }

    #[test]
    fn columns_colored_with_white_left_aligned_cards() {
        let svg = render_svg(&parse("kanban\n Todo\n  A\n In Progress\n  B").unwrap());
        // Two columns use distinct palette colors (container + header).
        assert!(
            svg.contains(COLUMN_COLORS[0]) && svg.contains(COLUMN_COLORS[1]),
            "column colors: {svg}"
        );
        // Cards are white (the surface fill), not the old lavender node-fill.
        assert!(
            svg.contains(r#"fill="var(--surface, #fff)""#),
            "white cards: {svg}"
        );
        assert!(
            !svg.contains("--mermaid-node-fill"),
            "no lavender cards: {svg}"
        );
        // A tinted column container behind the header (fill-opacity).
        assert!(
            svg.contains(r#"fill-opacity="0.3""#),
            "column container: {svg}"
        );
    }
}
