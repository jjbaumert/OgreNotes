//! Property tests for the flowchart pipeline (proptest, dev-only).
//! Added in the polish slice alongside the unified edge-op scanner —
//! the operator vocabulary (`- . = ~ < > o x`) is the input class most
//! worth fuzzing.

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup over the full operator vocabulary: every body
    // family, terminator, reverse head, and label spelling, plus
    // shapes, subgraphs, classes, and raw noise drawn from the
    // operator characters themselves.
    let stmt = prop_oneof![
        Just("A --> B".to_string()),
        Just("A --o B".to_string()),
        Just("A --x B".to_string()),
        Just("A---oB".to_string()),
        Just("A <--> B".to_string()),
        Just("A o--o B".to_string()),
        Just("A x--x B".to_string()),
        Just("A ~~~ B".to_string()),
        Just("A ----> B".to_string()),
        Just("A -.-> B".to_string()),
        Just("A -.- B".to_string()),
        Just("A ==> B".to_string()),
        Just("A === B".to_string()),
        Just("A--text-->B".to_string()),
        Just("A-.text.-B".to_string()),
        Just("A==text==>B".to_string()),
        Just("A-->|lbl|B".to_string()),
        Just("A[[sub]] --> B{d}".to_string()),
        Just("subgraph s".to_string()),
        Just("end".to_string()),
        Just("classDef default fill:#f9f".to_string()),
        Just("C:::default".to_string()),
        Just("s --> A".to_string()),
        "[a-zA-Z0-9_ <>ox~=.|&-]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40)
        .prop_map(|v| format!("flowchart TD\n{}", v.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    /// Any successful parse references only nodes it actually created.
    #[test]
    fn successful_parses_edges_reference_real_nodes(src in arb_source()) {
        if let Ok(g) = crate::flowchart::parse::parse(&src) {
            for e in &g.edges {
                prop_assert!(e.from < g.nodes.len() && e.to < g.nodes.len());
            }
        }
    }
}
