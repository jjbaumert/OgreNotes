//! Mermaid `kanban` diagrams: parser + SVG renderer (Tier 3).
//!
//! Indentation-structured: top-level lines are columns, more-indented lines
//! are the cards in the column above them. Both accept an optional
//! `id[Display text]` form (the display text is used; a bare word is its own
//! label). Rendered as columns of stacked card boxes under a column header.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const COL_GAP: f64 = 16.0;
const CARD_GAP: f64 = 10.0;
const INNER: f64 = 12.0;
const HEADER_H: f64 = 34.0;
const MIN_COL_W: f64 = 130.0;
const MAX_CARD_W: f64 = 240.0;
const MAX_ITEMS: usize = 1000;

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
            if raw.trim().strip_suffix(';').unwrap_or(raw.trim()).trim_end() != "kanban" {
                return Err(err("kanban diagram must start with `kanban`", line_no));
            }
            seen_header = true;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let text = label_of(raw.trim());
        if text.is_empty() {
            continue;
        }
        items += 1;
        if items > MAX_ITEMS {
            return Err(err(format!("kanban too large: more than {MAX_ITEMS} items"), line_no));
        }
        match base_indent {
            // First content line sets the column-indent level.
            None => {
                base_indent = Some(indent);
                columns.push(Column { title: text, cards: Vec::new() });
            }
            Some(base) if indent <= base => {
                columns.push(Column { title: text, cards: Vec::new() });
            }
            Some(_) => match columns.last_mut() {
                Some(col) => col.cards.push(text),
                None => columns.push(Column { title: text, cards: Vec::new() }),
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

/// `id[Display text]` → `Display text`; `id(text)` → `text`; else the trimmed
/// token itself.
fn label_of(s: &str) -> String {
    let s = s.trim();
    for (open, close) in [('[', ']'), ('(', ')')] {
        if let Some(o) = s.find(open) {
            if let Some(c) = s.rfind(close) {
                if c > o {
                    return s[o + 1..c].trim().to_string();
                }
            }
        }
    }
    s.to_string()
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(k: &Kanban) -> String {
    // Column widths: the wider of the header and its widest card, capped.
    let col_w: Vec<f64> = k
        .columns
        .iter()
        .map(|c| {
            let hw = measure::text_size(&c.title).0;
            let cw = c.cards.iter().map(|t| measure::text_size(t).0).fold(0.0_f64, f64::max);
            (hw.max(cw) + 2.0 * INNER).clamp(MIN_COL_W, MAX_CARD_W)
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
    let card_h = measure::LINE_H + 14.0;

    for (i, c) in k.columns.iter().enumerate() {
        let (l, w) = (lefts[i], col_w[i]);
        // header.
        body.push_str(&format!(
            r#"<rect x="{l:.1}" y="{:.1}" width="{w:.1}" height="{HEADER_H:.1}" fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" rx="4"/>"#,
            PAD
        ));
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
            l + w / 2.0,
            PAD + HEADER_H / 2.0 + 5.0,
            escape_xml(&c.title)
        ));
        // cards.
        let mut cy = PAD + HEADER_H + CARD_GAP;
        for card in &c.cards {
            body.push_str(&format!(
                r#"<rect x="{l:.1}" y="{cy:.1}" width="{w:.1}" height="{card_h:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"/>"#
            ));
            body.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
                l + w / 2.0,
                cy + card_h / 2.0 + 5.0,
                escape_xml(card)
            ));
            cy += card_h + CARD_GAP;
        }
        max_bottom = max_bottom.max(cy);
    }

    let total_h = max_bottom + PAD - CARD_GAP;
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
    fn columns_and_cards_by_indent() {
        let k = parse(
            "kanban\n  Todo\n    Create docs\n    task[Write blog]\n  In Progress\n    id6[Build renderer]\n  Done",
        )
        .unwrap();
        assert_eq!(k.columns.len(), 3);
        assert_eq!(k.columns[0].title, "Todo");
        assert_eq!(k.columns[0].cards, vec!["Create docs".to_string(), "Write blog".to_string()]);
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
}
