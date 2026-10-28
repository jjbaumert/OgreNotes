//! Mermaid `architecture-beta` diagrams: parser + SVG renderer (Tier 5).
//!
//! Cloud-architecture diagrams. `group id(icon)[Title]` and
//! `service id(icon)[Title] in group` declare boxed nodes with a built-in
//! icon glyph; `junction id` is a routing dot. Edges carry port sides —
//! `a:R -- L:b` means b sits to the right of a — which drive a constraint
//! based placement with group containment and orthogonal routing. Arrows: `--`, `-->`, `<--`,
//! `<-->`. Unsupported icon names and unresolved references are explicit errors.

use crate::{escape_xml, measure, ParseError};
use std::collections::HashMap;

const CELL_W: f64 = 150.0;
const CELL_H: f64 = 120.0;
const ICON: f64 = 46.0; // icon glyph box side
const NAME_BASELINE: f64 = ICON / 2.0 + 10.0;
// Bottom ports connect below the complete name, including descenders/halos.
// Anchoring them at the icon edge lets accented or solid glyphs erase the tip.
const BOTTOM_PORT_GAP: f64 = 12.0;
const JUNCTION_RADIUS: f64 = 5.0;
const PAD: f64 = 24.0;
const GROUP_ICON: f64 = 16.0;
const GROUP_ICON_GAP: f64 = 6.0;
const MAX_NODES: usize = 500;
const MAX_EDGES: usize = crate::layout::MAX_EDGES;
// One diagram-wide allowance, not a fresh allowance for each edge/branch.
const MAX_LAYOUT_WORK: usize = 20_000_000;

struct LayoutWork {
    remaining: usize,
}
impl LayoutWork {
    fn spend(&mut self, amount: usize) -> Result<(), ()> {
        self.remaining = self.remaining.checked_sub(amount).ok_or(())?;
        Ok(())
    }
}

fn work_error(line: usize) -> ParseError {
    err(
        format!(
            "architecture layout exceeds the cumulative work limit ({MAX_LAYOUT_WORK} operations)"
        ),
        line,
    )
}

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
    line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Edge {
    pub from: usize,
    pub to: usize,
    pub from_side: Side,
    pub to_side: Side,
    pub arrow_from: bool,
    pub arrow_to: bool,
    line: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Architecture {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub groups: Vec<usize>, // node indices that are groups
    layout: ArchitectureLayout,
}

pub(crate) fn parse(source: &str) -> Result<Architecture, ParseError> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut edges_raw = Vec::new();
    let mut parents: Vec<(Option<String>, usize)> = Vec::new();
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
                return Err(err(
                    "architecture diagram must start with `architecture-beta`",
                    line_no,
                ));
            }
            seen_header = true;
            continue;
        }
        if nodes.len() >= MAX_NODES
            && (line.starts_with("group ")
                || line.starts_with("service ")
                || line.starts_with("junction "))
        {
            return Err(err(
                format!("architecture too large: >{MAX_NODES} nodes"),
                line_no,
            ));
        }
        if let Some(rest) = line.strip_prefix("group ") {
            let (id, icon, title, parent) = parse_decl(rest, line_no)?;
            let gi = nodes.len();
            validate_decl(&id, &icon, &ids, line_no)?;
            ids.insert(id, gi);
            groups.push(gi);
            parents.push((parent, line_no));
            nodes.push(Node {
                icon,
                title,
                group: None,
                is_group: true,
                junction: false,
                line: line_no,
            });
        } else if let Some(rest) = line.strip_prefix("service ") {
            let (id, icon, title, parent) = parse_decl(rest, line_no)?;
            validate_decl(&id, &icon, &ids, line_no)?;
            parents.push((parent, line_no));
            ids.insert(id, nodes.len());
            nodes.push(Node {
                icon,
                title,
                group: None,
                is_group: false,
                junction: false,
                line: line_no,
            });
        } else if let Some(rest) = line.strip_prefix("junction ") {
            let (id, parent) = match rest.split_once(" in ") {
                Some((a, b)) => (a.trim().to_string(), Some(b.trim().to_string())),
                None => (rest.trim().to_string(), None),
            };
            validate_decl(&id, "", &ids, line_no)?;
            parents.push((parent, line_no));
            ids.insert(id, nodes.len());
            nodes.push(Node {
                icon: String::new(),
                title: String::new(),
                group: None,
                is_group: false,
                junction: true,
                line: line_no,
            });
        } else if let Some(e) = parse_edge(line) {
            if edges_raw.len() >= MAX_EDGES {
                return Err(err(
                    format!("architecture too large: >{MAX_EDGES} edges"),
                    line_no,
                ));
            }
            if line.contains('{') {
                return Err(err(
                    "architecture group-boundary edge qualifiers are not supported",
                    line_no,
                ));
            }
            edges_raw.push((e, line_no));
        } else {
            return Err(err(
                format!("unrecognized architecture line {line:?}"),
                line_no,
            ));
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
    // Resolve declarations and links after parsing so forward references are safe.
    for (i, (parent, line)) in parents.iter().enumerate() {
        if let Some(parent) = parent {
            let group = ids
                .get(parent)
                .copied()
                .filter(|&g| nodes[g].is_group)
                .ok_or_else(|| err(format!("unknown architecture group `{parent}`"), *line))?;
            nodes[i].group = Some(group);
        }
    }
    for i in 0..nodes.len() {
        let mut next = nodes[i].group;
        let mut depth = 0;
        while let Some(group) = next {
            depth += 1;
            if group == i || depth > crate::MAX_NESTING {
                return Err(err(
                    "architecture groups form a cycle or are nested too deeply",
                    parents[i].1,
                ));
            }
            next = nodes[group].group;
        }
    }
    let mut edges = Vec::new();
    for ((a, sa, b, sb, af, at), line) in edges_raw {
        let from = ids
            .get(&a)
            .copied()
            .ok_or_else(|| err(format!("unknown architecture endpoint `{a}`"), line))?;
        let to = ids
            .get(&b)
            .copied()
            .ok_or_else(|| err(format!("unknown architecture endpoint `{b}`"), line))?;
        if nodes[from].is_group || nodes[to].is_group {
            return Err(err(
                "architecture edges must connect services or junctions, not group ids",
                line,
            ));
        }
        if from == to && sa == sb {
            return Err(err(
                "architecture self edges using the same port are not supported",
                line,
            ));
        }
        edges.push(Edge {
            from,
            to,
            from_side: sa,
            to_side: sb,
            arrow_from: af,
            arrow_to: at,
            line,
        });
    }
    let mut diagram = Architecture {
        nodes,
        edges,
        groups,
        layout: ArchitectureLayout::default(),
    };
    diagram.layout = place_and_route(&diagram)?;
    Ok(diagram)
}

/// Mermaid's shared ID terminal: ASCII word endpoints, with interior dashes.
fn valid_identifier(id: &str) -> bool {
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    id.chars().next().is_some_and(word)
        && id.chars().next_back().is_some_and(word)
        && id.chars().all(|ch| word(ch) || ch == '-')
}

fn validate_decl(
    id: &str,
    icon: &str,
    ids: &HashMap<String, usize>,
    line: usize,
) -> Result<(), ParseError> {
    if !valid_identifier(id) {
        return Err(err("invalid architecture identifier", line));
    }
    if ids.contains_key(id) {
        return Err(err(
            format!("duplicate architecture identifier `{id}`"),
            line,
        ));
    }
    if !matches!(
        icon,
        "" | "database" | "disk" | "cloud" | "internet" | "server"
    ) {
        return Err(err(format!("unsupported architecture icon `{icon}`"), line));
    }
    Ok(())
}

/// `id(icon)[Title]` with an optional trailing ` in parent`.
fn parse_decl(
    s: &str,
    line: usize,
) -> Result<(String, String, String, Option<String>), ParseError> {
    let id_end = s
        .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-'))
        .unwrap_or(s.len());
    let id = s[..id_end].to_string();
    let mut rest = s[id_end..].trim_start();
    let mut icon = String::new();
    let mut title = id.clone();
    if let Some(body) = rest.strip_prefix('(') {
        let close = body
            .find(')')
            .ok_or_else(|| err("unclosed architecture icon", line))?;
        icon = body[..close].trim().to_string();
        rest = body[close + 1..].trim_start();
    }
    if let Some(body) = rest.strip_prefix('[') {
        let close = body
            .find(']')
            .ok_or_else(|| err("unclosed architecture label", line))?;
        title = crate::unquote_label(body[..close].trim()).to_string();
        rest = body[close + 1..].trim_start();
    }
    let parent = if rest.is_empty() {
        None
    } else if let Some(parent) = rest.strip_prefix("in ") {
        let parent = parent.trim();
        if !valid_identifier(parent) {
            return Err(err("invalid architecture parent group", line));
        }
        Some(parent.to_string())
    } else {
        return Err(err(
            "unexpected tokens after architecture declaration",
            line,
        ));
    };
    Ok((id, icon, title, parent))
}

/// `a:S <arrow> S:b` (arrow ∈ `--`, `-->`, `<--`, `<-->`). Group-boundary
/// qualifiers are recognized here so the parser can reject them explicitly.
fn parse_edge(line: &str) -> Option<(String, Side, String, Side, bool, bool)> {
    // Locate the arrow after the source port, so valid identifiers such as
    // `api--gateway` cannot be mistaken for the arrow token itself.
    let after_colon = line[line.find(':')? + 1..].trim_start();
    let port = after_colon.chars().next()?;
    Side::parse(&port.to_string())?;
    let after_port = after_colon[port.len_utf8()..].trim_start();
    let (arrow, af, at) = if after_port.starts_with("<-->") {
        ("<-->", true, true)
    } else if after_port.starts_with("-->") {
        ("-->", false, true)
    } else if after_port.starts_with("<--") {
        ("<--", true, false)
    } else if after_port.starts_with("--") {
        ("--", false, false)
    } else {
        return None;
    };
    let l = &line[..line.len() - after_port.len()];
    let r = &after_port[arrow.len()..];
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
    ParseError {
        message: message.into(),
        line: Some(line),
    }
}

/// A lower-bound graph: each arc requires `to >= from + gap`. Group boundary
/// variables participate in the same graph as service centers, so separating
/// containers cannot reorder the services they enclose.
#[derive(Clone)]
struct AxisConstraints {
    arcs: Vec<Vec<(usize, f64)>>,
    values: Vec<f64>,
}

impl AxisConstraints {
    fn new(n: usize) -> Self {
        Self {
            arcs: vec![Vec::new(); n],
            values: vec![0.0; n],
        }
    }

    fn add(&mut self, from: usize, to: usize, gap: f64, work: &mut LayoutWork) -> Result<bool, ()> {
        work.spend(1)?;
        self.arcs[from].push((to, gap));
        if self.values[to] + 0.01 >= self.values[from] + gap {
            return Ok(true);
        }
        self.values[to] = self.values[from] + gap;
        let mut queue = std::collections::VecDeque::from([to]);
        let mut queued = vec![false; self.values.len()];
        let mut depth = vec![0; self.values.len()];
        queued[to] = true;
        while let Some(u) = queue.pop_front() {
            queued[u] = false;
            for &(v, distance) in &self.arcs[u] {
                work.spend(1)?;
                let next = self.values[u] + distance;
                if self.values[v] + 0.01 >= next {
                    continue;
                }
                self.values[v] = next;
                depth[v] = depth[u] + 1;
                if depth[v] >= self.values.len() {
                    return Ok(false);
                }
                if !queued[v] {
                    queued[v] = true;
                    queue.push_back(v);
                }
            }
        }
        Ok(true)
    }
}

#[derive(Debug, Clone, Default)]
struct ArchitectureLayout {
    centers: Vec<(f64, f64)>,
    groups: Vec<Option<[f64; 4]>>,
    width: f64,
    height: f64,
    paths: Vec<String>,
}

fn boundary(node: &Node, i: usize, high: bool, vertical: bool) -> (usize, f64) {
    if node.is_group {
        (2 * i + usize::from(high), 0.0)
    } else {
        let half = if vertical { CELL_H / 2.0 } else { CELL_W / 2.0 };
        (2 * i, if high { half } else { -half })
    }
}

fn axis_bounds(axis: &AxisConstraints, a: &Architecture, i: usize, vertical: bool) -> (f64, f64) {
    let (lo, offset_lo) = boundary(&a.nodes[i], i, false, vertical);
    let (hi, offset_hi) = boundary(&a.nodes[i], i, true, vertical);
    (axis.values[lo] + offset_lo, axis.values[hi] + offset_hi)
}

#[derive(Debug)]
enum SeparationFailure {
    Limit(usize, usize),
    Impossible(usize, usize),
}

/// Search only separation choices, never service direction choices. The shared
/// work allowance covers pair scans, graph copies and constraint propagation.
fn separate_siblings(
    a: &Architecture,
    siblings: &[Vec<usize>],
    x: AxisConstraints,
    y: AxisConstraints,
    choices_left: &mut usize,
    depth: usize,
    work: &mut LayoutWork,
) -> Result<(AxisConstraints, AxisConstraints), SeparationFailure> {
    let mut overlap = None;
    'outer: for children in siblings {
        for (offset, &i) in children.iter().enumerate() {
            let (xl, xr) = axis_bounds(&x, a, i, false);
            let (yt, yb) = axis_bounds(&y, a, i, true);
            for &j in &children[offset + 1..] {
                work.spend(1).map_err(|_| SeparationFailure::Limit(i, j))?;
                let (jl, jr) = axis_bounds(&x, a, j, false);
                let (jt, jb) = axis_bounds(&y, a, j, true);
                let gap = if a.nodes[i].is_group || a.nodes[j].is_group {
                    12.0
                } else {
                    0.0
                };
                if xl < jr + gap && jl < xr + gap && yt < jb + gap && jt < yb + gap {
                    overlap = Some((i, j));
                    break 'outer;
                }
            }
        }
    }
    let Some((i, j)) = overlap else {
        return Ok((x, y));
    };
    if *choices_left == 0 || depth > MAX_NODES * 2 {
        return Err(SeparationFailure::Limit(i, j));
    }
    *choices_left -= 1;
    let mut choices = Vec::new();
    for vertical in [false, true] {
        let axis = if vertical { &y } else { &x };
        for (before, after) in [(i, j), (j, i)] {
            let (from, end) = boundary(&a.nodes[before], before, true, vertical);
            let (to, start) = boundary(&a.nodes[after], after, false, vertical);
            work.spend(2 * axis.values.len() + axis.arcs.iter().map(Vec::len).sum::<usize>())
                .map_err(|_| SeparationFailure::Limit(i, j))?;
            let mut candidate = axis.clone();
            let gap = if a.nodes[before].is_group || a.nodes[after].is_group {
                12.0
            } else {
                0.0
            };
            if candidate
                .add(from, to, end - start + gap, work)
                .map_err(|_| SeparationFailure::Limit(i, j))?
            {
                let cost: f64 = candidate
                    .values
                    .iter()
                    .zip(&axis.values)
                    .map(|(a, b)| a - b)
                    .sum();
                choices.push((cost, vertical, candidate));
            }
        }
    }
    choices.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, vertical, candidate) in choices {
        work.spend(
            x.values.len()
                + y.values.len()
                + x.arcs.iter().map(Vec::len).sum::<usize>()
                + y.arcs.iter().map(Vec::len).sum::<usize>(),
        )
        .map_err(|_| SeparationFailure::Limit(i, j))?;
        let (next_x, next_y) = if vertical {
            (x.clone(), candidate)
        } else {
            (candidate, y.clone())
        };
        match separate_siblings(a, siblings, next_x, next_y, choices_left, depth + 1, work) {
            Ok(result) => return Ok(result),
            Err(SeparationFailure::Limit(i, j)) => return Err(SeparationFailure::Limit(i, j)),
            Err(SeparationFailure::Impossible(_, _)) => {}
        }
    }
    Err(SeparationFailure::Impossible(i, j))
}

/// Preferred service cells keep aligned simple edges unchanged and spread
/// fan-outs/components before container constraints are solved. These are only
/// lower-bound seeds; the validated constraint graph remains authoritative.
fn preferred_cells(a: &Architecture) -> Vec<(i32, i32)> {
    let mut neighbors = vec![Vec::new(); a.nodes.len()];
    for edge in &a.edges {
        if edge.from == edge.to {
            continue;
        }
        let (dx, dy) = edge.from_side.delta();
        let (tx, ty) = edge.to_side.delta();
        let delta = (
            if dx != 0 { dx } else { -tx },
            if dy != 0 { dy } else { -ty },
        );
        neighbors[edge.from].push((edge.to, delta));
        neighbors[edge.to].push((edge.from, (-delta.0, -delta.1)));
    }
    let mut cells = vec![None; a.nodes.len()];
    let mut occupied = std::collections::HashSet::new();
    let mut column = 0;
    for start in 0..a.nodes.len() {
        if a.nodes[start].is_group || cells[start].is_some() {
            continue;
        }
        cells[start] = Some((column, 0));
        occupied.insert((column, 0));
        let mut queue = std::collections::VecDeque::from([start]);
        while let Some(i) = queue.pop_front() {
            let (x, y) = cells[i].unwrap();
            for &(j, (dx, dy)) in &neighbors[i] {
                if cells[j].is_some() {
                    continue;
                }
                let mut next = (x + dx, y + dy);
                let step = if dx != 0 { (0, 1) } else { (1, 0) };
                while occupied.contains(&next) {
                    next = (next.0 + step.0, next.1 + step.1);
                }
                cells[j] = Some(next);
                occupied.insert(next);
                queue.push_back(j);
            }
        }
        column = occupied.iter().map(|&(x, _)| x).max().unwrap_or(0) + 2;
    }
    let min_x = occupied.iter().map(|&(x, _)| x).min().unwrap_or(0);
    let min_y = occupied.iter().map(|&(_, y)| y).min().unwrap_or(0);
    cells
        .into_iter()
        .map(|cell| cell.map_or((0, 0), |(x, y)| (x - min_x, y - min_y)))
        .collect()
}

fn place_and_route(a: &Architecture) -> Result<ArchitectureLayout, ParseError> {
    let n = a.nodes.len();
    let mut work = LayoutWork {
        remaining: MAX_LAYOUT_WORK,
    };
    let cells = preferred_cells(a);
    let mut x = AxisConstraints::new(2 * n);
    let mut y = AxisConstraints::new(2 * n);
    let mut siblings = vec![Vec::new(); n + 1];
    for (i, node) in a.nodes.iter().enumerate() {
        siblings[node.group.unwrap_or(n)].push(i);
        if node.is_group {
            let icon_space = if node.icon.is_empty() {
                0.0
            } else {
                GROUP_ICON + GROUP_ICON_GAP
            };
            let caption_width =
                (measure::literal_overlap_width(&node.title, 12.0) * 1.04 + 16.0 + icon_space)
                    .min(CELL_W);
            x.add(2 * i, 2 * i + 1, 64.0_f64.max(caption_width), &mut work)
                .map_err(|_| work_error(node.line))?;
            y.add(2 * i, 2 * i + 1, 40.0, &mut work)
                .map_err(|_| work_error(node.line))?;
        } else {
            x.values[2 * i] = cells[i].0 as f64 * CELL_W + CELL_W / 2.0;
            y.values[2 * i] = cells[i].1 as f64 * CELL_H + CELL_H / 2.0;
        }
    }
    for (i, node) in a.nodes.iter().enumerate() {
        let Some(parent) = node.group else {
            continue;
        };
        for (vertical, axis) in [(false, &mut x), (true, &mut y)] {
            let (low, low_offset) = boundary(node, i, false, vertical);
            let (high, high_offset) = boundary(node, i, true, vertical);
            axis.add(
                2 * parent,
                low,
                if vertical { 28.0 } else { 12.0 } - low_offset,
                &mut work,
            )
            .map_err(|_| work_error(node.line))?;
            axis.add(high, 2 * parent + 1, high_offset + 12.0, &mut work)
                .map_err(|_| work_error(node.line))?;
        }
    }
    for edge in &a.edges {
        if edge.from == edge.to {
            continue;
        }
        let (dx, dy) = edge.from_side.delta();
        let (tx, ty) = edge.to_side.delta();
        // Opposite sides express the same relation. Orthogonal ports express
        // one relation per axis; equal-facing ports use an outward U bend.
        for (vertical, delta, axis) in [
            (false, if dx != 0 { dx } else { -tx }, &mut x),
            (true, if dy != 0 { dy } else { -ty }, &mut y),
        ] {
            if delta == 0 {
                continue;
            }
            let (from, to) = if delta > 0 {
                (edge.from, edge.to)
            } else {
                (edge.to, edge.from)
            };
            if !axis
                .add(
                    2 * from,
                    2 * to,
                    if vertical { CELL_H } else { CELL_W },
                    &mut work,
                )
                .map_err(|_| work_error(edge.line))?
            {
                return Err(err("inconsistent architecture port directions", edge.line));
            }
        }
    }
    let mut choices_left = MAX_NODES * 8;
    let (x, y) = separate_siblings(a, &siblings, x, y, &mut choices_left, 0, &mut work).map_err(
        |failure| {
            let (i, j, limited) = match failure {
                SeparationFailure::Limit(i, j) => (i, j, true),
                SeparationFailure::Impossible(i, j) => (i, j, false),
            };
            let relevant = if a.nodes[j].is_group { j } else { i };
            if limited {
                work_error(a.nodes[relevant].line)
            } else {
                err(
                    "architecture groups cannot be separated while preserving port directions",
                    a.nodes[relevant].line,
                )
            }
        },
    )?;
    let mut centers = vec![(0.0, 0.0); n];
    let mut groups = vec![None; n];
    let (mut width, mut height) = (2.0 * PAD, 2.0 * PAD);
    for (i, node) in a.nodes.iter().enumerate() {
        let (xl, xr) = axis_bounds(&x, a, i, false);
        let (yt, yb) = axis_bounds(&y, a, i, true);
        width = width.max(xr + 2.0 * PAD);
        height = height.max(yb + 2.0 * PAD);
        if node.is_group {
            groups[i] = Some([xl + PAD, yt + PAD, xr + PAD, yb + PAD]);
        } else {
            centers[i] = (x.values[2 * i] + PAD, y.values[2 * i] + PAD);
        }
    }
    let mut layout = ArchitectureLayout {
        centers,
        groups,
        width,
        height,
        paths: Vec::new(),
    };
    let mut router =
        ArchitectureRouter::new(routing_obstacles(a, &layout.centers, &layout.groups), work)
            .map_err(|_| work_error(a.nodes.first().map_or(1, |node| node.line)))?;
    for edge in &a.edges {
        let (path, route_right, route_bottom) =
            orthogonal_route(a, edge, &layout.centers, &mut router)
                .map_err(|_| work_error(edge.line))?
                .ok_or_else(|| {
                    err(
                        "architecture edge routing exceeds the search limit",
                        edge.line,
                    )
                })?;
        layout.width = layout.width.max(route_right + PAD);
        layout.height = layout.height.max(route_bottom + PAD);
        layout.paths.push(path);
    }
    Ok(layout)
}

fn group_caption_x(node: &Node, bounds: [f64; 4]) -> f64 {
    bounds[0]
        + 8.0
        + if node.icon.is_empty() {
            0.0
        } else {
            GROUP_ICON + GROUP_ICON_GAP
        }
}

fn group_caption(node: &Node, bounds: [f64; 4]) -> String {
    let available = bounds[2] - 8.0 - group_caption_x(node, bounds);
    measure::truncate_literal_to_width(&node.title, available, 12.0, true)
}

fn routing_obstacles(
    a: &Architecture,
    centers: &[(f64, f64)],
    groups: &[Option<[f64; 4]>],
) -> Vec<[f64; 4]> {
    let mut result = Vec::new();
    for (i, node) in a.nodes.iter().enumerate() {
        if node.is_group {
            if let Some(bounds) = groups[i] {
                // Match the emitted, clipped caption, including bold glyph
                // allowance and its baseline. Borders remain traversable.
                if !node.icon.is_empty() {
                    result.push([
                        bounds[0] + 6.0,
                        bounds[1] + 1.0,
                        bounds[0] + 10.0 + GROUP_ICON,
                        bounds[1] + 5.0 + GROUP_ICON,
                    ]);
                }
                let caption = group_caption(node, bounds);
                if !caption.is_empty() {
                    let width = measure::literal_overlap_width(&caption, 12.0) * 1.1;
                    result.push([
                        group_caption_x(node, bounds) - 2.0,
                        bounds[1] + 2.0,
                        group_caption_x(node, bounds) + 2.0 + width,
                        bounds[1] + 21.0,
                    ]);
                }
            }
            continue;
        }
        let (x, y) = centers[i];
        if node.junction {
            result.push([x - 7.0, y - 7.0, x + 7.0, y + 7.0]);
        } else {
            result.push([x - 26.0, y - 32.0, x + 26.0, y + 20.0]);
            let name = clip(&node.title, CELL_W - 10.0);
            let half = measure::literal_overlap_width(&name, 12.0) / 2.0 + 3.0;
            result.push([x - half, y + 20.0, x + half, y + NAME_BASELINE + 5.0]);
        }
    }
    result
}

/// Obstacle projections are shared by every edge. Each explored row/column
/// gets one sorted interval union, making segment queries logarithmic instead
/// of scanning every service for every A* step.
struct ArchitectureRouter {
    work: LayoutWork,
    xs: Vec<f64>,
    ys: Vec<f64>,
    obstacles: Vec<[f64; 4]>,
    horizontal: HashMap<u64, Vec<(f64, f64)>>,
    vertical: HashMap<u64, Vec<(f64, f64)>>,
}

impl ArchitectureRouter {
    fn new(obstacles: Vec<[f64; 4]>, mut work: LayoutWork) -> Result<Self, ()> {
        work.spend(obstacles.len().max(1) * (obstacles.len().max(1).ilog2() as usize + 1) * 4)?;
        let mut xs = vec![PAD / 2.0];
        let mut ys = vec![PAD / 2.0];
        for &[l, t, r, b] in &obstacles {
            xs.extend([l - 3.0, r + 3.0]);
            ys.extend([t - 3.0, b + 3.0]);
        }
        xs.push(xs.iter().copied().fold(0.0, f64::max) + 12.0);
        ys.push(ys.iter().copied().fold(0.0, f64::max) + 12.0);
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        ys.sort_by(f64::total_cmp);
        ys.dedup();
        Ok(Self {
            work,
            xs,
            ys,
            obstacles,
            horizontal: HashMap::new(),
            vertical: HashMap::new(),
        })
    }

    fn clear(&mut self, p: (f64, f64), q: (f64, f64)) -> Result<bool, ()> {
        self.work.spend(1)?;
        let horizontal = (p.1 - q.1).abs() < 0.01;
        let (fixed, low, high) = if horizontal {
            (p.1, p.0.min(q.0), p.0.max(q.0))
        } else {
            (p.0, p.1.min(q.1), p.1.max(q.1))
        };
        let cached = if horizontal {
            self.horizontal.contains_key(&fixed.to_bits())
        } else {
            self.vertical.contains_key(&fixed.to_bits())
        };
        if !cached {
            self.work.spend(
                self.obstacles.len().max(1) * (self.obstacles.len().max(1).ilog2() as usize + 1),
            )?;
        }
        let cache = if horizontal {
            &mut self.horizontal
        } else {
            &mut self.vertical
        };
        let intervals = cache.entry(fixed.to_bits()).or_insert_with(|| {
            let mut intervals: Vec<(f64, f64)> = self
                .obstacles
                .iter()
                .filter_map(|&[l, t, r, b]| {
                    if horizontal {
                        (t < fixed && fixed < b).then_some((l, r))
                    } else {
                        (l < fixed && fixed < r).then_some((t, b))
                    }
                })
                .collect();
            intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (l, r) in intervals {
                if let Some(last) = merged.last_mut().filter(|last| last.1 >= l) {
                    last.1 = last.1.max(r);
                } else {
                    merged.push((l, r));
                }
            }
            merged
        });
        let next = intervals.partition_point(|&(_, end)| end <= low);
        Ok(intervals.get(next).is_none_or(|&(start, _)| start >= high))
    }
}

/// A compressed rectilinear visibility grid. Rows/columns use obstacle edges,
/// not pixel coordinates; A* explores only the corridors needed for this edge.
/// Boundary travel is allowed, but every segment is checked against interiors.
fn orthogonal_route(
    a: &Architecture,
    edge: &Edge,
    centers: &[(f64, f64)],
    router: &mut ArchitectureRouter,
) -> Result<Option<(String, f64, f64)>, ()> {
    use std::cmp::Ordering;
    use std::collections::{BinaryHeap, HashMap};
    let start = port_point(&a.nodes[edge.from], centers[edge.from], edge.from_side);
    let end = port_point(&a.nodes[edge.to], centers[edge.to], edge.to_side);
    let stub = |p: (f64, f64), side: Side| {
        let (dx, dy) = side.delta();
        (p.0 + dx as f64 * 12.0, p.1 + dy as f64 * 12.0)
    };
    let s = stub(start, edge.from_side);
    let t = stub(end, edge.to_side);
    router.work.spend(router.xs.len() + router.ys.len())?;
    let mut xs = router.xs.clone();
    let mut ys = router.ys.clone();
    for coordinate in [s.0, t.0] {
        if let Err(at) = xs.binary_search_by(|x| x.total_cmp(&coordinate)) {
            xs.insert(at, coordinate);
        }
    }
    for coordinate in [s.1, t.1] {
        if let Err(at) = ys.binary_search_by(|y| y.total_cmp(&coordinate)) {
            ys.insert(at, coordinate);
        }
    }
    let nx = xs.len();
    let index = |p: (f64, f64)| {
        ys.binary_search_by(|v| v.total_cmp(&p.1)).unwrap() * nx
            + xs.binary_search_by(|v| v.total_cmp(&p.0)).unwrap()
    };
    let source = index(s);
    let target = index(t);
    let point = |index: usize| (xs[index % nx], ys[index / nx]);
    #[derive(Clone, Copy)]
    struct Visit {
        score: f64,
        cost: f64,
        node: usize,
        direction: usize,
    }
    impl PartialEq for Visit {
        fn eq(&self, other: &Self) -> bool {
            self.score == other.score
                && self.node == other.node
                && self.direction == other.direction
        }
    }
    impl Eq for Visit {}
    impl PartialOrd for Visit {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Visit {
        fn cmp(&self, other: &Self) -> Ordering {
            other
                .score
                .total_cmp(&self.score)
                .then_with(|| other.node.cmp(&self.node))
                .then_with(|| other.direction.cmp(&self.direction))
        }
    }
    let initial_direction = usize::from(matches!(edge.from_side, Side::T | Side::B));
    let mut heap = BinaryHeap::from([Visit {
        score: 0.0,
        cost: 0.0,
        node: source,
        direction: initial_direction,
    }]);
    let mut best = HashMap::from([((source, initial_direction), 0.0)]);
    let mut previous = HashMap::new();
    let mut found = None;
    while let Some(visit) = heap.pop() {
        router.work.spend(1)?;
        if best.len() > 100_000 {
            return Ok(None);
        }
        let key = (visit.node, visit.direction);
        if visit.cost > best[&key] + 0.01 {
            continue;
        }
        if visit.node == target {
            found = Some(key);
            break;
        }
        let (ix, iy) = (visit.node % nx, visit.node / nx);
        let neighbors = [
            ix.checked_sub(1).map(|i| iy * nx + i),
            (ix + 1 < nx).then_some(visit.node + 1),
            iy.checked_sub(1).map(|i| i * nx + ix),
            (iy + 1 < ys.len()).then_some(visit.node + nx),
        ];
        for next in neighbors.into_iter().flatten() {
            let p = point(visit.node);
            let q = point(next);
            if !router.clear(p, q)? {
                continue;
            }
            let direction = usize::from(next % nx == ix);
            let cost = visit.cost
                + (p.0 - q.0).abs()
                + (p.1 - q.1).abs()
                + if direction == visit.direction {
                    0.0
                } else {
                    12.0
                };
            let next_key = (next, direction);
            if best.get(&next_key).is_some_and(|&old| old <= cost + 0.01) {
                continue;
            }
            best.insert(next_key, cost);
            previous.insert(next_key, key);
            let score = cost + (q.0 - t.0).abs() + (q.1 - t.1).abs();
            heap.push(Visit {
                score,
                cost,
                node: next,
                direction,
            });
        }
    }
    let Some(mut key) = found else {
        return Ok(None);
    };
    let mut points = vec![end, t];
    while key.0 != source {
        key = previous[&key];
        points.push(point(key.0));
    }
    points.push(start);
    points.reverse();
    // Remove collinear grid vertices without changing port endpoint segments.
    let mut simplified: Vec<(f64, f64)> = Vec::new();
    for p in points {
        if simplified.last() == Some(&p) {
            continue;
        }
        while simplified.len() >= 2 {
            let n = simplified.len();
            let a = simplified[n - 2];
            let b = simplified[n - 1];
            if ((a.0 - b.0).abs() < 0.01
                && (b.0 - p.0).abs() < 0.01
                && (b.1 - a.1) * (p.1 - b.1) >= 0.0)
                || ((a.1 - b.1).abs() < 0.01
                    && (b.1 - p.1).abs() < 0.01
                    && (b.0 - a.0) * (p.0 - b.0) >= 0.0)
            {
                simplified.pop();
            } else {
                break;
            }
        }
        simplified.push(p);
    }
    let path = simplified
        .iter()
        .enumerate()
        .map(|(i, (x, y))| format!("{} {x:.1} {y:.1}", if i == 0 { "M" } else { "L" }))
        .collect::<Vec<_>>()
        .join(" ");
    let (right, bottom) = simplified
        .iter()
        .fold((0.0_f64, 0.0_f64), |(r, b), &(x, y)| (r.max(x), b.max(y)));
    Ok(Some((path, right, bottom)))
}

// ---- rendering -----------------------------------------------------------

pub(crate) fn render_svg(a: &Architecture) -> String {
    let layout = &a.layout;
    let centers = &layout.centers;
    let group_bounds = &layout.groups;
    let paths = &layout.paths;
    let (total_w, total_h) = (layout.width, layout.height);
    let mut group_order: Vec<(usize, usize)> = a
        .groups
        .iter()
        .map(|&group| {
            let mut depth = 0;
            let mut parent = a.nodes[group].group;
            while let Some(p) = parent {
                depth += 1;
                parent = a.nodes[p].group;
            }
            (depth, group)
        })
        .collect();
    group_order.sort_unstable();
    let center = |i: usize| centers[i];
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:12px"><defs><marker id="mmd-arch-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
    );

    // Parents paint first so their fills never obscure enclosed groups.
    for &(_, group) in &group_order {
        let Some([x0, y0, x1, y1]) = group_bounds[group] else {
            continue;
        };
        out.push_str(&format!(
            r#"<rect x="{x0:.1}" y="{y0:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-cluster-fill, #7773)" stroke="currentColor" stroke-opacity="0.4" rx="8"/>"#,
            x1 - x0, y1 - y0
        ));
        if !a.nodes[group].icon.is_empty() {
            out.push_str(r#"<g data-architecture-group-icon="true">"#);
            draw_icon(
                &mut out,
                &a.nodes[group].icon,
                x0 + 8.0,
                y0 + 3.0,
                GROUP_ICON,
            );
            out.push_str("</g>");
        }
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" font-weight="600" fill="currentColor">{}</text>"#,
            group_caption_x(&a.nodes[group], [x0, y0, x1, y1]),
            y0 + 16.0,
            escape_xml(&group_caption(&a.nodes[group], [x0, y0, x1, y1]))
        ));
    }

    // Preserve marker paint outside actual glyphs. The halo is wider than
    // the glyph; it must not erase a neighboring arrow tip. Glyph interiors
    // still need the surface background for readable foreground contrast.
    let mut mask = format!(r##"<rect width="{total_w}" height="{total_h}" fill="#fff"/>"##);
    let mut edge_mask = mask.clone();
    for (i, node) in a.nodes.iter().enumerate() {
        if node.is_group || node.junction {
            continue;
        }
        let (cx, cy) = center(i);
        // Preserve the original edges-before-icons paint order, including
        // diagrams whose endpoint sides do not match the incoming direction.
        mask.push_str(&format!(
            r##"<rect x="{:.1}" y="{:.1}" width="{}" height="{}" fill="#000"/>"##,
            cx - ICON / 2.0 - 1.0,
            cy - ICON / 2.0 - 7.0,
            ICON + 2.0,
            ICON + 2.0
        ));
        let glyphs = format!(
            r##"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="#000" stroke="#000" stroke-width="0.3">{}</text>"##,
            cy + NAME_BASELINE,
            escape_xml(&clip(&node.title, CELL_W - 10.0))
        );
        mask.push_str(&glyphs);
        edge_mask.push_str(&glyphs);
    }
    let mask_id = crate::theme::paint_mask_id("arch-names", &mask);
    let edge_mask_id = crate::theme::paint_mask_id("arch-edges", &edge_mask);
    out.push_str(&format!(r#"<defs><mask id="{mask_id}" maskUnits="userSpaceOnUse" x="0" y="0" width="{total_w}" height="{total_h}">{mask}</mask><mask id="{edge_mask_id}" maskUnits="userSpaceOnUse" x="0" y="0" width="{total_w}" height="{total_h}">{edge_mask}</mask></defs><g mask="url(#{edge_mask_id})">"#));
    // Mask original edge stems at complete label glyphs, retaining the
    // original icon paint order and separate arrowhead visibility mask.
    // edges.
    for (e, path) in a.edges.iter().zip(paths) {
        let ms = if e.arrow_from {
            r#" marker-start="url(#mmd-arch-arrow)""#
        } else {
            ""
        };
        let me = if e.arrow_to {
            r#" marker-end="url(#mmd-arch-arrow)""#
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<path d="{path}" fill="none" stroke="currentColor" stroke-width="1.5"{ms}{me}/>"#
        ));
    }

    out.push_str("</g>");

    // nodes: icon glyph + label (junctions are small dots).
    for (i, node) in a.nodes.iter().enumerate() {
        if node.is_group {
            continue; // drawn as a group box above
        }
        let (cx, cy) = center(i);
        if node.junction {
            out.push_str(&format!(
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{JUNCTION_RADIUS}" fill="currentColor"/>"#
            ));
            continue;
        }
        let ix = cx - ICON / 2.0;
        let iy = cy - ICON / 2.0 - 6.0;
        draw_icon(&mut out, &node.icon, ix, iy, ICON);
        // A glyph halo blocks edges beneath the name without hiding an entire
        // bottom-port arrowhead, as a rectangular label backing would.
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor" stroke="var(--surface, #fff)" stroke-width="3" stroke-linejoin="round" paint-order="stroke">{}</text>"#,
            cy + NAME_BASELINE,
            escape_xml(&clip(&node.title, CELL_W - 10.0))
        ));
    }

    out.push_str(&format!(r#"<g mask="url(#{mask_id})">"#));
    // Repaint arrowheads above name halos, without repainting edge stems.
    // Accented glyph halos can reach a bottom-port tip even when a plain
    // label does not. Stroke-free paths preserve the marker geometry.
    for (e, path) in a.edges.iter().zip(paths) {
        if !e.arrow_from && !e.arrow_to {
            continue;
        }
        let ms = if e.arrow_from {
            r#" marker-start="url(#mmd-arch-arrow)""#
        } else {
            ""
        };
        let me = if e.arrow_to {
            r#" marker-end="url(#mmd-arch-arrow)""#
        } else {
            ""
        };
        out.push_str(&format!(
            r#"<path d="{path}" fill="none" stroke="none" stroke-width="1.5"{ms}{me}/>"#
        ));
    }
    out.push_str("</g></svg>");
    out
}

fn port_point(node: &Node, (cx, cy): (f64, f64), side: Side) -> (f64, f64) {
    if node.junction {
        return match side {
            Side::L => (cx - JUNCTION_RADIUS, cy),
            Side::R => (cx + JUNCTION_RADIUS, cy),
            Side::T => (cx, cy - JUNCTION_RADIUS),
            Side::B => (cx, cy + JUNCTION_RADIUS),
        };
    }
    let hw = ICON / 2.0 + 4.0;
    let hh = ICON / 2.0 + 4.0;
    match side {
        Side::L => (cx - hw, cy - 6.0),
        Side::R => (cx + hw, cy - 6.0),
        Side::T => (cx, cy - 6.0 - hh),
        Side::B => (cx, cy + NAME_BASELINE + BOTTOM_PORT_GAP),
    }
}

/// Built-in icon glyphs; the parser rejects unsupported names. An omitted
/// icon uses a rounded box.
/// Each known name gets a distinct multi-color glyph approximating the
/// iconify icons mermaid renders (cloud, globe, rack, cylinder, drive) —
/// fixed colors, same pattern as the diagram palettes elsewhere.
fn draw_icon(out: &mut String, icon: &str, x: f64, y: f64, s: f64) {
    const BLUE: &str = "#2563eb";
    const BLUE_LIGHT: &str = "#dbeafe";
    const BLUE_MID: &str = "#bfdbfe";
    const SLATE: &str = "#475569";
    const SLATE_LIGHT: &str = "#e2e8f0";
    const GREY: &str = "#94a3b8";
    const GREEN: &str = "#22c55e";
    const AMBER: &str = "#f59e0b";
    match icon {
        "database" => {
            let ry = 6.0;
            out.push_str(&format!(
                r#"<path d="M {x:.1} {:.1} L {x:.1} {:.1} A {:.1} {ry} 0 0 0 {:.1} {:.1} L {:.1} {:.1} A {:.1} {ry} 0 0 0 {x:.1} {:.1} Z" fill="{BLUE_LIGHT}" stroke="{BLUE}" stroke-width="1.5"/>"#,
                y + ry, y + s - ry, s / 2.0, x + s, y + s - ry, x + s, y + ry, s / 2.0, y + ry,
            ));
            out.push_str(&format!(
                r#"<ellipse cx="{:.1}" cy="{:.1}" rx="{:.1}" ry="{ry}" fill="{BLUE_MID}" stroke="{BLUE}" stroke-width="1.5"/>"#,
                x + s / 2.0,
                y + ry,
                s / 2.0
            ));
        }
        "disk" => {
            // Drive: slate case, platter spindle, amber activity LED.
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{:.1}" width="{s:.1}" height="{:.1}" fill="{SLATE_LIGHT}" stroke="{SLATE}" stroke-width="1.5" rx="4"/>"#,
                y + s * 0.08,
                s * 0.84,
            ));
            out.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{GREY}" stroke="{SLATE}"/><circle cx="{:.1}" cy="{:.1}" r="{:.1}" fill="{SLATE}"/>"#,
                x + s * 0.42, y + s * 0.5, s * 0.2,
                x + s * 0.42, y + s * 0.5, s * 0.06,
            ));
            out.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{:.1}" r="2.5" fill="{AMBER}"/>"#,
                x + s * 0.82,
                y + s * 0.78,
            ));
        }
        "cloud" => {
            // Material-style cloud silhouette, scaled from a 24x24 grid.
            let k = s / 24.0;
            out.push_str(&format!(
                r#"<g transform="translate({x:.1} {y:.1}) scale({k:.3})"><path d="M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96z" fill="{BLUE_LIGHT}" stroke="{BLUE}" stroke-width="1.5"/></g>"#
            ));
        }
        "internet" => {
            // Globe: circle + equator + meridian ellipse + latitude chords.
            let (cx, cy, r) = (x + s / 2.0, y + s / 2.0, s * 0.44);
            out.push_str(&format!(
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{BLUE_LIGHT}" stroke="{BLUE}" stroke-width="1.5"/>"#
            ));
            out.push_str(&format!(
                r#"<ellipse cx="{cx:.1}" cy="{cy:.1}" rx="{:.1}" ry="{r:.1}" fill="none" stroke="{BLUE}" opacity="0.7"/>"#,
                r * 0.45
            ));
            let lat = r * 0.5;
            let half = (r * r - lat * lat).sqrt();
            out.push_str(&format!(
                r#"<line x1="{:.1}" y1="{cy:.1}" x2="{:.1}" y2="{cy:.1}" stroke="{BLUE}" opacity="0.7"/><line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{BLUE}" opacity="0.5"/><line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{BLUE}" opacity="0.5"/>"#,
                cx - r, cx + r,
                cx - half, cy - lat, cx + half, cy - lat,
                cx - half, cy + lat, cx + half, cy + lat,
            ));
        }
        "server" => {
            // Rack: slate chassis, unit dividers, green status LEDs + vents.
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{s:.1}" height="{s:.1}" fill="{SLATE_LIGHT}" stroke="{SLATE}" stroke-width="1.5" rx="3"/><line x1="{x:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{SLATE}" opacity="0.6"/><line x1="{x:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{SLATE}" opacity="0.6"/>"#,
                y + s / 3.0, x + s, y + s / 3.0,
                y + 2.0 * s / 3.0, x + s, y + 2.0 * s / 3.0,
            ));
            for u in 0..3 {
                let uy = y + (u as f64 + 0.5) * s / 3.0;
                out.push_str(&format!(
                    r#"<circle cx="{:.1}" cy="{uy:.1}" r="2.2" fill="{GREEN}"/><line x1="{:.1}" y1="{uy:.1}" x2="{:.1}" y2="{uy:.1}" stroke="{SLATE}" opacity="0.5"/>"#,
                    x + s * 0.18,
                    x + s * 0.45,
                    x + s * 0.82,
                ));
            }
        }
        _ => {
            out.push_str(&format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{s:.1}" height="{s:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"/>"#
            ));
        }
    }
}

fn clip(s: &str, max_w: f64) -> String {
    measure::truncate_literal_to_width(s, max_w, 12.0, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_work_allowance_is_cumulative_across_edges() {
        let diagram =
            parse("architecture-beta\nservice a(server)[A]\nservice b(server)[B]\na:R --> R:b")
                .unwrap();
        let obstacles =
            routing_obstacles(&diagram, &diagram.layout.centers, &diagram.layout.groups);
        let mut probe = ArchitectureRouter::new(
            obstacles.clone(),
            LayoutWork {
                remaining: MAX_LAYOUT_WORK,
            },
        )
        .unwrap();
        orthogonal_route(
            &diagram,
            &diagram.edges[0],
            &diagram.layout.centers,
            &mut probe,
        )
        .unwrap()
        .unwrap();
        let first_work = MAX_LAYOUT_WORK - probe.work.remaining;
        let mut limited = ArchitectureRouter::new(
            obstacles,
            LayoutWork {
                remaining: first_work + 1,
            },
        )
        .unwrap();
        orthogonal_route(
            &diagram,
            &diagram.edges[0],
            &diagram.layout.centers,
            &mut limited,
        )
        .unwrap()
        .unwrap();
        assert!(orthogonal_route(
            &diagram,
            &diagram.edges[0],
            &diagram.layout.centers,
            &mut limited
        )
        .is_err());
    }

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
    fn icons_are_distinct_multicolor_glyphs() {
        // Each known icon renders its own glyph, not a shared shape: cloud a
        // silhouette path, internet a globe (circle + meridian ellipse),
        // server a rack with green LEDs, database a blue cylinder, disk a
        // drive with spindle + amber LED.
        let svg = render_svg(
            &parse(
                "architecture-beta\n service a(cloud)[A]\n service b(internet)[B]\n service c(server)[C]\n service d(database)[D]\n service e(disk)[E]",
            )
            .unwrap(),
        );
        assert!(svg.contains("M19.35 10.04"), "cloud silhouette path: {svg}");
        assert!(
            svg.contains("<ellipse") && svg.contains(r#"opacity="0.7""#),
            "globe meridian: {svg}"
        );
        assert!(svg.contains("#22c55e"), "server status LEDs: {svg}");
        assert!(svg.contains("#bfdbfe"), "database cylinder top: {svg}");
        assert!(svg.contains("#f59e0b"), "disk activity LED: {svg}");
    }

    #[test]
    fn placement_respects_sides() {
        // b sits to the right of a (a leaves via R).
        let a =
            parse("architecture-beta\n service a(server)[A]\n service b(server)[B]\n a:R -- L:b")
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

    #[test]
    fn two_edges_leaving_the_same_left_side_terminate() {
        // Both b and c want the cell left of a; the collision nudge used
        // to step by (0, 0) and spin forever.
        let a = parse(
            "architecture-beta\n service a(server)[A]\n service b(disk)[B]\n service c(disk)[C]\n a:L -- R:b\n a:L -- R:c",
        )
        .unwrap();
        let svg = render_svg(&a);
        assert!(svg.contains(">B<") && svg.contains(">C<"));
    }
}
