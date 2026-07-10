// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use ogrenotes_common::metrics::{counter, histogram, MetricKey};
use ogrenotes_embeddings::VectorFilter;
use ogrenotes_search::{SearchError, SearchHit, SearchQuery};
use ogrenotes_storage::models::AccessLevel;

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::documents::check_doc_access;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(search))
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    #[serde(rename = "type")]
    doc_type: Option<String>,
    from: Option<String>,
    #[serde(rename = "in")]
    folder_id: Option<String>,
    count: Option<usize>,
    /// Search mode: "keyword", "semantic", or "hybrid" (default).
    mode: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    results: Vec<SearchResultItem>,
    total_estimate: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResultItem {
    id: String,
    title: String,
    snippet: String,
    doc_type: String,
    updated_at: i64,
}

/// GET /search?q=...&type=...&from=...&in=...&count=...&mode=...
async fn search(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, ApiError> {
    // Per-user rate limit (#36). Semantic mode hits Bedrock per call,
    // so an unlimited search loop directly runs up the AWS bill.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "search",
        &user_id,
        state.config.rate_limit_search_per_min,
        60,
    )
    .await?;

    let q = params.q.trim();
    if q.is_empty() {
        return Err(ApiError::BadRequest(
            "Search query cannot be empty".to_string(),
        ));
    }
    if q.len() > 200 {
        return Err(ApiError::BadRequest(
            "Search query too long (max 200 characters)".to_string(),
        ));
    }

    // Reject empty-string filter values
    let doc_type = params.doc_type.filter(|s| !s.is_empty());
    let owner_id = params.from.filter(|s| !s.is_empty());
    let folder_id = params.folder_id.filter(|s| !s.is_empty());

    // Clamp to [1, 50]: tantivy's TopDocs collector asserts limit >= 1,
    // so an unclamped count=0 would panic the handler (issue #7).
    let count = params.count.unwrap_or(20).clamp(1, 50);

    // Determine effective search mode (fall back to keyword if embeddings unavailable)
    let has_embeddings = state.embedding_pipeline.is_some();
    let mode = match params.mode.as_deref() {
        Some("keyword") => "keyword",
        Some("semantic") if has_embeddings => "semantic",
        Some("hybrid") if has_embeddings => "hybrid",
        None if has_embeddings => "hybrid",
        _ => "keyword",
    };

    // Over-fetch to account for permission filtering. Vector/hybrid modes
    // return globally-ranked results (not pre-filtered by ownership), so
    // more candidates may be filtered out — use a higher multiplier.
    let fetch_limit = match mode {
        "keyword" => count * 3,
        _ => count * 5,
    };

    counter::inc(MetricKey::new(
        "search.queries_total",
        &[("mode", mode)],
    ));
    let query_start = std::time::Instant::now();
    let hits = match mode {
        "keyword" => keyword_search(&state, q, &doc_type, &owner_id, &folder_id, fetch_limit)?,
        "semantic" => {
            semantic_search(&state, q, &doc_type, &owner_id, &folder_id, fetch_limit).await?
        }
        "hybrid" => {
            hybrid_search(&state, q, &doc_type, &owner_id, &folder_id, fetch_limit).await?
        }
        _ => unreachable!(),
    };
    let latency_ms = query_start.elapsed().as_secs_f64() * 1000.0;
    let histo_name: &'static str = match mode {
        "keyword" => "search.keyword_latency_ms",
        "semantic" => "search.semantic_latency_ms",
        _ => "search.hybrid_latency_ms",
    };
    histogram::record(MetricKey::new(histo_name, &[]), latency_ms);

    // Permission-filter results concurrently. `check_doc_access` returns
    // the DocumentMeta on success, which we keep for relationship ranking.
    let access_results = futures_util::future::join_all(
        hits.iter()
            .map(|hit| check_doc_access(&state, &hit.doc_id, &user_id, AccessLevel::View)),
    )
    .await;

    // Accessible candidates paired with their meta and original fusion
    // rank (text relevance). `enumerate` over the fused order so it can
    // break ties within a relationship tier below.
    let accessible: Vec<(usize, SearchHit, ogrenotes_storage::models::document::DocumentMeta)> =
        hits
            .into_iter()
            .zip(access_results)
            .enumerate()
            .filter_map(|(rank, (hit, access))| access.ok().map(|meta| (rank, hit, meta)))
            .collect();

    // #112: rank each accessible hit by the searcher's RELATIONSHIP to it,
    // then by text relevance within a tier:
    //   tier 0 — durable (owner / direct doc-member / folder-member),
    //   tier 1 — engaged (the searcher has opened it before),
    //   tier 2 — workspace-link with no relationship (the demoted floor).
    // Link-shared docs the searcher has no relationship to STILL appear
    // (discovery preserved per design/linksharing.md §9) — they're just
    // ranked below the searcher's own / engaged docs of comparable text
    // relevance. This is query-time ranking only; access gating
    // (check_doc_access above) and discoverability are unchanged.
    let tiers: Vec<u8> = futures_util::future::join_all(accessible.iter().map(|(_, _, meta)| {
        let state = &state;
        let user_id = &user_id;
        async move {
            if super::documents::has_durable_access(state, meta, user_id)
                .await
                .unwrap_or(false)
            {
                0
            } else if state
                .doc_repo
                .has_opened(&meta.doc_id, user_id)
                .await
                .unwrap_or(false)
            {
                1
            } else {
                2
            }
        }
    }))
    .await;

    let ranked: Vec<(u8, usize, SearchHit)> = accessible
        .into_iter()
        .zip(tiers)
        .map(|((rank, hit, _meta), tier)| (tier, rank, hit))
        .collect();

    let results: Vec<SearchResultItem> = order_by_relationship_tier(ranked)
        .into_iter()
        .take(count)
        .map(|hit| SearchResultItem {
            id: hit.doc_id,
            title: hit.title,
            snippet: hit.snippet,
            doc_type: hit.doc_type,
            updated_at: hit.updated_at,
        })
        .collect();

    let total_estimate = results.len();

    Ok(Json(SearchResponse {
        results,
        total_estimate,
    }))
}

// ─── Search mode implementations ───────────────────────────────

/// BM25 keyword search via Tantivy.
fn keyword_search(
    state: &AppState,
    query_text: &str,
    doc_type: &Option<String>,
    owner_id: &Option<String>,
    folder_id: &Option<String>,
    limit: usize,
) -> Result<Vec<SearchHit>, ApiError> {
    let query = SearchQuery {
        text: query_text.to_string(),
        doc_type: doc_type.clone(),
        owner_id: owner_id.clone(),
        folder_id: folder_id.clone(),
        limit,
        offset: 0,
    };
    state.search_index.search(&query).map_err(search_err)
}

/// Pure vector search via the embedding pipeline.
async fn semantic_search(
    state: &AppState,
    query_text: &str,
    doc_type: &Option<String>,
    owner_id: &Option<String>,
    folder_id: &Option<String>,
    limit: usize,
) -> Result<Vec<SearchHit>, ApiError> {
    let pipeline = state.embedding_pipeline.as_ref().unwrap();
    let filter = VectorFilter {
        doc_type: doc_type.clone(),
        owner_id: owner_id.clone(),
        folder_id: folder_id.clone(),
    };
    let vector_hits = pipeline.search(query_text, limit, Some(filter)).await.map_err(|e| {
        counter::inc(MetricKey::new("search.errors_total", &[("stage", "vector")]));
        tracing::error!(error = %e, "vector search error");
        ApiError::Internal("Search failed".to_string())
    })?;

    Ok(vector_hits
        .into_iter()
        .map(|vh| SearchHit {
            doc_id: vh.doc_id,
            title: vh.title,
            score: vh.score,
            snippet: String::new(), // no keyword snippet for semantic results
            doc_type: vh.doc_type,
            owner_id: vh.owner_id,
            updated_at: vh.updated_at,
            created_at: 0,
        })
        .collect())
}

/// Hybrid search: run BM25 + vector concurrently, merge with Reciprocal Rank Fusion.
async fn hybrid_search(
    state: &AppState,
    query_text: &str,
    doc_type: &Option<String>,
    owner_id: &Option<String>,
    folder_id: &Option<String>,
    limit: usize,
) -> Result<Vec<SearchHit>, ApiError> {
    let bm25_result = keyword_search(state, query_text, doc_type, owner_id, folder_id, limit);
    let vector_future = semantic_search(state, query_text, doc_type, owner_id, folder_id, limit);

    // Run vector search concurrently (BM25 is synchronous so it's already done)
    let bm25_hits = bm25_result?;
    // Graceful degradation: if vector search fails, fall back to keyword-only
    let vector_hits = vector_future.await.unwrap_or_default();

    // Reciprocal Rank Fusion: score = sum(1 / (k + rank)) across retrievers
    const K: f32 = 60.0;
    let mut rrf_scores: HashMap<String, (f32, SearchHit)> = HashMap::new();

    for (rank, hit) in bm25_hits.into_iter().enumerate() {
        let rrf = 1.0 / (K + rank as f32 + 1.0);
        rrf_scores
            .entry(hit.doc_id.clone())
            .and_modify(|(score, _)| *score += rrf)
            .or_insert((rrf, hit));
    }

    for (rank, hit) in vector_hits.into_iter().enumerate() {
        let rrf = 1.0 / (K + rank as f32 + 1.0);
        rrf_scores
            .entry(hit.doc_id.clone())
            .and_modify(|(score, existing)| {
                *score += rrf;
                // Prefer the BM25 hit's snippet if available
                if existing.snippet.is_empty() && !hit.snippet.is_empty() {
                    existing.snippet = hit.snippet.clone();
                }
            })
            .or_insert((rrf, hit));
    }

    let mut fused: Vec<SearchHit> = rrf_scores
        .into_values()
        .map(|(rrf_score, mut hit)| {
            hit.score = rrf_score;
            hit
        })
        .collect();

    fused.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    fused.truncate(limit);

    Ok(fused)
}

/// #112: order accessible search hits by relationship tier (0 = durable,
/// 1 = engaged, 2 = workspace-link floor), breaking ties by the fused
/// text-relevance rank (the second tuple element, ascending = better).
/// Pure + generic so the ranking rule is unit-tested without I/O.
fn order_by_relationship_tier<T>(mut ranked: Vec<(u8, usize, T)>) -> Vec<T> {
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, _, item)| item).collect()
}

fn search_err(e: SearchError) -> ApiError {
    match e {
        SearchError::QueryParse(_) => {
            // Don't echo the Tantivy parser's text to the client: it names
            // valid field names ("unknown field 'X'") and reflects query
            // fragments, letting a probing client map the index schema (#45).
            // Keep the detail server-side; return a generic syntax hint.
            tracing::debug!(error = %e, "search query parse rejected");
            ApiError::BadRequest("Invalid search query syntax".to_string())
        }
        _ => {
            counter::inc(MetricKey::new("search.errors_total", &[("stage", "index")]));
            tracing::error!(error = %e, "search index error");
            ApiError::Internal("Search failed".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_tier_ranks_durable_and_engaged_above_link_floor() {
        // #112: a workspace-link doc (tier 2) with the *best* text relevance
        // (fusion rank 0) must still sort below a durable doc (tier 0) and an
        // engaged doc (tier 1) that have worse relevance — relationship beats
        // raw relevance across tiers.
        let input = vec![
            (2u8, 0usize, "link-best-text"),
            (0, 5, "durable-worse-text"),
            (1, 3, "engaged-mid-text"),
            (2, 1, "link-second"),
        ];
        assert_eq!(
            order_by_relationship_tier(input),
            vec![
                "durable-worse-text",
                "engaged-mid-text",
                "link-best-text",
                "link-second",
            ],
        );
    }

    #[test]
    fn relationship_tier_preserves_text_relevance_within_a_tier() {
        // Within one tier the fused relevance rank is the sole tiebreaker, so
        // discovery order is unchanged for docs of the same relationship.
        let input = vec![(0u8, 2usize, "b"), (0, 0, "a"), (0, 1, "ab")];
        assert_eq!(order_by_relationship_tier(input), vec!["a", "ab", "b"]);
    }

    #[test]
    fn query_parse_error_does_not_echo_query_or_schema() {
        // #45: the Tantivy parser names fields and reflects the query text.
        // Build an error whose message contains both a "field name" and a
        // query fragment, then assert neither survives into the client body.
        let leaky = "field:secret_column ( unbalanced";
        let err = SearchError::query_parse_for_test(leaky);

        let ApiError::BadRequest(body) = search_err(err) else {
            panic!("QueryParse must map to BadRequest");
        };

        assert_eq!(body, "Invalid search query syntax");
        assert!(!body.contains("secret_column"), "must not leak field names: {body}");
        assert!(!body.contains("unbalanced"), "must not echo query text: {body}");
    }
}
