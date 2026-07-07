# Agentic Search

> **Distilled pointer doc (#88).** The full design rationale for OgreNotes'
> retrieval-augmented, agentic query layer lives in the RAG documents and the
> `ask` route; this page is the landing point criterion #9
> expects, distilling the key decisions and
> pointing at the authoritative sources. Treat the linked docs as canonical on
> intent.

## What it is

Agentic search is the query side of OgreNotes' RAG stack: a Claude-driven layer
that answers natural-language questions over the workspace's documents
(`POST /api/v1/ask`). Rather than a single vector lookup, the agent plans
retrieval — fusing semantic (vector) recall, BM25 full-text search, and the
document **relationship graph** (see [`knowledge-graph.md`](knowledge-graph.md))
— then synthesizes a grounded answer with citations.

## Key decisions (distilled)

- **Hybrid retrieval, not pure vector.** Standard vector RAG underperforms on
  engineering/planning artifacts; OgreNotes fuses vector recall with the
  existing Tantivy BM25 index and graph-walk context. See
  `rag-architecture-steering.md` §§2–3 and `rag-implementation-plan.md`
  §"Architecture Overview".
- **Built on the existing Tantivy foundation** rather than replacing it — the
  semantic layer augments full-text search (`rag-implementation-plan.md`
  §"Current State" + Phase 6.2).
- **Agentic query layer** plans multi-step retrieval and reranks before
  answering (`rag-architecture-steering.md` §3.4–3.5,
  `rag-implementation-plan.md` §3.3).
- **Cost controls** live on the endpoint: per-user call counters + a global
  daily circuit breaker (BYOK to shift cost is tracked separately as #29).

## Canonical sources

- [`design/rag-architecture-steering.md`](rag-architecture-steering.md) — the
  architecture rationale (use case, why standard RAG is insufficient, the
  recommended hybrid + agentic design, indexing pipeline, taxonomy).
- [`design/rag-implementation-plan.md`](rag-implementation-plan.md) — the
  phased build plan (embeddings + semantic search, KG + agentic layer,
  validation/tuning) and the files each phase touches.
- **Code:** `crates/api/src/routes/ask.rs` — the `/api/v1/ask` endpoint and
  agentic query flow.
- **Ops:** `runbook/qdrant-operations.md`, `runbook/rag-eval-set.md`.
