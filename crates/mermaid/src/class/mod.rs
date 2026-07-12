//! Mermaid class diagrams: model types shared by the parser and the SVG
//! renderer (this slice).

pub(crate) mod parse;
pub(crate) mod svg;
#[cfg(test)]
mod props;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelKind {
    Inheritance, // <|--   marker at `to` end, solid
    Realization, // <|..   marker at `to` end, dashed
    Composition, // *--    filled diamond at `to` end, solid
    Aggregation, // o--    hollow diamond at `to` end, solid
    Association, // --> or --  open arrow at `to` end (--> only), solid
    Dependency,  // ..>    open arrow at `to` end, dashed
    DashedLink,  // ..     no marker, dashed (the dashed peer of `--`)
}

#[derive(Debug, Clone)]
pub(crate) struct ClassBox {
    pub id: String,
    /// Display label from a `class Name["Label"]` declaration; the
    /// canonical `id` is still what members and relationships reference.
    pub display: Option<String>,
    pub annotation: Option<String>, // <<interface>> etc.
    pub attributes: Vec<String>,    // raw member text, verbatim
    pub methods: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Relation {
    pub from: usize,
    pub to: usize,   // the MARKER end (normalized during parse)
    pub kind: RelKind,
    pub arrow: bool, // Association `--` (false) vs `-->` (true)
    pub m_from: Option<String>, // multiplicity near `from`
    pub m_to: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassGraph {
    pub classes: Vec<ClassBox>,
    pub relations: Vec<Relation>,
}

/// Full class-diagram pipeline: parse -> size each class's compartment
/// box (name [+ annotation] / attributes / methods) -> lay out via the
/// shared `boxgraph` adapter -> SVG. Never panics; a layout failure
/// (diagram too large) surfaces as a `ParseError` with no source line,
/// same as `boxgraph::layout_boxgraph`'s other consumers.
pub(crate) fn render_class(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;

    let mut sizes = Vec::with_capacity(g.classes.len());
    let mut nodes = Vec::with_capacity(g.classes.len());
    for c in &g.classes {
        // Same line set the renderer draws: annotation (as `«name»`) if
        // present, then the id, then attributes, then methods verbatim.
        let mut lines: Vec<String> = Vec::new();
        if let Some(ann) = &c.annotation {
            lines.push(format!("«{ann}»"));
        }
        lines.push(c.display.clone().unwrap_or_else(|| c.id.clone()));
        lines.extend(c.attributes.iter().cloned());
        lines.extend(c.methods.iter().cloned());

        let max_w = lines
            .iter()
            .map(|l| crate::measure::text_size(l).0)
            .fold(0.0_f64, f64::max);
        let width = (max_w + 24.0).max(80.0);
        let height = lines.len() as f64 * (crate::measure::LINE_H + 4.0) + 16.0 + 8.0;

        sizes.push((width, height));
        nodes.push(crate::boxgraph::BoxNode { width, height, cluster: None });
    }

    let edges: Vec<crate::boxgraph::BoxEdge> = g
        .relations
        .iter()
        .map(|r| crate::boxgraph::BoxEdge {
            from: r.from,
            to: r.to,
            label: r.label.as_deref().map(|l| {
                let (w, h) = crate::measure::text_size(l);
                (w + 8.0, h + 4.0)
            }),
        })
        .collect();

    let layout = crate::boxgraph::layout_boxgraph(
        &nodes,
        &edges,
        &[],
        crate::layout::Direction::TB,
    )?;
    Ok(svg::emit(&g, &layout, &sizes))
}
