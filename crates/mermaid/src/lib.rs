#![forbid(unsafe_code)]

//! Pure-Rust Mermaid → SVG renderer. `render()` never panics; every
//! failure or not-yet-supported diagram kind returns a structured error
//! and no SVG, so callers can fall back to raw source. See
//! docs/superpowers/specs/2026-07-08-mermaid-support-design.md.

mod pie;

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
        let cases = [
            ("flowchart LR", "flowchart"),
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
