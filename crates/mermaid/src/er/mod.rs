//! Mermaid ER (entity-relationship) diagrams: model types shared by the
//! parser and the SVG renderer (Task 7).

// TODO(slice4): removed in Task 8
#![allow(dead_code)]
pub(crate) mod parse;
pub(crate) mod svg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cardinality {
    ExactlyOne, // ||
    ZeroOrOne,  // |o / o|
    OneOrMore,  // }| / |{
    ZeroOrMore, // }o / o{
}

#[derive(Debug, Clone)]
pub(crate) struct ErAttribute {
    pub ty: String,
    pub name: String,
    pub key: Option<String>, // "PK" | "FK"
}

#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub id: String,
    pub attributes: Vec<ErAttribute>,
}

#[derive(Debug, Clone)]
pub(crate) struct ErRelation {
    pub from: usize,
    pub to: usize,
    pub card_from: Cardinality,
    pub card_to: Cardinality,
    pub identifying: bool, // -- solid vs .. dashed
    pub label: String,     // required by grammar
}

#[derive(Debug, Clone)]
pub(crate) struct ErGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<ErRelation>,
}

/// Full ER-diagram pipeline: parse -> size each entity's title +
/// attribute-grid box -> lay out via the shared `boxgraph` adapter ->
/// SVG. Never panics; a layout failure (diagram too large) surfaces as
/// a `ParseError` with no source line, same as `boxgraph::layout_boxgraph`'s
/// other consumers.
///
/// Box width per the brief: `max(text_size(id).0, max over attrs of
/// (type_w + name_w + key_w + 3*12 column gaps)) + 24`, floored at 100.
/// Box height: `(1 + attrs.len()) * (LINE_H + 6) + 10` (title row plus
/// one row per attribute).
pub(crate) fn render_er(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;

    let mut sizes = Vec::with_capacity(g.entities.len());
    let mut nodes = Vec::with_capacity(g.entities.len());
    for e in &g.entities {
        let title_w = crate::measure::text_size(&e.id).0;
        let attrs_w = e
            .attributes
            .iter()
            .map(|a| {
                let ty_w = crate::measure::text_size(&a.ty).0;
                let name_w = crate::measure::text_size(&a.name).0;
                let key_w = crate::measure::text_size(a.key.as_deref().unwrap_or("")).0;
                ty_w + name_w + key_w + 3.0 * 12.0
            })
            .fold(0.0_f64, f64::max);
        let width = (title_w.max(attrs_w) + 24.0).max(100.0);
        let height = (1.0 + e.attributes.len() as f64) * (crate::measure::LINE_H + 6.0) + 10.0;

        sizes.push((width, height));
        nodes.push(crate::boxgraph::BoxNode { width, height, cluster: None });
    }

    let edges: Vec<crate::boxgraph::BoxEdge> = g
        .relations
        .iter()
        .map(|r| {
            let (w, h) = crate::measure::text_size(&r.label);
            crate::boxgraph::BoxEdge { from: r.from, to: r.to, label: Some((w + 8.0, h + 4.0)) }
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
