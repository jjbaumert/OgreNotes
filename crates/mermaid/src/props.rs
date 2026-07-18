// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Crate-wide property tests (proptest, dev-only).
//!
//! The five big parser families (flowchart, sequence, state, class, er)
//! carry their own `props.rs` with family-specific structural
//! invariants. This module extends the *panic-safety* net to the other
//! seventeen diagram types, which previously had only example-based
//! unit tests. All of them share the same trust boundary: `render()`
//! takes arbitrary user-authored text out of a document, so for every
//! input the pipeline must uphold
//!
//!   1. no panic,
//!   2. exactly one of `svg` / `error` is set (the XOR contract), and
//!   3. any emitted SVG contains no non-finite numbers — a division by
//!      zero in chart scaling doesn't panic in f64, it leaks "NaN"/"inf"
//!      into coordinates and renders a broken image.
//!
//! Each type gets a statement-soup strategy over its own grammar
//! vocabulary (drawn from that module's unit tests), deliberately
//! seeded with numeric edge cases — zeros, negatives, huge magnitudes,
//! equal ranges — plus raw line noise.
//!
//! NOTE: the noise alphabet excludes `N` and `f` so that the substrings
//! "NaN" and "inf" cannot be produced by *input* echoed into a label;
//! any occurrence in the output is therefore a genuine formatting of a
//! non-finite float. Keep fixed vocab strings free of them too.

use proptest::prelude::*;

/// Raw line noise: grammar metacharacters + lowercase text, but no `N`
/// and no `f` (see module docs). Leading spaces included — indentation
/// is significant for mindmap/kanban/timeline.
const NOISE: &str = r#"[ a-eg-z0-9_\[\]{}():,<>|"'&;.=~%*+-]{0,24}"#;

fn source_strategy(
    header: &'static str,
    stmts: &'static [&'static str],
) -> impl Strategy<Value = String> {
    let stmt = prop_oneof![
        4 => proptest::sample::select(stmts).prop_map(str::to_string),
        1 => NOISE.prop_map(|s: String| s),
    ];
    proptest::collection::vec(stmt, 0..24)
        .prop_map(move |v| format!("{header}\n{}", v.join("\n")))
}

fn assert_render_invariants(src: &str) -> Result<(), TestCaseError> {
    let out = crate::render(src);
    prop_assert!(
        out.svg.is_some() != out.error.is_some(),
        "svg XOR error violated for source:\n{src}"
    );
    if let Some(svg) = &out.svg {
        prop_assert!(!svg.contains("NaN"), "NaN leaked into SVG for source:\n{src}");
        prop_assert!(!svg.contains("inf"), "inf leaked into SVG for source:\n{src}");
    }
    Ok(())
}

macro_rules! panic_safety {
    ($name:ident, $header:expr, [$($stmt:expr),+ $(,)?]) => {
        proptest! {
            #![proptest_config(ProptestConfig::with_cases(128))]
            #[test]
            fn $name(src in source_strategy($header, &[$($stmt),+])) {
                assert_render_invariants(&src)?;
            }
        }
    };
}

panic_safety!(pie_render_safe, "pie", [
    "title T",
    "showData",
    "\"a\" : 5",
    "\"b\" : 0",
    "\"c\" : 0.0",
    "\"d\" : -3",
    "\"e\" : 1e308",
    "\"g\" : 1e999",
    "\"a\" : 5",
    ": 5",
    "\"unclosed : 1",
]);

panic_safety!(gantt_render_safe, "gantt", [
    "title T",
    "dateFormat YYYY-MM-DD",
    "axisFormat %m-%d",
    "excludes weekends",
    "section S",
    "A : 2024-01-01, 1d",
    "B :b1, 2024-01-01, 30d",
    "C :after b1, 20d",
    "D :crit, active, 2024-01-01, 0d",
    "E : 2024-13-99, 1d",
    "G : 2024-01-02, -5d",
    "H : x, 1d",
    "milestone M : 2024-01-01, 0d",
]);

panic_safety!(gitgraph_render_safe, "gitGraph", [
    "commit",
    "commit id: \"a\"",
    "commit tag: \"v1\"",
    "commit type: REVERSE",
    "branch dev",
    "branch dev",
    "checkout dev",
    "checkout main",
    "merge dev",
    "merge main",
    "cherry-pick id: \"a\"",
    "commit id: \"a\" tag:",
]);

panic_safety!(mindmap_render_safe, "mindmap", [
    "  root((r))",
    "  root",
    "    A",
    "      a1",
    "        deep",
    "    B[square]",
    "    C(rounded)",
    "    D{{hex}}",
    "  second-root",
    "      skip-level",
    "::icon(x)",
]);

panic_safety!(timeline_render_safe, "timeline", [
    " title T",
    " section 2000s",
    "  2002 : a",
    "  2004 : b : c",
    "  2005",
    "  : orphan",
    " section",
    "  2010 : d",
]);

panic_safety!(journey_render_safe, "journey", [
    " title T",
    " section S",
    "  Make tea: 5: Me",
    "  Work: 1: Me, Cat",
    "  T: 0: Me",
    "  T: -2: Me",
    "  T: 99: Me",
    "  T: x: Me",
    "  NoActor: 3",
    "  : :",
]);

panic_safety!(quadrant_render_safe, "quadrantChart", [
    " title T",
    " x-axis Low --> High",
    " y-axis Bottom --> Top",
    " quadrant-1 Q1",
    " quadrant-2 Q2",
    " quadrant-3 Q3",
    " quadrant-4 Q4",
    " A: [0.3, 0.5]",
    " B: [1.5, -0.2]",
    " C: [0, 0]",
    " D: [x, 0.5]",
    " E: 0.3, 0.5",
    " G: [1e999, 0.5]",
]);

panic_safety!(xychart_render_safe, "xychart-beta", [
    " title \"t\"",
    " horizontal",
    " x-axis [a, b, c]",
    " x-axis \"lbl\" 0 --> 10",
    " y-axis \"y\" 0 --> 100",
    " y-axis \"y\" 5 --> 5",
    " y-axis \"y\" 10 --> -10",
    " bar [10, 40, 90]",
    " line [10, 40, 90]",
    " bar [0]",
    " bar []",
    " line [1, x, 3]",
    " bar [1e308, 1e308]",
    " line [1e999]",
    " y-axis \"y\" 0 --> 1e999",
]);

panic_safety!(kanban_render_safe, "kanban", [
    "  Todo",
    "    a task",
    "    task[bracketed]",
    "    id6[another]",
    "  In Progress",
    "  Done",
    "    x",
    "  col[titled]",
    "@{ priority: 'high' }",
    "    only-card-no-column",
]);

panic_safety!(packet_render_safe, "packet-beta", [
    " title T",
    " 0-15: \"a\"",
    " 16-31: \"b\"",
    " 32: \"single\"",
    " 7-3: \"reversed\"",
    " 0-15: \"overlap\"",
    " 999999-1000000: \"huge\"",
    " x-y: \"bad\"",
    " 0-7: unquoted",
]);

panic_safety!(requirement_render_safe, "requirementDiagram", [
    " requirement r1 {",
    " id: 1",
    " text: some text",
    " risk: high",
    " verifymethod: test",
    " }",
    " element e1 {",
    " type: sim",
    " docref: d",
    " }",
    " e1 - satisfies -> r1",
    " r1 <- copies - e1",
    " e1 - bogus -> r1",
    " a - traces -> ghost",
]);

panic_safety!(block_render_safe, "block-beta", [
    " columns 3",
    " columns 0",
    " columns -1",
    " a b c",
    " d",
    " e:2 x",
    " e:99",
    " a[\"label\"]",
    " space",
    " space:3",
    " a --> b",
    " a --> ghost",
    " blockArrowId<[\"x\"]>(right)",
]);

panic_safety!(radar_render_safe, "radar-beta", [
    " title T",
    " axis a, b, c",
    " axis m[\"lbl\"]",
    " curve x{1, 5, 3}",
    " curve y[\"lbl\"]{0, 0, 0}",
    " curve z{-5, 10}",
    " curve w{1e308, 1e308, 1e308}",
    " curve q{1e999, 2}",
    " max 100",
    " max 0",
    " max 1e999",
    " max -10",
    " min 5",
    " graticule polygon",
    " ticks 0",
]);

panic_safety!(treemap_render_safe, "treemap-beta", [
    " title T",
    " \"root\"",
    "   \"a\": 10",
    "   \"b\": 0",
    "   \"c\": -5",
    "     \"deep\": 1e308",
    "   \"g\": 1e999",
    " \"section\"",
    "   \"x\": 5",
    "\"::icon\"",
    "   \"unparented\": 3",
]);

panic_safety!(sankey_render_safe, "sankey-beta", [
    " a,b,5",
    " a,c,3",
    " b,c,2",
    " a,a,4",
    " c,a,1",
    " a,b,0",
    " a,b,-5",
    " a,b,1e308",
    " c,d,1e308",
    " a,b,1e999",
    " \"q,x\",b,2",
    " a,b",
    " a,b,x",
]);

panic_safety!(c4_render_safe, "C4Context", [
    " title T",
    " Person(p, \"P\", \"d\")",
    " Person_Ext(px, \"PX\")",
    " System(s, \"S\")",
    " System_Ext(sx, \"SX\")",
    " SystemDb(db, \"DB\", \"pg\")",
    " System_Boundary(b1, \"B\") {",
    " Enterprise_Boundary(b2, \"E\") {",
    " }",
    " Rel(p, s, \"uses\", \"https\")",
    " Rel(s, ghost, \"x\")",
    " BiRel(p, s, \"both\")",
    " UpdateRelStyle(p, s, $lineColor=\"red\")",
]);

panic_safety!(architecture_render_safe, "architecture-beta", [
    " group api(cloud)[api]",
    " group inner(cloud)[inner] in api",
    " service db(database)[db] in api",
    " service srv(server)[srv] in api",
    " service lone(disk)[lone]",
    " junction j",
    " junction j2 in api",
    " db:L -- R:srv",
    " db:T --> B:lone",
    " lone:R <-- L:j",
    " db:L -- R:ghost",
    " db:X -- Y:srv",
]);

// ─── Escaping: every diagram type must XML-escape user text ─────────
//
// Every emitter routes user-visible text through `crate::escape_xml`,
// but before this test only 3 of 22 types asserted it. These SVGs are
// rendered inline inside documents, so one dropped `escape_xml` call in
// a future refactor is a stored-XSS vector. Each entry embeds a hostile
// label in a position that type actually renders; the assertions
// require the render to SUCCEED and the payload to appear escaped —
// a parse failure or a silently dropped label fails the test rather
// than passing it vacuously.

#[test]
fn every_diagram_type_escapes_user_text() {
    // `<script>&` exercises <, >, and & (quotes can't appear inside
    // quoted-label grammars, so they're covered by pie's dedicated
    // escape_xml unit test instead).
    const H: &str = "<script>&";

    let sources: [(&str, String); 22] = [
        ("pie", format!("pie title {H}\n \"a{H}\" : 5")),
        ("flowchart", format!("flowchart TD\n A[\"{H}\"] --> B")),
        ("sequence", format!("sequenceDiagram\n Alice->>Bob: {H}")),
        ("state", format!("stateDiagram-v2\n s1 : {H}")),
        ("class", format!("classDiagram\n class A\n A : +{H}()")),
        ("er", format!("erDiagram\n A ||--o{{ B : \"{H}\"")),
        (
            "gantt",
            format!("gantt\n title {H}\n section {H}\n a{H} : 2024-01-01, 1d"),
        ),
        ("gitGraph", format!("gitGraph\n commit id: \"{H}\"")),
        ("mindmap", format!("mindmap\n  root(({H}))")),
        ("timeline", format!("timeline\n title {H}\n 2002 : e{H}")),
        (
            "journey",
            format!("journey\n title {H}\n section {H}\n task {H}: 5: Me"),
        ),
        (
            "quadrantChart",
            format!("quadrantChart\n title {H}\n {H}: [0.3, 0.5]"),
        ),
        (
            "xychart-beta",
            format!("xychart-beta\n title \"{H}\"\n x-axis [a{H}]\n bar [3]"),
        ),
        ("kanban", format!("kanban\n col[{H}]\n  card[{H}]")),
        ("packet-beta", format!("packet-beta\n 0-7: \"{H}\"")),
        (
            "requirementDiagram",
            format!("requirementDiagram\n requirement r{{\n text: {H}\n }}\n element e {{\n }}\n e - satisfies -> r"),
        ),
        // Interior padding: block trims `<>` (and quotes/brackets) at label
        // EDGES as shape delimiters (`<["x"]>` syntax), so a payload
        // touching the edge tests trim semantics, not escaping.
        ("block-beta", format!("block-beta\n columns 1\n a[\"x{H}x\"]")),
        (
            "radar-beta",
            format!("radar-beta\n title {H}\n axis a[\"{H}\"], b\n curve c{{1, 2}}"),
        ),
        ("treemap-beta", format!("treemap-beta\n \"r{H}\"\n   \"x{H}\": 5")),
        ("sankey-beta", format!("sankey-beta\n \"a{H}\",b,5")),
        ("C4Context", format!("C4Context\n Person(p, \"{H}\")")),
        (
            "architecture-beta",
            format!("architecture-beta\n service a(server)[{H}]"),
        ),
    ];

    for (name, src) in &sources {
        let out = crate::render(src);
        let svg = out.svg.as_ref().unwrap_or_else(|| {
            panic!(
                "{name}: hostile-label source failed to parse ({}), escaping unverifiable",
                out.error
                    .as_ref()
                    .map_or("no error".to_string(), |e| e.message.clone())
            )
        });
        assert!(
            !svg.contains("<script"),
            "{name}: raw <script leaked into SVG"
        );
        assert!(
            svg.contains("&lt;script&gt;&amp;"),
            "{name}: escaped payload missing from SVG — label dropped or double-escaped"
        );
    }
}
