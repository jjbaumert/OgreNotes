//! Property tests for the class-diagram pipeline (proptest, dev-only).

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup: class decls, member-block open/close + member
    // lines, all 11 relationship-operator forms (mirrors
    // `class::parse::tests::all_relationship_kinds_normalized`), and
    // dotted members shuffled together — exercises the parser's error
    // paths and, when parse succeeds, the full pipeline.
    let stmt = prop_oneof![
        Just("class A".to_string()),
        Just("class A {".to_string()),
        Just("}".to_string()),
        Just("+String name".to_string()),
        Just("-int age".to_string()),
        Just("+speak() String".to_string()),
        Just("<<interface>>".to_string()),
        Just("A : +member".to_string()),
        Just("A <|-- B".to_string()),
        Just("A --|> B".to_string()),
        Just("A <|.. B".to_string()),
        Just("A *-- B".to_string()),
        Just("A --* B".to_string()),
        Just("A o-- B".to_string()),
        Just("A --> B".to_string()),
        Just("A <-- B".to_string()),
        Just("A -- B".to_string()),
        Just("A ..> B".to_string()),
        Just("A <.. B".to_string()),
        Just("Customer \"1\" --> \"0..*\" Order : places".to_string()),
        "[a-zA-Z0-9_\\-: <>*.\"]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40)
        .prop_map(|v| format!("classDiagram\n{}", v.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    /// Family-specific sanity: every relation's `from`/`to` must index
    /// into the parsed class list — `ensure_class` always creates the
    /// class before a relation referencing it is pushed, so an
    /// out-of-bounds index would be a parser bug.
    #[test]
    fn successful_parses_relation_indices_in_bounds(src in arb_source()) {
        if let Ok(g) = crate::class::parse::parse(&src) {
            for r in &g.relations {
                prop_assert!(r.from < g.classes.len());
                prop_assert!(r.to < g.classes.len());
            }
        }
    }
}
