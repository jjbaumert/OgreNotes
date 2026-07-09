#![forbid(unsafe_code)]

//! Pure-Rust Mermaid → SVG renderer. `render()` never panics; every
//! failure or not-yet-supported diagram kind returns a structured error
//! and no SVG, so callers can fall back to raw source. Supports pie
//! charts and flowcharts (graph/flowchart); other diagram kinds are
//! detected but not yet rendered. See
//! docs/superpowers/specs/2026-07-08-mermaid-support-design.md.

mod pie;
mod layout;
pub(crate) mod flowchart;

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
    Unknown,
}

impl DiagramKind {
    /// Human-facing name used in "‹label› not yet supported" errors.
    pub fn label(self) -> &'static str {
        match self {
            DiagramKind::Pie => "pie",
            DiagramKind::Flowchart => "flowchart",
            DiagramKind::Sequence => "sequence",
            DiagramKind::State => "state",
            DiagramKind::Class => "class",
            DiagramKind::Er => "entity-relationship",
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
    match keyword {
        "pie" => DiagramKind::Pie,
        "graph" | "flowchart" => DiagramKind::Flowchart,
        "sequenceDiagram" => DiagramKind::Sequence,
        "stateDiagram" | "stateDiagram-v2" => DiagramKind::State,
        "classDiagram" => DiagramKind::Class,
        "erDiagram" => DiagramKind::Er,
        _ => DiagramKind::Unknown,
    }
}

/// Render mermaid `source` to an SVG string. Never panics.
pub fn render(source: &str) -> RenderOutput {
    let kind = detect_kind(source);
    match kind {
        DiagramKind::Pie => match pie::parse(source) {
            Ok(p) => RenderOutput { kind, svg: Some(pie::render_svg(&p)), error: None },
            Err(e) => RenderOutput { kind, svg: None, error: Some(e) },
        },
        DiagramKind::Flowchart => match flowchart::render_flowchart(source) {
            Ok(svg) => RenderOutput { kind, svg: Some(svg), error: None },
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
        other => RenderOutput {
            kind,
            svg: None,
            error: Some(ParseError {
                message: format!("{} diagrams are not yet supported", other.label()),
                line: None,
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_known_kind() {
        assert_eq!(detect_kind("pie\n\"A\": 1"), DiagramKind::Pie);
        assert_eq!(detect_kind("pie showData"), DiagramKind::Pie);
        assert_eq!(detect_kind("graph TD\nA-->B"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("flowchart LR"), DiagramKind::Flowchart);
        assert_eq!(detect_kind("sequenceDiagram"), DiagramKind::Sequence);
        assert_eq!(detect_kind("stateDiagram-v2"), DiagramKind::State);
        assert_eq!(detect_kind("classDiagram"), DiagramKind::Class);
        assert_eq!(detect_kind("erDiagram"), DiagramKind::Er);
        assert_eq!(detect_kind("nonsense here"), DiagramKind::Unknown);
    }

    #[test]
    fn detection_skips_blank_and_comment_lines() {
        assert_eq!(detect_kind("\n\n  %% a comment\npie\n\"A\": 1"), DiagramKind::Pie);
    }

    #[test]
    fn unsupported_kind_returns_error_with_kind_preserved() {
        let out = render("sequenceDiagram\nAlice->>Bob: hi");
        assert_eq!(out.kind, DiagramKind::Sequence);
        assert!(out.svg.is_none());
        let err = out.error.expect("unsupported kind must carry an error");
        assert!(err.message.to_lowercase().contains("not yet supported"), "got: {}", err.message);
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
}
