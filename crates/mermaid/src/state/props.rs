//! Property tests for the state-diagram pipeline (proptest, dev-only).

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup: transitions, `[*]` endpoints, state decls
    // (plain/quoted-display/stereotype), composite open/close, and
    // notes shuffled together — exercises the parser's error paths and,
    // when parse succeeds, the full pipeline.
    let stmt = prop_oneof![
        Just("A --> B".to_string()),
        Just("A --> B: event".to_string()),
        Just("[*] --> A".to_string()),
        Just("A --> [*]".to_string()),
        Just("state A".to_string()),
        Just("state \"Long name\" as A".to_string()),
        Just("state C <<choice>>".to_string()),
        Just("state F <<fork>>".to_string()),
        Just("state J <<join>>".to_string()),
        Just("state X {".to_string()),
        Just("}".to_string()),
        Just("note left of A: n".to_string()),
        Just("note right of B: n".to_string()),
        Just("bareId".to_string()),
        Just("s2 : a description".to_string()),
        Just("[*] --> Still:::notMoving".to_string()),
        Just("a --> b %% trailing comment".to_string()),
        Just("note right of __start_0: boo".to_string()),
        "[a-zA-Z0-9_:%<>\\- ]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40)
        .prop_map(|v| format!("stateDiagram-v2\n{}", v.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    /// Family-specific sanity: any successful parse's node list must be
    /// large enough to cover every transition endpoint index it
    /// references (nodes are created before a transition can reference
    /// them, so this holds for any well-formed graph the parser emits).
    #[test]
    fn successful_parses_nodes_cover_transition_endpoints(src in arb_source()) {
        if let Ok(g) = crate::state::parse::parse(&src) {
            let max_endpoint = g.transitions.iter().flat_map(|t| [t.from, t.to]).max();
            if let Some(m) = max_endpoint {
                prop_assert!(g.nodes.len() > m);
            }
        }
    }
}
