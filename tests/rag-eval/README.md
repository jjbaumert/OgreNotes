# rag-eval

Phase 6 M-6.3 retrieval-quality evaluation harness for OgreNotes's
RAG stack (Tantivy + Qdrant + Claude agent).

## What lands when

| Piece | What |
|---|---|
| A | Crate skeleton + 50-query catalog + `summary` subcommand for sanity-checking the catalog. |
| B | `seed` + `run` subcommands: bootstrap the corpus on a clean test stack, iterate queries against `/search` and `/ask`, dump per-query recall@10 to CSV. |
| **C (this commit)** | Per-query latency + Claude token-cost capture; backend Usage SSE event carries token totals. |
| D | Tuning sweep across three knob configurations from the RAG plan §4.3 (chunk size 256/512/1024, RRF k 30/60/120, title boost 1.5/2.0/2.5). |
| E | `runbook/rag-eval-set.md` — operator recipe for adding queries / interpreting the CSV / re-running after a change. |

## Quick start

```bash
cd tests/rag-eval

# Catalog sanity check.
cargo run --bin rag-eval -- summary

# Seed the corpus on a running test stack. Idempotent — re-running
# deletes the prior corpus rows (matched by title prefix
# "[rag-eval] ") before re-seeding.
cargo run --bin rag-eval -- seed --base-url http://127.0.0.1:3000

# Execute every query, dump CSV. /ask half is skipped when the
# backend returns 503 (no ANTHROPIC_API_KEY); pass --no-ask to
# skip the /ask half entirely (saves API cost during corpus
# iteration).
cargo run --bin rag-eval -- run --base-url http://127.0.0.1:3000

# Same against the deployed test stack:
cargo run --bin rag-eval -- seed --base-url https://ogrenotes.example.com
cargo run --bin rag-eval -- run --base-url https://ogrenotes.example.com
```

## CSV output

`rag-eval-results.csv` columns:

| Column | Meaning |
|---|---|
| `query_id`              | Stable query id from the catalog (e.g. `kw-03`). |
| `category`              | `keyword` / `semantic` / `multi-hop` / `gap-analysis`. |
| `expected_count`        | Length of the query's `expectedDocIds` array. |
| `search_top10`          | Pipe-joined catalog ids the keyword search surfaced. |
| `search_recall_at_10`   | `\|retrieved ∩ expected\| / \|expected\|`. Gap queries score 1.0 on empty-and-empty, 0.0 on retrieving a false cite. |
| `search_latency_ms`     | Wall-clock time for the `/search` round trip. |
| `ask_cites`             | Pipe-joined catalog ids the agent cited as Source events. |
| `ask_recall_at_10`      | Recall@10 against the agent citations. |
| `ask_latency_ms`        | Wall-clock time for the `/ask` SSE round trip end-to-end. |
| `ask_input_tokens`      | Input tokens summed across every Claude call in the agent loop (from the Usage SSE event). 0 when the backend predates M-6.3 piece C. |
| `ask_output_tokens`     | Output tokens, same summing rule. |
| `ask_cost_usd`          | Computed cost in USD using `CLAUDE_INPUT_PER_MTOK` / `CLAUDE_OUTPUT_PER_MTOK` constants in `main.rs`. Update those when Anthropic changes the rate card. |
| `ask_skipped`           | `true` when the /ask call was skipped (no ANTHROPIC_API_KEY or `--no-ask`). |
| `error`                 | Search / ask error text if either path failed. |

Summary stats printed to stdout: mean recall (search + ask),
total Claude spend across all queries, and search/ask p95
latency in ms.

Piece D adds tuning-knob columns to support the chunk-size /
RRF-k / title-boost sweep.

## Catalog shape

`queries.json` at the crate root carries two arrays:

- `corpus` — seed documents. Each entry has a stable string id
  (e.g. `"auth-design"`) plus a `title` and `content` markdown
  body. The runner (piece B) maps these to server-generated doc
  ids at seed time.
- `queries` — 50 evaluation queries:
  - 10 `keyword` (exact-term lookups, identifier matches)
  - 15 `semantic` (natural-language rephrasing)
  - 15 `multi-hop` (cross-doc traversal)
  - 10 `gap-analysis` (no expected results — "what's missing")

Each query's `expectedDocIds` is the recall ground truth. Empty
for gap-analysis queries; one or more catalog ids otherwise.

## Cross-run trend comparison

Query ids (`kw-01`, `sem-04`, etc.) are stable. Piece B's CSV
output uses them as row keys so a recall regression on a
specific id is easy to spot across runs. Don't renumber an
existing query — append a new id instead.

## Overriding the catalog path

Set `RAG_EVAL_CATALOG=/path/to/queries.json` to load a custom
catalog. Useful for one-off tuning runs against a fork of the
canonical set, or for testing the runner against a smaller
sanity-check catalog.
