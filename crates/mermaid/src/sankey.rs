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
    line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Sankey {
    pub nodes: Vec<String>,
    pub links: Vec<Link>,
    layers: Vec<usize>,
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
        let fields = split_csv(line).ok_or_else(|| err("unclosed CSV quote", line_no))?;
        if fields.len() != 3 {
            return Err(err(
                format!("sankey row needs `source,target,value`, got {line:?}"),
                line_no,
            ));
        }
        let (src, tgt) = (fields[0].trim(), fields[1].trim());
        // Non-finite parses ("1e999", "inf", "NaN") take the same error as
        // non-numerics; the cap keeps per-node throughput sums finite (two
        // 1e308 links on one node would otherwise sum to inf and turn the
        // band scale into NaN).
        let value = crate::parse_number(&fields[2])
            .map(|v: f64| v.min(MAX_LINK_VALUE))
            .ok_or_else(|| {
                err(
                    format!("sankey value must be numeric, got {:?}", fields[2].trim()),
                    line_no,
                )
            })?;
        if value < 0.0 {
            return Err(err("sankey value must be non-negative", line_no));
        }
        if src.is_empty() || tgt.is_empty() {
            return Err(err("sankey row has an empty source or target", line_no));
        }
        if links.len() >= MAX_LINKS {
            return Err(err(
                format!("sankey too large: >{MAX_LINKS} links"),
                line_no,
            ));
        }
        let from = intern(src, &mut nodes);
        let to = intern(tgt, &mut nodes);
        links.push(Link {
            from,
            to,
            value,
            line: line_no,
        });
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
    // Kahn's traversal both rejects cycles and computes longest-path layers in
    // O(nodes + links), rather than repeatedly relaxing every edge.
    let mut outgoing = vec![Vec::new(); nodes.len()];
    let mut incoming = vec![0usize; nodes.len()];
    for (index, link) in links.iter().enumerate() {
        outgoing[link.from].push(index);
        incoming[link.to] += 1;
    }
    let mut ready: std::collections::VecDeque<_> = incoming
        .iter()
        .enumerate()
        .filter_map(|(i, &count)| (count == 0).then_some(i))
        .collect();
    let mut layers = vec![0usize; nodes.len()];
    let mut visited = 0;
    while let Some(from) = ready.pop_front() {
        visited += 1;
        for &edge in &outgoing[from] {
            let to = links[edge].to;
            layers[to] = layers[to].max(layers[from] + 1);
            incoming[to] -= 1;
            if incoming[to] == 0 {
                ready.push_back(to);
            }
        }
    }
    if visited != nodes.len() {
        return Err(ParseError {
            message: "sankey flows must not contain cycles".into(),
            line: cycle_line(&links, &outgoing),
        });
    }
    Ok(Sankey {
        nodes,
        links,
        layers,
    })
}

/// Locate a back-edge to a gray ancestor. Kahn's residual graph also
/// contains acyclic descendants of a cycle; reporting its first remaining
/// edge could blame a valid downstream flow. Iterative DFS identifies an
/// actual cycle edge without recursion proportional to source size.
fn cycle_line(links: &[Link], outgoing: &[Vec<usize>]) -> Option<usize> {
    let mut color = vec![0u8; outgoing.len()];
    for start in 0..outgoing.len() {
        if color[start] != 0 {
            continue;
        }
        color[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some((from, next)) = stack.last_mut() {
            let from = *from;
            let Some(&edge) = outgoing[from].get(*next) else {
                color[from] = 2;
                stack.pop();
                continue;
            };
            *next += 1;
            let target = links[edge].to;
            match color[target] {
                1 => return Some(links[edge].line),
                0 => {
                    color[target] = 1;
                    stack.push((target, 0));
                }
                _ => {}
            }
        }
    }
    None
}

/// Split one CSV line, honoring `"double quoted"` fields (which may contain
/// commas). Quotes are stripped from the returned fields.
fn split_csv(line: &str) -> Option<Vec<String>> {
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
    (!in_q).then_some(out)
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError {
        message: message.into(),
        line: Some(line),
    }
}

// ---- rendering -----------------------------------------------------------

struct Placed {
    x: f64,
    y: f64,
    h: f64,
    layer: usize,
}

/// Six hex digits of FNV-1a over the diagram's nodes and links: stable for
/// identical diagrams (whose gradient definitions are identical anyway)
/// and distinct across different ones, so gradient ids don't collide
/// between sankeys on one page.
fn gradient_tag(s: &Sankey) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for n in &s.nodes {
        feed(n.as_bytes());
        feed(&[0]);
    }
    for l in &s.links {
        feed(&l.from.to_le_bytes());
        feed(&l.to.to_le_bytes());
        feed(&l.value.to_bits().to_le_bytes());
    }
    format!("{:06x}", h & 0xff_ffff)
}

pub(crate) fn render_svg(s: &Sankey) -> String {
    let n = s.nodes.len();
    let layer = &s.layers;
    let n_layers = layer.iter().copied().max().unwrap_or(0) + 1;

    // Node throughput = max(inflow, outflow).
    let mut inflow = vec![0.0_f64; n];
    let mut outflow = vec![0.0_f64; n];
    for l in &s.links {
        outflow[l.from] += l.value;
        inflow[l.to] += l.value;
    }
    let throughput: Vec<f64> = (0..n)
        .map(|i| inflow[i].max(outflow[i]).max(1e-9))
        .collect();

    // Group node indices by layer.
    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n_layers];
    for i in 0..n {
        cols[layer[i]].push(i);
    }

    // Final-column captions face left; the preceding column's captions
    // face right into the same gap. Reserve both complete captions there,
    // including the numeric totals, instead of merely widening the canvas.
    let caption_width: Vec<f64> = cols
        .iter()
        .map(|column| {
            column
                .iter()
                .map(|&i| {
                    let name = measure::literal_overlap_width(&s.nodes[i], 13.0);
                    let value = format!("{}", throughput[i].round() as i64);
                    name.max(measure::literal_overlap_width(&value, 11.0))
                })
                .fold(0.0, f64::max)
        })
        .collect();
    let mut column_x = vec![PAD; n_layers];
    for column in 1..n_layers {
        let opposing = if column + 1 == n_layers {
            caption_width[column]
        } else {
            0.0
        };
        let gap = COL_GAP.max(caption_width[column - 1] + opposing + 20.0);
        column_x[column] = column_x[column - 1] + NODE_W + gap;
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
        .map(|i| Placed {
            x: 0.0,
            y: 0.0,
            h: (throughput[i] * scale).max(36.0),
            layer: layer[i],
        })
        .collect();
    // A node must accommodate each ribbon's minimum visible thickness too.
    let mut incoming_h = vec![0.0_f64; n];
    let mut outgoing_h = vec![0.0_f64; n];
    for link in &s.links {
        let h = (link.value * scale).max(MIN_LINK_H);
        outgoing_h[link.from] += h;
        incoming_h[link.to] += h;
    }
    for (i, p) in placed.iter_mut().enumerate() {
        p.h = p.h.max(incoming_h[i]).max(outgoing_h[i]);
    }
    let height = cols
        .iter()
        .map(|column| {
            column.iter().map(|&i| placed[i].h).sum::<f64>()
                + column.len().saturating_sub(1) as f64 * V_GAP
        })
        .fold(HEIGHT, f64::max);
    for (li, c) in cols.iter().enumerate() {
        let col_h =
            c.iter().map(|&i| placed[i].h).sum::<f64>() + c.len().saturating_sub(1) as f64 * V_GAP;
        let mut y = PAD + (height - col_h) / 2.0;
        let x = column_x[li];
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

    let total_w = column_x[n_layers - 1] + NODE_W + PAD + max_label_w(s);
    let total_h = height + 2.0 * PAD;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:13px">"#
    );

    // Gradient defs: each ribbon fades left-to-right from the source node's
    // color to the target node's color (Mermaid parity). The ids carry a
    // per-diagram hash: two sankeys on one page share a DOM, and
    // `url(#sk0)` would resolve to whichever rendered first, giving the
    // second the first's colours.
    let tag = gradient_tag(s);
    out.push_str("<defs>");
    for (li, l) in s.links.iter().enumerate() {
        let x0 = placed[l.from].x + NODE_W;
        let x1 = placed[l.to].x;
        out.push_str(&format!(
            r#"<linearGradient id="sk{tag}-{li}" gradientUnits="userSpaceOnUse" x1="{x0:.1}" y1="0" x2="{x1:.1}" y2="0"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
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
            r#"<path d="{d}" fill="url(#sk{tag}-{li})" fill-opacity="0.5"/>"#
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
        let (lx, anchor) = if last {
            (p.x - 5.0, "end")
        } else {
            (p.x + NODE_W + 5.0, "start")
        };
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
    s.nodes
        .iter()
        .map(|name| measure::literal_overlap_width(name, 13.0))
        .fold(0.0_f64, f64::max)
        + 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv_flows_and_interns_nodes() {
        let s = parse("sankey-beta\n A,B,5\n A,C,3\n B,C,2").unwrap();
        assert_eq!(s.nodes, vec!["A", "B", "C"]);
        assert_eq!(s.links.len(), 3);
        assert_eq!(
            (s.links[0].from, s.links[0].to, s.links[0].value),
            (0, 1, 5.0)
        );
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
        // Ids are `sk<6-hex-diagram-tag>-<link index>`; the first ribbon's
        // fill must reference the first gradient's id.
        let def_start = svg.find(r#"<linearGradient id="sk"#).expect("gradient def");
        let id_start = def_start + r#"<linearGradient id=""#.len();
        let id = &svg[id_start..id_start + svg[id_start..].find('"').unwrap()];
        assert!(
            id.ends_with("-0") && id.len() == "sk".len() + 6 + 2,
            "gradient id shape: {id}"
        );
        assert!(
            svg.contains(&format!(r#"fill="url(#{id})""#)),
            "ribbon gradient fill: {svg}"
        );
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

    #[test]
    fn opposing_captions_reserve_combined_width_and_remain_inside() {
        let wide_source = "W".repeat(40);
        let wide_target = "M".repeat(40);
        for source in [
            format!("sankey-beta\n{wide_source},{wide_target},1"),
            "sankey-beta\nA,B,1000000000000".to_string(),
            format!("sankey-beta\n{wide_source},Middle,1\nMiddle,{wide_target},1"),
        ] {
            let svg = crate::render(&source).svg.expect("valid Sankey renders");
            crate::extent::assert_inside(&svg);
            let (_, drawn) = crate::extent::scan(&svg);
            let labels: Vec<_> = drawn.iter().filter(|item| item.tag == "text").collect();
            for (index, left) in labels.iter().enumerate() {
                for right in labels.iter().skip(index + 1) {
                    assert!(
                        left.bbox.overlap(&right.bbox) <= 0.1,
                        "captions overlap: {:?} and {:?} for {source}",
                        left,
                        right
                    );
                }
            }
        }
    }

    #[test]
    fn literal_break_spellings_and_fallback_names_fit_opposing_caption_space() {
        let mut cases: Vec<(String, String)> = ["<br/>", "<br>", "<br />"]
            .into_iter()
            .map(|spelling| {
                (
                    ("W".to_string() + spelling).repeat(20),
                    ("M".to_string() + spelling).repeat(20),
                )
            })
            .collect();
        cases.push(("ДΩé가क्‍ष".repeat(20), "မြန်မာ가".repeat(20)));
        for (source_name, target_name) in cases {
            let source = format!("sankey-beta\n{source_name},{target_name},1");
            let svg = crate::render(&source)
                .svg
                .expect("literal Sankey names render");
            let (view, drawn) = crate::extent::scan(&svg);
            let source = drawn
                .iter()
                .find(|item| item.tag == "text" && item.text == source_name)
                .expect("complete literal source name");
            let target = drawn
                .iter()
                .find(|item| item.tag == "text" && item.text == target_name)
                .expect("complete literal target name");
            // The generic SVG checker splits HTML-style breaks when measuring
            // text. Here both names are escaped literal text: derive each
            // complete caption's width at its actual emitted font size.
            let source_left = source.bbox.x0;
            let source_right = source_left + measure::literal_overlap_width(&source_name, 13.0);
            let target_right = target.bbox.x1;
            let target_left = target_right - measure::literal_overlap_width(&target_name, 13.0);
            assert!(
                source_right < target_left,
                "literal captions overlap: {source_name} and {target_name}"
            );
            assert!(
                source_left >= view.x0 && target_right <= view.x1,
                "literal captions leave the canvas"
            );
        }
    }
}
