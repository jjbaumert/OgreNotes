//! Mermaid `block` (`block-beta`) diagrams: parser + SVG renderer (Tier 3).
//!
//! A fixed-column grid of blocks. `columns N` sets the grid width; block
//! tokens (`id`, `id["label"]`, `id:span`, or a `space` gap) fill the grid
//! left-to-right, wrapping to the next row; `a --> b` draws an arrow between
//! two placed blocks. (Nested `block:… end` groups are not yet handled.)

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const CELL_H: f64 = 44.0;
const GAP: f64 = 12.0;
const MIN_CELL_W: f64 = 80.0;
const MAX_BLOCKS: usize = 1000;

#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub label: String,
    pub span: usize,
    pub col: usize,
    pub row: usize,
    pub blank: bool, // a `space` filler cell
}

#[derive(Debug, Clone)]
pub(crate) struct BlockArrow {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockDiagram {
    pub columns: usize,
    pub blocks: Vec<Block>,
    pub arrows: Vec<BlockArrow>,
}

pub(crate) fn parse(source: &str) -> Result<BlockDiagram, ParseError> {
    let mut columns = 1usize;
    let mut blocks: Vec<Block> = Vec::new();
    let mut ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut arrows: Vec<(String, String, Option<String>)> = Vec::new();
    let mut seen_header = false;
    let mut next_row = 0usize; // each source block-line begins a new grid row

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if h != "block-beta" && h != "block" {
                return Err(err("block diagram must start with `block` or `block-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        if let Some(n) = line.strip_prefix("columns ") {
            columns = n.trim().parse().ok().filter(|c| *c >= 1).unwrap_or(1);
            continue;
        }
        if line.contains("-->") || line.contains("---") {
            let (a, b, lbl) = parse_arrow(line);
            arrows.push((a, b, lbl));
            continue;
        }
        // A row of block tokens — each source line starts on a fresh grid
        // row, flowing left-to-right and wrapping within the line if it
        // exceeds `columns`.
        let mut col = 0usize;
        let mut row = next_row;
        for tok in line.split_whitespace() {
            if blocks.len() >= MAX_BLOCKS {
                return Err(err(format!("block diagram too large: >{MAX_BLOCKS} blocks"), line_no));
            }
            let (id, label, span, blank) = parse_token(tok);
            let span = span.clamp(1, columns);
            if col + span > columns {
                col = 0;
                row += 1;
            }
            let bi = blocks.len();
            if !blank {
                ids.insert(id, bi);
            }
            blocks.push(Block { label, span, col, row, blank });
            col += span;
            if col >= columns {
                col = 0;
                row += 1;
            }
        }
        next_row = if col == 0 { row } else { row + 1 };
    }
    if !seen_header {
        return Err(ParseError {
            message: "block diagram must start with `block` or `block-beta`".into(),
            line: Some(1),
        });
    }
    // Resolve arrow endpoints (unknown ids drop the arrow — lenient).
    let arrows = arrows
        .into_iter()
        .filter_map(|(a, b, lbl)| Some(BlockArrow { from: *ids.get(&a)?, to: *ids.get(&b)?, label: lbl }))
        .collect();
    Ok(BlockDiagram { columns, blocks, arrows })
}

/// `id`, `id["label"]`, `id:span`, or `space` (a blank filler).
fn parse_token(tok: &str) -> (String, String, usize, bool) {
    if tok == "space" || tok.starts_with("space:") {
        let span = tok.strip_prefix("space:").and_then(|n| n.parse().ok()).unwrap_or(1);
        return (String::new(), String::new(), span, true);
    }
    let mut label = None;
    let mut rest = tok;
    if let Some(o) = rest.find('[') {
        if let Some(c) = rest.rfind(']') {
            if c > o {
                // Strip the shape delimiters + quotes so `[("x")]`, `("x")`,
                // `{"x"}`, `>"x"]`, etc. all yield `x` (we render every block
                // as a rectangle in v1).
                let inner = rest[o + 1..c].trim_matches(|ch| "()[]{}<>\"".contains(ch)).trim();
                label = Some(inner.to_string());
                rest = &rest[..o];
            }
        }
    }
    let (id, span) = match rest.split_once(':') {
        Some((i, s)) => (i, s.parse().unwrap_or(1)),
        None => (rest, 1),
    };
    let label = label.unwrap_or_else(|| id.to_string());
    (id.to_string(), label, span, false)
}

/// `a --> b`, `a --> |"label"| b`, or `a --- b`. Returns `(from, to, label)`.
fn parse_arrow(line: &str) -> (String, String, Option<String>) {
    let sep = if line.contains("-->") { "-->" } else { "---" };
    let (a, rest) = line.split_once(sep).unwrap_or((line, ""));
    let mut b = rest.trim();
    let mut label = None;
    if let Some(r) = b.strip_prefix('|') {
        if let Some(end) = r.find('|') {
            label = Some(r[..end].trim().trim_matches('"').to_string());
            b = r[end + 1..].trim();
        }
    }
    (a.trim().to_string(), b.trim().to_string(), label)
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

pub(crate) fn render_svg(d: &BlockDiagram) -> String {
    // Uniform cell width from the widest single-column block.
    let cell_w = d
        .blocks
        .iter()
        .filter(|b| !b.blank)
        .map(|b| (measure::text_size(&b.label).0 + 24.0) / b.span as f64)
        .fold(MIN_CELL_W, f64::max);
    let rows = d.blocks.iter().map(|b| b.row).max().unwrap_or(0) + 1;
    let total_w = 2.0 * PAD + d.columns as f64 * cell_w + (d.columns as f64 - 1.0).max(0.0) * GAP;
    let total_h = 2.0 * PAD + rows as f64 * (CELL_H + GAP) - GAP;

    let cell_x = |col: usize| PAD + col as f64 * (cell_w + GAP);
    let cell_y = |row: usize| PAD + row as f64 * (CELL_H + GAP);
    let center = |b: &Block| {
        let w = b.span as f64 * cell_w + (b.span as f64 - 1.0) * GAP;
        (cell_x(b.col) + w / 2.0, cell_y(b.row) + CELL_H / 2.0)
    };

    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px"><defs><marker id="mmd-blk-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
    );

    // The point where the center→center segment exits block `b`'s border,
    // heading toward `(tx, ty)`. Clipping both endpoints to the borders keeps
    // the arrow (and its head) in the visible gap between adjacent cells rather
    // than hidden behind the target block.
    let border = |b: &Block, tx: f64, ty: f64| -> (f64, f64) {
        let (cx, cy) = center(b);
        let w = b.span as f64 * cell_w + (b.span as f64 - 1.0) * GAP;
        let (hw, hh) = (w / 2.0, CELL_H / 2.0);
        let (dx, dy) = (tx - cx, ty - cy);
        let s = (hw / dx.abs().max(1e-6)).min(hh / dy.abs().max(1e-6));
        (cx + dx * s, cy + dy * s)
    };

    // arrows (drawn border-to-border so the head lands on the target edge).
    for a in &d.arrows {
        let (fcx, fcy) = center(&d.blocks[a.from]);
        let (tcx, tcy) = center(&d.blocks[a.to]);
        let (x1, y1) = border(&d.blocks[a.from], tcx, tcy);
        let (x2, y2) = border(&d.blocks[a.to], fcx, fcy);
        out.push_str(&format!(
            r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="currentColor" stroke-width="1.5" marker-end="url(#mmd-blk-arrow)"/>"#
        ));
        if let Some(lbl) = &a.label {
            out.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="18" fill="var(--surface, #fff)"/><text x="{:.1}" y="{:.1}" text-anchor="middle" font-size="12" fill="currentColor">{}</text>"#,
                (x1 + x2) / 2.0 - measure::text_size(lbl).0 / 2.0 - 2.0,
                (y1 + y2) / 2.0 - 9.0,
                measure::text_size(lbl).0 + 4.0,
                (x1 + x2) / 2.0,
                (y1 + y2) / 2.0 + 4.0,
                escape_xml(lbl)
            ));
        }
    }

    // blocks.
    for b in &d.blocks {
        if b.blank {
            continue;
        }
        let w = b.span as f64 * cell_w + (b.span as f64 - 1.0) * GAP;
        let (x, y) = (cell_x(b.col), cell_y(b.row));
        out.push_str(&format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{CELL_H:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"/>"#
        ));
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
            x + w / 2.0,
            y + CELL_H / 2.0 + 5.0,
            escape_xml(&b.label)
        ));
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_wrapping_and_spans() {
        let d = parse("block-beta\n columns 3\n a b c\n d\n e:2 f").unwrap();
        assert_eq!(d.columns, 3);
        // a,b,c fill row 0; d row 1; e (span 2) + f row 2.
        assert_eq!((d.blocks[0].col, d.blocks[0].row), (0, 0)); // a
        assert_eq!((d.blocks[2].col, d.blocks[2].row), (2, 0)); // c
        assert_eq!((d.blocks[3].col, d.blocks[3].row), (0, 1)); // d
        // order: a=0,b=1,c=2,d=3,e=4,f=5
        assert_eq!((d.blocks[4].col, d.blocks[4].row, d.blocks[4].span), (0, 2, 2)); // e
        assert_eq!((d.blocks[5].col, d.blocks[5].row), (2, 2)); // f
    }

    #[test]
    fn labels_spaces_and_arrows() {
        let d = parse("block-beta\n columns 2\n a[\"Start\"] space\n b\n a --> b").unwrap();
        assert_eq!(d.blocks[0].label, "Start");
        assert!(d.blocks[1].blank); // the `space` filler
        assert_eq!(d.arrows.len(), 1);
        assert_eq!(d.arrows[0].from, 0); // a is block 0
        assert_eq!(d.blocks[d.arrows[0].to].label, "b");
    }

    #[test]
    fn header_required() {
        assert!(parse("columns 2\n a b").is_err());
    }

    #[test]
    fn renders_grid_and_arrows() {
        let svg = render_svg(&parse("block-beta\n columns 2\n a b\n a --> b").unwrap());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">a<") && svg.contains(">b<"));
        assert!(svg.contains("<line") && svg.contains("marker-end"));
    }

    #[test]
    fn arrow_is_clipped_to_the_gap_not_center_to_center() {
        // The arrow between two adjacent blocks must span only the inter-cell
        // gap (border to border), so its head is visible — not run center to
        // center hidden behind the blocks. The visible x-span is therefore far
        // less than a cell width.
        let svg = render_svg(&parse("block-beta\n columns 2\n a b\n a --> b").unwrap());
        let line = svg.split("<line").nth(1).expect("an arrow line");
        let get = |k: &str| -> f64 {
            line.split(&format!("{k}=\"")).nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
        };
        let span = (get("x2") - get("x1")).abs();
        assert!(span > 0.0 && span < MIN_CELL_W, "arrow spans only the gap, got {span}px");
    }
}
