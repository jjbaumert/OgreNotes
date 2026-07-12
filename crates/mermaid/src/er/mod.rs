//! Mermaid ER (entity-relationship) diagrams: model types shared by the
//! parser and the SVG renderer (Task 7).

pub(crate) mod parse;
pub(crate) mod svg;
#[cfg(test)]
mod props;

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
    pub keys: Vec<String>,       // any of "PK" / "FK" / "UK", in source order
    pub comment: Option<String>, // trailing "quoted comment", verbatim
}

#[derive(Debug, Clone)]
pub(crate) struct Entity {
    pub id: String,
    /// Display label from an `ENTITY [alias]` declaration; the canonical
    /// `id` is still what relationships reference.
    pub display: Option<String>,
    pub attributes: Vec<ErAttribute>,
    pub classes: Vec<String>,
    pub style: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ErRelation {
    pub from: usize,
    pub to: usize,
    pub card_from: Cardinality,
    pub card_to: Cardinality,
    pub identifying: bool, // -- solid vs .. dashed
    pub label: String,     // required by grammar
    pub style: Option<String>, // sanitized `linkStyle` override
}

#[derive(Debug, Clone)]
pub(crate) struct ErGraph {
    pub entities: Vec<Entity>,
    pub relations: Vec<ErRelation>,
    pub class_defs: Vec<crate::style::ClassDef>,
}

/// Per-column pixel widths — `(type, name, key, comment)` — of an entity's
/// attribute grid. This is the single source of truth shared by box sizing
/// (`render_er`) and column layout (`svg.rs`); computing them in one place
/// keeps the box from ever being narrower than the columns it must hold
/// (else a wide comment overflows and clips). Type/name/key each carry a
/// +12 inter-column gap; the trailing comment column is bare (its right
/// padding comes from the box's own margin).
pub(crate) fn attr_columns(e: &Entity) -> (f64, f64, f64, f64) {
    let mut ty = 0.0_f64;
    let mut name = 0.0_f64;
    let mut key = 0.0_f64;
    let mut comment = 0.0_f64;
    for a in &e.attributes {
        ty = ty.max(crate::measure::text_size(&a.ty).0);
        name = name.max(crate::measure::text_size(&a.name).0);
        key = key.max(crate::measure::text_size(&a.keys.join(", ")).0);
        comment = comment.max(crate::measure::text_size(a.comment.as_deref().unwrap_or("")).0);
    }
    (ty + 12.0, name + 12.0, key + 12.0, comment)
}

/// Full ER-diagram pipeline: parse -> size each entity's title +
/// attribute-grid box -> lay out via the shared `boxgraph` adapter ->
/// SVG. Never panics; a layout failure (diagram too large) surfaces as
/// a `ParseError` with no source line, same as `boxgraph::layout_boxgraph`'s
/// other consumers.
///
/// Box width: `max(text_size(title).0, sum of the per-column widths from
/// `attr_columns`) + 24`, floored at 100 — matching the column offsets
/// `svg.rs` lays out. `title` is the entity's alias when present, else its
/// id. Box height: `(1 + attrs.len()) * (LINE_H + 6) + 10` (title row
/// plus one row per attribute).
pub(crate) fn render_er(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;

    let mut sizes = Vec::with_capacity(g.entities.len());
    let mut nodes = Vec::with_capacity(g.entities.len());
    for e in &g.entities {
        let title_w = crate::measure::text_size(e.display.as_deref().unwrap_or(&e.id)).0;
        // Sum the same per-column widths svg.rs positions with, so the box
        // always contains its widest column stack (incl. the comment).
        let (ty_col, name_col, key_col, comment_col) = attr_columns(e);
        let attrs_w =
            if e.attributes.is_empty() { 0.0 } else { ty_col + name_col + key_col + comment_col };
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
