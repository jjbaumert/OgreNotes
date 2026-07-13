//! Mermaid C4 diagrams (`C4Context` / `C4Container` / `C4Component` /
//! `C4Dynamic` / `C4Deployment`): parser + SVG renderer (Tier 5).
//!
//! One model covers every C4 header. Element declarations
//! (`Person(alias, "Label", "Descr")`, `System(...)`, `Container(...)`,
//! `Component(...)`, plus `_Ext` / `Db` / `Queue` variants) become boxes;
//! `*_Boundary(alias, "Label") { … }` (and deployment `Node`s) become dashed
//! clusters; `Rel(from, to, "Label", "Tech")` becomes a labeled arrow. Laid
//! out with the shared boxgraph adapter. Styling directives (`Update*`) are
//! accepted and ignored.

use crate::{escape_xml, measure, ParseError};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Person,
    Db,
    Queue,
    Box,
}

#[derive(Debug, Clone)]
pub(crate) struct El {
    pub kind: String, // display tag: "Person", "Container", "System_Ext", …
    pub name: String,
    pub descr: Vec<String>, // technology + description lines
    shape: Shape,
    external: bool,
    tier: u8, // 0 person, 1 system, 2 container, 3 component/node
    boundary: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Boundary {
    pub label: String,
    parent: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct Rel {
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub tech: Option<String>,
    pub bidir: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct C4 {
    pub title: Option<String>,
    pub els: Vec<El>,
    pub boundaries: Vec<Boundary>,
    pub rels: Vec<Rel>,
}

pub(crate) fn parse(source: &str) -> Result<C4, ParseError> {
    let mut c4 = C4 { title: None, els: Vec::new(), boundaries: Vec::new(), rels: Vec::new() };
    let mut ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut stack: Vec<usize> = Vec::new(); // open boundary indices
    let mut seen_header = false;
    let mut line_no = 0;

    for (idx, raw) in source.lines().enumerate() {
        line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if !is_c4_header(h) {
                return Err(err("C4 diagram must start with a `C4…` header", line_no));
            }
            seen_header = true;
            continue;
        }
        // Close the innermost boundary.
        if line == "}" {
            if stack.pop().is_none() {
                return Err(err("unmatched `}`", line_no));
            }
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            c4.title = Some(t.trim().to_string());
            continue;
        }
        // Ignore styling / layout directives.
        if line.starts_with("Update") || line.starts_with("UpdateLayoutConfig") {
            continue;
        }
        let Some((kw, args_str)) = split_call(line) else {
            return Err(err(format!("unrecognized C4 line {line:?}"), line_no));
        };
        let args = split_args(args_str);

        if kw.ends_with("Boundary") || kw == "Node" || kw == "Deployment_Node" {
            // Boundary opener: `Kind(alias, "Label"[, …]) {`
            let alias = args.first().cloned().unwrap_or_default();
            let label = args.get(1).cloned().unwrap_or_else(|| alias.clone());
            let bi = c4.boundaries.len();
            c4.boundaries.push(Boundary { label, parent: stack.last().copied() });
            if !alias.is_empty() {
                ids.insert(alias, usize::MAX - bi); // boundary marker (not linkable as node)
            }
            stack.push(bi);
            continue;
        }
        if is_rel(kw) {
            // `Rel(from, to, "Label"[, "Tech"])`
            if args.len() < 2 {
                return Err(err(format!("{kw} needs a source and target"), line_no));
            }
            let from = *ids.get(&args[0]).filter(|v| **v < usize::MAX / 2).ok_or_else(|| {
                err(format!("{kw} references unknown element `{}`", args[0]), line_no)
            })?;
            let to = *ids.get(&args[1]).filter(|v| **v < usize::MAX / 2).ok_or_else(|| {
                err(format!("{kw} references unknown element `{}`", args[1]), line_no)
            })?;
            let label = args.get(2).cloned().unwrap_or_default();
            let tech = args.get(3).cloned().filter(|s| !s.is_empty());
            c4.rels.push(Rel { from, to, label, tech, bidir: kw.starts_with("BiRel") });
            continue;
        }
        // Otherwise an element declaration.
        let (shape, external, tier) = classify(kw);
        let alias = args.first().cloned().unwrap_or_default();
        if alias.is_empty() {
            return Err(err(format!("{kw} needs an alias"), line_no));
        }
        let name = args.get(1).cloned().unwrap_or_else(|| alias.clone());
        let descr: Vec<String> =
            args.iter().skip(2).filter(|s| !s.is_empty() && !s.starts_with('$')).cloned().collect();
        let ei = c4.els.len();
        ids.insert(alias, ei);
        c4.els.push(El {
            kind: kw.to_string(),
            name,
            descr,
            shape,
            external,
            tier,
            boundary: stack.last().copied(),
        });
    }
    if !seen_header {
        return Err(ParseError {
            message: "C4 diagram must start with a `C4…` header".into(),
            line: Some(1),
        });
    }
    if !stack.is_empty() {
        return Err(err("unclosed boundary `{`", line_no));
    }
    if c4.els.is_empty() {
        return Err(err("C4 diagram has no elements", 1));
    }
    Ok(c4)
}

fn is_c4_header(h: &str) -> bool {
    matches!(h, "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment")
}

fn is_rel(kw: &str) -> bool {
    matches!(
        kw,
        "Rel" | "BiRel" | "Rel_Up" | "Rel_U" | "Rel_Down" | "Rel_D" | "Rel_Left" | "Rel_L"
            | "Rel_Right" | "Rel_R" | "Rel_Back" | "Rel_Back_Neighbor" | "Rel_Neighbor"
            | "BiRel_Up" | "BiRel_Down" | "BiRel_Left" | "BiRel_Right"
    )
}

/// Map an element keyword to (shape, is-external, tier).
fn classify(kw: &str) -> (Shape, bool, u8) {
    let external = kw.contains("_Ext");
    let base = kw.replace("_Ext", "");
    let shape = if base.starts_with("Person") {
        Shape::Person
    } else if base.contains("Db") {
        Shape::Db
    } else if base.contains("Queue") {
        Shape::Queue
    } else {
        Shape::Box
    };
    let tier = if base.starts_with("Person") {
        0
    } else if base.starts_with("System") {
        1
    } else if base.starts_with("Container") {
        2
    } else {
        3 // Component / Node / other
    };
    (shape, external, tier)
}

/// Split `Kind(args…)` into (`Kind`, `args…`). Returns None if there's no
/// balanced call on the line.
fn split_call(line: &str) -> Option<(&str, &str)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let kw = line[..open].trim();
    if kw.is_empty() || !kw.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some((kw, &line[open + 1..close]))
}

/// Split a C4 argument list on top-level commas, honoring `"quotes"`, and
/// strip surrounding quotes/whitespace from each field.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        match c {
            '"' => in_q = !in_q,
            ',' if !in_q => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|f| f.trim().trim_matches('"').trim().to_string()).collect()
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

// ---- rendering -----------------------------------------------------------

const LINE_H: f64 = 16.0;
// Text colors on the colored element fills. Kept as consts so the `#` never
// sits directly after a `"` in a raw string literal (which would close it).
const TXT_NAME: &str = "#fff";
const TXT_TAG: &str = "#e8eef7";
const TXT_DESC: &str = "#eaf1fb";

/// Fill for an element by tier / external flag (C4 blues; externals gray).
fn fill_for(el: &El) -> &'static str {
    if el.external {
        return "#8598b0";
    }
    match el.tier {
        0 => "#08427b", // person — darkest
        1 => "#1168bd", // system
        2 => "#438dd5", // container
        _ => "#85bbf0", // component / node
    }
}

pub(crate) fn render_svg(c4: &C4) -> Result<String, ParseError> {
    // Build boxgraph inputs. Element boxes sized from their text.
    let mut sizes = Vec::with_capacity(c4.els.len());
    let nodes: Vec<crate::boxgraph::BoxNode> = c4
        .els
        .iter()
        .map(|e| {
            let mut widest = measure::text_size(&e.name).0;
            widest = widest.max(measure::text_size(&format!("[{}]", e.kind)).0);
            for d in &e.descr {
                widest = widest.max(measure::text_size(d).0);
            }
            let w = (widest + 28.0).max(120.0).min(240.0);
            let head = if e.shape == Shape::Person { 14.0 } else { 0.0 };
            let h = (2 + e.descr.len()) as f64 * LINE_H + 20.0 + head;
            sizes.push((w, h));
            crate::boxgraph::BoxNode { width: w, height: h, cluster: e.boundary }
        })
        .collect();
    let edges: Vec<crate::boxgraph::BoxEdge> = c4
        .rels
        .iter()
        .map(|r| {
            let (w, h) = measure::text_size(&r.label);
            crate::boxgraph::BoxEdge { from: r.from, to: r.to, label: Some((w + 8.0, h + 4.0)) }
        })
        .collect();
    let clusters: Vec<crate::boxgraph::BoxCluster> = c4
        .boundaries
        .iter()
        .map(|b| crate::boxgraph::BoxCluster {
            parent: b.parent,
            title: (measure::text_size(&b.label).0 + 16.0, LINE_H + 6.0),
        })
        .collect();

    let l = crate::boxgraph::layout_boxgraph(&nodes, &edges, &clusters, crate::layout::Direction::TB)?;
    let (mut w, mut h) = l.size;
    let title_h = if c4.title.is_some() { 30.0 } else { 0.0 };
    h += title_h;
    w = w.max(200.0);

    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:13px"><defs><marker id="mmd-c4-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
    );
    let dy = title_h;

    if let Some(title) = &c4.title {
        out.push_str(&format!(
            r#"<text x="{:.1}" y="20" text-anchor="middle" font-weight="bold" font-size="17" fill="currentColor">{}</text>"#,
            w / 2.0,
            escape_xml(title)
        ));
    }

    // boundaries (dashed clusters) behind everything.
    for (i, b) in c4.boundaries.iter().enumerate() {
        let r = &l.cluster_rects[i];
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="none" stroke="currentColor" stroke-dasharray="6 4" rx="4" opacity="0.7"/>"#,
            r.x, r.y + dy, r.w, r.h
        ));
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" font-weight="600" fill="currentColor">{}</text>"#,
            r.x + 8.0,
            r.y + dy + LINE_H,
            escape_xml(&b.label)
        ));
    }

    // relationship arrows.
    for ep in &l.edge_paths {
        let r = &c4.rels[ep.edge];
        let pts: Vec<(f64, f64)> = ep.points.iter().map(|(x, y)| (*x, y + dy)).collect();
        let d = crate::curved_path(&pts, true);
        let start_marker = if r.bidir { r#" marker-start="url(#mmd-c4-arrow)""# } else { "" };
        out.push_str(&format!(
            r#"<path d="{d}" fill="none" stroke="currentColor" stroke-dasharray="3 3"{start_marker} marker-end="url(#mmd-c4-arrow)"/>"#
        ));
        if let Some((lx, ly)) = ep.label_at {
            let mut txt = r.label.clone();
            if let Some(t) = &r.tech {
                txt = format!("{txt} [{t}]");
            }
            let (tw, _) = measure::text_size(&txt);
            out.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="16" fill="var(--surface, #fff)"/><text x="{lx:.1}" y="{:.1}" text-anchor="middle" font-size="11" fill="currentColor">{}</text>"#,
                lx - tw / 2.0 - 3.0,
                ly + dy - 8.0,
                tw + 6.0,
                ly + dy + 4.0,
                escape_xml(&txt)
            ));
        }
    }

    // element boxes.
    for (i, e) in c4.els.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let cy = cy + dy;
        let (bw, bh) = sizes[i];
        let (bx, by) = (cx - bw / 2.0, cy - bh / 2.0);
        let fill = fill_for(e);
        draw_shape(&mut out, e.shape, bx, by, bw, bh, fill);
        // text (white on the colored fill). Person boxes leave room for a head.
        let mut ty = by + 16.0 + if e.shape == Shape::Person { 14.0 } else { 0.0 };
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{ty:.1}" text-anchor="middle" font-weight="700" fill="{TXT_NAME}">{}</text>"#,
            escape_xml(&e.name)
        ));
        ty += LINE_H;
        let tag = if e.external { format!("[{}]", e.kind.replace('_', " ")) } else { format!("[{}]", e.kind) };
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{ty:.1}" text-anchor="middle" font-size="11" font-style="italic" fill="{TXT_TAG}">{}</text>"#,
            escape_xml(&tag)
        ));
        for d in &e.descr {
            ty += LINE_H;
            out.push_str(&format!(
                r#"<text x="{cx:.1}" y="{ty:.1}" text-anchor="middle" font-size="11" fill="{TXT_DESC}">{}</text>"#,
                escape_xml(d)
            ));
        }
    }

    out.push_str("</svg>");
    Ok(out)
}

fn draw_shape(out: &mut String, shape: Shape, x: f64, y: f64, w: f64, h: f64, fill: &str) {
    match shape {
        Shape::Person => {
            // head circle + rounded body box.
            let hr = 11.0;
            out.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="{hr}" fill="{fill}"/>"#,
                x + w / 2.0,
                y + hr + 1.0
            ));
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{:.1}" width="{w:.1}" height="{:.1}" fill="{fill}" rx="8"/>"#,
                y + 2.0 * hr,
                h - 2.0 * hr
            ));
        }
        Shape::Db => {
            // cylinder: body rect + top/bottom ellipses.
            let ry = 7.0;
            out.push_str(&format!(
                r#"<path d="M {x:.1} {:.1} L {x:.1} {:.1} A {:.1} {ry} 0 0 0 {:.1} {:.1} L {:.1} {:.1} A {:.1} {ry} 0 0 0 {x:.1} {:.1} Z" fill="{fill}"/>"#,
                y + ry, y + h - ry, w / 2.0, x + w, y + h - ry, x + w, y + ry, w / 2.0, y + ry,
            ));
            out.push_str(&format!(
                r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{ry}" fill="{fill}" stroke="{TXT_NAME}" stroke-opacity="0.4"/>"#,
                x + w / 2.0,
                y + ry,
                w / 2.0
            ));
        }
        Shape::Queue => {
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}" rx="{:.1}"/>"#,
                h / 2.0
            ));
        }
        Shape::Box => {
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" fill="{fill}" rx="3"/>"#
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_elements_boundaries_and_rels() {
        let c4 = parse(
            "C4Context\n title Sys\n Person(cust, \"Customer\", \"A user\")\n System_Boundary(b1, \"Internal\") {\n   System(sys, \"Banking\", \"Core\")\n }\n System_Ext(mail, \"Mail\")\n Rel(cust, sys, \"Uses\", \"HTTPS\")\n Rel(sys, mail, \"Sends\")",
        )
        .unwrap();
        assert_eq!(c4.title.as_deref(), Some("Sys"));
        assert_eq!(c4.els.len(), 3); // cust, sys, mail
        assert_eq!(c4.els[0].name, "Customer");
        assert_eq!(c4.els[0].shape, Shape::Person);
        assert!(c4.els[2].external);
        assert_eq!(c4.els[1].boundary, Some(0)); // sys is inside b1
        assert_eq!(c4.boundaries.len(), 1);
        assert_eq!(c4.rels.len(), 2);
        assert_eq!((c4.rels[0].from, c4.rels[0].to), (0, 1));
        assert_eq!(c4.rels[0].tech.as_deref(), Some("HTTPS"));
    }

    #[test]
    fn all_c4_headers_and_db_shape() {
        for h in ["C4Context", "C4Container", "C4Component", "C4Dynamic", "C4Deployment"] {
            assert!(parse(&format!("{h}\n Person(a, \"A\")")).is_ok(), "{h}");
        }
        let c4 = parse("C4Container\n ContainerDb(db, \"DB\", \"Postgres\")").unwrap();
        assert_eq!(c4.els[0].shape, Shape::Db);
        assert_eq!(c4.els[0].descr, vec!["Postgres".to_string()]);
    }

    #[test]
    fn errors() {
        assert!(parse("Person(a, \"A\")").is_err()); // no header
        assert!(parse("C4Context").is_err()); // no elements
        assert!(parse("C4Context\n System_Boundary(b, \"B\") {\n System(s, \"S\")").is_err()); // unclosed
        assert!(parse("C4Context\n Person(a,\"A\")\n Rel(a, ghost, \"x\")").is_err()); // unknown target
    }

    #[test]
    fn renders_shapes_boundary_and_rel() {
        let svg = render_svg(
            &parse("C4Context\n title T\n Person(c, \"Cust\")\n System_Boundary(b, \"Bank\") {\n System(s, \"Core\")\n }\n Rel(c, s, \"Uses\")")
                .unwrap(),
        )
        .unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">T<") && svg.contains(">Cust<") && svg.contains(">Bank<"));
        assert!(svg.contains("<circle")); // the person head
        assert!(svg.contains("stroke-dasharray=\"6 4\"")); // the boundary
        assert!(svg.contains("marker-end")); // the rel arrow
    }
}
