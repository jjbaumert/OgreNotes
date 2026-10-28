//! Rendered geometry contracts. These run in the ordinary CI library tests.
//! Node labels belong inside their own shapes; foreign labels and edge curves
//! must remain outside node interiors. Bounded-error curves come from emitted
//! SVG, not the layout polyline that an SVG emitter can accidentally change.

use crate::extent::{self, Box2};
use proptest::prelude::*;
use quick_xml::{events::Event, Reader};

fn contains(outer: Box2, inner: Box2, tolerance: f64) -> bool {
    inner.x0 >= outer.x0 - tolerance
        && inner.y0 >= outer.y0 - tolerance
        && inner.x1 <= outer.x1 + tolerance
        && inner.y1 <= outer.y1 + tolerance
}

struct NodeShape {
    bbox: Box2,
    polygon: Vec<(f64, f64)>,
    ellipse: bool,
    labels: Vec<extent::Drawn>,
}

impl NodeShape {
    fn interior(&self, x: f64, y: f64) -> bool {
        let b = self.bbox;
        if self.ellipse {
            let (rx, ry) = ((b.x1 - b.x0) / 2.0 - 2.0, (b.y1 - b.y0) / 2.0 - 2.0);
            return rx > 0.0
                && ry > 0.0
                && ((x - (b.x0 + b.x1) / 2.0) / rx).powi(2)
                    + ((y - (b.y0 + b.y1) / 2.0) / ry).powi(2)
                    < 1.0;
        }
        if self.polygon.is_empty() {
            return x > b.x0 + 2.0 && x < b.x1 - 2.0 && y > b.y0 + 2.0 && y < b.y1 - 2.0;
        }
        let mut inside = false;
        let mut previous = *self.polygon.last().unwrap();
        for &next in &self.polygon {
            let (dx, dy) = (next.0 - previous.0, next.1 - previous.1);
            let length = dx * dx + dy * dy;
            let t = if length > 0.0 {
                (((x - previous.0) * dx + (y - previous.1) * dy) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if (x - previous.0 - t * dx).powi(2) + (y - previous.1 - t * dy).powi(2) <= 4.0 {
                return false;
            }
            if (previous.1 > y) != (next.1 > y)
                && x < (next.0 - previous.0) * (y - previous.1) / (next.1 - previous.1) + previous.0
            {
                inside = !inside;
            }
            previous = next;
        }
        inside
    }
    /// Exact line-segment intersection with the same two-pixel inset used by
    /// point checks. Long segments cannot jump across a small pseudostate.
    fn intersects_segment(&self, line: kurbo::Line) -> bool {
        let b = self.bbox;
        if line.p0.x.min(line.p1.x) >= b.x1
            || line.p0.x.max(line.p1.x) <= b.x0
            || line.p0.y.min(line.p1.y) >= b.y1
            || line.p0.y.max(line.p1.y) <= b.y0
        {
            return false;
        }
        if self.ellipse {
            let (rx, ry) = ((b.x1 - b.x0) / 2.0 - 2.0, (b.y1 - b.y0) / 2.0 - 2.0);
            if rx <= 0.0 || ry <= 0.0 {
                return false;
            }
            let center = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
            let a = kurbo::Vec2::new((line.p0.x - center.0) / rx, (line.p0.y - center.1) / ry);
            let d = kurbo::Vec2::new((line.p1.x - line.p0.x) / rx, (line.p1.y - line.p0.y) / ry);
            let length = d.hypot2();
            let t = if length > 0.0 {
                (-a.dot(d) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return (a + d * t).hypot2() < 1.0;
        }
        if self.polygon.is_empty() {
            let (mut enter, mut exit) = (0.0_f64, 1.0_f64);
            for (from, to, low, high) in [
                (line.p0.x, line.p1.x, b.x0 + 2.0, b.x1 - 2.0),
                (line.p0.y, line.p1.y, b.y0 + 2.0, b.y1 - 2.0),
            ] {
                if low >= high {
                    return false;
                }
                let delta = to - from;
                if delta == 0.0 {
                    if from <= low || from >= high {
                        return false;
                    }
                } else {
                    let (a, b) = ((low - from) / delta, (high - from) / delta);
                    enter = enter.max(a.min(b));
                    exit = exit.min(a.max(b));
                    if enter >= exit {
                        return false;
                    }
                }
            }
            return enter < exit;
        }
        let direction = line.p1 - line.p0;
        if direction.hypot2() == 0.0 {
            return self.interior(line.p0.x, line.p0.y);
        }
        // Polygon membership or two-pixel boundary clearance can change only
        // at a polygon edge, an edge's parallel offset, or a radius-two circle
        // around a vertex. Partition at those exact roots, not fixed samples.
        let mut cuts = vec![0.0, 1.0];
        let mut add = |t: f64| {
            if (0.0..=1.0).contains(&t) {
                cuts.push(t);
            }
        };
        let mut previous = *self.polygon.last().unwrap();
        for &next in &self.polygon {
            let a = kurbo::Point::new(previous.0, previous.1);
            let z = kurbo::Point::new(next.0, next.1);
            previous = next;
            let edge = z - a;
            let crossing = direction.cross(edge);
            if crossing != 0.0 {
                add((a - line.p0).cross(edge) / crossing);
                let perpendicular = (line.p0 - a).cross(edge);
                for offset in [-2.0 * edge.hypot(), 2.0 * edge.hypot()] {
                    add((offset - perpendicular) / crossing);
                }
            }
            for vertex in [a, z] {
                let relative = line.p0 - vertex;
                for root in kurbo::common::solve_quadratic(
                    relative.hypot2() - 4.0,
                    2.0 * relative.dot(direction),
                    direction.hypot2(),
                ) {
                    add(root);
                }
            }
        }
        cuts.sort_by(f64::total_cmp);
        cuts.dedup();
        self.interior(line.p0.x, line.p0.y)
            || self.interior(line.p1.x, line.p1.y)
            || cuts.windows(2).any(|pair| {
                let point = line.p0.lerp(line.p1, pair[0] + (pair[1] - pair[0]) * 0.5);
                self.interior(point.x, point.y)
            })
    }
}

/// One XML traversal supplies ownership and global coordinates to every
/// geometry check. Definitions never participate in the visible scene.
struct SvgElement {
    tag: String,
    attrs: std::collections::HashMap<String, String>,
    parent: Option<usize>,
    children: Vec<usize>,
    offset: (f64, f64),
    range: std::ops::Range<usize>,
}

fn translation(transform: Option<&String>) -> (f64, f64) {
    let Some(transform) = transform else {
        return (0.0, 0.0);
    };
    let mut remaining = transform.trim();
    let mut offset = (0.0, 0.0);
    while !remaining.is_empty() {
        let arguments = remaining
            .strip_prefix("translate(")
            .expect("geometry checker must implement every emitted transform");
        let (arguments, rest) = arguments.split_once(')').expect("closed SVG transform");
        let values: Vec<f64> = arguments
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|value| !value.is_empty())
            .map(|value| value.parse().expect("numeric translation"))
            .collect();
        assert!(
            (1..=2).contains(&values.len()),
            "translate needs one or two coordinates"
        );
        offset.0 += values[0];
        offset.1 += values.get(1).copied().unwrap_or(0.0);
        remaining = rest.trim();
    }
    offset
}

fn svg_elements(svg: &str) -> Vec<SvgElement> {
    struct Frame {
        index: Option<usize>,
        offset: (f64, f64),
        definition: bool,
    }
    let mut reader = Reader::from_str(svg);
    let mut elements: Vec<SvgElement> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event().expect("well-formed emitted SVG");
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = String::from_utf8(tag.name().as_ref().to_vec()).unwrap();
                let attrs: std::collections::HashMap<_, _> = tag
                    .attributes()
                    .map(|attribute| {
                        let attribute = attribute.unwrap();
                        (
                            String::from_utf8(attribute.key.as_ref().to_vec()).unwrap(),
                            attribute.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                let definition = stack.last().is_some_and(|frame| frame.definition)
                    || matches!(
                        name.as_str(),
                        "defs"
                            | "marker"
                            | "mask"
                            | "clipPath"
                            | "linearGradient"
                            | "radialGradient"
                    );
                let inherited = stack.last().map_or((0.0, 0.0), |frame| frame.offset);
                let local = if definition {
                    (0.0, 0.0)
                } else {
                    translation(attrs.get("transform"))
                };
                let offset = (inherited.0 + local.0, inherited.1 + local.1);
                let index = if definition {
                    None
                } else {
                    let parent = stack.last().and_then(|frame| frame.index);
                    let index = elements.len();
                    elements.push(SvgElement {
                        tag: name,
                        attrs,
                        parent,
                        children: Vec::new(),
                        offset,
                        range: start..reader.buffer_position() as usize,
                    });
                    if let Some(parent) = parent {
                        elements[parent].children.push(index);
                    }
                    Some(index)
                };
                if !empty {
                    stack.push(Frame {
                        index,
                        offset,
                        definition,
                    });
                }
            }
            Event::End(_) => {
                let frame = stack.pop().expect("balanced emitted SVG");
                if let Some(index) = frame.index {
                    elements[index].range.end = reader.buffer_position() as usize;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    elements
}

/// Remove only the element's own transform before local measurement; its
/// accumulated translation is applied exactly once afterward. Nested group
/// offsets have already been resolved by the shared traversal.
fn local_fragment(svg: &str, element: &SvgElement) -> String {
    let fragment = &svg[element.range.clone()];
    if !element.attrs.contains_key("transform") {
        return fragment.to_string();
    }
    let mut reader = Reader::from_str(fragment);
    let mut writer = quick_xml::Writer::new(Vec::new());
    let mut first = true;
    loop {
        let event = reader.read_event().unwrap();
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) if first => {
                let mut clean = quick_xml::events::BytesStart::new(element.tag.as_str());
                for attribute in tag.attributes() {
                    let attribute = attribute.unwrap();
                    if attribute.key.as_ref() != b"transform" {
                        clean.push_attribute(attribute);
                    }
                }
                writer
                    .write_event(if empty {
                        Event::Empty(clean)
                    } else {
                        Event::Start(clean)
                    })
                    .unwrap();
            }
            Event::Eof => break,
            other => writer.write_event(other).unwrap(),
        }
        first = false;
    }
    String::from_utf8(writer.into_inner()).unwrap()
}

fn drawn_element(svg: &str, element: &SvgElement) -> Option<extent::Drawn> {
    let (_, drawn) = extent::scan(&format!(
        r#"<svg viewBox="0 0 1 1">{}</svg>"#,
        local_fragment(svg, element)
    ));
    let mut drawn = drawn.into_iter().find(|drawn| drawn.tag == element.tag)?;
    let (dx, dy) = element.offset;
    drawn.bbox = Box2 {
        x0: drawn.bbox.x0 + dx,
        y0: drawn.bbox.y0 + dy,
        x1: drawn.bbox.x1 + dx,
        y1: drawn.bbox.y1 + dy,
    };
    Some(drawn)
}

fn descendants_have(
    elements: &[SvgElement],
    start: usize,
    predicate: impl Fn(&SvgElement) -> bool,
) -> bool {
    let mut pending = vec![start];
    while let Some(index) = pending.pop() {
        if predicate(&elements[index]) {
            return true;
        }
        pending.extend(elements[index].children.iter().copied());
    }
    false
}

fn is_shape(element: &SvgElement) -> bool {
    matches!(
        element.tag.as_str(),
        "rect" | "polygon" | "circle" | "ellipse"
    ) || (element.tag == "path" && element.attrs.get("fill").is_some_and(|fill| fill != "none"))
}

fn node_groups(elements: &[SvgElement]) -> std::collections::HashMap<usize, usize> {
    let mut groups = std::collections::HashMap::new();
    for (index, group) in elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.tag == "g")
    {
        // A wrapper can contain translated node groups, raw pseudostates and
        // edges. It is never an additional node of its own. Label-only nested
        // groups remain descendants of their actual shape-owning node group.
        if group
            .children
            .iter()
            .any(|&child| elements[child].tag == "g" && descendants_have(elements, child, is_shape))
        {
            continue;
        }
        if group
            .children
            .iter()
            .any(|&child| elements[child].tag == "path" && !is_shape(&elements[child]))
        {
            continue;
        }
        let Some(&primary) = group
            .children
            .iter()
            .find(|&&child| is_shape(&elements[child]))
        else {
            continue;
        };
        let shape = &elements[primary];
        let fill = shape.attrs.get("fill").map(String::as_str).unwrap_or("");
        if fill.contains("--mermaid-cluster-fill")
            || fill.contains("--surface")
            || fill.contains("--mermaid-note-fill")
        {
            continue;
        }
        let has_label = descendants_have(elements, index, |element| element.tag == "text");
        if !has_label
            && ((shape.tag == "circle" && matches!(fill, "currentColor" | "none"))
                || (shape.tag == "rect" && fill == "currentColor"))
        {
            continue;
        }
        groups.insert(index, primary);
    }
    groups
}

fn owner_group(
    elements: &[SvgElement],
    groups: &std::collections::HashMap<usize, usize>,
    index: usize,
) -> Option<usize> {
    let mut parent = elements[index].parent;
    while let Some(index) = parent {
        if groups.contains_key(&index) {
            return Some(index);
        }
        parent = elements[index].parent;
    }
    None
}

struct RenderedPath {
    data: String,
    offset: (f64, f64),
}
struct DiagramGeometry {
    nodes: Vec<NodeShape>,
    labels: Vec<extent::Drawn>,
    edges: Vec<RenderedPath>,
}

fn diagram_geometry(svg: &str) -> DiagramGeometry {
    let elements = svg_elements(svg);
    let groups = node_groups(&elements);
    let labels: Vec<_> = elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.tag == "text")
        .filter_map(|(index, element)| drawn_element(svg, element).map(|drawn| (index, drawn)))
        .collect();
    let mut nodes: Vec<NodeShape> = Vec::new();
    for (index, _) in elements
        .iter()
        .enumerate()
        .filter(|(index, _)| groups.contains_key(index))
    {
        let primary = &elements[groups[&index]];
        let drawn = drawn_element(svg, primary).expect("visible node geometry");
        let polygon = primary
            .attrs
            .get("points")
            .filter(|_| primary.tag == "polygon")
            .map(|points| {
                points
                    .split_whitespace()
                    .map(|point| {
                        let (x, y) = point.split_once(',').unwrap();
                        (
                            x.parse::<f64>().unwrap() + primary.offset.0,
                            y.parse::<f64>().unwrap() + primary.offset.1,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        nodes.push(NodeShape {
            bbox: drawn.bbox,
            polygon,
            ellipse: matches!(primary.tag.as_str(), "circle" | "ellipse"),
            labels: labels
                .iter()
                .filter(|(label, _)| owner_group(&elements, &groups, *label) == Some(index))
                .map(|(_, label)| label.clone())
                .collect(),
        });
    }
    // Ungrouped state pseudostates and fork/join bars follow the exact same
    // transform traversal as regular nodes. A smaller concentric final ring
    // is decoration; coincident equal-size nodes are still separate nodes.
    for (index, element) in elements.iter().enumerate() {
        if owner_group(&elements, &groups, index).is_some() {
            continue;
        }
        let ellipse = element.tag == "circle";
        let fork_or_join = element.tag == "rect"
            && element
                .attrs
                .get("fill")
                .is_some_and(|fill| fill == "currentColor");
        if !(ellipse || fork_or_join) {
            continue;
        }
        let drawn = drawn_element(svg, element).expect("visible pseudostate geometry");
        let bbox = drawn.bbox;
        let x = (bbox.x0 + bbox.x1) / 2.0;
        let y = (bbox.y0 + bbox.y1) / 2.0;
        if ellipse
            && nodes.iter().any(|other| {
                other.ellipse
                    && ((other.bbox.x0 + other.bbox.x1) / 2.0 - x).abs() < 0.1
                    && ((other.bbox.y0 + other.bbox.y1) / 2.0 - y).abs() < 0.1
                    && other.bbox.x0 < bbox.x0 - 0.1
                    && other.bbox.x1 > bbox.x1 + 0.1
            })
        {
            continue;
        }
        nodes.push(NodeShape {
            bbox,
            polygon: Vec::new(),
            ellipse,
            labels: Vec::new(),
        });
    }
    let edges = elements
        .iter()
        .enumerate()
        .filter(|(index, element)| {
            element.tag == "path" && owner_group(&elements, &groups, *index).is_none()
        })
        .map(|(_, element)| RenderedPath {
            data: element.attrs.get("d").expect("emitted edge path").clone(),
            offset: element.offset,
        })
        .collect();
    DiagramGeometry {
        nodes,
        labels: labels.into_iter().map(|(_, label)| label).collect(),
        edges,
    }
}

fn node_boxes(svg: &str) -> Vec<NodeShape> {
    diagram_geometry(svg).nodes
}

/// Flatten emitted SVG curves to line segments with a 0.05px geometric error
/// bound. Segment/node intersections are analytic; work depends on curvature,
/// not path length or canvas pixel dimensions.
fn curve_segments(path: &str) -> Vec<kurbo::Line> {
    let path = kurbo::BezPath::from_svg(path).expect("supported emitted SVG path");
    let mut segments = Vec::new();
    let mut current = None;
    let mut start = None;
    kurbo::flatten(
        path.elements().iter().copied(),
        0.05,
        |element| match element {
            kurbo::PathEl::MoveTo(point) => {
                current = Some(point);
                start = Some(point);
            }
            kurbo::PathEl::LineTo(point) => {
                segments.push(kurbo::Line::new(current.expect("path start"), point));
                current = Some(point);
            }
            kurbo::PathEl::ClosePath => {
                let point = start.expect("closed subpath start");
                segments.push(kurbo::Line::new(current.expect("path start"), point));
                current = Some(point);
            }
            _ => panic!("flattening must produce only line segments"),
        },
    );
    segments
}

#[track_caller]
fn assert_layered_separation(svg: &str) {
    let geometry = diagram_geometry(svg);
    let nodes = geometry.nodes;
    assert!(!nodes.is_empty(), "fixture must exercise visible nodes");
    for (index, node) in nodes.iter().enumerate() {
        for other in &nodes[index + 1..] {
            assert!(
                node.bbox.overlap(&other.bbox) <= 1.0,
                "visible nodes overlap: {:?} / {:?}",
                node.bbox,
                other.bbox
            );
        }
    }
    let labels = &geometry.labels;
    for (index, label) in labels.iter().enumerate() {
        for other in &labels[index + 1..] {
            assert!(
                label.bbox.overlap(&other.bbox) <= 1.0,
                "labels overlap: {} and {}",
                label.text,
                other.text
            );
        }
        let owner = nodes.iter().position(|shape| {
            shape
                .labels
                .iter()
                .any(|owned| owned.text == label.text && owned.bbox == label.bbox)
        });
        if let Some(owner) = owner {
            assert!(
                contains(nodes[owner].bbox, label.bbox, 1.0),
                "label {} escapes its own node: {:?} / {:?}",
                label.text,
                label.bbox,
                nodes[owner].bbox
            );
        }
        for (index, shape) in nodes.iter().enumerate() {
            if Some(index) == owner {
                continue;
            }
            assert!(
                label.bbox.overlap(&shape.bbox) <= 1.0,
                "foreign label {} overlaps node {:?}",
                label.text,
                shape.bbox
            );
        }
    }
    for path in geometry.edges {
        for segment in curve_segments(&path.data) {
            let translate = |point: kurbo::Point| {
                kurbo::Point::new(point.x + path.offset.0, point.y + path.offset.1)
            };
            let segment = kurbo::Line::new(translate(segment.p0), translate(segment.p1));
            for node in &nodes {
                assert!(
                    !node.intersects_segment(segment),
                    "edge enters a node interior along {:?}: {:?}: {}",
                    segment,
                    node.bbox,
                    path.data
                );
            }
        }
    }
}

#[test]
fn golden_sources_stay_inside_the_viewbox() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut sources: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "mmd"))
        .collect();
    sources.sort();
    assert!(
        sources.len() >= 22,
        "exercise every supported diagram family"
    );
    for path in sources {
        let source = std::fs::read_to_string(&path).unwrap();
        let rendered = crate::render(&source);
        let svg = rendered
            .svg
            .unwrap_or_else(|| panic!("{}: {:?}", path.display(), rendered.error));
        let outside = extent::outside(&svg);
        assert!(outside.is_empty(), "{}: {outside:#?}", path.display());
    }
}

#[test]
fn layered_golden_labels_and_edges_are_separate() {
    for source in [
        include_str!("../tests/golden/flowchart.mmd"),
        include_str!("../tests/golden/state.mmd"),
        include_str!("../tests/golden/class.mmd"),
        include_str!("../tests/golden/entity-relationship.mmd"),
    ] {
        let svg = crate::render(source).svg.unwrap();
        assert_layered_separation(&svg);
    }
}

#[test]
fn parallel_edges_keep_their_routed_waypoints() {
    let svg = crate::render("flowchart TD\nA --> L\nA --> B\nA --> R\nB --> C\nA --> C\nA --> C")
        .svg
        .unwrap();
    assert_layered_separation(&svg);
}

#[test]
fn parallel_side_ports_never_turn_back_into_the_source_node() {
    let source = include_str!("../tests/fixtures/parallel-source-port.mmd");
    let svg = crate::render(source).svg.unwrap();
    assert_layered_separation(&svg);
}

#[test]
fn geometry_checks_reject_overlap_in_emitted_svg() {
    let base = r#"<svg viewBox="0 0 200 200"><g><rect x="20" y="20" width="60" height="30"/><text x="30" y="40">Node</text></g>"#;
    for bad in [
        r#"<g><rect x="20" y="20" width="60" height="30"/></g>"#,
        r#"<text x="65" y="40">Foreign</text>"#,
        r#"<path d="M 0 35 L 100 35"/>"#,
        r#"<text x="30" y="40">Second</text>"#,
    ] {
        let svg = format!("{base}{bad}</svg>");
        assert!(std::panic::catch_unwind(|| assert_layered_separation(&svg)).is_err());
    }
}

#[test]
fn label_ownership_rejects_fully_contained_foreign_and_escaped_owned_text() {
    for svg in [
        r#"<svg viewBox="0 0 200 200"><g><rect x="20" y="20" width="100" height="60"/></g><text x="30" y="50">Foreign</text></svg>"#,
        r#"<svg viewBox="0 0 200 200"><g><rect x="20" y="20" width="60" height="30"/><text x="120" y="140">Escaped</text></g></svg>"#,
    ] {
        assert!(std::panic::catch_unwind(|| assert_layered_separation(svg)).is_err());
    }
}

#[test]
fn state_pseudostates_and_fork_bars_participate_in_separation_checks() {
    let svg = crate::render("stateDiagram-v2\n[*] --> A\nA --> [*]")
        .svg
        .unwrap();
    assert_eq!(
        node_boxes(&svg).len(),
        3,
        "two final rings represent one node"
    );
    assert_layered_separation(&svg);
    let svg=crate::render("stateDiagram-v2\nstate F <<fork>>\nstate J <<join>>\n[*] --> F\nF --> A\nF --> B\nA --> J\nB --> J\nJ --> [*]").svg.unwrap();
    assert_eq!(node_boxes(&svg).len(), 6);
    assert_layered_separation(&svg);
    for node in [
        r#"<circle cx="120" cy="120" r="7" fill="currentColor"/>"#,
        r#"<circle cx="120" cy="120" r="7" fill="none" stroke="currentColor"/><circle cx="120" cy="120" r="5" fill="currentColor"/>"#,
        r#"<rect x="100" y="116" width="40" height="8" fill="currentColor"/>"#,
    ] {
        let svg = format!(
            r#"<svg viewBox="0 0 200 200"><g><rect x="20" y="20" width="40" height="30"/></g>{node}<path d="M 0 120 L 200 120"/></svg>"#
        );
        assert_eq!(node_boxes(&svg).len(), 2);
        assert!(std::panic::catch_unwind(|| assert_layered_separation(&svg)).is_err());
    }
}

#[test]
fn translated_state_edges_pseudostates_and_forks_are_all_checked() {
    let source = "stateDiagram-v2\nstate F <<fork>>\nstate J <<join>>\n[*] --> F\nF --> A\nF --> B\nA --> J\nB --> J\nJ --> [*]\nnote left of A : A long note extending to the left.";
    let svg = crate::render(source)
        .svg
        .expect("state diagram with a left note");
    assert!(
        svg.contains("translate("),
        "fixture must exercise the renderer's translation wrapper"
    );
    let geometry = diagram_geometry(&svg);
    assert_eq!(
        geometry.nodes.len(),
        6,
        "wrappers are not nodes; final rings count once"
    );
    assert_eq!(
        geometry.edges.len(),
        6,
        "every translated transition must be checked"
    );
    assert_eq!(
        geometry.nodes.iter().filter(|node| node.ellipse).count(),
        2,
        "initial and final pseudostates"
    );
    assert_eq!(
        geometry
            .nodes
            .iter()
            .filter(|node| !node.ellipse && node.bbox.y1 - node.bbox.y0 <= 10.1)
            .count(),
        2,
        "fork and join bars"
    );
    assert_layered_separation(&svg);
}

#[test]
fn nested_translations_preserve_geometry_and_nearest_node_label_ownership() {
    let svg = r#"<svg viewBox="0 0 500 500"><defs><g transform="translate(90,80)"><rect x="20" y="20" width="60" height="30"/><circle cx="50" cy="35" r="20"/><path d="M 0 35 L 100 35"/></g></defs><g transform="translate(80,60)"><g transform="translate(10 20)"><g><rect x="20" y="20" width="60" height="30"/><g transform="translate(0,0)"><text x="30" y="40">Node</text></g></g><g transform="translate(-10,0)"><path transform="translate(5,0)" d="M 0 80 L 200 80"/></g></g></g></svg>"#;
    let geometry = diagram_geometry(svg);
    assert_eq!(
        geometry.nodes.len(),
        1,
        "wrappers and definitions must not become duplicate nodes"
    );
    assert_eq!(
        geometry.nodes[0].bbox,
        Box2 {
            x0: 110.0,
            y0: 100.0,
            x1: 170.0,
            y1: 130.0
        }
    );
    assert_eq!(
        geometry.nodes[0].labels.len(),
        1,
        "a label-only subgroup belongs to its nearest shape-owning group"
    );
    assert_eq!(geometry.nodes[0].labels[0].text, "Node");
    assert_eq!(geometry.edges.len(), 1, "definition paths are excluded");
    assert_eq!(geometry.edges[0].offset, (85.0, 80.0));
    assert_layered_separation(svg);
}

#[test]
fn translated_nested_collisions_cannot_bypass_geometry_checks() {
    let prefix = r#"<svg viewBox="0 0 500 500"><g transform="translate(80,60)"><g transform="translate(10 20)"><g><rect x="20" y="20" width="60" height="30"/><text x="30" y="40">Node</text></g>"#;
    for bad in [
        r#"<g transform="translate(5,0)"><g><rect x="20" y="20" width="60" height="30"/></g></g>"#,
        r#"<g transform="translate(10,0)"><text x="40" y="40">Foreign</text></g>"#,
        r#"<g transform="translate(10,0)"><g transform="translate(-10,0)"><path d="M 0 35 L 100 35"/></g></g>"#,
    ] {
        let svg = format!("{prefix}{bad}</g></g></svg>");
        assert!(
            std::panic::catch_unwind(|| assert_layered_separation(&svg)).is_err(),
            "translated collision was ignored: {bad}"
        );
    }
    let escaped = r#"<svg viewBox="0 0 500 500"><g transform="translate(80,60)"><g><rect x="20" y="20" width="60" height="30"/><g transform="translate(100,100)"><text x="30" y="40">Escaped</text></g></g></g></svg>"#;
    assert!(
        std::panic::catch_unwind(|| assert_layered_separation(escaped)).is_err(),
        "translated owned labels must stay inside their node"
    );
}

#[test]
fn nested_translated_edges_cannot_cross_state_pseudostates() {
    let prefix = r#"<svg viewBox="0 0 500 500"><g transform="translate(80,60)"><g transform="translate(10 20)"><g><rect x="20" y="20" width="60" height="30"/><text x="30" y="40">Node</text></g><g transform="translate(30,-10)">"#;
    for pseudo in [
        r#"<circle cx="120" cy="120" r="7" fill="currentColor"/>"#,
        r#"<circle cx="120" cy="120" r="7" fill="none" stroke="currentColor"/><circle cx="120" cy="120" r="5" fill="currentColor"/>"#,
        r#"<rect x="100" y="116" width="40" height="8" fill="currentColor"/>"#,
    ] {
        let pass = format!(
            r#"{prefix}{pseudo}<g transform="translate(5,0)"><path d="M 0 140 L 200 140"/></g></g></g></g></svg>"#
        );
        let geometry = diagram_geometry(&pass);
        assert_eq!(
            geometry.nodes.len(),
            2,
            "one ordinary node and one translated pseudostate"
        );
        assert_eq!(geometry.edges.len(), 1);
        assert_layered_separation(&pass);
        let collision = pass.replace("M 0 140 L 200 140", "M 0 120 L 200 120");
        assert!(
            std::panic::catch_unwind(|| assert_layered_separation(&collision)).is_err(),
            "edge through translated pseudostate was ignored: {pseudo}"
        );
    }
}

#[test]
fn long_lines_and_curves_cannot_jump_over_small_pseudostates() {
    let prefix = r#"<svg viewBox="0 0 2200 400"><g><rect x="20" y="20" width="40" height="30"/></g><circle cx="120" cy="120" r="7" fill="currentColor"/>"#;
    for data in [
        "M 0 120 L 2000 120",
        "M 0 120 C 666.6667 120 1333.3333 120 2000 120",
        "M 0 100 C 666.6667 200 1333.3333 40 2000 120",
    ] {
        let svg = format!(r#"{prefix}<path d="{data}"/></svg>"#);
        assert!(
            std::panic::catch_unwind(|| assert_layered_separation(&svg)).is_err(),
            "long crossing escaped detection: {data}"
        );
    }
    for data in [
        "M 0 125.25 L 2000 125.25",
        "M 0 125.25 C 666.6667 125.25 1333.3333 125.25 2000 125.25",
        "M 0 98 C 666.6667 198 1333.3333 38 2000 118",
    ] {
        let svg = format!(r#"{prefix}<path d="{data}"/></svg>"#);
        assert_layered_separation(&svg);
    }
}

#[test]
fn long_segments_intersect_inset_boxes_and_polygons_analytically() {
    let base =
        r#"<svg viewBox="0 0 2200 2200"><g><rect x="20" y="20" width="40" height="30"/></g>"#;
    for (node, crossing, near_miss) in [
        (
            r#"<rect x="100" y="116" width="40" height="8" fill="currentColor"/>"#,
            "M 120 0 L 120 2000",
            "M 139 0 L 139 2000",
        ),
        (
            r#"<g><polygon points="120,113 127,120 120,127 113,120"/></g>"#,
            "M 0 120 L 2000 120",
            "M 0 125.25 L 2000 125.25",
        ),
    ] {
        let bad = format!(r#"{base}{node}<path d="{crossing}"/></svg>"#);
        assert!(
            std::panic::catch_unwind(|| assert_layered_separation(&bad)).is_err(),
            "long line skipped an inset shape"
        );
        let good = format!(r#"{base}{node}<path d="{near_miss}"/></svg>"#);
        assert_layered_separation(&good);
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 96,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x289_2026),
        ..ProptestConfig::default()
    })]

    #[test]
    fn seeded_flowcharts_keep_labels_nodes_and_emitted_edges_separate(
        direction in prop_oneof![Just("TD"),Just("BT"),Just("LR"),Just("RL")],
        count in 2usize..10,
        edges in proptest::collection::vec((0usize..10,0usize..10), 0..14),
        labels in proptest::collection::vec("[A-Z][a-z ]{2,14}", 10),
    ) {
        let mut source = format!("flowchart {direction}\n");
        for (index, label) in labels.iter().enumerate().take(count) {
            source.push_str(&format!("n{index}[\"{label}\"]\n"));
        }
        for (from, to) in edges {
            if from < to && to < count {
                source.push_str(&format!("n{from} -->|edge {from}-{to}| n{to}\n"));
            }
        }
        let svg = crate::render(&source).svg.expect("bounded valid graph renders");
        extent::assert_inside(&svg);
        assert_layered_separation(&svg);
    }
}
