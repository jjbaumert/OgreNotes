//! Property tests for the sequence pipeline (proptest, dev-only).

use proptest::prelude::*;

fn arb_source() -> impl Strategy<Value = String> {
    // Statement soup: valid-ish lines shuffled together — exercises the
    // parser's error paths and, when parse succeeds, the full pipeline.
    let stmt = prop_oneof![
        Just("A->>B: hi".to_string()),
        Just("B-->>-A: bye".to_string()),
        Just("A->>+B: go".to_string()),
        Just("A->>A: self".to_string()),
        Just("Note over A,B: n".to_string()),
        Just("Note left of A: l".to_string()),
        Just("loop lbl".to_string()),
        Just("alt c".to_string()),
        Just("else o".to_string()),
        Just("par p".to_string()),
        Just("and q".to_string()),
        Just("end".to_string()),
        Just("activate A".to_string()),
        Just("deactivate A".to_string()),
        Just("autonumber".to_string()),
        Just("participant Z as Zed".to_string()),
        "[a-zA-Z<>:\\-x)+ ]{0,24}",
    ];
    proptest::collection::vec(stmt, 0..40).prop_map(|v| {
        format!("sequenceDiagram\n{}", v.join("\n"))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn render_never_panics_and_xor_holds(src in arb_source()) {
        let out = crate::render(&src);
        prop_assert!(out.svg.is_some() != out.error.is_some());
    }

    #[test]
    fn successful_layouts_are_sane(src in arb_source()) {
        if let Ok(d) = crate::sequence::parse::parse(&src) {
            let l = crate::sequence::layout::run(&d);
            let mut prev = f64::NEG_INFINITY;
            for m in &l.messages {
                prop_assert!(m.y.is_finite() && m.y >= prev);
                prev = m.y;
            }
            prop_assert!(l.size.0.is_finite() && l.size.1.is_finite());
            for f in &l.frames {
                prop_assert!(f.rect.w > 0.0 && f.rect.h > 0.0);
            }
        }
    }
}
