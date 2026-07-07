// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.3 piece A — typed loaders for the RAG eval set.
//!
//! The data file `queries.json` carries two top-level arrays:
//!
//! - `corpus`: seed documents the runner creates on a clean test
//!   stack before iterating queries. Each entry has a **stable
//!   string id** (e.g. `"auth-design"`) that the runner maps to
//!   the server-generated doc id at seed time. Queries reference
//!   corpus entries by these stable ids so the source data is
//!   self-contained — no environment-specific ids leak into the
//!   query catalog.
//!
//! - `queries`: 50 evaluation queries split four ways
//!   (keyword / semantic / multi-hop / gap-analysis). Each carries
//!   `expected_doc_ids: Vec<String>` pointing at the corpus
//!   entries the agent should retrieve.
//!
//! The catalog is consumed by the runner binary in
//! `tests/rag-eval/src/main.rs` (piece B) which performs the
//! seed + execute + score loop.

use serde::{Deserialize, Serialize};

/// Top-level shape of `queries.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCatalog {
    pub corpus: Vec<CorpusDoc>,
    pub queries: Vec<EvalQuery>,
}

/// One seed document. The runner creates it via
/// `POST /api/v1/documents` with `title` as the title and `content`
/// joined into the doc body. `id` is the **catalog id** — stable
/// across runs and used by queries to declare expected results.
/// The server-generated doc id is captured at seed time and
/// resolved during scoring.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusDoc {
    pub id: String,
    pub title: String,
    /// Body text. The runner imports it via the markdown route so
    /// headings + paragraphs round-trip into the editor's block
    /// shape. Keep entries focused (a few paragraphs each) — the
    /// eval is about retrieval, not ingestion stress-testing.
    pub content: String,
}

/// Query categories per the RAG plan §4.1. Affects how the
/// scorer interprets a miss — semantic / multi-hop queries are
/// allowed to also exercise the agent loop (recall measured
/// against the union of all tool calls), whereas keyword queries
/// are scored against the keyword_search hit list directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryCategory {
    /// Exact-term lookups, identifier matches, named entities. The
    /// expectation is that Tantivy BM25 alone returns the right
    /// docs; the agent doesn't have to do anything clever.
    Keyword,
    /// Natural-language questions where the wording differs from
    /// the source doc's wording. Tests the semantic-search half
    /// (Qdrant vectors).
    Semantic,
    /// Questions requiring traversal across relationships or
    /// follow-up tool calls (keyword_search → get_document →
    /// get_related → answer).
    MultiHop,
    /// "What's missing" or "which docs don't cover X" style
    /// questions. The expected set may be empty (the answer is
    /// "nothing covers it") and the scorer treats empty-expected
    /// as a no-cite-correct outcome.
    GapAnalysis,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalQuery {
    /// Stable id for the query — used as the CSV row key when
    /// the runner writes results, and for cross-run trend
    /// comparison (recall improvements / regressions show up
    /// per-id).
    pub id: String,
    pub category: QueryCategory,
    /// The user-facing question text the runner sends to the
    /// /search and /ask endpoints.
    pub text: String,
    /// Catalog ids of corpus docs that should appear in the
    /// retrieval results. Empty Vec is legal for gap-analysis
    /// queries.
    pub expected_doc_ids: Vec<String>,
}

impl EvalCatalog {
    /// Load the canonical catalog from the bundled `queries.json`
    /// next to this crate root. Falls back to a path override via
    /// the `RAG_EVAL_CATALOG` env var so a custom run can target
    /// a different corpus.
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var("RAG_EVAL_CATALOG")
            .unwrap_or_else(|_| {
                concat!(env!("CARGO_MANIFEST_DIR"), "/queries.json").to_string()
            });
        let raw = std::fs::read_to_string(&path)?;
        let catalog: EvalCatalog = serde_json::from_str(&raw)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Cross-check that every `expected_doc_ids` entry references
    /// a corpus doc that actually exists. Catches typos at load
    /// time rather than producing a confusing "doc not found"
    /// during scoring.
    pub fn validate(&self) -> anyhow::Result<()> {
        let known: std::collections::HashSet<&str> =
            self.corpus.iter().map(|c| c.id.as_str()).collect();
        for q in &self.queries {
            for d in &q.expected_doc_ids {
                if !known.contains(d.as_str()) {
                    anyhow::bail!(
                        "query {:?} references unknown corpus id {:?}",
                        q.id, d,
                    );
                }
            }
        }
        Ok(())
    }

    /// Count of queries per category — useful for the CSV summary
    /// header and for quickly verifying the 10/15/15/10 split.
    pub fn category_counts(&self) -> std::collections::HashMap<QueryCategory, usize> {
        let mut m = std::collections::HashMap::new();
        for q in &self.queries {
            *m.entry(q.category).or_insert(0) += 1;
        }
        m
    }
}

// ─── Recall scoring ────────────────────────────────────────────

/// Recall@k: fraction of expected docs that appear in the
/// retrieved top-k.
///
///   |retrieved ∩ expected| / |expected|
///
/// Gap-analysis queries carry an empty `expected` set; for those
/// the score is 1.0 when `retrieved` is also empty (the agent
/// correctly declined to cite) and 0.0 otherwise. This matches
/// the "no false cite" reward that the RAG plan §4.2 implies
/// without spelling out — gap recall measures restraint.
pub fn recall_at_k(retrieved: &[String], expected: &[String]) -> f64 {
    if expected.is_empty() {
        return if retrieved.is_empty() { 1.0 } else { 0.0 };
    }
    let exp: std::collections::HashSet<&str> =
        expected.iter().map(|s| s.as_str()).collect();
    let hit = retrieved.iter().filter(|r| exp.contains(r.as_str())).count();
    hit as f64 / expected.len() as f64
}

// ─── Seed state — bridges `seed` to `run` ──────────────────────

/// Mapping from catalog id → server-generated doc id, written
/// by `seed` and consumed by `run`. Lives at the path passed via
/// `--state` to both subcommands (defaults to
/// `./rag-eval-state.json`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedState {
    /// Backend the corpus was seeded against. `run` refuses to
    /// continue if the configured base URL doesn't match — a
    /// stale state file from a different stack would compute
    /// nonsense recall numbers.
    pub base_url: String,
    /// Dev-login email used to seed. `run` uses the same user
    /// (and therefore the same permission-filtered search
    /// results).
    pub email: String,
    /// catalog id → server doc id.
    pub doc_ids: std::collections::BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_and_validates() {
        let cat = EvalCatalog::load().expect("catalog should load");
        assert!(!cat.corpus.is_empty(), "corpus must not be empty");
        assert_eq!(cat.queries.len(), 50, "expected 50 queries per RAG plan §4.1");

        let counts = cat.category_counts();
        assert_eq!(counts.get(&QueryCategory::Keyword).copied(), Some(10));
        assert_eq!(counts.get(&QueryCategory::Semantic).copied(), Some(15));
        assert_eq!(counts.get(&QueryCategory::MultiHop).copied(), Some(15));
        assert_eq!(counts.get(&QueryCategory::GapAnalysis).copied(), Some(10));
    }

    #[test]
    fn recall_at_k_full_hit() {
        let retrieved = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let expected = vec!["a".to_string(), "b".to_string()];
        assert!((recall_at_k(&retrieved, &expected) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_partial_hit() {
        let retrieved = vec!["a".to_string(), "c".to_string()];
        let expected = vec!["a".to_string(), "b".to_string()];
        assert!((recall_at_k(&retrieved, &expected) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_gap_empty_retrieved_scores_one() {
        let retrieved: Vec<String> = vec![];
        let expected: Vec<String> = vec![];
        assert!((recall_at_k(&retrieved, &expected) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recall_at_k_gap_with_false_cite_scores_zero() {
        let retrieved = vec!["a".to_string()];
        let expected: Vec<String> = vec![];
        assert!((recall_at_k(&retrieved, &expected)).abs() < 1e-9);
    }

    #[test]
    fn validate_catches_dangling_expected_id() {
        let cat = EvalCatalog {
            corpus: vec![CorpusDoc {
                id: "real-doc".to_string(),
                title: "Real".to_string(),
                content: "x".to_string(),
            }],
            queries: vec![EvalQuery {
                id: "q1".to_string(),
                category: QueryCategory::Keyword,
                text: "search".to_string(),
                expected_doc_ids: vec!["nonexistent".to_string()],
            }],
        };
        let err = cat.validate().expect_err("must reject dangling id");
        assert!(err.to_string().contains("unknown corpus id"));
    }
}
