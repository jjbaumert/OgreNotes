#![forbid(unsafe_code)]

//! Pure-Rust Mermaid → SVG renderer. `render()` never panics; every
//! failure or not-yet-supported diagram kind returns a structured error
//! and no SVG, so callers can fall back to raw source. Supports pie
//! charts and flowcharts (graph/flowchart); other diagram kinds are
//! detected but not yet rendered. See
//! docs/superpowers/specs/2026-07-08-mermaid-support-design.md.

mod pie;
mod layout;
pub(crate) mod measure;
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
    // Self-contained gate: `crates/collab`'s write-gate validator already
    // rejects over-cap sources before they're ever stored, but `render()`
    // is a public crate entry point other callers can reach directly
    // (tests, future callers, anything that bypasses the write gate), so
    // it must not rely on an upstream caller to have checked this. Kept
    // AFTER `detect_kind` (which is only ever O(first line), cheap even
    // on a huge string) so the returned `kind` is still meaningful.
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
    fn each_unsupported_kind_error_names_its_label() {
        // "flowchart LR" left this list when slice 2 made flowcharts
        // render (see flowchart_renders_svg_via_public_render); the
        // remaining kinds are slice-4 territory.
        let cases = [
            ("stateDiagram-v2", "state"),
            ("classDiagram", "class"),
            ("erDiagram", "entity-relationship"),
        ];
        for (src, label) in cases {
            let out = render(src);
            assert!(out.svg.is_none());
            let err = out.error.expect("unsupported kind must carry an error");
            assert!(
                err.message.contains(label),
                "error for {src:?} should mention {label:?}, got: {}",
                err.message
            );
        }
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
                svg.matches("<text x=\"320\"").count(),
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
