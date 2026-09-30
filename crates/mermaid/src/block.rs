//! Mermaid `block` (`block-beta`) diagrams: parser + SVG renderer (Tier 3).
//!
//! Blocks flow left-to-right into `columns N` and wrap; line breaks in the
//! source don't start rows (mermaid.js semantics). Without `columns`, a
//! group's children share one row. Items:
//!
//! - `id`, `id["label"]`, `id("label")`, … — every flowchart node shape,
//!   quoted or bare labels — with an optional `:N` column span;
//! - `space` / `space:N` — a blank gap;
//! - `block` / `block:id` / `block:id:N` … `end` — a nested group with
//!   its own `columns`;
//! - `a --> b`, `a --- b`, `a -- "label" --> b`, `a -->|label| b` — arrows
//!   between declared blocks;
//! - `style`, `classDef`, `class` — the shared style allowlist.
//!
//! Anything else (block arrows, `accTitle`, unknown ids) is a parse error
//! with a line number rather than a silently wrong diagram.

use crate::flowchart::{shapes, ShapeKind};
use crate::style::{self, ClassDef};
use crate::{escape_xml, measure, ParseError};

const PAD: f64 = 20.0;
const CELL_H: f64 = 44.0;
const GAP: f64 = 12.0;
const MIN_CELL_W: f64 = 80.0;
/// Padding between a group's border and its children.
const GROUP_PAD: f64 = 10.0;
const MAX_BLOCKS: usize = 1000;

#[derive(Debug, Clone)]
pub(crate) enum Item {
    Leaf { shape: ShapeKind, label: String, classes: Vec<String>, style: Option<String> },
    Space,
    Group { label: Option<String>, columns: Option<usize>, children: Vec<usize> },
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub item: Item,
    pub span: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockArrow {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    /// `---`: no arrowhead.
    pub open: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct BlockDiagram {
    /// `nodes[0]` is the root group.
    pub nodes: Vec<Node>,
    pub arrows: Vec<BlockArrow>,
    pub class_defs: Vec<ClassDef>,
}

impl BlockDiagram {
    fn children(&self, i: usize) -> &[usize] {
        match &self.nodes[i].item {
            Item::Group { children, .. } => children,
            _ => &[],
        }
    }
}

fn err(message: impl Into<String>, line: usize) -> ParseError {
    ParseError { message: message.into(), line: Some(line) }
}

struct Parser {
    nodes: Vec<Node>,
    ids: std::collections::HashMap<String, usize>,
    /// Open groups, innermost last, with the line that opened each.
    stack: Vec<(usize, usize)>,
    arrows: Vec<(String, String, Option<String>, bool, usize)>,
    styles: Vec<(String, String, usize)>,
    class_assigns: Vec<(Vec<String>, String, usize)>,
    class_defs: Vec<ClassDef>,
}

pub(crate) fn parse(source: &str) -> Result<BlockDiagram, ParseError> {
    let mut p = Parser {
        nodes: vec![Node { item: Item::Group { label: None, columns: None, children: Vec::new() }, span: 1 }],
        ids: Default::default(),
        stack: vec![(0, 0)],
        arrows: Vec::new(),
        styles: Vec::new(),
        class_assigns: Vec::new(),
        class_defs: Vec::new(),
    };
    let mut seen_header = false;
    for (idx, raw) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let h = line.strip_suffix(';').unwrap_or(line).trim_end();
            if h != "block-beta" && h != "block" {
                return Err(err("block diagram must start with `block` or `block-beta`", line_no));
            }
            seen_header = true;
            continue;
        }
        p.statement(line, line_no)?;
    }
    if !seen_header {
        return Err(ParseError {
            message: "block diagram must start with `block` or `block-beta`".into(),
            line: Some(1),
        });
    }
    if let Some(&(_, open_line)) = p.stack.get(1) {
        return Err(err("`block` group is never closed with `end`", open_line));
    }
    p.finish()
}

impl Parser {
    fn statement(&mut self, line: &str, line_no: usize) -> Result<(), ParseError> {
        let first = line.split_whitespace().next().unwrap_or("");
        if crate::acc_directive_keyword(line).is_some() {
            let kw = crate::acc_directive_keyword(line).unwrap();
            return Err(err(format!("`{kw}` statements are not supported in block diagrams"), line_no));
        }
        match first {
            "columns" => {
                let arg = line["columns".len()..].trim();
                let cols = if arg == "auto" {
                    None
                } else {
                    match arg.parse::<usize>() {
                        Ok(n) if (1..=MAX_BLOCKS).contains(&n) => Some(n),
                        _ => return Err(err(format!("`columns` needs a number from 1 to {MAX_BLOCKS} or `auto`, got {arg:?}"), line_no)),
                    }
                };
                let (g, _) = *self.stack.last().unwrap();
                if let Item::Group { columns, .. } = &mut self.nodes[g].item {
                    *columns = cols;
                }
                return Ok(());
            }
            "style" => {
                let rest = line["style".len()..].trim();
                let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
                    return Err(err("`style` needs a block id and styles", line_no));
                };
                self.styles.push((id.trim().to_string(), style::sanitize_style(styles), line_no));
                return Ok(());
            }
            "classDef" => {
                let rest = line["classDef".len()..].trim();
                let Some((names, styles)) = rest.split_once(char::is_whitespace) else {
                    return Err(err("`classDef` needs a class name and styles", line_no));
                };
                let style = style::sanitize_style(styles);
                for name in names.split(',') {
                    self.class_defs.push(ClassDef { name: name.trim().to_string(), style: style.clone() });
                }
                return Ok(());
            }
            "class" => {
                let rest = line["class".len()..].trim();
                let Some((ids, name)) = rest.rsplit_once(char::is_whitespace) else {
                    return Err(err("`class` needs block ids and a class name", line_no));
                };
                let ids = ids.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                self.class_assigns.push((ids, name.trim().to_string(), line_no));
                return Ok(());
            }
            "linkStyle" | "click" => {
                return Err(err(format!("`{first}` statements are not supported in block diagrams"), line_no));
            }
            _ => {}
        }
        let mut lx = Lexer { s: line, line_no };
        let mut pending_edge: Option<(String, Option<String>, bool)> = None;
        let mut last_id: Option<String> = None;
        while !lx.at_end() {
            if let Some((label, open)) = lx.edge_op()? {
                let Some(from) = last_id.take() else {
                    return Err(err("arrow has no block on its left", line_no));
                };
                if pending_edge.is_some() {
                    return Err(err("two arrows in a row", line_no));
                }
                pending_edge = Some((from, label, open));
                continue;
            }
            let tok = lx.item()?;
            let id = match tok {
                Tok::End => {
                    if pending_edge.is_some() {
                        return Err(err("arrow has no block on its right", line_no));
                    }
                    if self.stack.len() == 1 {
                        return Err(err("`end` without an open `block` group", line_no));
                    }
                    self.stack.pop();
                    last_id = None;
                    continue;
                }
                Tok::Space(span) => {
                    if pending_edge.is_some() {
                        return Err(err("an arrow can't point at `space`", line_no));
                    }
                    self.push(Node { item: Item::Space, span }, line_no)?;
                    last_id = None;
                    continue;
                }
                Tok::Group { id, label, span } => {
                    if pending_edge.is_some() {
                        return Err(err("an arrow can't point at a `block` group", line_no));
                    }
                    if self.stack.len() > crate::MAX_NESTING {
                        return Err(err("block groups nested too deeply", line_no));
                    }
                    let idx = self.push(Node { item: Item::Group { label, columns: None, children: Vec::new() }, span }, line_no)?;
                    if let Some(id) = id {
                        self.declare(id, idx, line_no)?;
                    }
                    self.stack.push((idx, line_no));
                    last_id = None;
                    continue;
                }
                Tok::Block { id, shape, label, span } => {
                    // A bare id at either end of an arrow is a reference
                    // (resolved once the whole diagram is read); anything
                    // else declares a block.
                    let in_arrow = pending_edge.is_some() || lx.s.trim_start().starts_with("--");
                    if !(in_arrow && shape.is_none() && span == 1) {
                        let node = Node {
                            item: Item::Leaf {
                                shape: shape.unwrap_or(ShapeKind::Rect),
                                label: label.unwrap_or_else(|| id.clone()),
                                classes: Vec::new(),
                                style: None,
                            },
                            span,
                        };
                        let idx = self.push(node, line_no)?;
                        self.declare(id.clone(), idx, line_no)?;
                    }
                    id
                }
            };
            if let Some((from, label, open)) = pending_edge.take() {
                self.arrows.push((from, id.clone(), label, open, line_no));
            }
            last_id = Some(id);
        }
        if pending_edge.is_some() {
            return Err(err("arrow has no block on its right", line_no));
        }
        Ok(())
    }

    fn push(&mut self, node: Node, line_no: usize) -> Result<usize, ParseError> {
        if self.nodes.len() > MAX_BLOCKS {
            return Err(err(format!("block diagram too large: >{MAX_BLOCKS} blocks"), line_no));
        }
        let idx = self.nodes.len();
        self.nodes.push(node);
        let (g, _) = *self.stack.last().unwrap();
        if let Item::Group { children, .. } = &mut self.nodes[g].item {
            children.push(idx);
        }
        Ok(idx)
    }

    fn declare(&mut self, id: String, idx: usize, line_no: usize) -> Result<(), ParseError> {
        if self.ids.insert(id.clone(), idx).is_some() {
            return Err(err(format!("block `{id}` is declared twice"), line_no));
        }
        Ok(())
    }

    fn lookup(&self, id: &str, line_no: usize) -> Result<usize, ParseError> {
        self.ids.get(id).copied().ok_or_else(|| err(format!("unknown block `{id}`"), line_no))
    }

    fn finish(mut self) -> Result<BlockDiagram, ParseError> {
        let mut arrows = Vec::new();
        for (a, b, label, open, line_no) in std::mem::take(&mut self.arrows) {
            let (from, to) = (self.lookup(&a, line_no)?, self.lookup(&b, line_no)?);
            arrows.push(BlockArrow { from, to, label, open });
        }
        for (id, s, line_no) in std::mem::take(&mut self.styles) {
            let i = self.lookup(&id, line_no)?;
            match &mut self.nodes[i].item {
                Item::Leaf { style, .. } => *style = Some(s).filter(|s| !s.is_empty()),
                _ => return Err(err(format!("`style` can't be applied to group `{id}`"), line_no)),
            }
        }
        for (ids, name, line_no) in std::mem::take(&mut self.class_assigns) {
            for id in ids {
                let i = self.lookup(&id, line_no)?;
                if let Item::Leaf { classes, .. } = &mut self.nodes[i].item {
                    classes.push(name.clone());
                }
            }
        }
        Ok(BlockDiagram { nodes: self.nodes, arrows, class_defs: self.class_defs })
    }
}

enum Tok {
    Block { id: String, shape: Option<ShapeKind>, label: Option<String>, span: usize },
    Space(usize),
    Group { id: Option<String>, label: Option<String>, span: usize },
    End,
}

struct Lexer<'a> {
    s: &'a str,
    line_no: usize,
}

/// (opener, [(closer, shape)]) — longest openers first; the flowchart
/// shape vocabulary.
const SHAPES: &[(&str, &[(&str, ShapeKind)])] = &[
    ("(((", &[(")))", ShapeKind::DoubleCircle)]),
    ("((", &[("))", ShapeKind::Circle)]),
    ("([", &[("])", ShapeKind::Stadium)]),
    ("[[", &[("]]", ShapeKind::Subroutine)]),
    ("[(", &[(")]", ShapeKind::Cylinder)]),
    ("[/", &[("/]", ShapeKind::Parallelogram), ("\\]", ShapeKind::Trapezoid)]),
    ("[\\", &[("\\]", ShapeKind::ParallelogramRev), ("/]", ShapeKind::TrapezoidRev)]),
    ("{{", &[("}}", ShapeKind::Hexagon)]),
    ("{", &[("}", ShapeKind::Diamond)]),
    ("[", &[("]", ShapeKind::Rect)]),
    ("(", &[(")", ShapeKind::Rounded)]),
    (">", &[("]", ShapeKind::Flag)]),
];

impl<'a> Lexer<'a> {
    fn at_end(&mut self) -> bool {
        self.s = self.s.trim_start();
        self.s.is_empty()
    }

    fn err(&self, m: impl Into<String>) -> ParseError {
        err(m, self.line_no)
    }

    /// An arrow operator at the cursor: `-->`, `---`, `-->|label|`,
    /// `-- "label" -->` / `-- label -->`. `Some((label, open))`.
    fn edge_op(&mut self) -> Result<Option<(Option<String>, bool)>, ParseError> {
        let s = self.s;
        let op = if s.starts_with("-->") {
            Some(("-->", false))
        } else if s.starts_with("---") {
            Some(("---", true))
        } else {
            None
        };
        if let Some((op, open)) = op {
            let mut rest = &s[op.len()..];
            let mut label = None;
            if let Some(r) = rest.trim_start().strip_prefix('|') {
                let Some(end) = r.find('|') else { return Err(self.err("unclosed `|label|` on arrow")) };
                label = Some(unquote(r[..end].trim()).to_string());
                rest = &r[end + 1..];
            }
            self.s = rest;
            return Ok(Some((label, open)));
        }
        if let Some(r) = s.strip_prefix("--") {
            // Inline label: `-- text -->` / `-- text ---`.
            let (end, open, len) = match (r.find("-->"), r.find("---")) {
                (Some(a), Some(b)) if b < a => (b, true, 3),
                (Some(a), _) => (a, false, 3),
                (None, Some(b)) => (b, true, 3),
                (None, None) => return Err(self.err("`--` starts an arrow label that never ends in `-->` or `---`")),
            };
            let label = unquote(r[..end].trim()).to_string();
            self.s = &r[end + len..];
            return Ok(Some((Some(label).filter(|l| !l.is_empty()), open)));
        }
        Ok(None)
    }

    fn item(&mut self) -> Result<Tok, ParseError> {
        let s = self.s;
        // An id runs to whitespace, a shape opener, `:` or an arrow.
        let mut end = s.len();
        for (i, c) in s.char_indices() {
            if c.is_whitespace() || "[({>:\"|".contains(c) || s[i..].starts_with("--") {
                end = i;
                break;
            }
            if c == '<' {
                return Err(self.err(format!(
                    "block arrows (`{}`) are not supported",
                    s.split_whitespace().next().unwrap_or(s)
                )));
            }
        }
        let id = &s[..end];
        if id.is_empty() {
            let bad = s.chars().next().unwrap_or(' ');
            return Err(self.err(format!("expected a block id, found `{bad}`")));
        }
        self.s = &s[end..];
        match id {
            "end" => return Ok(Tok::End),
            "space" => return Ok(Tok::Space(self.span()?)),
            "block" => {
                let gid = match self.s.strip_prefix(':') {
                    Some(r) if !r.starts_with(|c: char| c.is_ascii_digit()) => {
                        let e = r.find(|c: char| c.is_whitespace() || "[({>:".contains(c)).unwrap_or(r.len());
                        if e == 0 {
                            return Err(self.err("`block:` needs a group id"));
                        }
                        self.s = &r[e..];
                        Some(r[..e].to_string())
                    }
                    _ => None,
                };
                let label = self.shape()?.map(|(_, l)| l);
                return Ok(Tok::Group { id: gid, label, span: self.span()? });
            }
            _ => {}
        }
        let shape = self.shape()?;
        let span = self.span()?;
        if self.s.starts_with('<') {
            return Err(self.err(format!("block arrows (`{id}<…>`) are not supported")));
        }
        let (shape, label) = match shape {
            Some((sh, l)) => (Some(sh), Some(l)),
            None => (None, None),
        };
        Ok(Tok::Block { id: id.to_string(), shape, label, span })
    }

    fn span(&mut self) -> Result<usize, ParseError> {
        let Some(r) = self.s.strip_prefix(':') else { return Ok(1) };
        let e = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        match r[..e].parse::<usize>() {
            Ok(n) if n >= 1 => {
                self.s = &r[e..];
                Ok(n.min(MAX_BLOCKS))
            }
            _ => Err(self.err("`:` must be followed by a column span (1 or more)")),
        }
    }

    fn shape(&mut self) -> Result<Option<(ShapeKind, String)>, ParseError> {
        for (opener, closers) in SHAPES {
            let Some(body) = self.s.strip_prefix(opener) else { continue };
            // A quoted label may contain closer characters and spaces.
            if let Some(q) = body.trim_start().strip_prefix('"') {
                let Some(qi) = q.find('"') else { return Err(self.err("unclosed `\"` in block label")) };
                let after = q[qi + 1..].trim_start();
                for (closer, shape) in *closers {
                    if let Some(rest) = after.strip_prefix(closer) {
                        self.s = rest;
                        return Ok(Some((*shape, q[..qi].to_string())));
                    }
                }
                return Err(self.err(format!("unclosed `{opener}` after a quoted label")));
            }
            let best = closers
                .iter()
                .filter_map(|(c, sh)| body.find(c).map(|i| (i, *c, *sh)))
                .min_by_key(|(i, _, _)| *i);
            let Some((i, closer, shape)) = best else {
                return Err(self.err(format!("unclosed `{opener}` in block shape")));
            };
            self.s = &body[i + closer.len()..];
            return Ok(Some((shape, body[..i].trim().to_string())));
        }
        Ok(None)
    }
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s)
}

// ── Layout ──────────────────────────────────────────────────────────

/// Cells of a group's children: `(child, col, row)` flowed into the
/// group's column count.
fn flow(d: &BlockDiagram, g: usize) -> (usize, Vec<(usize, usize, usize)>) {
    let kids = d.children(g);
    let cols = match &d.nodes[g].item {
        Item::Group { columns: Some(c), .. } => *c,
        // No `columns`: one row holding every child.
        _ => kids.iter().map(|&k| d.nodes[k].span).sum::<usize>().max(1),
    };
    let (mut col, mut row) = (0, 0);
    let mut cells = Vec::with_capacity(kids.len());
    for &k in kids {
        let span = d.nodes[k].span.min(cols);
        if col + span > cols {
            col = 0;
            row += 1;
        }
        cells.push((k, col, row));
        col += span;
    }
    (cols, cells)
}

fn label_size(label: &str) -> (f64, f64) {
    measure::text_size(label)
}

/// Group header height (a labelled group's title strip).
fn header_h(d: &BlockDiagram, g: usize) -> f64 {
    match &d.nodes[g].item {
        Item::Group { label: Some(l), .. } if g != 0 && !l.is_empty() => label_size(l).1 + 6.0,
        _ => 0.0,
    }
}

/// Minimum `(w, h)` of every node, children first.
fn min_sizes(d: &BlockDiagram) -> Vec<(f64, f64)> {
    let mut size = vec![(0.0, 0.0); d.nodes.len()];
    // Children always have larger indices than their group.
    for i in (0..d.nodes.len()).rev() {
        size[i] = match &d.nodes[i].item {
            Item::Space => (0.0, 0.0),
            Item::Leaf { shape, label, .. } => {
                let (tw, th) = label_size(label);
                let (w, h) = shapes::size_for(*shape, tw, th);
                (w, h.max(CELL_H))
            }
            Item::Group { label, .. } => {
                let (cols, cells) = flow(d, i);
                let (cell_w, row_h) = grid_mins(d, &size, &cells);
                let inner_w = cols as f64 * cell_w + (cols as f64 - 1.0) * GAP;
                let title_w = label.as_deref().map(|l| label_size(l).0).unwrap_or(0.0);
                let rows_h: f64 = row_h.iter().sum::<f64>() + (row_h.len().max(1) as f64 - 1.0) * GAP;
                (inner_w.max(title_w) + 2.0 * GROUP_PAD, rows_h + 2.0 * GROUP_PAD + header_h(d, i))
            }
        };
    }
    size
}

/// A group's minimum uniform cell width and per-row heights.
fn grid_mins(d: &BlockDiagram, size: &[(f64, f64)], cells: &[(usize, usize, usize)]) -> (f64, Vec<f64>) {
    let mut cell_w = MIN_CELL_W;
    let mut row_h: Vec<f64> = Vec::new();
    for &(k, _, row) in cells {
        let span = d.nodes[k].span as f64;
        cell_w = cell_w.max((size[k].0 - (span - 1.0) * GAP) / span);
        if row_h.len() <= row {
            row_h.resize(row + 1, CELL_H);
        }
        row_h[row] = row_h[row].max(size[k].1);
    }
    (cell_w, row_h)
}

pub(crate) struct Placed {
    /// Drawn rect per node (x, y, w, h); `None` for spaces and the root.
    pub rect: Vec<Option<(f64, f64, f64, f64)>>,
    pub size: (f64, f64),
}

pub(crate) fn layout(d: &BlockDiagram) -> Placed {
    let size = min_sizes(d);
    let mut rect = vec![None; d.nodes.len()];
    let (root_w, root_h) = size[0];
    // The root's own padding is the canvas margin instead.
    let (w, h) = (root_w - 2.0 * GROUP_PAD, root_h - 2.0 * GROUP_PAD);
    place_children(d, &size, &mut rect, 0, PAD, PAD, w);
    Placed { rect, size: (w + 2.0 * PAD, h + 2.0 * PAD) }
}

/// Lay out group `g`'s children in the box starting at `(x, y)` with
/// inner width `w` (cells stretch to fill it).
fn place_children(d: &BlockDiagram, size: &[(f64, f64)], rect: &mut [Option<(f64, f64, f64, f64)>], g: usize, x: f64, y: f64, w: f64) {
    let (cols, cells) = flow(d, g);
    let (_, row_h) = grid_mins(d, size, &cells);
    let cell_w = (w - (cols as f64 - 1.0) * GAP) / cols as f64;
    let mut row_y = Vec::with_capacity(row_h.len());
    let mut acc = y;
    for h in &row_h {
        row_y.push(acc);
        acc += h + GAP;
    }
    for (k, col, row) in cells {
        let span = d.nodes[k].span.min(cols) as f64;
        let cx = x + col as f64 * (cell_w + GAP);
        let cw = span * cell_w + (span - 1.0) * GAP;
        let (ry, rh) = (row_y[row], row_h[row]);
        match &d.nodes[k].item {
            Item::Space => {}
            Item::Leaf { .. } => {
                // Own height, centered in the row; full cell width.
                let h = size[k].1.min(rh);
                rect[k] = Some((cx, ry + (rh - h) / 2.0, cw, h));
            }
            Item::Group { .. } => {
                rect[k] = Some((cx, ry, cw, rh));
                let top = ry + GROUP_PAD + header_h(d, k);
                place_children(d, size, rect, k, cx + GROUP_PAD, top, cw - 2.0 * GROUP_PAD);
            }
        }
    }
}

pub(crate) fn render_svg(d: &BlockDiagram) -> String {
    let placed = layout(d);
    let (total_w, total_h) = placed.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {total_w:.0} {total_h:.0}" width="{total_w:.0}" height="{total_h:.0}" style="font-family:sans-serif;font-size:14px"><defs><marker id="mmd-blk-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker></defs>"#
    );

    // Groups first (outer before inner: parents have smaller indices).
    for (i, n) in d.nodes.iter().enumerate() {
        let (Item::Group { label, .. }, Some((x, y, w, h))) = (&n.item, placed.rect[i]) else { continue };
        out.push_str(&format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="4" fill="var(--mermaid-cluster-fill, #7771)" stroke="currentColor" stroke-dasharray="4 3"/>"#
        ));
        if let Some(l) = label.as_deref().filter(|l| !l.is_empty()) {
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">{}</text>"#,
                x + w / 2.0,
                y + GROUP_PAD + 12.0,
                escape_xml(l)
            ));
        }
    }

    let center = |i: usize| {
        let (x, y, w, h) = placed.rect[i].unwrap_or_default();
        (x + w / 2.0, y + h / 2.0)
    };
    // The point where the center→center segment leaves node `i`'s box,
    // heading toward `(tx, ty)`, so the arrowhead lands on the border.
    let border = |i: usize, tx: f64, ty: f64| -> (f64, f64) {
        let (cx, cy) = center(i);
        let (_, _, w, h) = placed.rect[i].unwrap_or_default();
        let (dx, dy) = (tx - cx, ty - cy);
        let s = (w / 2.0 / dx.abs().max(1e-6)).min(h / 2.0 / dy.abs().max(1e-6));
        (cx + dx * s, cy + dy * s)
    };
    for a in &d.arrows {
        let (fcx, fcy) = center(a.from);
        let (tcx, tcy) = center(a.to);
        let (x1, y1) = border(a.from, tcx, tcy);
        let (x2, y2) = border(a.to, fcx, fcy);
        let head = if a.open { "" } else { r#" marker-end="url(#mmd-blk-arrow)""# };
        out.push_str(&format!(
            r#"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="currentColor" stroke-width="1.5"{head}/>"#
        ));
        if let Some(lbl) = &a.label {
            let tw = measure::text_size(lbl).0 * 12.0 / measure::FONT_PX;
            let (mx, my) = ((x1 + x2) / 2.0, (y1 + y2) / 2.0);
            out.push_str(&format!(
                r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="18" fill="var(--surface, #fff)"/><text x="{mx:.1}" y="{:.1}" text-anchor="middle" font-size="12" fill="currentColor">{}</text>"#,
                mx - tw / 2.0 - 2.0,
                my - 9.0,
                tw + 4.0,
                my + 4.0,
                escape_xml(lbl)
            ));
        }
    }

    for (i, n) in d.nodes.iter().enumerate() {
        let (Item::Leaf { shape, label, classes, style: inline, .. }, Some((x, y, w, h))) = (&n.item, placed.rect[i]) else {
            continue;
        };
        let style = style::resolve(classes, inline.as_deref(), &d.class_defs);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        match &style {
            Some(s) => out.push_str(&format!(r#"<g style="{}">"#, escape_xml(s))),
            None => out.push_str("<g>"),
        }
        out.push_str(&shapes::emit(*shape, cx, cy, w, h, style.as_deref()));
        let lines = measure::lines(label);
        let n_lines = lines.len() as f64;
        out.push_str(&format!(r#"<text x="{cx:.1}" y="{cy:.1}" text-anchor="middle" fill="currentColor">"#));
        for (li, line) in lines.iter().enumerate() {
            let dy = if li == 0 { -(n_lines - 1.0) / 2.0 * measure::LINE_H + 5.0 } else { measure::LINE_H };
            out.push_str(&format!(r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#, escape_xml(line)));
        }
        out.push_str("</text></g>");
    }

    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf_labels(d: &BlockDiagram) -> Vec<&str> {
        d.nodes
            .iter()
            .filter_map(|n| match &n.item {
                Item::Leaf { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect()
    }

    /// `(col, row)` of the block labelled `label` in its group's grid.
    fn cell(d: &BlockDiagram, label: &str) -> (usize, usize) {
        let i = d
            .nodes
            .iter()
            .position(|n| matches!(&n.item, Item::Leaf { label: x, .. } if x == label))
            .unwrap();
        let g = (0..d.nodes.len()).find(|&g| d.children(g).contains(&i)).unwrap();
        let (_, cells) = flow(d, g);
        cells.iter().find(|c| c.0 == i).map(|c| (c.1, c.2)).unwrap()
    }

    #[test]
    fn blocks_flow_into_columns_across_lines() {
        // #279: line breaks don't start rows; blocks flow and wrap.
        let d = parse("block-beta\n columns 3\n a b c\n d\n e:2 f").unwrap();
        assert_eq!(cell(&d, "a"), (0, 0));
        assert_eq!(cell(&d, "c"), (2, 0));
        assert_eq!(cell(&d, "d"), (0, 1));
        assert_eq!(cell(&d, "e"), (1, 1));
        assert_eq!(cell(&d, "f"), (0, 2));
        let d = parse("block-beta\ncolumns 3\na\nb\nc").unwrap();
        assert_eq!([cell(&d, "a"), cell(&d, "b"), cell(&d, "c")], [(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn without_columns_blocks_share_one_row() {
        let d = parse("block-beta\na b c").unwrap();
        assert_eq!([cell(&d, "a"), cell(&d, "b"), cell(&d, "c")], [(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn quoted_labels_with_spaces_are_one_block() {
        let d = parse("block-beta\na[\"Hello world\"] b").unwrap();
        assert_eq!(leaf_labels(&d), ["Hello world", "b"]);
        let d = parse("block-beta\na[Hello world] b(\"has ] inside\")").unwrap();
        assert_eq!(leaf_labels(&d), ["Hello world", "has ] inside"]);
    }

    #[test]
    fn shapes_parse_and_render() {
        let d = parse("block-beta\nz((\"circle\")) y{\"rhombus\"} x([stadium]) w[(db)] v{{hex}} u>flag] t[[sub]]").unwrap();
        let shapes: Vec<ShapeKind> = d
            .nodes
            .iter()
            .filter_map(|n| match &n.item {
                Item::Leaf { shape, .. } => Some(*shape),
                _ => None,
            })
            .collect();
        use ShapeKind::*;
        assert_eq!(shapes, [Circle, Diamond, Stadium, Cylinder, Hexagon, Flag, Subroutine]);
        let svg = render_svg(&d);
        assert!(svg.contains("<circle") && svg.contains("<polygon"), "{svg}");
        assert!(svg.contains(">circle<") && svg.contains(">rhombus<"));
        crate::extent::assert_inside(&svg);
    }

    #[test]
    fn groups_nest_with_their_own_columns() {
        let d = parse("block-beta\ncolumns 3\na\nblock:grp:2\n  columns 2\n  b c d\nend\ne").unwrap();
        let g = d.nodes.iter().position(|n| matches!(n.item, Item::Group { .. }) && n.span == 2).unwrap();
        assert_eq!(d.children(g).len(), 3, "b, c and d are inside the group");
        assert_eq!(cell(&d, "b"), (0, 0));
        assert_eq!(cell(&d, "d"), (0, 1), "the group wraps at its own 2 columns");
        assert_eq!(cell(&d, "e"), (0, 1), "e flows after the 2-wide group in the root");
        assert_eq!(leaf_labels(&d), ["a", "b", "c", "d", "e"], "`block:grp:2` is not a block");
        let svg = render_svg(&d);
        crate::extent::assert_inside(&svg);
        // Group rect encloses its children.
        let p = layout(&d);
        let (gx, gy, gw, gh) = p.rect[g].unwrap();
        for &k in d.children(g) {
            let (x, y, w, h) = p.rect[k].unwrap();
            assert!(x >= gx && y >= gy && x + w <= gx + gw && y + h <= gy + gh);
        }
    }

    #[test]
    fn style_and_classes_apply() {
        let d = parse("block-beta\na b\nstyle a fill:#f9f,stroke:#333\nclassDef hot fill:#f00\nclass b hot").unwrap();
        let svg = render_svg(&d);
        assert!(svg.contains("fill:#f9f;stroke:#333"), "{svg}");
        assert!(svg.contains("fill:#f00"), "{svg}");
        assert!(!svg.contains(">style<") && leaf_labels(&d) == ["a", "b"]);
    }

    #[test]
    fn arrows_between_declared_blocks() {
        let d = parse("block-beta\na b c\na --> b\nb -- \"go\" --> c\nc -->|back| a\na --- c").unwrap();
        assert_eq!(d.arrows.len(), 4);
        assert_eq!(d.arrows[1].label.as_deref(), Some("go"));
        assert_eq!(d.arrows[2].label.as_deref(), Some("back"));
        assert!(d.arrows[3].open && !d.arrows[0].open);
        assert_eq!(leaf_labels(&d), ["a", "b", "c"], "arrow lines reference, not declare");
        let svg = render_svg(&d);
        assert_eq!(svg.matches("marker-end").count(), 3);
        crate::extent::assert_inside(&svg);
    }

    #[test]
    fn a_new_block_can_be_declared_on_an_arrow_line() {
        let d = parse("block-beta\na\na --> b[\"Bee\"]").unwrap();
        assert_eq!(leaf_labels(&d), ["a", "Bee"]);
        assert_eq!(d.arrows.len(), 1);
    }

    #[test]
    fn unsupported_and_unknown_are_line_errors() {
        for (src, line, needle) in [
            ("block-beta\na\na --> zz", 3, "unknown block `zz`"),
            ("block-beta\na\nstyle zz fill:#f00", 3, "unknown block `zz`"),
            ("block-beta\naccTitle: hi\na", 2, "accTitle"),
            ("block-beta\nblockArrowId<[\"x\"]>(right)", 2, "block arrows"),
            ("block-beta\nblock:g\na", 2, "never closed"),
            ("block-beta\na\nend", 3, "without an open"),
            ("block-beta\na a", 2, "declared twice"),
            ("block-beta\ncolumns x", 2, "columns"),
            ("block-beta\na[\"open", 2, "unclosed"),
            ("block-beta\na:0", 2, "span"),
            ("block-beta\na -->", 2, "right"),
            ("block-beta\nlinkStyle 0 stroke:red", 2, "not supported"),
        ] {
            let e = parse(src).unwrap_err();
            assert_eq!(e.line, Some(line), "{src}: {e:?}");
            assert!(e.message.contains(needle), "{src}: {}", e.message);
        }
    }

    #[test]
    fn header_required() {
        assert!(parse("columns 2\n a b").is_err());
    }

    #[test]
    fn deep_nesting_is_an_error() {
        let src = format!("block-beta\n{}a\n{}", "block\n".repeat(100), "end\n".repeat(100));
        assert!(parse(&src).unwrap_err().message.contains("nested too deeply"));
    }

    #[test]
    fn arrow_is_clipped_to_the_gap_not_center_to_center() {
        let svg = render_svg(&parse("block-beta\n columns 2\n a b\n a --> b").unwrap());
        let line = svg.split("<line").nth(1).expect("an arrow line");
        let get = |k: &str| -> f64 {
            line.split(&format!("{k}=\"")).nth(1).unwrap().split('"').next().unwrap().parse().unwrap()
        };
        let span = (get("x2") - get("x1")).abs();
        assert!(span > 0.0 && span < MIN_CELL_W, "arrow spans only the gap, got {span}px");
    }

    #[test]
    fn wide_labels_and_spans_fit_the_canvas() {
        for src in [
            "block-beta\ncolumns 2\na[\"a very long label that is much wider than a cell\"]:2\nb c",
            "block-beta\ncolumns 1\nblock:g[\"A very long group title here, wider than its content\"]\nx\nend",
            "block-beta\ncolumns 2\na:5 b",
            "block-beta\na[\"line one<br/>line two<br/>line three\"] b",
        ] {
            crate::extent::assert_inside(&render_svg(&parse(src).unwrap()));
        }
    }
}
