//! Mermaid `treemap-beta` diagrams: parser + SVG renderer (Tier 4).
//!
//! Indentation defines a hierarchy of nested nodes; leaves carry a numeric
//! value (`"Leaf": 42`), branches sum their descendants. Rendered as a
//! squarified treemap — each node a rectangle whose area is proportional to
//! its value, packed to keep cell aspect ratios near 1.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 4.0;
const WIDTH: f64 = 720.0;
const HEIGHT: f64 = 480.0;
const HEADER_H: f64 = 18.0;
const TITLE_H: f64 = 30.0;
const MAX_NODES: usize = 2000;
const MAX_DEPTH: usize = 32;

/// Depth-cycled fill palette (outer→inner tint by depth).
const PALETTE: &[&str] =
    &["#dbeafe", "#bfdbfe", "#c7d2fe", "#ddd6fe", "#fbcfe8", "#bbf7d0", "#fef3c7", "#fed7aa"];

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub name: String,
    pub value: Option<f64>, // explicit leaf value; branches derive it
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub(crate) struct Treemap {
    pub title: Option<String>,
    pub roots: Vec<Node>,
}

impl Node {
    /// Effective area weight: explicit value, else the sum over children
    /// (min 1.0 so an all-branch tree still lays out).
    fn weight(&self) -> f64 {
        if let Some(v) = self.value {
            return v.max(0.0);
        }
        let s: f64 = self.children.iter().map(Node::weight).sum();
        s.max(1.0)
    }
}

pub(crate) fn parse(source: &str) -> Result<Treemap, ParseError> {
    let mut title = None;
    let mut seen_header = false;
    // Stack of (indent, path-into-tree). We build a forest of roots.
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new(); // (indent, index path)
    let mut count = 0usize;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        if raw.trim().is_empty() || raw.trim().starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = raw.trim();
            if h.strip_suffix(';').unwrap_or(h).trim_end() != "treemap-beta" {
                return Err(err("treemap must start with `treemap-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        let trimmed = raw.trim();
        if let Some(t) = trimmed.strip_prefix("title ") {
            title = Some(t.trim().to_string());
            continue;
        }
        let indent = indent_of(raw);
        let (name, value) = parse_node(trimmed)
            .ok_or_else(|| err(format!("unrecognized treemap line {trimmed:?}"), line_no))?;
        if count >= MAX_NODES {
            return Err(err(format!("treemap too large: >{MAX_NODES} nodes"), line_no));
        }
        count += 1;
        let node = Node { name, value, children: Vec::new() };

        // Pop deeper-or-equal entries so `stack.last()` is this node's parent.
        while stack.last().map(|(i, _)| *i >= indent).unwrap_or(false) {
            stack.pop();
        }
        if stack.len() >= MAX_DEPTH {
            return Err(err("treemap nesting too deep", line_no));
        }
        let path = match stack.last() {
            None => {
                roots.push(node);
                vec![roots.len() - 1]
            }
            Some((_, parent_path)) => {
                let parent = node_at_mut(&mut roots, parent_path);
                parent.children.push(node);
                let mut p = parent_path.clone();
                p.push(parent.children.len() - 1);
                p
            }
        };
        stack.push((indent, path));
    }
    if !seen_header {
        return Err(ParseError {
            message: "treemap must start with `treemap-beta`".into(),
            line: Some(1),
        });
    }
    if roots.is_empty() {
        return Err(err("treemap has no nodes", 1));
    }
    Ok(Treemap { title, roots })
}

fn indent_of(raw: &str) -> usize {
    raw.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 2 } else { 1 }).sum()
}

/// `"name": value`, `"name"`, `id["name"]: value`, or a bare token.
fn parse_node(s: &str) -> Option<(String, Option<f64>)> {
    let (head, value) = match s.rsplit_once(':') {
        // Only treat the trailing `: n` as a value when `n` parses as a number
        // (a colon inside a quoted name stays part of the name).
        Some((h, v)) if v.trim().parse::<f64>().is_ok() => {
            (h.trim(), Some(v.trim().parse::<f64>().unwrap()))
        }
        _ => (s, None),
    };
    let name = extract_name(head);
    if name.is_empty() {
        return None;
    }
    Some((name, value))
}

/// Pull a display name out of `"quoted"`, `id["quoted"]`, or a bare token.
fn extract_name(s: &str) -> String {
    let s = s.trim();
    if let Some(start) = s.find('"') {
        if let Some(end) = s[start + 1..].find('"') {
            return s[start + 1..start + 1 + end].to_string();
        }
    }
    // `id[...]` with no quotes, or a bare id.
    if let Some(o) = s.find('[') {
        return s[..o].trim().to_string();
    }
    s.to_string()
}

fn node_at_mut<'a>(roots: &'a mut [Node], path: &[usize]) -> &'a mut Node {
    let (first, rest) = path.split_first().expect("non-empty path");
    let mut node = &mut roots[*first];
    for &i in rest {
        node = &mut node.children[i];
    }
    node
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

// ---- rendering -----------------------------------------------------------

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

pub(crate) fn render_svg(t: &Treemap) -> String {
    let title_h = if t.title.is_some() { TITLE_H } else { 0.0 };
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:13px">"#,
        w = WIDTH + 2.0 * PAD,
        h = HEIGHT + title_h + 2.0 * PAD,
    );
    if let Some(title) = &t.title {
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" font-weight="bold" font-size="18" fill="currentColor">{}</text>"#,
            WIDTH / 2.0 + PAD,
            PAD + 20.0,
            escape_xml(title)
        ));
    }
    // Lay the roots out as one virtual level filling the canvas.
    let area = Rect { x: PAD, y: PAD + title_h, w: WIDTH, h: HEIGHT };
    layout_level(&t.roots, area, 0, &mut out);
    out.push_str("</svg>");
    out
}

/// Squarify `nodes` into `rect`, then recurse into each branch (reserving a
/// header strip for its label).
fn layout_level(nodes: &[Node], rect: Rect, depth: usize, out: &mut String) {
    if nodes.is_empty() || rect.w < 2.0 || rect.h < 2.0 {
        return;
    }
    let weights: Vec<f64> = nodes.iter().map(Node::weight).collect();
    let rects = squarify(&weights, rect);
    for (node, r) in nodes.iter().zip(rects) {
        let fill = PALETTE[depth % PALETTE.len()];
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="{fill}" stroke="var(--surface, #fff)" stroke-width="1.5"/>"#,
            r.x, r.y, r.w.max(0.0), r.h.max(0.0)
        ));
        if node.children.is_empty() {
            draw_leaf_label(node, r, out);
        } else {
            // Branch: header strip with the name, children in the remainder.
            draw_header_label(node, r, out);
            if r.h > HEADER_H + 4.0 {
                let inner = Rect { x: r.x, y: r.y + HEADER_H, w: r.w, h: r.h - HEADER_H };
                layout_level(&node.children, inner, depth + 1, out);
            }
        }
    }
}

fn draw_header_label(node: &Node, r: Rect, out: &mut String) {
    if r.w < 24.0 {
        return;
    }
    out.push_str(&format!(
        r#"<text x="{:.1}" y="{:.1}" font-weight="600" font-size="12" fill="currentColor">{}</text>"#,
        r.x + 5.0,
        r.y + 13.0,
        escape_xml(&clip(&node.name, r.w - 10.0))
    ));
}

fn draw_leaf_label(node: &Node, r: Rect, out: &mut String) {
    if r.w < 24.0 || r.h < 18.0 {
        return;
    }
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    out.push_str(&format!(
        r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
        cy - 2.0,
        escape_xml(&clip(&node.name, r.w - 8.0))
    ));
    if let Some(v) = node.value {
        if r.h > 34.0 {
            out.push_str(&format!(
                r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-size="11" fill="var(--color-text-secondary, #666)">{}</text>"#,
                cy + 14.0,
                fmt_num(v)
            ));
        }
    }
}

/// Truncate `s` with an ellipsis to fit `max_w` pixels (approx).
fn clip(s: &str, max_w: f64) -> String {
    if measure::text_size(s).0 <= max_w || max_w < 8.0 {
        return s.to_string();
    }
    let mut trimmed = s.to_string();
    while !trimmed.is_empty() && measure::text_size(&format!("{trimmed}…")).0 > max_w {
        trimmed.pop();
    }
    format!("{trimmed}…")
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Squarified treemap of `weights` into `rect`, preserving input order.
fn squarify(weights: &[f64], rect: Rect) -> Vec<Rect> {
    let total: f64 = weights.iter().sum::<f64>().max(1e-9);
    let scale = (rect.w * rect.h) / total;
    let areas: Vec<f64> = weights.iter().map(|w| w * scale).collect();

    let mut out = vec![Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; areas.len()];
    let (mut x, mut y, mut w, mut h) = (rect.x, rect.y, rect.w, rect.h);
    let mut start = 0usize;
    while start < areas.len() {
        let short = w.min(h);
        let mut end = start + 1;
        let mut best = worst_ratio(&areas[start..end], short);
        while end < areas.len() {
            let cand = worst_ratio(&areas[start..end + 1], short);
            if cand <= best {
                end += 1;
                best = cand;
            } else {
                break;
            }
        }
        let row = &areas[start..end];
        let row_sum: f64 = row.iter().sum();
        let thick = if short > 0.0 { row_sum / short } else { 0.0 };
        if w >= h {
            // vertical strip on the left; cells stacked top→bottom.
            let mut cy = y;
            for (k, &a) in row.iter().enumerate() {
                let ch = if thick > 0.0 { a / thick } else { 0.0 };
                out[start + k] = Rect { x, y: cy, w: thick, h: ch };
                cy += ch;
            }
            x += thick;
            w -= thick;
        } else {
            // horizontal strip on top; cells left→right.
            let mut cx = x;
            for (k, &a) in row.iter().enumerate() {
                let cw = if thick > 0.0 { a / thick } else { 0.0 };
                out[start + k] = Rect { x: cx, y, w: cw, h: thick };
                cx += cw;
            }
            y += thick;
            h -= thick;
        }
        start = end;
    }
    out
}

/// Worst (max) aspect ratio if `row` were laid along a strip of length `short`.
fn worst_ratio(row: &[f64], short: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    if sum <= 0.0 || short <= 0.0 {
        return f64::INFINITY;
    }
    let max = row.iter().cloned().fold(0.0_f64, f64::max);
    let min = row.iter().cloned().fold(f64::INFINITY, f64::min);
    let s2 = short * short;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_hierarchy_and_values() {
        let t = parse(
            "treemap-beta\n \"Root\"\n   \"A\"\n     \"a1\": 10\n     \"a2\": 20\n   \"B\": 30",
        )
        .unwrap();
        assert_eq!(t.roots.len(), 1);
        let root = &t.roots[0];
        assert_eq!(root.name, "Root");
        assert_eq!(root.children.len(), 2); // A, B
        assert_eq!(root.children[0].name, "A");
        assert_eq!(root.children[0].children.len(), 2); // a1, a2
        assert_eq!(root.children[0].children[1].value, Some(20.0));
        assert_eq!(root.children[1].value, Some(30.0));
        // branch weight = sum of leaves.
        assert_eq!(root.children[0].weight(), 30.0);
    }

    #[test]
    fn title_and_bare_and_id_names() {
        let t = parse("treemap-beta\n title Files\n root[\"Project\"]\n   x: 5").unwrap();
        assert_eq!(t.title.as_deref(), Some("Files"));
        assert_eq!(t.roots[0].name, "Project");
        assert_eq!(t.roots[0].children[0].name, "x");
    }

    #[test]
    fn errors() {
        assert!(parse("\"Root\"\n x: 1").is_err()); // no header
        assert!(parse("treemap-beta").is_err()); // no nodes
    }

    #[test]
    fn squarify_fills_and_preserves_order() {
        let r = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let rects = squarify(&[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0], r);
        assert_eq!(rects.len(), 7);
        // total area ≈ rect area.
        let a: f64 = rects.iter().map(|c| c.w * c.h).sum();
        assert!((a - 10000.0).abs() < 1.0, "area {a}");
        // every cell stays inside the rect.
        for c in &rects {
            assert!(c.x >= -0.01 && c.y >= -0.01 && c.x + c.w <= 100.01 && c.y + c.h <= 100.01);
        }
    }

    #[test]
    fn renders_rects_and_labels() {
        let svg = render_svg(
            &parse("treemap-beta\n title T\n \"Root\"\n   \"A\": 10\n   \"B\": 20").unwrap(),
        );
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">Root<") && svg.contains(">A<"));
        assert!(svg.matches("<rect").count() >= 3); // root + 2 leaves
    }
}
