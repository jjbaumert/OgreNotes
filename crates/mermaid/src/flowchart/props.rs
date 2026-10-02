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
        Just("direction LR".to_string()),
        Just("direction TB".to_string()),
        Just("end".to_string()),
        Just("classDef default fill:#f9f".to_string()),
        Just("C:::default".to_string()),
        Just("s --> A".to_string()),
        Just("A[\"x;y\"] --> B".to_string()),
        "[a-zA-Z0-9_ <>ox~=.|&;\"-]{0,24}",
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// The crate-wide invariants (`svg` XOR `error`, no NaN/inf, and
    /// well-formed XML) over this family's own grammar; the fuzz net in
    /// `src/props.rs` only drives the chart-style kinds through them.
    #[test]
    fn render_output_satisfies_crate_invariants(src in arb_source()) {
        crate::props::assert_render_invariants(&src)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x0006_5245),
        ..ProptestConfig::default()
    })]

    /// Exercise the public source-to-SVG pipeline in every flow direction.
    /// Checking the rendered nodes also catches sizing/emission mismatches.
    #[test]
    fn seeded_dags_keep_nodes_separate_and_inside_the_canvas(
        direction in prop_oneof![Just("TD"), Just("BT"), Just("LR"), Just("RL")],
        count in 2usize..12,
        edges in proptest::collection::vec((0usize..12, 0usize..12), 0..20),
    ) {
        let mut source = format!("flowchart {direction}\n");
        for index in 0..count {
            source.push_str(&format!("n{index}[Node {index}]\n"));
        }
        for (a, b) in edges {
            if a < b && b < count {
                source.push_str(&format!("n{a} --> n{b}\n"));
            }
        }
        let svg = crate::render(&source).svg.expect("valid DAG renders");
        crate::extent::assert_inside(&svg);
        let (_, elements) = crate::extent::scan(&svg);
        let nodes: Vec<_> = elements.iter().filter(|element| element.tag == "rect").collect();
        prop_assert_eq!(nodes.len(), count);
        for (index, node) in nodes.iter().enumerate() {
            for other in &nodes[index + 1..] {
                prop_assert_eq!(node.bbox.overlap(&other.bbox), 0.0);
            }
        }
    }
}
