//! Property tests for the ER-diagram pipeline (proptest, dev-only).

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup: entity attribute-block open/close + attribute
    // rows, and relationship-token statements covering every
    // cardinality symbol pairing and both identifying/non-identifying
    // lines, shuffled together — exercises the parser's error paths
    // and, when parse succeeds, the full pipeline.
    let stmt = prop_oneof![
        Just("A {".to_string()),
        Just("}".to_string()),
        Just("string name".to_string()),
        Just("int id PK".to_string()),
        Just("int org_id FK".to_string()),
        Just("A ||--o{ B : has".to_string()),
        Just("A |o--o| B : rel".to_string()),
        Just("A }|--|{ B : rel".to_string()),
        Just("A }o--o{ B : rel".to_string()),
        Just("A ||..|| B : rel".to_string()),
        Just("A |o..o| B : rel".to_string()),
        // New-syntax coverage: hyphenated + quoted names, aliases,
        // word-form cardinalities, combined keys, comments.
        Just("LINE-ITEM ||--|{ DELIVERY-ADDRESS : uses".to_string()),
        Just("\"Order Detail\" ||--|| A : x".to_string()),
        Just("CUSTOMER[\"Cust Acct\"] {".to_string()),
        Just("A only one to zero or more B : has".to_string()),
        Just("A 1 optionally to 0+ B : maybe".to_string()),
        Just("int id PK, FK".to_string()),
        Just("string name \"the label\"".to_string()),
        Just("int code UK".to_string()),
        "[a-zA-Z0-9_\\-:\" {}|.]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40)
        .prop_map(|v| format!("erDiagram\n{}", v.join("\n")))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    /// Family-specific sanity: every relation's `from`/`to` must index
    /// into the parsed entity list — `ensure_entity` always creates the
    /// entity before a relation referencing it is pushed, so an
    /// out-of-bounds index would be a parser bug.
    #[test]
    fn successful_parses_relation_indices_in_bounds(src in arb_source()) {
        if let Ok(g) = crate::er::parse::parse(&src) {
            for r in &g.relations {
                prop_assert!(r.from < g.entities.len());
                prop_assert!(r.to < g.entities.len());
            }
        }
    }
}
