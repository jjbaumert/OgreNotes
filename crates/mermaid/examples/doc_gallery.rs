//! Renders the example snippets from the official mermaid flowchart
//! docs (https://mermaid.ai/open-source/syntax/flowchart.html) through
//! our renderer, writing one .svg (success) or .err.txt (error) per
//! example into target/doc-gallery/ for side-by-side visual comparison
//! with the doc's reference images.
//!
//! Run: cargo run -p ogrenotes-mermaid --example doc_gallery
//!
//! Expectation notes reflect the post-polish-slice behavior (issue #32;
//! docs/superpowers/specs/2026-07-11-mermaid-polish-design.md).

use std::fs;
use std::path::Path;

fn main() {
    // (name, source, expectation-note)
    let cases: &[(&str, &str, &str)] = &[
        // ── Basic ────────────────────────────────────────────────
        ("basic_bare_node", "flowchart TD\n    A", "match"),
        ("basic_text_node", "flowchart TD\n    A[This is the text in the box]", "match"),
        ("basic_unicode_quoted", "flowchart TD\n    A[\"This is the text in the box\"]", "match"),
        ("basic_markdown_backticks", "flowchart TD\n    A[\"`This is the text in the box`\"]", "diverge: backticks render literally (no markdown)"),
        ("front_matter", "---\ntitle: Node\n---\nflowchart LR\n    id", "match: front matter skipped (title not rendered in v1)"),
        ("direction_td", "flowchart TD\n    A --> B", "match"),
        ("direction_lr", "flowchart LR\n    A --> B", "match"),
        // ── Legacy shapes ────────────────────────────────────────
        ("shape_round", "flowchart TD\n    A(This is the text in the box)", "match"),
        ("shape_stadium", "flowchart TD\n    A([This is the text in the box])", "match"),
        ("shape_subroutine", "flowchart TD\n    A[[This is the text in the box]]", "match (added in polish slice)"),
        ("shape_cylinder", "flowchart TD\n    A[(Database)]", "match"),
        ("shape_circle", "flowchart TD\n    A((This is the text in the box))", "match"),
        ("shape_asymmetric", "flowchart TD\n    A>This is the text in the box]", "match"),
        ("shape_rhombus", "flowchart TD\n    A{This is the text in the box}", "match"),
        ("shape_hexagon", "flowchart TD\n    A{{This is the text in the box}}", "match"),
        ("shape_parallelogram", "flowchart TD\n    A[/This is the text in the box/]", "match"),
        ("shape_parallelogram_alt", "flowchart TD\n    A[\\This is the text in the box\\]", "match"),
        ("shape_trapezoid", "flowchart TD\n    A[/This is the text in the box\\]", "match"),
        ("shape_trapezoid_alt", "flowchart TD\n    A[\\This is the text in the box/]", "match"),
        ("shape_double_circle", "flowchart TD\n    A(((This is the text in the box)))", "match"),
        // ── v11.3+ @-syntax (expected to error loudly) ───────────
        ("at_syntax_multi", "flowchart TD\n    A@{ shape: rect, label: \"Rectangle\" }\n    B@{ shape: circle, label: \"Circle\" }\n    A --> B", "error (out of scope: @-syntax)"),
        ("at_syntax_icon", "flowchart TD\n    A@{ shape: icon, icon: \"fa:fa-heart\", form: \"circle\", label: \"Heart\" }", "error (out of scope)"),
        // ── Links ────────────────────────────────────────────────
        ("link_arrow", "flowchart TD\n    A-->B", "match"),
        ("link_open", "flowchart TD\n    A---B", "match"),
        ("link_pipe_text", "flowchart TD\n    A-->|text|B", "match"),
        ("link_chain_text_node", "flowchart TD\n    A-->text-->B", "match (creates a node named 'text' — same as mermaid)"),
        ("link_inline_text_spaced", "flowchart TD\n    A-- text -->B", "match"),
        ("link_dotted", "flowchart TD\n    A-.->B", "match"),
        ("link_dotted_open", "flowchart TD\n    A-.-B", "match: dotted open, no arrowhead (fixed in polish slice)"),
        ("link_dotted_text_nospace", "flowchart TD\n    A-.text.-B", "match (no-space spelling added in polish slice)"),
        ("link_dotted_text_spaced", "flowchart TD\n    A-. text .-> B", "match"),
        ("link_thick", "flowchart TD\n    A==>B", "match"),
        ("link_thick_text_nospace", "flowchart TD\n    A==text==>B", "match (no-space spelling added in polish slice)"),
        ("link_thick_text_spaced", "flowchart TD\n    A== text ==>B", "match"),
        ("link_invisible", "flowchart TD\n    A ~~~ B", "match: renders both nodes, no visible edge (layout-only)"),
        // ── Chaining ─────────────────────────────────────────────
        ("chain_simple", "flowchart TD\n    A-->B-->C", "match"),
        ("chain_fanout_mid", "flowchart TD\n    A-->B & C-->D", "match"),
        ("chain_fanout_both", "flowchart TD\n    A & B-->C & D", "match"),
        ("chain_multiline", "flowchart TD\n    A-->B-->C\n    A-->D-->C\n    B-->E", "match"),
        // ── Edge ids / animation (v11.10+) ───────────────────────
        ("edge_id", "flowchart TD\n    e1@-->A & B", "error (out of scope: edge ids)"),
        // ── New arrow types (fixed silent divergences) ───────────
        ("circle_edge", "flowchart LR\n    A --o B", "match: circle-ended edge (was a loud error pre-polish)"),
        ("cross_edge", "flowchart LR\n    A --x B", "match: cross-ended edge"),
        ("circle_edge_nospace", "flowchart TD\n    A---oB", "match: circle edge to B — mermaid's documented terminator binding (was a phantom node 'oB')"),
        ("cross_edge_nospace", "flowchart TD\n    A---xB", "match: cross edge to B (was a phantom node 'xB')"),
        ("arrow_then_o_node", "flowchart TD\n    A-->oB", "match: arrow to a node named 'oB' — the '>' already terminated the link, in mermaid too"),
        // ── Multi-directional / lengths ──────────────────────────
        ("multidir", "flowchart LR\n    A o--o B\n    B <--> C\n    C x--x D", "match: per-end heads via marker-start/marker-end"),
        ("min_length", "flowchart TD\n    A[Start] --> B{Rhombus}\n    B --> C[rect_1]\n    B --> D[rect_2]\n    B --> E[rect_3]\n    C --> F[A]\n    D --> E\n    E --> F[B]\n    F --> G[End]\n    A -----> E", "match: parses; extra length's rank-span hint not honored (quiet cosmetic divergence)"),
        ("length_with_text", "flowchart TD\n    A --text---- E", "match: long closer run with inline label"),
        // ── Special characters ───────────────────────────────────
        ("special_html", "flowchart TD\n    A[\"This is a <strong>test</strong>\"]", "diverge-by-design: we escape (literal text); mermaid renders the HTML"),
        ("entity_codes", "flowchart TD\n    A[\"This is a #35; test\"]", "diverge: entity code renders literally, not decoded to #"),
        // ── Subgraphs ────────────────────────────────────────────
        ("subgraph_titled", "flowchart TD\n    subgraph sgID[A Subgraph]\n        A-->B\n    end", "match"),
        ("subgraph_edge_to_id", "flowchart TD\n    subgraph sgID[A Subgraph]\n        A-->B\n    end\n    sgID-->C\n    C-->D", "LOUD ERROR: edge-to-subgraph routing deferred (was a silent phantom node 'sgID')"),
        ("subgraph_direction", "flowchart TD\n    subgraph sgID[A Subgraph]\n        direction LR\n        A-->B\n    end", "error (direction-in-subgraph out of scope)"),
        // ── Interaction / styling ────────────────────────────────
        ("click_callback", "flowchart TD\n    A-->B\n    click A callback \"Tooltip text\"", "error (out of scope: click)"),
        ("comments", "flowchart TD\n    A[Auslan]\n    %%this is a comment\n    A-->B[\"Christmas\"]", "match"),
        ("link_style", "flowchart LR\n    A-->B-->C-->D\n    linkStyle 3 stroke:#ff3,stroke-width:4px,color:red;", "error (out of scope: linkStyle)"),
        ("node_style", "flowchart TD\n    A-->B\n    style A fill:#f9f,stroke:#333,stroke-width:4px;", "error (out of scope: style)"),
        ("classes_inline", "flowchart TD\n    A:::someclass --> B\n    classDef someclass fill:#f9f,stroke:#333,stroke-width:4px;", "match (allowlisted props apply)"),
        ("classes_two", "flowchart TD\n    A:::first --> B:::second\n    classDef first fill:#f9f,stroke:#333,stroke-width:4px;\n    classDef second fill:#bbf,stroke:#f66,stroke-width:2px,color:#fff;", "match"),
        ("class_default", "flowchart TD\n    A --> B\n    classDef default fill:#f9f,stroke:#333,stroke-width:4px;", "match: default auto-applied to unclassed nodes (added in polish slice)"),
        ("fontawesome", "flowchart TD\n    B[fa:fa-twitter]", "diverge-by-design: renders literal text, no icon"),
        ("spaced_no_semicolons", "flowchart LR\n    A --> B --> C\n    A --> D --> C\n    B --> E", "match"),
    ];

    let out_dir = Path::new("target/doc-gallery");
    fs::create_dir_all(out_dir).expect("create out dir");

    let (mut ok, mut err) = (0usize, 0usize);
    for (name, source, note) in cases {
        let result = ogrenotes_mermaid::render(source);
        match (result.svg, result.error) {
            (Some(svg), None) => {
                fs::write(out_dir.join(format!("{name}.svg")), svg).expect("write svg");
                ok += 1;
                println!("RENDERED  {name:<28} — {note}");
            }
            (None, Some(e)) => {
                let msg = format!(
                    "line {:?}: {}\n\nsource:\n{}\n\nexpectation: {}\n",
                    e.line, e.message, source, note
                );
                fs::write(out_dir.join(format!("{name}.err.txt")), msg).expect("write err");
                err += 1;
                println!("ERRORED   {name:<28} — {note}");
            }
            other => {
                // XOR invariant means this is unreachable; keep loud anyway.
                println!("INVARIANT VIOLATION {name}: {other:?}");
            }
        }
    }
    println!("\n{ok} rendered, {err} errored → target/doc-gallery/");
    println!("Compare SVGs against the doc images; .err.txt files list the loud-error cases.");
}
