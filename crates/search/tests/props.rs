// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Property tests for the two seams the example-based suite can't cover:
//!
//! 1. **No panic on arbitrary input.** `query.text` is raw search-box
//!    input handed to tantivy's `QueryParser` (a large grammar: field
//!    syntax, ranges, boosts, fuzzy, wildcards, parens), and title/body
//!    are user-authored content that flows into snippet generation.
//!    `search()`/`count()`/`index_document()` must return `Ok` or `Err`
//!    for anything — never panic.
//!
//! 2. **count == search agreement.** `count()` duplicates `search()`'s
//!    query building line for line; it drives `totalEstimate` in the UI.
//!    If one side's filter logic drifts, pagination silently lies —
//!    this property is the guard.

use ogrenotes_search::{SearchDocument, SearchIndex, SearchQuery};
use proptest::prelude::*;

fn doc(id: &str, title: &str, body: &str, owner: &str, dt: &str, folder: Option<&str>) -> SearchDocument {
    SearchDocument {
        doc_id: id.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        owner_id: owner.to_string(),
        doc_type: dt.to_string(),
        folder_id: folder.map(str::to_string),
        workspace_id: None,
        updated_at: 1,
        created_at: 1,
    }
}

/// Query text shaped like what users (and hostile users) actually type:
/// plain words mixed with tantivy grammar metacharacters.
fn arb_query_text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        "[a-z]{1,8}",
        Just("shared".to_string()),
        Just("title:x".to_string()),
        Just("body:\"quoted phrase\"".to_string()),
        Just("bogusfield:val".to_string()),
        Just("[a TO z]".to_string()),
        Just("{1 TO 10}".to_string()),
        Just("boost^2".to_string()),
        Just("fuzzy~1".to_string()),
        Just("fuzzy~~~".to_string()),
        Just("wild*card".to_string()),
        Just("*".to_string()),
        Just("(open".to_string()),
        Just("))".to_string()),
        Just("AND".to_string()),
        Just("OR NOT".to_string()),
        Just("+".to_string()),
        Just("-".to_string()),
        Just("\"unclosed".to_string()),
        Just("\\".to_string()),
        Just("世界 🎉".to_string()),
        r#"[ -~]{0,12}"#,
    ];
    proptest::collection::vec(piece, 0..6).prop_map(|v| v.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any query text against a seeded index: Ok or Err, never panic —
    /// and when it parses, count agrees with an unpaginated search.
    #[test]
    fn arbitrary_query_never_panics_and_count_matches_search(text in arb_query_text()) {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&doc("d1", "shared alpha", "beta content", "u1", "document", None)).unwrap();
        idx.index_document(&doc("d2", "shared gamma", "delta content", "u2", "spreadsheet", Some("f1"))).unwrap();

        let q = SearchQuery {
            text,
            doc_type: None,
            owner_id: None,
            folder_id: None,
            limit: 100,
            offset: 0,
        };
        let hits = idx.search(&q);
        let count = idx.count(&q);
        match (hits, count) {
            (Ok(h), Ok(c)) => prop_assert_eq!(h.len(), c, "count must agree with unpaginated search"),
            (Err(_), Err(_)) => {} // both reject — consistent
            (h, c) => prop_assert!(
                false,
                "search and count disagree on validity: search={:?} count={:?}",
                h.map(|v| v.len()),
                c
            ),
        }
    }

}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Arbitrary user-authored content indexes and round-trips through
    /// search + snippet generation without panicking.
    #[test]
    fn arbitrary_content_indexes_and_snippets_safely(
        title in "\\PC{0,40}",
        body in "\\PC{0,200}",
    ) {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&doc("d1", &title, &body, "u1", "document", None)).unwrap();
        // Search for a term guaranteed present in a second doc so snippet
        // generation runs against a real match set, plus a query drawn
        // from the arbitrary body itself.
        idx.index_document(&doc("d2", "anchor term", &body, "u1", "document", None)).unwrap();
        let _ = idx.search(&SearchQuery {
            text: "anchor".to_string(),
            doc_type: None, owner_id: None, folder_id: None,
            limit: 10, offset: 0,
        });
        if let Some(word) = body.split_whitespace().next() {
            let _ = idx.search(&SearchQuery {
                text: word.to_string(),
                doc_type: None, owner_id: None, folder_id: None,
                limit: 10, offset: 0,
            });
        }
    }

}

proptest! {
    // The input space is only 2^6 = 64 combos — don't oversample it
    // (each case pays full index setup).
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// count == unpaginated search across every filter combination —
    /// the guard against the duplicated clause-building drifting apart.
    #[test]
    fn count_matches_search_across_filter_combos(
        use_dt in proptest::bool::ANY,
        use_owner in proptest::bool::ANY,
        use_folder in proptest::bool::ANY,
        dt_pick in 0usize..2,
        owner_pick in 0usize..2,
        folder_pick in 0usize..2,
    ) {
        let idx = SearchIndex::open_in_memory().unwrap();
        // Corpus crossing two values per filter dimension, all matching
        // the text term.
        let mut n = 0;
        for dt in ["document", "spreadsheet"] {
            for owner in ["u1", "u2"] {
                for folder in [None, Some("f1")] {
                    n += 1;
                    idx.index_document(&doc(
                        &format!("d{n}"), "shared title", "shared body",
                        owner, dt, folder,
                    )).unwrap();
                }
            }
        }

        let q = SearchQuery {
            text: "shared".to_string(),
            doc_type: use_dt.then(|| ["document", "spreadsheet"][dt_pick].to_string()),
            owner_id: use_owner.then(|| ["u1", "u2"][owner_pick].to_string()),
            folder_id: use_folder.then(|| ["f1", "f2"][folder_pick].to_string()),
            limit: 100,
            offset: 0,
        };
        let hits = idx.search(&q).unwrap();
        let count = idx.count(&q).unwrap();
        prop_assert_eq!(hits.len(), count, "filters: {:?}/{:?}/{:?}", q.doc_type, q.owner_id, q.folder_id);
    }
}
