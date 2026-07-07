# RAG evaluation set runbook

Phase 6 M-6.3 piece E. Operator-facing recipes for the 50-query
retrieval-quality eval harness at `tests/rag-eval/`. Covers the
common day-to-day questions:

- How do I run the eval against a stack?
- What's a normal-looking result?
- When should I re-run?
- How do I add a new query?
- What about the deferred tuning-sweep work?

The harness ships as a workspace-excluded Rust crate. Reference
docs at `tests/rag-eval/README.md` (catalog shape + CSV columns);
this runbook focuses on the operator-side workflow.

## Quick start

```bash
cd tests/rag-eval

# 1. Sanity-check the catalog without touching a backend.
cargo run --bin rag-eval -- summary

# 2. Seed the corpus on a running stack. Idempotent — rerunning
#    deletes the prior corpus (matched by title prefix
#    "[rag-eval] ") before reseeding.
cargo run --bin rag-eval -- seed \
    --base-url http://127.0.0.1:3000

# 3. Run every query, dump CSV. Use --no-ask to skip the /ask
#    half (saves Claude API cost during corpus iteration).
cargo run --bin rag-eval -- run \
    --base-url http://127.0.0.1:3000

# Same against the deployed test stack:
cargo run --bin rag-eval -- seed --base-url https://ogrenotes.example.com
cargo run --bin rag-eval -- run  --base-url https://ogrenotes.example.com
```

## What a normal-looking result looks like

The eval prints per-query stdout while it runs:

```
  kw-01  keyword       search=1.00 (45ms)  ask=1.00 (3120ms, $0.0042)
  kw-02  keyword       search=1.00 (38ms)  ask=1.00 (2890ms, $0.0038)
  ...
  sem-04 semantic      search=1.00 (52ms)  ask=1.00 (3450ms, $0.0051)
  ...
  gap-01 gap-analysis  search=1.00 (41ms)  ask=1.00 (1780ms, $0.0019)
```

Then a summary:

```
Summary
  queries:                50
  mean search recall@10:  0.94
  mean ask recall@10:     0.91 (n=50)
  total Claude spend:     $0.18
  search latency p95:     72 ms
  ask latency p95:        4820 ms
  CSV: rag-eval-results.csv
```

**Expected ranges** (rough, no production baseline yet):

| Metric | Typical |
|---|---|
| Mean search recall@10 | 0.85–0.95 |
| Mean ask recall@10 | 0.80–0.95 |
| Total run cost | $0.10–0.30 |
| Search p95 latency | 50–150 ms |
| Ask p95 latency | 3–8 s |

Anything substantially below these and the harness is detecting
a regression worth investigating. The CSV's per-query rows
identify which categories drove the drop.

## When to re-run

- **After every PR that touches**: `crates/search/`, `crates/embeddings/`,
  `crates/api/src/routes/{search,ask}.rs`, `crates/api/src/claude.rs`,
  the agent system prompt in `routes/ask.rs::build_system_prompt`,
  or the chunker config defaults.
- **After a Bedrock model swap** (e.g. Titan v2 → v3 or Cohere).
  Different embedding model = different recall on the same queries.
- **After a Claude model swap** (e.g. claude-sonnet-4-6 →
  claude-sonnet-5). Update `ANTHROPIC_MODEL` first, then re-run.
  Bonus: update `CLAUDE_INPUT_PER_MTOK` / `CLAUDE_OUTPUT_PER_MTOK`
  in `tests/rag-eval/src/main.rs` if the new model's pricing
  differs.
- **Quarterly cadence** against the deployed test stack as a
  drift check, even without a code change. Bedrock + Claude
  models evolve under the hood; an unchanged stack can score
  differently month-over-month.

Not necessary for: frontend-only PRs, infra PRs that don't touch
the data path, documentation changes.

## Reading the CSV

`rag-eval-results.csv` has one row per query and the columns
documented in `tests/rag-eval/README.md`. The first investigation
moves when something looks off:

1. **Mean recall dropped.** Open the CSV; sort by
   `search_recall_at_10` ascending. Look at the bottom 5 rows —
   the categories they belong to point at where the regression
   lives. A `keyword` recall drop usually means a Tantivy
   indexing or query-parser change; a `semantic` drop usually
   means Qdrant / embedding-model; a `multi-hop` drop usually
   means the agent's tool-use loop or the get_related path.

2. **Cost jumped.** Sort by `ask_cost_usd` desc. The agent
   loop has a 5-round ceiling (`MAX_TOOL_ROUNDS`); a per-query
   cost above $0.02 hints the agent's making more rounds than
   the question needs. Check `ask_input_tokens` — high input
   typically means the agent is re-feeding the same context
   to Claude across rounds, which is a system-prompt issue.

3. **Latency jumped.** `ask_latency_ms` includes the full
   agent loop end-to-end. p95 above 8 s is worth a look — the
   RAG plan §4.2 budgets 10 s but anything sustained beyond
   5 s is a degraded experience. Check for n+1 tool calls
   (per-query trace in CloudWatch logs).

4. **Gap-analysis recall dropped.** Gap queries expect an
   empty cite list; a recall of 0.0 on a gap row means the
   agent hallucinated a citation. Open the row's `ask_cites`
   to see what got falsely cited; the system prompt's
   "don't cite if you can't find a relevant doc" line may
   need strengthening.

## Adding a new query

1. Pick a stable id that doesn't collide with the catalog
   (the convention: `<category-prefix>-<NN>`, where prefixes
   are `kw`, `sem`, `mh`, `gap`).
2. Add an entry to `queries.json::queries`:
   ```jsonc
   {
     "id": "sem-16",
     "category": "semantic",
     "text": "Your question",
     "expectedDocIds": ["existing-corpus-id"]
   }
   ```
3. If the query depends on a doc the corpus doesn't cover
   yet, add a new `corpus` entry first (with a stable string
   id). Then reference it.
4. Run `cargo test --lib` in `tests/rag-eval/` — the validate
   test catches dangling `expectedDocIds` at compile time.
5. Re-seed (`rag-eval seed`) so the new corpus entry exists
   on the stack you're evaluating against.

**Don't renumber existing entries.** Stable ids are the CSV's
row key; renumbering invalidates trend comparison across runs.

## Cost-conscious runs

The /ask half of each query costs $0.001–$0.005 of Claude
spend (claude-sonnet-4-6 at current rates). Full eval = ~$0.10–
$0.30. Two flags help:

- `--no-ask` skips the /ask half entirely. Use when iterating
  on the corpus or query set — keyword recall is enough signal
  to verify a new query lands.
- The runner skips /ask automatically when the backend returns
  503 (no `ANTHROPIC_API_KEY` configured). The CSV's
  `ask_skipped` column reads `true` for those rows; recall
  averages drop the unattempted rows from the denominator.

## Pre-deploy regression check (suggested cadence)

Before merging to `main`:

```bash
# Against the local dev stack (cheap path):
cargo run --bin rag-eval -- run --base-url http://127.0.0.1:3000

# Or against the deployed test stack:
cargo run --bin rag-eval -- run --base-url https://ogrenotes.example.com
```

Compare `mean search recall@10` and `mean ask recall@10` to
the prior run. A drop of more than 0.05 (absolute) on either is
worth a deeper look before merging. If the drop is concentrated
in one category, the CSV's per-query rows show which queries
broke.

## Tuning knobs — current state and the deferred sweep

M-6.3 piece D was scoped for a chunk-
size / RRF-k / title-boost sweep. Deferred to v2 per the audit
at piece-D start; this section documents the current values
and what's needed for tuning to land.

| Knob | Current value | Where it lives |
|---|---|---|
| Chunk size | 2048 chars (≈512 tokens) | `crates/embeddings/src/chunker.rs::ChunkerConfig::default` |
| Chunk overlap | 204 chars (≈10%) | same file |
| Title BM25 boost | `2.0` | `crates/search/src/lib.rs::search` (set on the QueryParser) |
| RRF k | `60.0` | `const K` in `crates/api/src/routes/search.rs` |
| Embedding model | `amazon.titan-embed-text-v2:0` | `EMBEDDING_MODEL_ID` env var, default in `common::config` |
| Embedding dimensions | 1024 | `EMBEDDING_DIMENSIONS` env var, default in `common::config` |
| Claude model | `claude-sonnet-4-6` | `ANTHROPIC_MODEL` env var, default in `common::config` |
| `MAX_TOOL_ROUNDS` | 5 | `crates/api/src/routes/ask.rs` |

**Why we deferred the sweep:** chunk size and title boost
aren't runtime-swappable today (chunk size requires a full
re-embed of the corpus; title boost requires the API to
reopen the Tantivy parser). Only RRF k is a true runtime
constant. The deferred plumbing (the v2
carry-forwards) is the cost of doing the sweep properly.

**Lightweight knob nudge in the meantime:** the embedding
model + Claude model + token-cost-per-MTok constants are all
env-var-driven (or single-line code constants); changing any
of them is a fast iteration loop. The other four require
backend changes.

## Common failures

### `seed` says "dev-login returned 401"

The backend doesn't have `DEV_MODE=true` set. Either flip it
on the task definition + redeploy, or run the seed against a
local stack started with `DEV_MODE=true` in the env. Don't
turn `DEV_MODE` on in a production stack — see the warning in
the deploy script.

### `run` says "seed state was for X but --base-url is Y"

You're targeting a different stack than the one you seeded.
Either:
- `rag-eval run --base-url X` (use the stack you actually seeded), or
- `rag-eval seed --base-url Y` first (re-seed against the new stack).

The state mismatch refusal is intentional — running queries
against an unseeded stack just measures the eval user's prior
docs, which is noise.

### Every /ask row shows `ask_skipped=true`

The backend's `ANTHROPIC_API_KEY` isn't set. Either:
- Set it via SSM (per `runbook/qdrant-operations.md` — same SSM
  parameter as the production agent), and redeploy.
- Run with `--no-ask` to score search recall only.

### Recall@10 for keyword queries is suddenly 0

Tantivy index is likely empty or stale. Symptoms:
- `/api/v1/search?q=<known-doc-title>` returns no results.
- Recent ECS task restart history shows the API task replaced
  recently — the search index lives on the task's local
  filesystem (`/data/search-index`), so a task restart with a
  fresh ephemeral filesystem starts with an empty index. The
  search index repopulates as the indexing-hooks fire on doc
  saves; until then, recall is 0.

Fix: trigger an index rebuild by re-saving each catalog doc.
The simplest path: `rag-eval seed` again (the delete + create
cycle exercises the indexing hooks).

## v2 carry-forwards

Tracked in the v2 carry-forwards section.
The short list:

- Configurable chunk size + overlap (env-driven, requires
  re-index orchestrator).
- Configurable RRF k (env-driven, runtime-swappable).
- Configurable title boost (config plumbing + parser
  re-construction).
- Re-index orchestrator script (or runner subcommand).
- Production-data baseline + tuning sweep (when the prereqs
  land).
