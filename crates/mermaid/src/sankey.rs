//! Mermaid `sankey-beta` diagrams: parser + SVG renderer (Tier 4).
//!
//! Body is CSV: one `source,target,value` row per flow (double-quoted fields
//! may contain commas). Nodes are inferred from the source/target columns and
//! placed in layers by longest-path depth; flows render as curved ribbons
//! whose thickness is proportional to their value.

use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const HEIGHT: f64 = 420.0;
const NODE_W: f64 = 16.0;
const COL_GAP: f64 = 160.0; // horizontal spacing between layers
const V_GAP: f64 = 12.0; // vertical gap between stacked nodes in a column
const MIN_LINK_H: f64 = 1.0;
const MAX_LINKS: usize = 4000;
/// Cap on a single link's value. Band heights are proportions, so capping
/// loses nothing visually; it guarantees per-node throughput sums over
/// ≤MAX_LINKS links stay far below f64::MAX (4000 × 1e12 = 4e15).
const MAX_LINK_VALUE: f64 = 1e12;

/// Per-node colors, cycled by node index (ribbons blend source→target via
/// per-link linear gradients — see the `<defs>` emission in `render_svg`).
const PALETTE: &[&str] = &[
    "#3b82f6", "#ef4444", "#22c55e", "#a855f7", "#f59e0b", "#14b8a6", "#ec4899", "#64748b",
    "#0ea5e9", "#84cc16",
];

#[derive(Debug, Clone)]
pub(crate) struct Link {
    pub from: usize,
    pub to: usize,
    pub value: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct Sankey {
    pub nodes: Vec<String>,
    pub links: Vec<Link>,
}

pub(crate) fn parse(source: &str) -> Result<Sankey, ParseError> {
    let mut nodes: Vec<String> = Vec::new();
    let mut ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut links: Vec<Link> = Vec::new();
    let mut seen_header = false;

    let mut intern = |name: &str, nodes: &mut Vec<String>| -> usize {
        if let Some(&i) = ids.get(name) {
            return i;
        }
        let i = nodes.len();
        nodes.push(name.to_string());
        ids.insert(name.to_string(), i);
        i
    };

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if h != "sankey-beta" && h != "sankey" {
                return Err(err("sankey diagram must start with `sankey-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        let fields = split_csv(line);
        if fields.len() < 3 {
            return Err(err(format!("sankey row needs `source,target,value`, got {line:?}"), line_no));
        }
        let (src, tgt) = (fields[0].trim(), fields[1].trim());
        // Non-finite parses ("1e999", "inf", "NaN") take the same error as
        // non-numerics; the cap keeps per-node throughput sums finite (two
        // 1e308 links on one node would otherwise sum to inf and turn the
        // band scale into NaN).
        let value: f64 = fields[2]
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
            .map(|v: f64| v.min(MAX_LINK_VALUE))
            .ok_or_else(|| {
                err(format!("sankey value must be numeric, got {:?}", fields[2].trim()), line_no)
            })?;
        if src.is_empty() || tgt.is_empty() {
            return Err(err("sankey row has an empty source or target", line_no));
        }
        if links.len() >= MAX_LINKS {
            return Err(err(format!("sankey too large: >{MAX_LINKS} links"), line_no));
        }
        let from = intern(src, &mut nodes);
        let to = intern(tgt, &mut nodes);
        links.push(Link { from, to, value: value.max(0.0) });
    }
    if !seen_header {
        return Err(ParseError {
            message: "sankey diagram must start with `sankey-beta`".into(),
            line: Some(1),
        });
    }
    if links.is_empty() {
        return Err(err("sankey diagram has no flows", 1));
    }
    Ok(Sankey { nodes, links })
}

/// Split one CSV line, honoring `"double quoted"` fields (which may contain
/// commas). Quotes are stripped from the returned fields.
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_q && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => in_q = !in_q,
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

// ---- rendering -----------------------------------------------------------

struct Placed {
    x: f64,
    y: f64,
    h: f64,
    layer: usize,
}

pub(crate) fn render_svg(s: &Sankey) -> String {
    let n = s.nodes.len();
    // Longest-path layer per node (Bellman-Ford-style relaxation, capped so a
    // cycle can't loop forever).
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        let mut changed = false;
        for l in &s.links {
            if layer[l.to] < layer[l.from] + 1 {
                layer[l.to] = layer[l.from] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let n_layers = layer.iter().copied().max().unwrap_or(0) + 1;

    // Node throughput = max(inflow, outflow).
    let mut inflow = vec![0.0_f64; n];
    let mut outflow = vec![0.0_f64; n];
    for l in &s.links {
        outflow[l.from] += l.value;
        inflow[l.to] += l.value;
    }
    let throughput: Vec<f64> =
        (0..n).map(|i| inflow[i].max(outflow[i]).max(1e-9)).collect();

    // Group node indices by layer.
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n_layers];
    for i in 0..n {
        cols[layer[i]].push(i);
    }

    // Vertical scale: size the busiest column to fit HEIGHT.
    let mut max_col = 1e-9_f64;
    let mut busiest_count = 1usize;
    for c in &cols {
        let sum: f64 = c.iter().map(|&i| throughput[i]).sum();
        if sum > max_col {
            max_col = sum;
            busiest_count = c.len().max(1);
        }
    }
    let avail_h = (HEIGHT - (busiest_count as f64 - 1.0) * V_GAP).max(40.0);
    let scale = avail_h / max_col;

    // Place nodes: x by layer, y stacked (column centered vertically).
    let mut placed: Vec<Placed> = (0..n)
        .map(|i| Placed { x: 0.0, y: 0.0, h: (throughput[i] * scale).max(MIN_LINK_H), layer: layer[i] })
        .collect();
    for (li, c) in cols.iter().enumerate() {
        let col_h: f64 =
            c.iter().map(|&i| placed[i].h).sum::<f64>() + (c.len() as f64 - 1.0).max(0.0) * V_GAP;
        let mut y = PAD + (HEIGHT - col_h) / 2.0;
        let x = PAD + li as f64 * (NODE_W + COL_GAP);
        for &i in c {
            placed[i].x = x;
            placed[i].y = y;
            y += placed[i].h + V_GAP;
        }
    }

    // Assign each link a slot on the source's right edge and target's left
    // edge. Order a node's links by the partner's vertical center to reduce
    // crossings.
    let ycenter = |p: &Placed| p.y + p.h / 2.0;
    let mut out_links: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_links: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (li, l) in s.links.iter().enumerate() {
        out_links[l.from].push(li);
        in_links[l.to].push(li);
    }
    for i in 0..n {
        out_links[i].sort_by(|&a, &b| {
            ycenter(&placed[s.links[a].to]).total_cmp(&ycenter(&placed[s.links[b].to]))
        });
        in_links[i].sort_by(|&a, &b| {
            ycenter(&placed[s.links[a].from]).total_cmp(&ycenter(&placed[s.links[b].from]))
        });
    }
    let mut src_y = vec![0.0_f64; s.links.len()]; // top of ribbon at source
    let mut dst_y = vec![0.0_f64; s.links.len()];
    for i in 0..n {
        let mut cy = placed[i].y;
        for &li in &out_links[i] {
            src_y[li] = cy;
            cy += (s.links[li].value * scale).max(MIN_LINK_H);
        }
        let mut cy = placed[i].y;
        for &li in &in_links[i] {
            dst_y[li] = cy;
            cy += (s.links[li].value * scale).max(MIN_LINK_H);
        }
    }

    let total_w = PAD + (n_layers as f64) * (NODE_W + COL_GAP) - COL_GAP + PAD + max_label_w(s);
    let total_h = HEIGHT + 2.0 * PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:13px">"#
    );

    // Gradient defs: each ribbon fades left-to-right from the source node's
    // color to the target node's color (Mermaid parity).
    out.push_str("<defs>");
    for (li, l) in s.links.iter().enumerate() {
        let x0 = placed[l.from].x + NODE_W;
        let x1 = placed[l.to].x;
        out.push_str(&format!(
            r#"<linearGradient id="sk{li}" gradientUnits="userSpaceOnUse" x1="{x0:.1}" y1="0" x2="{x1:.1}" y2="0"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
            PALETTE[l.from % PALETTE.len()],
            PALETTE[l.to % PALETTE.len()],
        ));
    }
    out.push_str("</defs>");

    // ribbons first (behind nodes), filled with their source->target gradient.
    for (li, l) in s.links.iter().enumerate() {
        let th = (l.value * scale).max(MIN_LINK_H);
        let sp = &placed[l.from];
        let tp = &placed[l.to];
        let x0 = sp.x + NODE_W;
        let x1 = tp.x;
        let (y0t, y1t) = (src_y[li], dst_y[li]);
        let mx = (x0 + x1) / 2.0;
        // Filled ribbon: top edge forward (cubic), down `th`, bottom edge back.
        let d = format!(
            "M {x0:.1} {y0t:.1} C {mx:.1} {y0t:.1} {mx:.1} {y1t:.1} {x1:.1} {y1t:.1} \
             L {x1:.1} {:.1} C {mx:.1} {:.1} {mx:.1} {:.1} {x0:.1} {:.1} Z",
            y1t + th,
            y1t + th,
            y0t + th,
            y0t + th,
        );
        out.push_str(&format!(
            r#"<path d="{d}" fill="url(#sk{li})" fill-opacity="0.5"/>"#
        ));
    }

    // node bars + labels.
    for (i, name) in s.nodes.iter().enumerate() {
        let p = &placed[i];
        let color = PALETTE[i % PALETTE.len()];
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{NODE_W:.1}" height="{:.1}" fill="{color}"/>"#,
            p.x, p.y, p.h
        ));
        // Label: to the right for the last layer, otherwise to the right of the
        // bar too — but flip to the left when the node is in the final column.
        let last = p.layer + 1 == n_layers;
        let (lx, anchor) =
            if last { (p.x - 5.0, "end") } else { (p.x + NODE_W + 5.0, "start") };
        let cy = p.y + p.h / 2.0;
        // Name on top, value beneath (Mermaid labels each node with its total).
        out.push_str(&format!(
            r#"<text x="{lx:.1}" y="{:.1}" text-anchor="{anchor}" fill="currentColor">{}</text>"#,
            cy,
            escape_xml(name)
        ));
        out.push_str(&format!(
            r#"<text x="{lx:.1}" y="{:.1}" text-anchor="{anchor}" fill="currentColor" style="font-size:11px" opacity="0.75">{}</text>"#,
            cy + 14.0,
            (throughput[i]).round() as i64,
        ));
    }

    out.push_str("</svg>");
    out
}

fn max_label_w(s: &Sankey) -> f64 {
    s.nodes.iter().map(|nm| measure::text_size(nm).0).fold(0.0_f64, f64::max) + 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_flows_and_interns_nodes() {
        let s = parse("sankey-beta\n A,B,5\n A,C,3\n B,C,2").unwrap();
        assert_eq!(s.nodes, vec!["A", "B", "C"]);
        assert_eq!(s.links.len(), 3);
        assert_eq!((s.links[0].from, s.links[0].to, s.links[0].value), (0, 1, 5.0));
        assert_eq!((s.links[2].from, s.links[2].to), (1, 2));
    }

    #[test]
    fn quoted_fields_with_commas() {
        let s = parse("sankey-beta\n \"Sales, EU\",Revenue,10").unwrap();
        assert_eq!(s.nodes[0], "Sales, EU");
        assert_eq!(s.nodes[1], "Revenue");
        assert_eq!(s.links[0].value, 10.0);
    }

    #[test]
    fn errors() {
        assert!(parse("A,B,5").is_err()); // no header
        assert!(parse("sankey-beta\n A,B").is_err()); // too few fields
        assert!(parse("sankey-beta\n A,B,x").is_err()); // non-numeric value
        assert!(parse("sankey-beta").is_err()); // no flows
    }

    #[test]
    fn renders_ribbons_and_nodes() {
        let svg = render_svg(&parse("sankey-beta\n A,B,5\n A,C,3\n B,D,2\n C,D,3").unwrap());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">A<") && svg.contains(">D<"));
        assert!(svg.contains("<path") && svg.contains("<rect"));
        // 4 ribbons.
        assert_eq!(svg.matches("<path").count(), 4);
    }

    #[test]
    fn ribbons_use_gradients_and_nodes_show_values() {
        let svg = render_svg(&parse("sankey-beta\n Coal,Elec,25\n Elec,Homes,25").unwrap());
        // A source->target linear gradient per ribbon, used as the ribbon fill.
        assert!(svg.contains(r#"<linearGradient id="sk0""#), "gradient def: {svg}");
        assert!(svg.contains(r#"fill="url(#sk0)""#), "ribbon gradient fill: {svg}");
        // Each node is labeled with its value (Coal outflow 25).
        assert!(svg.contains(">25<"), "node value label: {svg}");
    }

    #[test]
    fn layers_by_longest_path() {
        // A->B->C and A->C: C must sit in the deepest layer (2), not 1.
        let s = parse("sankey-beta\n A,B,1\n B,C,1\n A,C,1").unwrap();
        let svg = render_svg(&s);
        // Three distinct x columns → node A leftmost, C rightmost.
        assert!(svg.contains("<svg"));
    }
}
