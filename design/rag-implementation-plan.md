# RAG Implementation Plan for OgreNotes
## Building on the Existing Tantivy Search Foundation

**Status:** Draft
**Date:** 2026-04-12
**Prereqs:** Phase 6.1 (BM25 full-text search via Tantivy) is complete and deployed

> **Numbering note (2026-05-05):** project Phase 6 = RAG / agentic
> search. Internal sub-phases below are numbered **Phase 6.2 → 6.4**
> (with Phase 6.1 being the already-shipped Tantivy foundation in
> `rag-architecture-steering.md` §9). Older revisions of this doc
> labeled them "Phase 2 / 3 / 4" — the renumbering is editorial.

---

## Current State

OgreNotes already has a working search stack:

- **`crates/search`** — Tantivy-based BM25 index with title (2x boost) + body fields, doc_type/owner_id/folder_id filters, snippet generation with HTML highlighting
- **`GET /api/v1/search`** — Permission-filtered endpoint with over-fetch strategy, debounced frontend dialog (Ctrl+K)
- **Indexing hooks** — Fire-and-forget re-indexing on document create, update, delete, content save, import, and version restore
- **Text extraction** — `to_plain_text()` extracts searchable text from Y.Doc CRDT content
- **Infrastructure** — DynamoDB (metadata), S3 (snapshots), Redis (pub/sub), ECS Fargate deployment

The RAG architecture steering doc (`design/rag-architecture-steering.md`) proposes a four-layer system. This plan adapts it to build incrementally on what exists, using OgreNotes' actual infrastructure rather than introducing new external services prematurely.

---

## Architecture Overview

```
User query
    │
    ├── Keyword-shaped? (short, identifiers, known terms)
    │       └── Tantivy BM25 → permission filter → return     [EXISTS]
    │
    └── Natural language / conceptual / multi-hop?
            └── Agentic loop:
                  ├── tool: keyword_search  → Tantivy BM25     [EXISTS]
                  ├── tool: semantic_search → vector index      [PHASE 6.2]
                  ├── tool: get_document    → S3 + DynamoDB     [EXISTS as API]
                  ├── tool: get_related     → graph store       [PHASE 6.3]
                  └── rerank → synthesize → respond             [PHASE 6.3]
```

### Key Decisions vs. the Steering Doc

| Steering Doc Proposes | This Plan Does | Why |
|---|---|---|
| Neo4j for graph store | DynamoDB adjacency list | Already deployed; avoids new managed service. Neo4j can be added later when graph queries outgrow DynamoDB. |
| pgvector or Qdrant for vectors | Qdrant (self-hosted on ECS) or Bedrock Knowledge Base | pgvector requires Postgres (not in stack). Qdrant is a single container. Bedrock is zero-ops. |
| Elasticsearch or Tantivy for BM25 | Tantivy (already built) | Done. |
| LightRAG framework | Custom tool-calling loop with Claude | LightRAG is Python. OgreNotes is Rust. A thin Rust agent loop calling Claude's tool-use API is simpler and avoids a Python sidecar. |
| LangGraph / LlamaIndex for agent | Claude tool-use directly | Same reasoning — stay in Rust, avoid Python runtime. |

---

## Phase 6.2 — Vector Embeddings + Semantic Search (Weeks 1–4)

### Goal
Add vector-based semantic search so queries like "how does the auth system handle expired sessions" find relevant documents even when they don't contain those exact keywords.

### 2.1 Embedding Pipeline

**New crate: `crates/embeddings`**

Responsibilities:
- Chunk documents into ~512-token segments with 10% overlap
- Prepend contextual header per chunk (doc title, type, section — lightweight, no LLM call initially)
- Call embedding API (Amazon Bedrock `amazon.titan-embed-text-v2` or Anthropic/Cohere via API) to generate vectors
- Store vectors with metadata (doc_id, chunk_index, chunk_text)

**Chunking strategy:**
```
Document Y.Doc
    → to_plain_text() [exists]
    → split into ~512-token chunks with 50-token overlap
    → prepend header: "[{title} | {doc_type} | chunk {n}/{total}]\n"
    → embed each chunk → store vector + metadata
```

**Embedding model choice:**
- **Amazon Bedrock Titan Embed v2** — if staying AWS-native, zero-ops, pay-per-call
- **Cohere embed-english-v3** — higher quality, available via API
- **Open-source (BGE-large, E5)** — self-hosted on ECS if cost or data residency matters

Start with Bedrock Titan (simplest ops) and swap later if retrieval quality is insufficient.

### 2.2 Vector Store

**Option A: Qdrant on ECS** (recommended for flexibility)
- Single Qdrant container as an ECS service, EFS-backed for persistence
- REST API from Rust via `qdrant-client` crate
- Supports filtering by doc_id, doc_type, owner_id at query time
- Cost: ~$15/month on Fargate Spot for small corpus

**Option B: Amazon Bedrock Knowledge Base** (recommended for zero-ops)
- Managed service: S3 source → automatic chunking + embedding → OpenSearch Serverless
- Zero infrastructure to manage
- Less control over chunking/embedding strategy
- Cost: pay-per-query + OpenSearch Serverless baseline

**Recommendation:** Start with Qdrant on ECS. It gives full control over the embedding and retrieval pipeline, and aligns with the Rust-native approach. If ops burden grows, migrate to Bedrock KB.

### 2.3 Indexing Hooks

Extend the existing fire-and-forget pattern in `crates/api/src/routes/documents.rs`:

```
Document mutation (create/update/delete/import/restore)
    ├── Tantivy index update     [exists]
    └── Embedding pipeline       [new, async background task]
         ├── Extract plain text
         ├── Chunk + prepend context
         ├── Call embedding API
         └── Upsert vectors to Qdrant
```

For deletes: remove all vectors with matching `doc_id` from Qdrant.

### 2.4 Hybrid Search Endpoint

Extend `GET /api/v1/search` (or add `GET /api/v1/search/semantic`):

```
1. Run Tantivy BM25 query           → top 20 keyword hits
2. Run Qdrant vector query           → top 20 semantic hits
3. Merge + deduplicate by doc_id
4. Permission-filter via check_doc_access
5. Return fused results
```

**Fusion strategy:** Reciprocal Rank Fusion (RRF) — simple, parameter-free, well-studied. Each result gets score `1/(k + rank)` from each retriever, scores are summed across retrievers.

### 2.5 Files to Create/Modify

| File | Action |
|------|--------|
| `crates/embeddings/Cargo.toml` | Create — new crate |
| `crates/embeddings/src/lib.rs` | Create — chunking, embedding client, vector store client |
| `Cargo.toml` | Modify — add embeddings to workspace |
| `crates/api/Cargo.toml` | Modify — add embeddings dep |
| `crates/api/src/state.rs` | Modify — add embedding pipeline handle |
| `crates/api/src/routes/documents.rs` | Modify — add embedding indexing hooks |
| `crates/api/src/routes/search.rs` | Modify — add hybrid search mode |
| `crates/common/src/config.rs` | Modify — add qdrant_url, embedding_model config |

---

## Phase 6.3 — Knowledge Graph + Agentic Layer (Weeks 5–8)

### Goal
Add document relationship tracking and an LLM-powered agent that can reason across documents, follow relationships, and answer multi-hop questions.

### 3.1 Document Relationship Graph

**Storage: DynamoDB adjacency list** (no new service)

Store relationships as DynamoDB items alongside existing document data:

```
PK: DOC#<doc_id>    SK: REL#<relation_type>#<target_doc_id>
Attributes: relation_type, target_doc_id, created_at, created_by
```

**Relationship types** (from steering doc):
- `implements` — design implements requirement
- `derived-from` — subdesign derived from parent design
- `depends-on` — document depends on another
- `references` — document cites another
- `supersedes` — newer document supersedes older

**How relationships are created:**
1. **Manual** — UI button "Link to document" (simplest, most accurate)
2. **LLM-extracted** — During embedding pipeline, ask Claude Haiku to identify references to other documents. Store extracted relationships with `source: "auto"` flag.
3. **Folder hierarchy** — Documents in the same folder implicitly share a `sibling` relationship via folder_id (already indexed in Tantivy).

### 3.2 Contextual Chunk Enrichment (Upgraded)

Upgrade the Phase 6.2 plain-text header to an LLM-generated context block:

```
Before embedding:
  [Authentication Subsystem Design | Design Document |
   Implements: token refresh, session management |
   Related: System Architecture v2, Auth Requirements]

  <chunk content>
```

**Cost control:** Use Claude Haiku ($0.25/MTok input, $1.25/MTok output). For a 10-page document (~4K tokens), context generation costs ~$0.01 per document. At 1000 documents, total enrichment cost is ~$10.

### 3.3 Agentic Query Endpoint

**New endpoint: `POST /api/v1/ask`**

```json
{
  "question": "What requirements are not yet addressed in any design document?",
  "mode": "agentic"
}
```

**Implementation: Rust tool-calling loop with Claude**

```rust
loop {
    let response = claude_api.messages_create(
        model: "claude-sonnet-4-6",
        system: AGENT_SYSTEM_PROMPT,
        messages: conversation,
        tools: [keyword_search, semantic_search, get_document, get_related, list_documents],
    ).await;

    match response {
        ToolUse(calls) => {
            // Execute tool calls, append results to conversation
            for call in calls {
                let result = execute_tool(call, &state).await;
                conversation.push(tool_result(result));
            }
        }
        TextResponse(answer) => {
            return answer; // Agent is done reasoning
        }
    }
}
```

**Agent tools** (all permission-filtered):

| Tool | Backed By | Exists? |
|---|---|---|
| `keyword_search(query, filters)` | Tantivy BM25 | Yes |
| `semantic_search(query, doc_type)` | Qdrant vectors | Phase 6.2 |
| `get_document(doc_id)` | S3 + DynamoDB | Yes (as API) |
| `get_related(doc_id, relation_type)` | DynamoDB adjacency list | Phase 6.3 |
| `list_documents(type, folder_id)` | DynamoDB GSI query | Yes (as API) |

**Permission enforcement:** Every tool call filters through `check_doc_access`. The agent never sees documents the user can't access.

**Streaming:** Use Claude's streaming API + SSE to stream the agent's reasoning and final answer to the frontend in real-time.

### 3.4 Reranking

After the agent's retrieval tools return candidates, apply a reranking step before synthesis:

- **Option A: Cohere Rerank API** — hosted, simple, ~200ms latency
- **Option B: BGE Reranker on ECS** — self-hosted, ~100ms, no external dependency
- **Option C: Claude itself** — pass candidates to Claude with "rank these by relevance" prompt (simplest, uses existing API, but slower)

Start with Option C (no new service) and upgrade to Cohere if latency matters.

### 3.5 Files to Create/Modify

| File | Action |
|------|--------|
| `crates/api/src/routes/ask.rs` | Create — agentic query endpoint |
| `crates/api/src/routes/mod.rs` | Modify — register /ask route |
| `crates/storage/src/models/document.rs` | Modify — add relationship model |
| `crates/storage/src/repo/doc_repo.rs` | Modify — add relationship CRUD |
| `frontend/src/api/ask.rs` | Create — ask API wrapper |
| `frontend/src/components/ask_dialog.rs` | Create — agentic chat UI |

---

## Phase 6.4 — Validation & Tuning (Weeks 9–10)

### 4.1 Evaluation Set

Build a set of 50 representative queries spanning:
- Keyword lookups (10) — exact term matches, requirement IDs
- Semantic queries (15) — conceptual questions about document content
- Multi-hop queries (15) — questions requiring relationship traversal
- Gap analysis queries (10) — "what's missing" style questions

### 4.2 Metrics

| Metric | Target | How Measured |
|---|---|---|
| Keyword search latency | < 50ms | Timer in search handler |
| Semantic search latency | < 500ms | Timer including embedding + Qdrant |
| Agent response time | < 10s | End-to-end for agentic queries |
| Retrieval recall@10 | > 80% | Evaluation set with known-relevant docs |
| Permission correctness | 100% | Automated test: user B never sees user A's private docs |

### 4.3 Tuning Knobs

- Chunk size (256 / 512 / 1024 tokens)
- Overlap percentage (0% / 10% / 20%)
- BM25 title boost factor (currently 2.0)
- RRF k parameter (typically 60)
- Embedding model (Titan vs. Cohere vs. BGE)
- Context block prompt
- Agent system prompt
- Number of retrieval rounds before synthesis

---

## Infrastructure Cost Estimate

| Component | Service | Monthly Cost |
|---|---|---|
| Tantivy BM25 | In-process (ECS task) | $0 (existing) |
| Qdrant | ECS Fargate Spot + EFS | ~$15 |
| Embeddings | Bedrock Titan Embed | ~$5 (at 10K queries/month) |
| Agent LLM | Claude Sonnet via API | ~$20 (at 1K agentic queries/month) |
| Context enrichment | Claude Haiku via API | ~$10 (one-time for 1K docs) |
| **Total incremental** | | **~$50/month** |

This is on top of the existing ~$50–80/month base infrastructure (ECS, Redis, DynamoDB, S3).

---

## Migration Path from Steering Doc

The steering doc proposes LightRAG + Neo4j + LangGraph. This plan deliberately avoids those for v1 to minimize new infrastructure and stay Rust-native. The migration path if needed:

1. **DynamoDB graph → Neo4j**: If relationship queries become complex (multi-hop with weighted traversal, community detection), export the adjacency list to Neo4j. The tool interface (`get_related`) stays the same.
2. **Qdrant → pgvector**: If Postgres is added for other reasons, migrate vectors there. Same embedding pipeline, different store.
3. **Custom agent loop → LangGraph**: If the agent needs complex branching/state machines beyond a simple tool-calling loop, introduce a Python sidecar running LangGraph. The Rust API calls it via HTTP.
4. **Titan Embed → Cohere/BGE**: Swap embedding model via config change. Re-embed corpus (batch job).

None of these migrations require changing the API contract or the frontend.

---

## Summary of Phases

| Phase | Weeks | Deliverable | New Services |
|---|---|---|---|
| **1** (done) | — | BM25 keyword search + UI | Tantivy (in-process) |
| **2** | 1–4 | Vector embeddings + hybrid search | Qdrant (ECS), Bedrock Titan |
| **3** | 5–8 | Knowledge graph + agentic Q&A | Claude API |
| **4** | 9–10 | Evaluation, tuning, production hardening | — |
