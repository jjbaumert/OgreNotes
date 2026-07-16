#![forbid(unsafe_code)]

//! Pure-Rust Mermaid → SVG renderer. `render()` never panics; every
//! failure returns a structured error and no SVG, so callers can fall
//! back to raw source. Supports pie charts, flowcharts (graph/flowchart),
//! sequence diagrams, state diagrams (stateDiagram/stateDiagram-v2),
//! class diagrams, entity-relationship (ER) diagrams, gantt charts,
//! git graphs (gitGraph), mindmaps, timelines, user journeys, quadrant
//! charts, xy charts, kanban boards,
//! packet diagrams, requirement diagrams,
//! and block diagrams. Any other/unrecognized kind is
//! `DiagramKind::Unknown` and always errors. See
//! docs/superpowers/specs/2026-07-08-mermaid-support-design.md (index)
//! and docs/superpowers/specs/2026-07-10-mermaid-slice4-state-class-er-design.md
//! (state/class/ER).

mod pie;
mod gantt;
mod gitgraph;
mod mindmap;
mod timeline;
mod journey;
mod quadrant;
mod xychart;
mod kanban;
mod packet;
mod requirement;
mod block;
mod radar;
mod treemap;
mod sankey;
mod c4;
mod architecture;
mod layout;
pub(crate) mod measure;
pub(crate) mod style;
pub(crate) mod flowchart;
pub(crate) mod sequence;
pub(crate) mod boxgraph;
pub(crate) mod state;
pub(crate) mod class;
pub(crate) mod er;

/// Max diagram source length (chars). Shared cap: the single source of
/// truth for both the `crates/collab` write-gate validator
/// (`blocks::mermaid::MAX_SOURCE_LEN` re-exports this) and the frontend
/// modal's client-side guard, so the two can never drift apart.
pub const MAX_SOURCE_LEN: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramKind {
    Pie,
    Flowchart,
    Sequence,
    State,
    Class,
    Er,
    Gantt,
    GitGraph,
    Mindmap,
    Timeline,
    Journey,
    Quadrant,
    XyChart,
    Kanban,
    Packet,
    Requirement,
    Block,
    Radar,
    Treemap,
    Sankey,
    C4,
    Architecture,
    Unknown,
}

impl DiagramKind {
    /// Human-facing name for a diagram kind. Retained as public API; formerly used by the now-removed 'not yet supported' error path.
    pub fn label(self) -> &'static str {
        match self {
            DiagramKind::Pie => "pie",
            DiagramKind::Flowchart => "flowchart",
            DiagramKind::Sequence => "sequence",
            DiagramKind::State => "state",
            DiagramKind::Class => "class",
            DiagramKind::Er => "entity-relationship",
            DiagramKind::Gantt => "gantt",
            DiagramKind::GitGraph => "git-graph",
            DiagramKind::Mindmap => "mindmap",
            DiagramKind::Timeline => "timeline",
            DiagramKind::Journey => "user-journey",
            DiagramKind::Quadrant => "quadrant-chart",
            DiagramKind::XyChart => "xy-chart",
            DiagramKind::Kanban => "kanban",
            DiagramKind::Packet => "packet",
            DiagramKind::Requirement => "requirement",
            DiagramKind::Block => "block",
            DiagramKind::Radar => "radar",
            DiagramKind::Treemap => "treemap",
            DiagramKind::Sankey => "sankey",
            DiagramKind::C4 => "c4",
            DiagramKind::Architecture => "architecture",
            DiagramKind::Unknown => "unknown",
        }
    }
}

/// XML-escape a user-supplied string before interpolating into SVG.
/// Order matters: `&` first so earlier escapes aren't double-escaped.
pub(crate) fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// SVG path `d` drawing a smooth curve through `points` — one cubic
/// Bézier per segment whose control points are pulled along the layout's
/// flow axis (`vertical` = true for TB/BT, false for LR/RL). At every
/// interior waypoint both the incoming and outgoing tangents are
/// axis-aligned, so the joins are smooth (C1), and edges leave/enter
/// their nodes along the flow direction — matching Mermaid's curved look
/// instead of straight diagonals. A 2-point edge becomes a gentle
/// S-curve; a single point (or fewer) degenerates to a bare `M`.
pub(crate) fn curved_path(points: &[(f64, f64)], vertical: bool) -> String {
    let Some(first) = points.first() else {
        return String::new();
    };
    let mut d = format!("M {:.1} {:.1}", first.0, first.1);
    for w in points.windows(2) {
        let (p0, p1) = (w[0], w[1]);
        let (c1, c2) = if vertical {
            let dy = (p1.1 - p0.1) * 0.5;
            ((p0.0, p0.1 + dy), (p1.0, p1.1 - dy))
        } else {
            let dx = (p1.0 - p0.0) * 0.5;
            ((p0.0 + dx, p0.1), (p1.0 - dx, p1.1))
        };
        d.push_str(&format!(
            " C {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
            c1.0, c1.1, c2.0, c2.1, p1.0, p1.1
        ));
    }
    d
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// 1-based source line the error points at, when known.
    pub line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderOutput {
    pub kind: DiagramKind,
    pub svg: Option<String>,
    pub error: Option<ParseError>,
}

/// First meaningful (non-blank, non-`%%`-comment) line's leading keyword
/// selects the diagram kind.
pub fn detect_kind(source: &str) -> DiagramKind {
    let header = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%"));
    let Some(header) = header else {
        return DiagramKind::Unknown;
    };
    let keyword = header.split_whitespace().next().unwrap_or("");
    // Strip ONE trailing `;` before matching: `sequenceDiagram;` (and,
    // ahead of slice 4, `classDiagram;` etc.) is valid mermaid and the
    // per-kind parsers already tolerate the trailing `;` on the header
    // line — but only if `detect_kind` routes them there in the first
    // place. `graph`/`flowchart` are unaffected: their keyword is the
    // first *token*, and the `;` (if any) lands on a later token like
    // `graph TD;`.
    let keyword = keyword.strip_suffix(';').unwrap_or(keyword);
    // `gitGraph:` glues a colon to the keyword (git-graph's header form).
    let keyword = keyword.strip_suffix(':').unwrap_or(keyword);
    match keyword {
        "pie" => DiagramKind::Pie,
        "graph" | "flowchart" => DiagramKind::Flowchart,
        "sequenceDiagram" => DiagramKind::Sequence,
        "stateDiagram" | "stateDiagram-v2" => DiagramKind::State,
        "classDiagram" | "classDiagram-v2" => DiagramKind::Class,
        "erDiagram" => DiagramKind::Er,
        "gantt" => DiagramKind::Gantt,
        "gitGraph" => DiagramKind::GitGraph,
        "mindmap" => DiagramKind::Mindmap,
        "timeline" => DiagramKind::Timeline,
        "journey" => DiagramKind::Journey,
        "quadrantChart" => DiagramKind::Quadrant,
        "xychart-beta" => DiagramKind::XyChart,
        "kanban" => DiagramKind::Kanban,
        "packet" | "packet-beta" => DiagramKind::Packet,
        "requirementDiagram" => DiagramKind::Requirement,
        "block" | "block-beta" => DiagramKind::Block,
        "radar-beta" => DiagramKind::Radar,
        "treemap" | "treemap-beta" => DiagramKind::Treemap,
        "sankey" | "sankey-beta" => DiagramKind::Sankey,
        "C4Context" | "C4Container" | "C4Component" | "C4Dynamic" | "C4Deployment" => {
            DiagramKind::C4
        }
        "architecture" | "architecture-beta" => DiagramKind::Architecture,
        _ => DiagramKind::Unknown,
    }
}

/// Mermaid sources may open with a YAML front-matter block
/// (`---` … `---`) carrying config/theme data we don't consume. When
/// the very first line is `---`, blank the block's lines (delimiters
/// inclusive) rather than slicing them away, so every downstream error
/// still points at the original 1-based line numbers. Contents are
/// ignored in v1.
///
/// Only ever called from `render()` AFTER the `MAX_SOURCE_LEN` gate, so
/// its line scan and the `Vec`/`String::join` allocation it performs are
/// bounded by the cap — this function must not be called on untrusted,
/// unbounded input directly.
fn strip_front_matter(source: &str) -> Result<Option<String>, ParseError> {
    match source.lines().next() {
        Some(first) if first.trim() == "---" => {}
        _ => return Ok(None),
    }
    let Some(close_rel) = source.lines().skip(1).position(|l| l.trim() == "---") else {
        return Err(ParseError {
            message: "unterminated front matter: missing closing `---`".into(),
            line: Some(1),
        });
    };
    let close_idx = close_rel + 1; // position() counted from line 2
    let blanked: Vec<&str> = source
        .lines()
        .enumerate()
        .map(|(i, l)| if i <= close_idx { "" } else { l })
        .collect();
    Ok(Some(blanked.join("\n")))
}

/// Render mermaid `source` to an SVG string. Never panics.
pub fn render(source: &str) -> RenderOutput {
    let kind = detect_kind(source);
    // Self-contained gate: `crates/collab`'s write-gate validator already
    // rejects over-cap sources before they're ever stored, but `render()`
    // is a public crate entry point other callers can reach directly
    // (tests, future callers, anything that bypasses the write gate), so
    // it must not rely on an upstream caller to have checked this. Kept
    // AFTER `detect_kind` (which is only ever O(first line), cheap even
    // on a huge string) so the returned `kind` is still meaningful, but
    // BEFORE `strip_front_matter` (which is an O(n) scan plus a
    // source-sized allocation) so an oversized source is rejected before
    // that work is ever done.
    if source.chars().count() > MAX_SOURCE_LEN {
        return RenderOutput {
            kind,
            svg: None,
            error: Some(ParseError {
                message: format!("diagram source too large (max {MAX_SOURCE_LEN} chars)"),
                line: None,
            }),
        };
    }
    let stripped;
    let (source, kind) = match strip_front_matter(source) {
        Ok(None) => (source, kind),
        Ok(Some(s)) => {
            stripped = s;
            let kind = detect_kind(&stripped);
            (stripped.as_str(), kind)
        }
        Err(e) => {
            return RenderOutput { kind: DiagramKind::Unknown, svg: None, error: Some(e) }
        }
    };
    match kind {
        DiagramKind::Pie => match pie::parse(source) {
            Ok(p) => RenderOutput { kind, svg: Some(pie::render_svg(&p)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Flowchart => match flowchart::render_flowchart(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Sequence => match sequence::render_sequence(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::State => match state::render_state(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Class => match class::render_class(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Er => match er::render_er(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Gantt => match gantt::parse(source) {
            Ok(g) => RenderOutput { kind, svg: Some(gantt::render_svg(&g)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::GitGraph => match gitgraph::parse(source) {
            Ok(g) => RenderOutput { kind, svg: Some(gitgraph::render_svg(&g)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Mindmap => match mindmap::parse(source) {
            Ok(m) => RenderOutput { kind, svg: Some(mindmap::render_svg(&m)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Timeline => match timeline::parse(source) {
            Ok(t) => RenderOutput { kind, svg: Some(timeline::render_svg(&t)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Journey => match journey::parse(source) {
            Ok(j) => RenderOutput { kind, svg: Some(journey::render_svg(&j)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Quadrant => match quadrant::parse(source) {
            Ok(q) => RenderOutput { kind, svg: Some(quadrant::render_svg(&q)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::XyChart => match xychart::parse(source) {
            Ok(x) => RenderOutput { kind, svg: Some(xychart::render_svg(&x)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Kanban => match kanban::parse(source) {
            Ok(k) => RenderOutput { kind, svg: Some(kanban::render_svg(&k)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Packet => match packet::parse(source) {
            Ok(pk) => RenderOutput { kind, svg: Some(packet::render_svg(&pk)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Requirement => {
            match requirement::parse(source).and_then(|g| requirement::render_svg(&g)) {
                Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
                Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
            }
        }
        DiagramKind::Radar => match radar::parse(source) {
            Ok(r) => RenderOutput { kind, svg: Some(radar::render_svg(&r)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Treemap => match treemap::parse(source) {
            Ok(tm) => RenderOutput { kind, svg: Some(treemap::render_svg(&tm)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Sankey => match sankey::parse(source) {
            Ok(sk) => RenderOutput { kind, svg: Some(sankey::render_svg(&sk)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::C4 => match c4::parse(source).and_then(|g| c4::render_svg(&g)) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Architecture => match architecture::parse(source) {
            Ok(ar) => RenderOutput { kind, svg: Some(architecture::render_svg(&ar)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Block => match block::parse(source) {
            Ok(b) => RenderOutput { kind, svg: Some(block::render_svg(&b)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Unknown => RenderOutput {
            kind,
            svg: None,
            error: Some(ParseError {
                message: "unrecognized diagram type".to_string(),
                line: None,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curved_path_smooths_edges_and_degenerates() {
        // 2-point vertical edge -> one cubic whose control points sit
        // directly below/above the endpoints (leaves and enters vertically).
        assert_eq!(
            curved_path(&[(10.0, 0.0), (30.0, 100.0)], true),
            "M 10.0 0.0 C 10.0 50.0 30.0 50.0 30.0 100.0"
        );
        // Horizontal (LR/RL) pulls the controls along x instead.
        assert_eq!(
            curved_path(&[(0.0, 10.0), (100.0, 30.0)], false),
            "M 0.0 10.0 C 50.0 10.0 50.0 30.0 100.0 30.0"
        );
        // A single point is a bare move; empty is empty.
        assert_eq!(curved_path(&[(5.0, 5.0)], true), "M 5.0 5.0");
        assert_eq!(curved_path(&[], true), "");
    }

    #[test]
    fn detects_each_known_kind() {
        assert_eq!(detect_kind("pie\n\"A\": 1"), DiagramKind::Pie);
        assert_eq!(detect_kind("pie showData"), DiagramKind::Pie);
        assert_eq!(detect_kind("graph TD\nA-->B"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("flowchart LR"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("sequenceDiagram"), DiagramKind::Sequence);
        assert_eq!(detect_kind("stateDiagram-v2"), DiagramKind::State);
        assert_eq!(detect_kind("classDiagram"), DiagramKind::Class);
        assert_eq!(detect_kind("classDiagram-v2"), DiagramKind::Class);
        assert_eq!(detect_kind("erDiagram"), DiagramKind::Er);
        assert_eq!(detect_kind("gantt"), DiagramKind::Gantt);
        assert_eq!(detect_kind("gitGraph"), DiagramKind::GitGraph);
        assert_eq!(detect_kind("gitGraph:"), DiagramKind::GitGraph);
        assert_eq!(detect_kind("gitGraph LR:"), DiagramKind::GitGraph);
        assert_eq!(detect_kind("mindmap"), DiagramKind::Mindmap);
        assert_eq!(detect_kind("nonsense here"), DiagramKind::Unknown);
    }

    #[test]
    fn detection_skips_blank_and_comment_lines() {
        assert_eq!(detect_kind("\n\n  %% a comment\npie\n\"A\": 1"), DiagramKind::Pie);
    }

    #[test]
    fn unknown_kind_returns_error() {
        let out = render("total gibberish");
        assert_eq!(out.kind, DiagramKind::Unknown);
        assert!(out.svg.is_none());
        assert!(out.error.is_some());
    }

    #[test]
    fn pie_renders_svg_via_public_render() {
        let out = render("pie title T\n\"A\" : 1\n\"B\" : 1");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.error.is_none());
        let svg = out.svg.expect("pie should render");
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn pie_parse_error_flows_through_render() {
        let out = render("pie\n\"A\" : notanumber");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.svg.is_none());
        assert!(out.error.is_some());
    }

    #[test]
    fn flowchart_renders_svg_via_public_render() {
        let out = render("graph TD\nA[Start] --> B{Go?} -->|yes| C(Done)");
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        let svg = out.svg.expect("flowchart should render");
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn flowchart_parse_error_flows_through_render() {
        let out = render("graph TD\nA[unclosed");
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.svg.is_none());
        let e = out.error.expect("error");
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn render_gates_oversized_source_without_parsing() {
        // Self-contained cap: render() must reject an over-`MAX_SOURCE_LEN`
        // source itself, not rely on an upstream write-gate having already
        // checked it. Uses a source that's cheap to detect_kind() on
        // (short first line) but long overall, and asserts the error and
        // the XOR invariant rather than any timing.
        let big = format!("graph TD\n{}", "A --> B\n".repeat(MAX_SOURCE_LEN));
        assert!(big.chars().count() > MAX_SOURCE_LEN);
        let out = render(&big);
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.svg.is_none());
        let err = out.error.expect("oversized source must error");
        assert!(err.message.contains("too large"), "got: {}", err.message);
        assert!(err.line.is_none());
    }

    #[test]
    fn oversized_front_matter_source_is_gated_before_stripping() {
        // The cap must reject before any front-matter scan/allocation;
        // kind comes from the raw source (`---` header -> Unknown).
        let big = format!("---\ntitle: x\n---\npie\n{}", "\"A\" : 1\n".repeat(MAX_SOURCE_LEN));
        assert!(big.chars().count() > MAX_SOURCE_LEN);
        let out = render(&big);
        assert_eq!(out.kind, DiagramKind::Unknown);
        let err = out.error.expect("oversized source must error");
        assert!(err.message.contains("too large"), "got: {}", err.message);
    }

    #[test]
    fn flowchart_with_subgraph_and_classes_renders() {
        let src = "flowchart LR\nclassDef hot fill:#f00\nsubgraph s[Sub]\nA:::hot --> B\nend\nB --> C";
        let out = render(src);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn render_never_panics_on_adversarial_input() {
        let inputs = [
            "",
            " ",
            "\n\n\n",
            "%%",
            "pie",
            "pie\n:",
            "pie\n\"\":",
            "pie\n\"x\": notanumber",
            &"pie\n".repeat(100_000),
            &"\"a\": 1\n".repeat(100_000),
            "🥧 pie 🥧",
            "graph TD",
            "graph XX",
            "flowchart LR\nA --> ",
            "graph TD\nA[",
            "graph TD\nsubgraph s\nsubgraph t\nA",
            "graph TD\nend\nend",
            &format!("graph TD\n{}", "A --> B\n".repeat(2000)),
            &format!("graph LR\n{}", (0..300).map(|i| format!("n{i} --> n{} \n", (i * 7) % 300)).collect::<String>()),
            "graph TD\nA --> A --> A",
            "graph TD\nA[🥧<br/>🥧] -->|🥧| B",
            &format!("graph TD\n{}", "A --> B\n".repeat(3000)), // over MAX_SOURCE_LEN
            "sequenceDiagram",
            "sequenceDiagram\nA->>",
            "sequenceDiagram\n->>B: x",
            "sequenceDiagram\nend\nend",
            &format!("sequenceDiagram\n{}", "loop l\n".repeat(50)),
            &format!("sequenceDiagram\n{}", "A->>B: x\n".repeat(2000)),
            &format!("sequenceDiagram\n{}", (0..60).map(|i| format!("participant p{i}\n")).collect::<String>()),
            "sequenceDiagram\nautonumber\nA->>A: 🎭<br/>🎭\nNote left of A: 🎉",
            "sequenceDiagram\nactivate A",
            "sequenceDiagram\nA-->>-B: under",
            "stateDiagram-v2",
            "stateDiagram-v2\n[*] --> [*]",
            "classDiagram\nclass A {",
            "classDiagram\nA <|-- A",
            "erDiagram\nA ||--o{ A : self",
            &format!("stateDiagram-v2\n{}", "state s {\n".repeat(30)),
            &format!("classDiagram\n{}", "A --> B\n".repeat(2000)),
            "erDiagram\nÉ ||--|| 中 : 🎉",
        ];
        for inp in inputs {
            let out = render(inp); // must return, not panic
            assert!(
                out.svg.is_some() != out.error.is_some(),
                "exactly one of svg/error must be set for input {:?}, got {:?}",
                inp,
                out
            );
        }
    }

    #[test]
    fn header_trailing_semicolon_still_detects_kind() {
        // Regression: the first-token match used to compare the WHOLE
        // token, so "sequenceDiagram;" fell through to Unknown and
        // sequence::parse (which tolerates the trailing `;`, see its own
        // `header_required` test) was never reached via render().
        assert_eq!(detect_kind("sequenceDiagram;"), DiagramKind::Sequence);
    }

    #[test]
    fn header_trailing_semicolon_renders_via_public_render() {
        let out = render("sequenceDiagram;\nA->>B: x");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.is_some());
    }

    #[test]
    fn header_with_same_line_statement_renders_via_public_render() {
        // The sequence parser splits statements on `;` (seq-polish);
        // detect_kind's first-token match must still route the header.
        let out = render("sequenceDiagram; A->>B: hi");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.is_some());
    }

    #[test]
    fn detection_requires_exact_keyword_match() {
        // Keywords must match whole-token: prefixes, suffixes, and case
        // variants are not diagram headers.
        assert_eq!(detect_kind("piechart\n\"A\": 1"), DiagramKind::Unknown);
        assert_eq!(detect_kind("PIE\n\"A\": 1"), DiagramKind::Unknown);
        assert_eq!(detect_kind("graphs TD"), DiagramKind::Unknown);
        assert_eq!(detect_kind("stateDiagram-v3"), DiagramKind::Unknown);
    }

    #[test]
    fn comment_or_blank_only_source_is_unknown() {
        assert_eq!(detect_kind(""), DiagramKind::Unknown);
        assert_eq!(detect_kind("%% just a comment\n\n  %% another"), DiagramKind::Unknown);
    }

    #[test]
    fn sequence_renders_svg_via_public_render() {
        let out = render("sequenceDiagram\nAlice->>+Bob: Hello\nBob-->>-Alice: Hi");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.expect("sequence should render").starts_with("<svg"));
    }

    #[test]
    fn sequence_parse_error_flows_through_render() {
        let out = render("sequenceDiagram\nloop forever\nA->>B: x");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.svg.is_none());
        assert_eq!(out.error.expect("error").line, Some(2));
    }

    #[test]
    fn sequence_with_fragments_and_notes_renders() {
        let src = "sequenceDiagram\nautonumber\nactor U as User\nU->>+S: request\nalt cached\nS-->>U: fast\nelse miss\nS->>D: query\nNote over S,D: slow path\nD-->>S: rows\nend\nS-->>-U: response";
        let out = render(src);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    // Sanctioned retirement (Task 8, see
    // docs/superpowers/specs/2026-07-10-mermaid-slice4-state-class-er-design.md):
    // `each_unsupported_kind_error_names_its_label` and
    // `unsupported_kind_returns_error_with_kind_preserved` both existed to
    // exercise the generic "‹kind› diagrams are not yet supported" catch-all
    // arm in `render()`. Slice 4 wires state/class/er into `render()`,
    // leaving `Unknown` as the only kind that still errors — the catch-all
    // arm has no variants left to hit, so both tests are deleted rather
    // than retargeted (there is no more "still unsupported" family left).
    // `unknown_kind_still_errors` (below) covers the one remaining error
    // path.

    #[test]
    fn state_renders_svg_via_public_render() {
        let out = render("stateDiagram-v2\n[*] --> A\nA --> [*]");
        assert_eq!(out.kind, DiagramKind::State);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        assert!(out.svg.unwrap().starts_with("<svg"));
    }

    #[test]
    fn class_renders_svg_via_public_render() {
        let out = render("classDiagram\nAnimal <|-- Dog");
        assert_eq!(out.kind, DiagramKind::Class);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn er_renders_svg_via_public_render() {
        let out = render("erDiagram\nA ||--o{ B : has");
        assert_eq!(out.kind, DiagramKind::Er);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn unknown_kind_still_errors() {
        let out = render("total gibberish");
        assert_eq!(out.kind, DiagramKind::Unknown);
        assert!(out.svg.is_none() && out.error.is_some());
    }

    #[test]
    fn family_parse_errors_flow_through_render() {
        for (src, line) in [
            ("stateDiagram-v2\n}", 2),
            ("classDiagram\nnamespace N {", 2),
            ("erDiagram\nA ||--o{ B", 2),
        ] {
            let out = render(src);
            assert!(out.svg.is_none(), "for {src:?}");
            assert_eq!(out.error.expect("err").line, Some(line), "for {src:?}");
        }
    }

    #[test]
    fn front_matter_is_skipped_for_kind_detection_and_render() {
        let src = "---\ntitle: My chart\nconfig:\n  theme: forest\n---\ngraph TD\nA --> B";
        let out = render(src);
        assert_eq!(out.kind, DiagramKind::Flowchart);
        assert!(out.error.is_none(), "err: {:?}", out.error);
        // Works for a non-flowchart kind too (stripping precedes detection).
        let out = render("---\ntitle: t\n---\npie\n\"A\" : 1");
        assert_eq!(out.kind, DiagramKind::Pie);
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }

    #[test]
    fn front_matter_preserves_original_error_lines() {
        // Front matter occupies lines 1-3; the broken statement is on
        // line 5 of the ORIGINAL source and must be reported as 5.
        let out = render("---\ntitle: x\n---\ngraph TD\nA[unclosed");
        assert_eq!(out.error.expect("err").line, Some(5));
    }

    #[test]
    fn unterminated_front_matter_errors_at_line_1() {
        let out = render("---\ntitle: x\ngraph TD\nA");
        assert_eq!(out.kind, DiagramKind::Unknown);
        let e = out.error.expect("err");
        assert_eq!(e.line, Some(1));
        assert!(e.message.contains("front matter"), "got: {}", e.message);
    }

    #[test]
    fn dashes_not_on_line_one_are_not_front_matter() {
        // `---` as an EDGE on a later line must be untouched.
        let out = render("graph TD\nA --- B");
        assert!(out.error.is_none(), "err: {:?}", out.error);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            .. ProptestConfig::default()
        })]

        /// `render` must never panic on arbitrary unicode input and must
        /// always produce exactly one of svg/error.
        #[test]
        fn render_never_panics_on_arbitrary_input(src in "\\PC*") {
            let out = render(&src);
            prop_assert!(
                out.svg.is_some() != out.error.is_some(),
                "exactly one of svg/error must be set, got {out:?}"
            );
        }

        /// Same invariant with a `pie` header forced on, so the pie parser
        /// and renderer (not just kind detection) see arbitrary bodies.
        /// Any produced SVG must be a complete element.
        #[test]
        fn pie_bodies_never_panic_and_svg_is_complete(body in "\\PC*") {
            let out = render(&format!("pie\n{body}"));
            prop_assert!(out.svg.is_some() != out.error.is_some());
            if let Some(svg) = &out.svg {
                prop_assert!(svg.starts_with("<svg"));
                prop_assert!(svg.ends_with("</svg>"));
            }
        }

        /// Well-formed pies always render: one legend entry per slice, and
        /// no raw `<` from a label survives into the SVG.
        #[test]
        fn well_formed_pies_always_render(
            slices in proptest::collection::vec(
                ("[A-Za-z0-9<&>][A-Za-z0-9 <&>]{0,11}", 0.0f64..1e6),
                1..10,
            )
        ) {
            let src = std::iter::once("pie".to_string())
                .chain(slices.iter().map(|(l, v)| format!("\"{l}\" : {v}")))
                .collect::<Vec<_>>()
                .join("\n");
            let out = render(&src);
            prop_assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
            let svg = out.svg.expect("well-formed pie must render");
            prop_assert_eq!(
                svg.matches("<text x=\"463\"").count(),
                slices.len(),
                "one legend row per slice"
            );
            // Every `<` in the output must open a tag the renderer itself
            // emits; any other `<` is an unescaped label character.
            let renderer_tags =
                ["<svg", "</svg>", "<text", "</text>", "<rect", "<path", "<circle"];
            let tag_lt: usize =
                renderer_tags.iter().map(|t| svg.matches(t).count()).sum();
            prop_assert_eq!(
                svg.matches('<').count(),
                tag_lt,
                "unescaped `<` leaked into the SVG: {}",
                svg
            );
        }
    }
}
