//! Mermaid `packet` (`packet-beta`) diagrams: parser + SVG renderer (Tier 3).
//!
//! A byte/bit-field layout. Optional `title`, then one field per line —
//! `start-end: "Label"` (a bit range) or `bit: "Label"` (a single bit).
//! Rendered as a grid 32 bits wide: each field is a labeled box spanning its
//! bits, split across rows where it crosses a 32-bit boundary, with bit
//! indices along the top of each row.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const BITS_PER_ROW: usize = 32;
const BIT_W: f64 = 26.0;
const ROW_H: f64 = 40.0;
const ROW_GAP: f64 = 16.0; // room for the bit-index labels above each row
const MAX_BITS: usize = 4096;

#[derive(Debug, Clone)]
pub(crate) struct Field {
    pub start: usize,
    pub end: usize,
    pub label: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Packet {
    pub title: Option<String>,
    pub fields: Vec<Field>,
}

pub(crate) fn parse(source: &str) -> Result<Packet, ParseError> {
    let mut title = None;
    let mut fields: Vec<Field> = Vec::new();
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if h != "packet-beta" && h != "packet" {
                return Err(err("packet diagram must start with `packet` or `packet-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            title = Some(t.trim().to_string());
            continue;
        }
        let Some((range, label)) = line.split_once(':') else {
            return Err(err("packet field needs `bits: \"label\"`", line_no));
        };
        let (start, end) = parse_range(range.trim())
            .ok_or_else(|| err("packet field range must be `start-end` or `bit`", line_no))?;
        if start > end {
            return Err(err("packet field start bit must be <= end bit", line_no));
        }
        if end >= MAX_BITS {
            return Err(err(format!("packet too large: bit {end} >= {MAX_BITS}"), line_no));
        }
        let label = label.trim();
        let label = label.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(label);
        fields.push(Field { start, end, label: label.to_string() });
    }
    if !seen_header {
        return Err(ParseError {
            message: "packet diagram must start with `packet` or `packet-beta`".into(),
            line: Some(1),
        });
    }
    Ok(Packet { title, fields })
}

/// `start-end` or a single `bit`.
fn parse_range(s: &str) -> Option<(usize, usize)> {
    match s.split_once('-') {
        Some((a, b)) => Some((a.trim().parse().ok()?, b.trim().parse().ok()?)),
        None => {
            let b: usize = s.parse().ok()?;
            Some((b, b))
        }
    }
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(p: &Packet) -> String {
    let row_pitch = ROW_H + ROW_GAP;
    let grid_w = BITS_PER_ROW as f64 * BIT_W;
    let total_w = 2.0 * PAD + grid_w;
    let mut top = PAD;
    let mut body = String::new();

    if let Some(title) = &p.title {
        top += 24.0;
        body.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            PAD + grid_w / 2.0,
            PAD + 18.0,
            escape_xml(title)
        ));
    }

    let max_bit = p.fields.iter().map(|f| f.end).max().unwrap_or(0);
    let rows = max_bit / BITS_PER_ROW + 1;
    let bit_xy = |bit: usize| {
        let r = bit / BITS_PER_ROW;
        let col = bit % BITS_PER_ROW;
        (PAD + col as f64 * BIT_W, top + ROW_GAP + r as f64 * row_pitch)
    };

    for f in &p.fields {
        // Split the field at each 32-bit row boundary it crosses.
        let mut b = f.start;
        while b <= f.end {
            let r = b / BITS_PER_ROW;
            let row_last = r * BITS_PER_ROW + BITS_PER_ROW - 1;
            let seg_end = f.end.min(row_last);
            let (x, y) = bit_xy(b);
            let w = (seg_end - b + 1) as f64 * BIT_W;
            body.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{ROW_H:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>"#
            ));
            // Bit indices at the segment's start and (if wider than 1) end.
            body.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="start" font-size="10" fill="currentColor">{b}</text>"#,
                x + 2.0,
                y - 3.0
            ));
            if seg_end > b {
                body.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" text-anchor="end" font-size="10" fill="currentColor">{seg_end}</text>"#,
                    x + w - 2.0,
                    y - 3.0
                ));
            }
            // Label centered on the widest (first) segment.
            if b == f.start {
                body.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
                    x + w / 2.0,
                    y + ROW_H / 2.0 + 5.0,
                    escape_xml(&f.label)
                ));
            }
            b = seg_end + 1;
        }
    }

    let total_h = top + ROW_GAP + rows as f64 * row_pitch - ROW_GAP + PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    out.push_str(&body);
    out.push_str("</svg>");
    let _ = measure::LINE_H;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges_and_single_bits() {
        let p = parse(
            "packet-beta\n title UDP\n 0-15: \"Source Port\"\n 16-31: \"Dest Port\"\n 32: \"Flag\"",
        )
        .unwrap();
        assert_eq!(p.title.as_deref(), Some("UDP"));
        assert_eq!(p.fields.len(), 3);
        assert_eq!((p.fields[0].start, p.fields[0].end), (0, 15));
        assert_eq!(p.fields[0].label, "Source Port");
        assert_eq!((p.fields[2].start, p.fields[2].end), (32, 32)); // single bit
    }

    #[test]
    fn plain_packet_header_and_errors() {
        assert!(parse("packet\n 0-7: \"x\"").is_ok());
        assert!(parse("title X\n 0-7: \"x\"").is_err()); // missing header
        assert!(parse("packet\n 7-3: \"x\"").is_err()); // start > end
        assert!(parse("packet\n 0-7 no colon").is_err());
    }

    #[test]
    fn field_crossing_a_row_boundary_splits() {
        // A field spanning bits 30..33 crosses the 32-bit row boundary → two
        // rects (one per row).
        let svg = render_svg(&parse("packet\n 30-33: \"Split\"").unwrap());
        assert!(svg.matches("<rect").count() >= 2, "field must split across rows: {svg}");
        assert!(svg.contains(">Split<"));
    }

    #[test]
    fn renders_grid() {
        let svg = render_svg(&parse("packet-beta\n title T\n 0-15: \"A\"\n 16-31: \"B\"").unwrap());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">A<") && svg.contains(">B<"));
    }
}
