//! Mermaid `requirementDiagram`: parser + SVG renderer (Tier 3).
//!
//! Requirement and element boxes joined by typed relationships. A
//! `requirement Name { id: … text: … risk: … verifymethod: … }` block (or
//! any `…Requirement` keyword) and an `element Name { type: … docref: … }`
//! block become boxes; `A - <type> -> B` (and `A <- <type> - B`) become
//! labeled arrows. Laid out via the shared boxgraph adapter.

use crate::{escape_xml, measure, ParseError};

#[derive(Debug, Clone)]
pub(crate) struct ReqNode {
    pub title: String,      // «type» / name compartment
    pub subtitle: String,   // the node name
    pub body: Vec<String>,  // field lines
}

#[derive(Debug, Clone)]
pub(crate) struct ReqRelation {
    pub from: usize,
    pub to: usize,
    pub kind: String, // satisfies / contains / traces / …
}

#[derive(Debug, Clone)]
pub(crate) struct ReqGraph {
    pub nodes: Vec<ReqNode>,
    pub relations: Vec<ReqRelation>,
}

const REL_KINDS: &[&str] =
    &["contains", "copies", "derives", "satisfies", "verifies", "refines", "traces"];

struct Parser {
    g: ReqGraph,
    ids: std::collections::HashMap<String, usize>,
    line: usize,
    open: Option<usize>, // index of the node whose `{ }` block is open
}

pub(crate) fn parse(source: &str) -> Result<ReqGraph, ParseError> {
    let mut p = Parser {
        g: ReqGraph { nodes: Vec::new(), relations: Vec::new() },
        ids: std::collections::HashMap::new(),
        line: 0,
        open: None,
    };
    let mut seen_header = false;

    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            if line.strip_suffix(';').unwrap_or(line).trim_end() != "requirementDiagram" {
                return Err(p.err("requirement diagram must start with `requirementDiagram`"));
            }
            seen_header = true;
            continue;
        }
        if p.open.is_some() {
            p.block_line(line)?;
        } else {
            p.statement(line)?;
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "requirement diagram must start with `requirementDiagram`".into(),
            line: Some(1),
        });
    }
    if p.open.is_some() {
        return Err(p.err("unclosed `{` block"));
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, m: impl Into<String>) -> ParseError {
        ParseError { message: m.into(), line: Some(self.line) }
    }

    fn ensure(&mut self, name: &str, title: String, subtitle: String) -> usize {
        if let Some(&i) = self.ids.get(name) {
            return i;
        }
        let i = self.g.nodes.len();
        self.g.nodes.push(ReqNode { title, subtitle, body: Vec::new() });
        self.ids.insert(name.to_string(), i);
        i
    }

    fn statement(&mut self, line: &str) -> Result<(), ParseError> {
        // A relationship: `A - kind -> B` or `A <- kind - B`.
        if let Some(rel) = self.try_relation(line)? {
            self.g.relations.push(rel);
            return Ok(());
        }
        // A `keyword Name {` block opener.
        let opener = line.strip_suffix('{').unwrap_or(line).trim();
        let (kw, name) = opener.split_once(char::is_whitespace).unwrap_or((opener, ""));
        let name = name.trim();
        if name.is_empty() || !line.trim_end().ends_with('{') {
            return Err(self.err(format!("unrecognized statement {line:?}")));
        }
        let (title, sub) = if kw == "element" {
            (String::new(), name.to_string())
        } else if kw == "requirement" || kw.ends_with("Requirement") {
            (format!("«{}»", kw), name.to_string())
        } else {
            return Err(self.err(format!("unknown block keyword `{kw}`")));
        };
        let idx = self.ensure(name, title, sub);
        self.open = Some(idx);
        Ok(())
    }

    fn block_line(&mut self, line: &str) -> Result<(), ParseError> {
        if line == "}" {
            self.open = None;
            return Ok(());
        }
        // `key: value` field — stored verbatim as a body line.
        let idx = self.open.expect("block open");
        if let Some((k, v)) = line.split_once(':') {
            self.g.nodes[idx].body.push(format!("{}: {}", k.trim(), v.trim()));
        }
        Ok(())
    }

    /// `A - kind -> B` (or `A <- kind - B`). Returns `None` when `line` is not
    /// a relationship.
    fn try_relation(&mut self, line: &str) -> Result<Option<ReqRelation>, ParseError> {
        // Reverse (`<src> <- <kind> - <dst>`) is checked first: its ` - `
        // would otherwise be captured by the forward split below.
        let (src, dst, kind, reverse) = if let Some((a, rest)) = line.split_once(" <- ") {
            match rest.split_once(" - ") {
                Some((k, b)) => (a.trim(), b.trim(), k.trim(), true),
                None => return Ok(None),
            }
        } else if let Some((a, rest)) = line.split_once(" - ") {
            match rest.split_once(" -> ") {
                Some((k, b)) => (a.trim(), b.trim(), k.trim(), false),
                None => return Ok(None),
            }
        } else {
            return Ok(None);
        };
        if !REL_KINDS.contains(&kind) {
            return Err(self.err(format!("unknown relationship type `{kind}`")));
        }
        let s = self.ensure(src, String::new(), src.to_string());
        let d = self.ensure(dst, String::new(), dst.to_string());
        // `<-` reverses the arrow direction (dst points at src).
        let (from, to) = if reverse { (d, s) } else { (s, d) };
        Ok(Some(ReqRelation { from, to, kind: kind.to_string() }))
    }
}

pub(crate) fn render_svg(g: &ReqGraph) -> Result<String, ParseError> {
    let line_h = measure::LINE_H + 4.0;
    let mut sizes = Vec::with_capacity(g.nodes.len());
    let nodes: Vec<crate::boxgraph::BoxNode> = g
        .nodes
        .iter()
        .map(|n| {
            let mut lines: Vec<&str> = Vec::new();
            if !n.title.is_empty() {
                lines.push(&n.title);
            }
            lines.push(&n.subtitle);
            lines.extend(n.body.iter().map(String::as_str));
            let w = lines.iter().map(|l| measure::text_size(l).0).fold(0.0_f64, f64::max) + 24.0;
            let w = w.max(90.0);
            let h = lines.len() as f64 * line_h + 14.0;
            sizes.push((w, h));
            crate::boxgraph::BoxNode { width: w, height: h, cluster: None }
        })
        .collect();
    let edges: Vec<crate::boxgraph::BoxEdge> = g
        .relations
        .iter()
        .map(|r| {
            let (w, h) = measure::text_size(&r.kind);
            crate::boxgraph::BoxEdge { from: r.from, to: r.to, label: Some((w + 8.0, h + 4.0)) }
        })
        .collect();

    let l = crate::boxgraph::layout_boxgraph(&nodes, &edges, &[], crate::layout::Direction::TB)?;

    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px"><defs><marker id="mmd-req-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10" stroke="currentColor" stroke-width="1.2" fill="none"/></marker></defs>"#
    );

    // edges (dashed, open arrowhead) with the relationship label.
    for ep in &l.edge_paths {
        let r = &g.relations[ep.edge];
        let d = crate::curved_path(&ep.points, true);
        out.push_str(&format!(
            r#"<path d="{d}" fill="none" stroke="currentColor" stroke-dasharray="4 3" marker-end="url(#mmd-req-arrow)"/>"#
        ));
        if let Some((lx, ly)) = ep.label_at {
            let (tw, th) = measure::text_size(&r.kind);
            out.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--surface, #fff)"/>"#,
                lx - tw / 2.0 - 2.0,
                ly - th / 2.0 - 2.0,
                tw + 4.0,
                th + 4.0
            ));
            out.push_str(&format!(
                r#"<text x="{lx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">«{}»</text>"#,
                ly + 4.0,
                escape_xml(&r.kind)
            ));
        }
    }

    // node boxes: title + subtitle (name) + body lines.
    for (i, n) in g.nodes.iter().enumerate() {
        let (cx, cy) = l.node_centers[i];
        let (bw, bh) = sizes[i];
        let (bx, by) = (cx - bw / 2.0, cy - bh / 2.0);
        out.push_str(&format!(
            r#"<rect x="{bx:.1}" y="{by:.1}" width="{bw:.1}" height="{bh:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"/>"#
        ));
        let mut y = by + 8.0;
        if !n.title.is_empty() {
            y += line_h;
            out.push_str(&format!(
                r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-style="italic" fill="currentColor">{}</text>"#,
                y - 4.0,
                escape_xml(&n.title)
            ));
        }
        y += line_h;
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" font-weight="600" fill="currentColor">{}</text>"#,
            y - 4.0,
            escape_xml(&n.subtitle)
        ));
        // Mermaid separates the «type»/name header compartment from the
        // field body with a full-width horizontal rule.
        if !n.body.is_empty() {
            let ry = y + 2.0;
            out.push_str(&format!(
                r#"<line x1="{bx:.1}" y1="{ry:.1}" x2="{:.1}" y2="{ry:.1}" stroke="currentColor"/>"#,
                bx + bw
            ));
        }
        for b in &n.body {
            y += line_h;
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" fill="currentColor">{}</text>"#,
                bx + 8.0,
                y - 4.0,
                escape_xml(b)
            ));
        }
    }

    out.push_str("</svg>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requirements_elements_and_relations() {
        let g = parse(
            "requirementDiagram\n requirement r1 {\n id: 1\n text: must work\n risk: high\n }\n element e1 {\n type: simulation\n }\n e1 - satisfies -> r1",
        )
        .unwrap();
        assert_eq!(g.nodes.len(), 2);
        let r = &g.nodes[0];
        assert_eq!(r.title, "«requirement»");
        assert_eq!(r.subtitle, "r1");
        assert!(r.body.contains(&"id: 1".to_string()) && r.body.contains(&"risk: high".to_string()));
        assert_eq!(g.relations.len(), 1);
        assert_eq!(g.relations[0].kind, "satisfies");
    }

    #[test]
    fn reverse_arrow_and_bad_kind() {
        // `a <- traces - b` points from b to a. Nodes are created in first-
        // mention order: a=0, b=1.
        let g = parse("requirementDiagram\n requirement a {\n }\n element b {\n }\n a <- traces - b").unwrap();
        assert_eq!(g.relations[0].kind, "traces");
        assert_eq!((g.relations[0].from, g.relations[0].to), (1, 0)); // b -> a
        // an unknown relationship type errors.
        assert!(parse("requirementDiagram\n requirement a {\n }\n element b {\n }\n a - bogus -> b").is_err());
    }

    #[test]
    fn header_and_unclosed_block() {
        assert!(parse("requirement r {\n}").is_err());
        assert!(parse("requirementDiagram\n requirement r {").is_err());
    }

    #[test]
    fn renders_boxes_and_labeled_edges() {
        let svg = render_svg(
            &parse("requirementDiagram\n requirement r1 {\n id: 1\n }\n element e1 {\n type: sim\n }\n e1 - satisfies -> r1")
                .unwrap(),
        )
        .unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains(">r1<") && svg.contains(">e1<"));
        assert!(svg.contains("«satisfies»") && svg.contains("«requirement»"));
    }

    #[test]
    fn compartment_rule_only_when_fields_exist() {
        // Boxes with body fields get a full-width divider under the name
        // header (mermaid's compartment rule); field-less boxes don't.
        let with_fields = render_svg(
            &parse("requirementDiagram\n requirement r1 {\n id: 1\n }\n element e1 {\n }\n e1 - satisfies -> r1").unwrap(),
        )
        .unwrap();
        assert_eq!(
            with_fields.matches("<line x1=").count(),
            1,
            "exactly one divider (r1 has fields, e1 doesn't): {with_fields}"
        );
        let no_fields = render_svg(
            &parse("requirementDiagram\n requirement r1 {\n }\n element e1 {\n }\n e1 - satisfies -> r1").unwrap(),
        )
        .unwrap();
        assert!(!no_fields.contains("<line x1="), "no divider without fields: {no_fields}");
    }
}
