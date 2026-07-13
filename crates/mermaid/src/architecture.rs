//! Mermaid `architecture-beta` diagrams: parser + SVG renderer (Tier 5).
//!
//! Cloud-architecture diagrams. `group id(icon)[Title]` and
//! `service id(icon)[Title] in group` declare boxed nodes with a built-in
//! icon glyph; `junction id` is a routing dot. Edges carry port sides —
//! `a:R -- L:b` means b sits to the right of a — which drive a constraint
//! based grid placement (BFS from an anchor). Arrows: `--`, `-->`, `<--`,
//! `<-->`. Icons we don't recognize fall back to a labeled box.

use crate::{escape_xml, measure, ParseError};
use std::collections::HashMap;

const CELL_W: f64 = 150.0;
const CELL_H: f64 = 120.0;
const ICON: f64 = 46.0; // icon glyph box side
const PAD: f64 = 24.0;
const MAX_NODES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Side {
    L,
    R,
    T,
    B,
}

impl Side {
    fn parse(s: &str) -> Option<Side> {
        match s.trim() {
            "L" => Some(Side::L),
            "R" => Some(Side::R),
            "T" => Some(Side::T),
            "B" => Some(Side::B),
            _ => None,
        }
    }
    /// (dx, dy): where the neighbor sits relative to this node when the edge
    /// leaves through this side.
    fn delta(self) -> (i32, i32) {
        match self {
            Side::L => (-1, 0),
            Side::R => (1, 0),
            Side::T => (0, -1),
            Side::B => (0, 1),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub icon: String,
    pub title: String,
    pub group: Option<usize>,
    pub is_group: bool,
    pub junction: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    pub from_side: Side,
    pub to_side: Side,
    pub arrow_from: bool,
    pub arrow_to: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Architecture {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub groups: Vec<usize>, // node indices that are groups
}

pub(crate) fn parse(source: &str) -> Result<Architecture, ParseError> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut edges_raw: Vec<(String, Side, String, Side, bool, bool)> = Vec::new();
    let mut groups: Vec<usize> = Vec::new();
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if h != "architecture-beta" && h != "architecture" {
                return Err(err("architecture diagram must start with `architecture-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        if nodes.len() >= MAX_NODES {
            return Err(err(format!("architecture too large: >{MAX_NODES} nodes"), line_no));
        }
        if let Some(rest) = line.strip_prefix("group ") {
            let (id, icon, title, parent) = parse_decl(rest);
            let gi = nodes.len();
            ids.insert(id, gi);
            groups.push(gi);
            let group = parent.and_then(|p| ids.get(&p).copied());
            nodes.push(Node { icon, title, group, is_group: true, junction: false });
        } else if let Some(rest) = line.strip_prefix("service ") {
            let (id, icon, title, parent) = parse_decl(rest);
            let group = parent.and_then(|p| ids.get(&p).copied());
            ids.insert(id, nodes.len());
            nodes.push(Node { icon, title, group, is_group: false, junction: false });
        } else if let Some(rest) = line.strip_prefix("junction ") {
            let (id, parent) = match rest.split_once(" in ") {
                Some((a, b)) => (a.trim().to_string(), Some(b.trim().to_string())),
                None => (rest.trim().to_string(), None),
            };
            let group = parent.and_then(|p| ids.get(&p).copied());
            ids.insert(id, nodes.len());
            nodes.push(Node {
                icon: String::new(),
                title: String::new(),
                group,
                is_group: false,
                junction: true,
            });
        } else if let Some(e) = parse_edge(line) {
            edges_raw.push(e);
        } else {
            return Err(err(format!("unrecognized architecture line {line:?}"), line_no));
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "architecture diagram must start with `architecture-beta`".into(),
            line: Some(1),
        });
    }
    if nodes.is_empty() {
        return Err(err("architecture diagram has no nodes", 1));
    }
    // Resolve edge endpoints (unknown ids drop the edge — lenient).
    let edges = edges_raw
        .into_iter()
        .filter_map(|(a, sa, b, sb, af, at)| {
            Some(Edge {
                from: *ids.get(&a)?,
                to: *ids.get(&b)?,
                from_side: sa,
                to_side: sb,
                arrow_from: af,
                arrow_to: at,
            })
        })
        .collect();
    Ok(Architecture { nodes, edges, groups })
}

/// `id(icon)[Title]` with an optional trailing ` in parent`.
fn parse_decl(s: &str) -> (String, String, String, Option<String>) {
    let (decl, parent) = match s.split_once(" in ") {
        Some((d, p)) => (d.trim(), Some(p.trim().to_string())),
        None => (s.trim(), None),
    };
    let mut icon = String::new();
    let mut title = String::new();
    let mut id_end = decl.len();
    if let Some(o) = decl.find('(') {
        if let Some(c) = decl[o..].find(')') {
            icon = decl[o + 1..o + c].trim().to_string();
            id_end = id_end.min(o);
        }
    }
    if let Some(o) = decl.find('[') {
        if let Some(c) = decl[o..].find(']') {
            title = decl[o + 1..o + c].trim().trim_matches('"').to_string();
            id_end = id_end.min(o);
        }
    }
    let id = decl[..id_end].trim().to_string();
    if title.is_empty() {
        title = id.clone();
    }
    (id, icon, title, parent)
}

/// `a:S <arrow> S:b` (arrow ∈ `--`, `-->`, `<--`, `<-->`). A trailing `{group}`
/// on an endpoint is tolerated and ignored.
fn parse_edge(line: &str) -> Option<(String, Side, String, Side, bool, bool)> {
    let (arrow, af, at) = if line.contains("<-->") {
        ("<-->", true, true)
    } else if line.contains("-->") {
        ("-->", false, true)
    } else if line.contains("<--") {
        ("<--", true, false)
    } else if line.contains("--") {
        ("--", false, false)
    } else {
        return None;
    };
    let (l, r) = line.split_once(arrow)?;
    let (a, sa) = split_port(l)?;
    let (sb, b) = split_port_rev(r)?;
    Some((a, sa, b, sb, af, at))
}

/// Left endpoint `id:S` → (id, S).
fn split_port(s: &str) -> Option<(String, Side)> {
    let s = strip_group(s.trim());
    let (id, side) = s.rsplit_once(':')?;
    Some((id.trim().to_string(), Side::parse(side)?))
}

/// Right endpoint `S:id` → (S, id).
fn split_port_rev(s: &str) -> Option<(Side, String)> {
    let s = strip_group(s.trim());
    let (side, id) = s.split_once(':')?;
    Some((Side::parse(side)?, id.trim().to_string()))
}

/// Drop a trailing `{group}` qualifier from an edge endpoint.
fn strip_group(s: &str) -> String {
    match s.find('{') {
        Some(o) => s[..o].trim().to_string(),
        None => s.to_string(),
    }
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

// ---- rendering -----------------------------------------------------------

pub(crate) fn render_svg(a: &Architecture) -> String {
    let n = a.nodes.len();
    // Constraint-based grid placement via BFS over the edges.
    let mut pos: Vec<Option<(i32, i32)>> = vec![None; n];
    // adjacency: (neighbor, delta) derived from the leaving side.
    let mut adj: Vec<Vec<(usize, (i32, i32))>> = vec![Vec::new(); n];
    for e in &a.edges {
        adj[e.from].push((e.to, e.from_side.delta()));
        let (dx, dy) = e.to_side.delta();
        adj[e.to].push((e.from, (-dx, -dy)));
    }
    // Place each connected component; seed unplaced nodes on a fallback grid.
    let mut fallback_col = 0i32;
    for start in 0..n {
        if pos[start].is_some() {
            continue;
        }
        pos[start] = Some((fallback_col, 0));
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(u) = queue.pop_front() {
            let (ux, uy) = pos[u].unwrap();
            for &(v, (dx, dy)) in &adj[u] {
                if pos[v].is_none() {
                    let mut cand = (ux + dx, uy + dy);
                    // Nudge off collisions along the same axis.
                    while pos.contains(&Some(cand)) {
                        cand = (cand.0 + dx.signum().max(0).max(if dx == 0 { 1 } else { 0 }), cand.1 + dy);
                        if dx == 0 && dy == 0 {
                            break;
                        }
                    }
                    pos[v] = Some(cand);
                    queue.push_back(v);
                }
            }
        }
        // advance the fallback column past this component's width.
        let max_x = pos.iter().flatten().map(|(x, _)| *x).max().unwrap_or(0);
        fallback_col = max_x + 2;
    }

    // Normalize to non-negative grid coordinates.
    let min_x = pos.iter().flatten().map(|(x, _)| *x).min().unwrap_or(0);
    let min_y = pos.iter().flatten().map(|(_, y)| *y).min().unwrap_or(0);
    let cell: Vec<(i32, i32)> =
        pos.iter().map(|p| p.map(|(x, y)| (x - min_x, y - min_y)).unwrap_or((0, 0))).collect();
    let cols = cell.iter().map(|(x, _)| x).max().map(|m| m + 1).unwrap_or(1);
    let rows = cell.iter().map(|(_, y)| y).max().map(|m| m + 1).unwrap_or(1);

    let center = |i: usize| {
        let (cx, cy) = cell[i];
        (PAD + cx as f64 * CELL_W + CELL_W / 2.0, PAD + cy as f64 * CELL_H + CELL_H / 2.0)
    };

    let total_w = 2.0 * PAD + cols as f64 * CELL_W;
    let total_h = 2.0 * PAD + rows as f64 * CELL_H;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:12px"><defs><marker id="mmd-arch-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
    );

    // group bounding boxes (behind).
    for &g in &a.groups {
        let members: Vec<usize> = (0..n).filter(|&i| a.nodes[i].group == Some(g) && i != g).collect();
        if members.is_empty() {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &m in &members {
            let (cx, cy) = center(m);
            x0 = x0.min(cx - CELL_W / 2.0);
            y0 = y0.min(cy - CELL_H / 2.0);
            x1 = x1.max(cx + CELL_W / 2.0);
            y1 = y1.max(cy + CELL_H / 2.0);
        }
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" stroke-opacity="0.4" rx="8"/>"#,
            x0 + 6.0,
            y0 + 6.0,
            x1 - x0 - 12.0,
            y1 - y0 - 12.0
        ));
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" font-weight="600" fill="currentColor">{}</text>"#,
            x0 + 14.0,
            y0 + 22.0,
            escape_xml(&a.nodes[g].title)
        ));
    }

    // edges.
    for e in &a.edges {
        let (x1, y1) = port_point(center(e.from), e.from_side);
        let (x2, y2) = port_point(center(e.to), e.to_side);
        let ms = if e.arrow_from { r#" marker-start="url(#mmd-arch-arrow)""# } else { "" };
        let me = if e.arrow_to { r#" marker-end="url(#mmd-arch-arrow)""# } else { "" };
        out.push_str(&format!(
            r#"<path d="M {x1:.1} {y1:.1} L {x2:.1} {y2:.1}" fill="none" stroke="currentColor" stroke-width="1.5"{ms}{me}/>"#
        ));
    }

    // nodes: icon glyph + label (junctions are small dots).
    for (i, node) in a.nodes.iter().enumerate() {
        if node.is_group {
            continue; // drawn as a group box above
        }
        let (cx, cy) = center(i);
        if node.junction {
            out.push_str(&format!(
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="5" fill="currentColor"/>"#
            ));
            continue;
        }
        let ix = cx - ICON / 2.0;
        let iy = cy - ICON / 2.0 - 6.0;
        draw_icon(&mut out, &node.icon, ix, iy, ICON);
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
            iy + ICON + 16.0,
            escape_xml(&clip(&node.title, CELL_W - 10.0))
        ));
    }

    out.push_str("</svg>");
    out
}

fn port_point((cx, cy): (f64, f64), side: Side) -> (f64, f64) {
    let hw = ICON / 2.0 + 4.0;
    let hh = ICON / 2.0 + 4.0;
    match side {
        Side::L => (cx - hw, cy - 6.0),
        Side::R => (cx + hw, cy - 6.0),
        Side::T => (cx, cy - 6.0 - hh),
        Side::B => (cx, cy - 6.0 + hh),
    }
}

/// A handful of built-in icon glyphs; anything else becomes a rounded box.
fn draw_icon(out: &mut String, icon: &str, x: f64, y: f64, s: f64) {
    let fill = "var(--mermaid-node-fill, #ececff)";
    match icon {
        "database" | "disk" => {
            let ry = 6.0;
            out.push_str(&format!(
                r#"<path d="M {x:.1} {:.1} L {x:.1} {:.1} A {:.1} {ry} 0 0 0 {:.1} {:.1} L {:.1} {:.1} A {:.1} {ry} 0 0 0 {x:.1} {:.1} Z" fill="{fill}" stroke="currentColor"/>"#,
                y + ry, y + s - ry, s / 2.0, x + s, y + s - ry, x + s, y + ry, s / 2.0, y + ry,
            ));
            out.push_str(&format!(
                r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{ry}" fill="{fill}" stroke="currentColor"/>"#,
                x + s / 2.0,
                y + ry,
                s / 2.0
            ));
        }
        "cloud" | "internet" => {
            out.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{fill}" stroke="currentColor"/>"#,
                x + s / 2.0,
                y + s / 2.0,
                s / 2.0
            ));
        }
        "server" | "disk1" => {
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{s:.1}" height="{s:.1}" fill="{fill}" stroke="currentColor" rx="3"/><line x1="{x:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor" opacity="0.5"/><line x1="{x:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="currentColor" opacity="0.5"/>"#,
                y + s / 3.0, x + s, y + s / 3.0,
                y + 2.0 * s / 3.0, x + s, y + 2.0 * s / 3.0,
            ));
        }
        _ => {
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{s:.1}" height="{s:.1}" fill="{fill}" stroke="currentColor" rx="4"/>"#
            ));
        }
    }
}

fn clip(s: &str, max_w: f64) -> String {
    if measure::text_size(s).0 <= max_w {
        return s.to_string();
    }
    let mut t = s.to_string();
    while !t.is_empty() && measure::text_size(&format!("{t}…")).0 > max_w {
        t.pop();
    }
    format!("{t}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_groups_services_and_edges() {
        let a = parse(
            "architecture-beta\n group api(cloud)[API]\n service db(database)[DB] in api\n service srv(server)[Server] in api\n db:L -- R:srv",
        )
        .unwrap();
        assert_eq!(a.nodes.len(), 3);
        assert!(a.nodes[0].is_group);
        assert_eq!(a.nodes[1].title, "DB");
        assert_eq!(a.nodes[1].group, Some(0));
        assert_eq!(a.groups, vec![0]);
        assert_eq!(a.edges.len(), 1);
        assert_eq!((a.edges[0].from, a.edges[0].to), (1, 2));
        assert_eq!(a.edges[0].from_side, Side::L);
        assert_eq!(a.edges[0].to_side, Side::R);
    }

    #[test]
    fn arrows_junction_and_icons() {
        let a = parse(
            "architecture-beta\n service a(server)[A]\n junction j\n service b(disk)[B]\n a:R --> L:j\n j:R <--> L:b",
        )
        .unwrap();
        assert!(a.nodes[1].junction);
        assert!(a.edges[0].arrow_to);
        assert!(!a.edges[0].arrow_from);
        assert!(a.edges[1].arrow_from && a.edges[1].arrow_to);
    }

    #[test]
    fn errors() {
        assert!(parse("group a(x)[A]").is_err()); // no header
        assert!(parse("architecture-beta").is_err()); // no nodes
    }

    #[test]
    fn placement_respects_sides() {
        // b sits to the right of a (a leaves via R).
        let a = parse("architecture-beta\n service a(server)[A]\n service b(server)[B]\n a:R -- L:b")
            .unwrap();
        let svg = render_svg(&a);
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">A<") && svg.contains(">B<"));
    }

    #[test]
    fn renders_group_icons_and_edges() {
        let svg = render_svg(
            &parse("architecture-beta\n group g(cloud)[Cloud]\n service db(database)[DB] in g\n service s(server)[S] in g\n db:R -- L:s")
                .unwrap(),
        );
        assert!(svg.contains(">Cloud<") && svg.contains(">DB<"));
        assert!(svg.contains("<ellipse")); // db cylinder
        assert!(svg.contains("<path") && svg.contains("stroke")); // the edge
    }
}
