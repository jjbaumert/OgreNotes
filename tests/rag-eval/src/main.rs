// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.3 piece B — eval runner.
//!
//! Three subcommands:
//!
//!   `summary` — print the catalog stats (piece A).
//!   `seed`    — dev-login as the eval user, create one doc per
//!               corpus entry via /documents/import, write a
//!               `SeedState` JSON mapping catalog ids → server doc
//!               ids. Idempotent: re-seeding deletes the prior
//!               corpus rows (matched by title prefix) first.
//!   `run`     — read the seed state, dev-login, execute every
//!               query against /search and /ask, score recall@10
//!               per query, write a CSV summary.
//!
//! The /ask path streams SSE; we parse Source events directly
//! from the bytes_stream — same shape as
//! `frontend/src/api/ask.rs`'s in-browser parser, in
//! single-threaded async Rust.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use bytes::BytesMut;
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use ogrenotes_rag_eval::{
    recall_at_k, CorpusDoc, EvalCatalog, QueryCategory, SeedState,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;

const DEFAULT_BASE_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_STATE_PATH: &str = "rag-eval-state.json";
const DEFAULT_EVAL_EMAIL: &str = "rag-eval@ogrenotes.example.com";
const DEFAULT_EVAL_NAME: &str = "RAG Eval";
/// Title prefix prepended to every seeded doc. The runner uses it
/// to detect + delete a prior corpus on re-seed without disturbing
/// other docs in the user's account.
const SEED_TITLE_PREFIX: &str = "[rag-eval] ";

#[derive(Parser)]
#[command(
    name = "rag-eval",
    about = "OgreNotes RAG retrieval-quality evaluation harness",
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a summary of the catalog (counts, validation status).
    Summary,
    /// Bootstrap the catalog's corpus on a clean test stack via
    /// /documents/import. Writes a state file mapping catalog ids
    /// to server doc ids for the `run` subcommand to consume.
    Seed {
        #[arg(long, default_value = DEFAULT_BASE_URL)]
        base_url: String,
        #[arg(long, default_value = DEFAULT_EVAL_EMAIL)]
        email: String,
        #[arg(long, default_value = DEFAULT_STATE_PATH)]
        state: PathBuf,
    },
    /// Execute every query against /search and /ask, score recall
    /// per query, dump a CSV.
    Run {
        #[arg(long, default_value = DEFAULT_BASE_URL)]
        base_url: String,
        #[arg(long, default_value = DEFAULT_STATE_PATH)]
        state: PathBuf,
        /// CSV output path. Defaults to `rag-eval-results.csv`
        /// next to the state file.
        #[arg(long, default_value = "rag-eval-results.csv")]
        out: PathBuf,
        /// Skip the /ask half of each query (only score /search
        /// recall). Useful for cost control during iteration on
        /// the corpus or query set itself.
        #[arg(long, default_value_t = false)]
        no_ask: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let catalog = EvalCatalog::load()?;
    match cli.cmd {
        Cmd::Summary => {
            print_summary(&catalog);
            Ok(())
        }
        Cmd::Seed { base_url, email, state } => {
            run_seed(catalog, base_url, email, state).await
        }
        Cmd::Run { base_url, state, out, no_ask } => {
            run_eval(catalog, base_url, state, out, no_ask).await
        }
    }
}

fn print_summary(catalog: &EvalCatalog) {
    println!("RAG eval catalog summary");
    println!("  corpus docs: {}", catalog.corpus.len());
    println!("  queries:     {}", catalog.queries.len());
    println!();
    println!("  by category:");
    let counts = catalog.category_counts();
    for (cat, count) in [
        ("keyword", counts.get(&QueryCategory::Keyword)),
        ("semantic", counts.get(&QueryCategory::Semantic)),
        ("multi-hop", counts.get(&QueryCategory::MultiHop)),
        ("gap-analysis", counts.get(&QueryCategory::GapAnalysis)),
    ] {
        let n = count.copied().unwrap_or(0);
        println!("    {cat:<14} {n}");
    }
}

// ─── seed ──────────────────────────────────────────────────────

async fn run_seed(
    catalog: EvalCatalog,
    base_url: String,
    email: String,
    state_path: PathBuf,
) -> Result<()> {
    let client = Client::builder().build()?;
    let token = dev_login(&client, &base_url, &email).await?;

    // Idempotency: scan the user's docs for our title prefix and
    // delete them before re-seeding. Avoids accumulating drift
    // across iterations.
    let existing = list_seeded_docs(&client, &base_url, &token).await?;
    if !existing.is_empty() {
        eprintln!(
            "Found {} previously-seeded docs; deleting before re-seed",
            existing.len(),
        );
        for id in &existing {
            let _ = delete_doc(&client, &base_url, &token, id).await;
        }
    }

    let mut doc_ids: BTreeMap<String, String> = BTreeMap::new();
    for entry in &catalog.corpus {
        let server_id = create_corpus_doc(&client, &base_url, &token, entry).await?;
        println!("seeded {} -> {server_id}", entry.id);
        doc_ids.insert(entry.id.clone(), server_id);
    }

    let state = SeedState {
        base_url: base_url.clone(),
        email,
        doc_ids,
    };
    std::fs::write(&state_path, serde_json::to_string_pretty(&state)?)?;
    println!("wrote seed state -> {}", state_path.display());
    Ok(())
}

async fn dev_login(client: &Client, base_url: &str, email: &str) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TokenResponse {
        access_token: String,
    }
    let resp = client
        .post(format!("{base_url}/api/v1/auth/dev-login"))
        .json(&json!({ "email": email, "name": DEFAULT_EVAL_NAME }))
        .send()
        .await
        .context("POST /auth/dev-login")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("dev-login returned {status}: {body}"));
    }
    let token: TokenResponse = resp.json().await?;
    Ok(token.access_token)
}

async fn list_seeded_docs(
    client: &Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<String>> {
    // /search is permission-filtered + indexed; querying the title
    // prefix catches the seeded set.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SearchHit {
        id: String,
        title: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SearchResponse {
        results: Vec<SearchHit>,
    }
    let resp = client
        .get(format!("{base_url}/api/v1/search?q=rag-eval&count=100"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /search for cleanup")?;
    if !resp.status().is_success() {
        return Ok(Vec::new());
    }
    let body: SearchResponse = resp.json().await?;
    Ok(body
        .results
        .into_iter()
        .filter(|h| h.title.starts_with(SEED_TITLE_PREFIX))
        .map(|h| h.id)
        .collect())
}

async fn delete_doc(
    client: &Client,
    base_url: &str,
    token: &str,
    doc_id: &str,
) -> Result<()> {
    let resp = client
        .delete(format!("{base_url}/api/v1/documents/{doc_id}"))
        .bearer_auth(token)
        .send()
        .await
        .context("DELETE /documents/:id for cleanup")?;
    if !resp.status().is_success() {
        return Err(anyhow!("delete failed: {}", resp.status()));
    }
    Ok(())
}

async fn create_corpus_doc(
    client: &Client,
    base_url: &str,
    token: &str,
    entry: &CorpusDoc,
) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct DocResponse {
        id: String,
    }
    let title = format!("{SEED_TITLE_PREFIX}{}", entry.title);
    let resp = client
        .post(format!("{base_url}/api/v1/documents/import"))
        .bearer_auth(token)
        .json(&json!({
            "format": "markdown",
            "title": title,
            "content": entry.content,
        }))
        .send()
        .await
        .context("POST /documents/import")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("create {} returned {status}: {body}", entry.id));
    }
    let doc: DocResponse = resp.json().await?;
    Ok(doc.id)
}

// ─── run ──────────────────────────────────────────────────────

async fn run_eval(
    catalog: EvalCatalog,
    base_url: String,
    state_path: PathBuf,
    out_path: PathBuf,
    no_ask: bool,
) -> Result<()> {
    let raw = std::fs::read_to_string(&state_path)
        .with_context(|| format!("read {state_path:?} — did you run `seed`?"))?;
    let state: SeedState = serde_json::from_str(&raw)?;
    if state.base_url != base_url {
        return Err(anyhow!(
            "seed state was for {} but --base-url is {} — re-seed first",
            state.base_url, base_url,
        ));
    }

    let server_to_catalog: BTreeMap<String, String> = state
        .doc_ids
        .iter()
        .map(|(k, v)| (v.clone(), k.clone()))
        .collect();

    let client = Client::builder().build()?;
    let token = dev_login(&client, &base_url, &state.email).await?;

    let mut csv_w = csv::Writer::from_path(&out_path)?;
    csv_w.write_record([
        "query_id",
        "category",
        "expected_count",
        "search_top10",
        "search_recall_at_10",
        "search_latency_ms",
        "ask_cites",
        "ask_recall_at_10",
        "ask_latency_ms",
        "ask_input_tokens",
        "ask_output_tokens",
        "ask_cost_usd",
        "ask_skipped",
        "error",
    ])?;

    let mut total_search_recall = 0.0;
    let mut total_ask_recall = 0.0;
    let mut ask_attempted = 0usize;
    // p95 buckets — small enough to sort at the end rather than
    // running a streaming p95 estimator.
    let mut search_latencies: Vec<u64> = Vec::with_capacity(catalog.queries.len());
    let mut ask_latencies: Vec<u64> = Vec::with_capacity(catalog.queries.len());
    let mut total_ask_cost_usd = 0.0;
    let total = catalog.queries.len();

    for q in &catalog.queries {
        let search_start = std::time::Instant::now();
        let search_hits =
            run_search(&client, &base_url, &token, &q.text).await;
        let search_latency_ms = search_start.elapsed().as_millis() as u64;
        let (search_top10, search_err) = match search_hits {
            Ok(v) => (
                v.into_iter()
                    .filter_map(|id| server_to_catalog.get(&id).cloned())
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(e) => (Vec::new(), Some(format!("search: {e}"))),
        };
        let search_recall = recall_at_k(&search_top10, &q.expected_doc_ids);
        total_search_recall += search_recall;
        if search_err.is_none() {
            search_latencies.push(search_latency_ms);
        }

        let (ask_cites, ask_recall, ask_latency_ms, ask_input, ask_output,
             ask_cost, ask_skipped, ask_err) = if no_ask {
            (Vec::<String>::new(), 0.0, 0u64, 0u32, 0u32, 0.0, true, None)
        } else {
            let ask_start = std::time::Instant::now();
            let outcome = run_ask(&client, &base_url, &token, &q.text).await;
            let lat = ask_start.elapsed().as_millis() as u64;
            match outcome {
                AskOutcome::Skipped => {
                    (Vec::new(), 0.0, lat, 0, 0, 0.0, true, None)
                }
                AskOutcome::Ok(ok) => {
                    let mapped: Vec<String> = ok.sources
                        .into_iter()
                        .filter_map(|id| server_to_catalog.get(&id).cloned())
                        .collect();
                    let r = recall_at_k(&mapped, &q.expected_doc_ids);
                    let cost = claude_cost_usd(ok.input_tokens, ok.output_tokens);
                    total_ask_recall += r;
                    total_ask_cost_usd += cost;
                    ask_latencies.push(lat);
                    ask_attempted += 1;
                    (mapped, r, lat, ok.input_tokens, ok.output_tokens, cost, false, None)
                }
                AskOutcome::Err(msg) => {
                    (Vec::new(), 0.0, lat, 0, 0, 0.0, false, Some(format!("ask: {msg}")))
                }
            }
        };

        csv_w.write_record(&[
            q.id.clone(),
            category_str(q.category).to_string(),
            q.expected_doc_ids.len().to_string(),
            search_top10.join("|"),
            format!("{search_recall:.3}"),
            search_latency_ms.to_string(),
            ask_cites.join("|"),
            format!("{ask_recall:.3}"),
            ask_latency_ms.to_string(),
            ask_input.to_string(),
            ask_output.to_string(),
            format!("{ask_cost:.5}"),
            ask_skipped.to_string(),
            search_err.or(ask_err).unwrap_or_default(),
        ])?;

        println!(
            "  {:>6}  {:<12}  search={:.2} ({}ms)  ask={:.2} ({}ms, ${:.4}){}",
            q.id,
            category_str(q.category),
            search_recall,
            search_latency_ms,
            ask_recall,
            ask_latency_ms,
            ask_cost,
            if ask_skipped { "  (ask skipped)" } else { "" },
        );
    }

    csv_w.flush()?;
    println!();
    println!("Summary");
    println!("  queries:                {total}");
    println!(
        "  mean search recall@10:  {:.3}",
        total_search_recall / total as f64,
    );
    if ask_attempted > 0 {
        println!(
            "  mean ask recall@10:     {:.3} (n={ask_attempted})",
            total_ask_recall / ask_attempted as f64,
        );
        println!("  total Claude spend:     ${total_ask_cost_usd:.4}");
    } else {
        println!("  mean ask recall@10:     skipped");
    }
    if let Some(p95) = percentile(&mut search_latencies, 95) {
        println!("  search latency p95:     {p95} ms");
    }
    if let Some(p95) = percentile(&mut ask_latencies, 95) {
        println!("  ask latency p95:        {p95} ms");
    }
    println!("  CSV: {}", out_path.display());
    Ok(())
}

/// Naive p95 from a `Vec<u64>` — sorts in place and picks the
/// element at the `pct`-th percentile by linear interpolation
/// between bracketing indices. Returns `None` for an empty
/// input.
fn percentile(samples: &mut Vec<u64>, pct: u8) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let pct = pct.clamp(1, 99) as f64 / 100.0;
    let idx = (pct * (samples.len() - 1) as f64).round() as usize;
    Some(samples[idx])
}

fn category_str(c: QueryCategory) -> &'static str {
    match c {
        QueryCategory::Keyword => "keyword",
        QueryCategory::Semantic => "semantic",
        QueryCategory::MultiHop => "multi-hop",
        QueryCategory::GapAnalysis => "gap-analysis",
    }
}

async fn run_search(
    client: &Client,
    base_url: &str,
    token: &str,
    query: &str,
) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SearchHit {
        id: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SearchResponse {
        results: Vec<SearchHit>,
    }
    let encoded = urlencoding(query);
    let resp = client
        .get(format!("{base_url}/api/v1/search?q={encoded}&count=10"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /search")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("search returned {status}: {body}"));
    }
    let body: SearchResponse = resp.json().await?;
    Ok(body.results.into_iter().map(|h| h.id).collect())
}

struct AskOk {
    sources: Vec<String>,
    /// Sum of input tokens across every Claude call in the agent
    /// loop. 0 when the Usage SSE event didn't arrive (legacy
    /// backend before M-6.3 piece C).
    input_tokens: u32,
    output_tokens: u32,
}

enum AskOutcome {
    Ok(AskOk),
    /// Endpoint returned 503 (no API key configured) — score as
    /// no-data, not a failure.
    Skipped,
    Err(String),
}

async fn run_ask(
    client: &Client,
    base_url: &str,
    token: &str,
    query: &str,
) -> AskOutcome {
    let resp = match client
        .post(format!("{base_url}/api/v1/ask"))
        .bearer_auth(token)
        .header("accept", "text/event-stream")
        .json(&json!({ "question": query }))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return AskOutcome::Err(e.to_string()),
    };
    let status = resp.status();
    if status.as_u16() == 503 {
        return AskOutcome::Skipped;
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return AskOutcome::Err(format!("HTTP {status}: {body}"));
    }

    // Stream SSE frames; collect source doc_ids + usage totals;
    // stop on done.
    let mut stream = resp.bytes_stream();
    let mut buffer = BytesMut::with_capacity(4096);
    let mut sources: Vec<String> = Vec::new();
    let mut input_tokens: u32 = 0;
    let mut output_tokens: u32 = 0;
    while let Some(next) = stream.next().await {
        let chunk = match next {
            Ok(b) => b,
            Err(e) => return AskOutcome::Err(format!("stream: {e}")),
        };
        buffer.extend_from_slice(&chunk);
        // Frames separated by blank line.
        while let Some(idx) = find_double_newline(&buffer) {
            let frame_bytes = buffer.split_to(idx + 2);
            let frame_str = match std::str::from_utf8(&frame_bytes[..idx]) {
                Ok(s) => s,
                Err(_) => continue,
            };
            match parse_frame(frame_str) {
                Some(SseFrame::Source(doc_id)) => sources.push(doc_id),
                Some(SseFrame::Usage { input_tokens: i, output_tokens: o }) => {
                    input_tokens = i;
                    output_tokens = o;
                }
                Some(SseFrame::Done) => {
                    return AskOutcome::Ok(AskOk {
                        sources,
                        input_tokens,
                        output_tokens,
                    });
                }
                Some(SseFrame::Error(msg)) => return AskOutcome::Err(msg),
                _ => {}
            }
        }
    }
    AskOutcome::Ok(AskOk {
        sources,
        input_tokens,
        output_tokens,
    })
}

enum SseFrame {
    /// Backend's progress messages ("Thinking…", "Using tool: …").
    /// The runner doesn't score on them but parses them so the
    /// SSE stream advances; payload is dropped at the match site.
    #[allow(dead_code)]
    Status(String),
    /// Final-answer text chunks. Not used for recall scoring —
    /// recall is measured against `Source` events.
    #[allow(dead_code)]
    Text(String),
    Source(String),
    /// Per-question token totals summed across every Claude call
    /// in the agent loop. M-6.3 piece C addition. Runner converts
    /// to USD via the model's per-MTok rates.
    Usage { input_tokens: u32, output_tokens: u32 },
    Done,
    Error(String),
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

fn parse_frame(frame: &str) -> Option<SseFrame> {
    let mut event: Option<&str> = None;
    let mut data: Vec<&str> = Vec::new();
    for line in frame.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    let data_str = data.join("\n");
    match event? {
        "status" => Some(SseFrame::Status(data_str)),
        "text" => Some(SseFrame::Text(data_str)),
        "source" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct SrcPayload {
                doc_id: String,
            }
            serde_json::from_str::<SrcPayload>(&data_str)
                .ok()
                .map(|p| SseFrame::Source(p.doc_id))
        }
        "usage" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct UsagePayload {
                input_tokens: u32,
                output_tokens: u32,
            }
            serde_json::from_str::<UsagePayload>(&data_str)
                .ok()
                .map(|p| SseFrame::Usage {
                    input_tokens: p.input_tokens,
                    output_tokens: p.output_tokens,
                })
        }
        "done" => Some(SseFrame::Done),
        "error" => Some(SseFrame::Error(data_str)),
        _ => None,
    }
}

// ─── Claude pricing (M-6.3 piece C) ────────────────────────────
//
// Per-MTok rates for claude-sonnet-4-6 — Anthropic's published
// pricing as of 2026. Update when the rate card changes; the
// runner has no automated source for these. Used by the CSV
// `ask_cost_usd` column.
const CLAUDE_INPUT_PER_MTOK: f64 = 3.00;
const CLAUDE_OUTPUT_PER_MTOK: f64 = 15.00;

fn claude_cost_usd(input_tokens: u32, output_tokens: u32) -> f64 {
    let in_cost = (input_tokens as f64) * CLAUDE_INPUT_PER_MTOK / 1_000_000.0;
    let out_cost = (output_tokens as f64) * CLAUDE_OUTPUT_PER_MTOK / 1_000_000.0;
    in_cost + out_cost
}

fn urlencoding(s: &str) -> String {
    // Lightweight inline encoder — the runner only needs query
    // strings, not full URI escaping. Spaces → %20; everything
    // else passes through verbatim, which is fine for our ASCII
    // query text.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                out.push(ch);
            }
            ' ' => out.push_str("%20"),
            _ => {
                let mut b = [0u8; 4];
                let s = ch.encode_utf8(&mut b);
                for byte in s.bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    out
}
