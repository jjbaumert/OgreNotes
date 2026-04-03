# RAG Architecture Steering Document
## Intelligent Document Retrieval for Engineering & Planning Artifacts

**Version:** 1.0  
**Status:** Draft  
**Purpose:** Architectural guidance for implementing graph-enhanced retrieval-augmented generation (RAG) across the project document corpus

---

## 1. Use Case

This project maintains a corpus of interrelated engineering and planning documents including:

- **Requirements documents** — functional and non-functional specifications that define what must be built
- **Design documents** — architectural and system-level designs that describe how requirements are implemented
- **Subdesign documents** — component-level or subsystem-level elaborations derived from parent design documents
- **Planning documents** — project plans, roadmaps, and scheduling artifacts that reference requirements and designs
- **Decision records** — records of architectural and design decisions and their rationale

These documents do not exist in isolation. A requirements document traces forward to one or more design documents. A design document spawns subdesign documents that elaborate specific components. Planning documents reference both requirements and designs. Decision records are anchored to specific requirements or design choices. This web of relationships is not incidental — it is the semantic structure of the corpus, and any retrieval system must be able to surface and traverse it.

### Interaction Modes

The system must support two distinct interaction patterns:

**Direct Search** — A user knows a specific term, identifier, component name, or keyword and wants to find all documents that reference it. This must be fast, precise, and require no LLM involvement. Examples: searching for a specific requirement ID, a component name, an author, or a decision keyword.

**Agentic Exploration** — A user has a higher-level question that requires reasoning across multiple documents. The system should iteratively retrieve relevant context, follow relationships between documents, and synthesize an answer. Examples: "What are the unresolved dependencies for the authentication subsystem?", "Which requirements are not yet addressed in any design document?", "What planning assumptions were made based on the Q2 design review?"

Both modes must coexist in the same system and share the same underlying index.

---

## 2. Why Standard Vector RAG Is Insufficient

Standard retrieval-augmented generation — embedding document chunks and retrieving by cosine similarity — addresses the semantic search problem but falls short for this corpus in three important ways:

**It does not model relationships.** Embedding-based retrieval finds chunks that are semantically similar to a query. It does not know that Document A is a subdesign of Document B, or that Requirement R-42 is implemented by Design D-07. These structural relationships require explicit representation.

**It loses document-level context at the chunk level.** A chunk extracted from the middle of a design document loses its context — which system it describes, which requirement it implements, where it sits in the document hierarchy. Without that context, retrieval accuracy degrades for queries that depend on understanding the document's role in the corpus.

**It cannot handle multi-hop reasoning.** Answering "why was this architectural decision made?" may require retrieving the decision record, then the requirement it addresses, then the design constraints that shaped it. Standard RAG retrieves in one pass and cannot follow that chain.

---

## 3. Recommended Architecture

The recommended architecture is a layered system with four components: a graph-enhanced hybrid retrieval index, a full-text keyword search index, contextual chunk enrichment at indexing time, and an agentic query layer for complex multi-document reasoning. Each layer is independently replaceable.

### 3.1 Primary Retrieval: LightRAG with Hybrid Mode

**Technology:** [LightRAG](https://github.com/HKUDS/LightRAG)  
**Storage Backend:** Neo4j (preferred for production) or Postgres with pgvector

LightRAG builds a knowledge graph from your document corpus by extracting entities and relationships during indexing. For this corpus, entities include components, systems, subsystems, requirements, design decisions, and authors. Relationships include implements, refines, depends-on, supersedes, derived-from, and references.

At query time, LightRAG's hybrid mode combines vector similarity search with graph traversal. A query for "authentication token refresh" will find semantically similar chunks via vector search and also traverse the graph to surface related entities — the requirement that specifies the behavior, the design document that describes the implementation, and any subdesign documents that elaborate specific aspects.

**Why LightRAG over Microsoft GraphRAG:**  
GraphRAG produces richer global synthesis (thematic summaries across an entire corpus) but is expensive to index, difficult to update incrementally, and requires more infrastructure. LightRAG offers:

- Incremental document updates without full re-indexing — essential for a living document corpus
- Multiple storage backends that fit existing infrastructure
- Hybrid query mode that handles both exploratory and precise queries from a single system
- Simpler operational footprint

LightRAG's global synthesis is less powerful than GraphRAG's community-detection approach, but is sufficient for the majority of planning and decision-support queries in this use case.

**Query Modes to Expose:**

| Mode | Use When |
|---|---|
| `hybrid` | Default for all agentic queries — combines vector and graph |
| `local` | Entity-specific questions where graph traversal from a known node is sufficient |
| `global` | Thematic queries across the entire corpus ("what are the major open questions?") |
| `naive` | Fallback to pure vector search when graph traversal is not needed |

### 3.2 Supplementary Retrieval: BM25 Full-Text Search

**Technology:** Elasticsearch, OpenSearch, or Tantivy (embedded)

A BM25 full-text index runs in parallel with LightRAG for direct keyword searches. This index handles:

- Exact term matches (requirement IDs, component names, specific identifiers)
- Prefix and wildcard searches
- Boolean queries ("authentication AND token AND NOT refresh")
- Author and metadata filtering

BM25 search bypasses the LLM entirely for direct search queries, returning results in milliseconds. It should be the first index called for any query that looks like a keyword lookup (short queries, identifiers, known terms). The agentic layer can also invoke it as a tool when it needs precise term matching within a reasoning loop.

**Index fields to maintain:**

- Document title and metadata (type, author, date, status, parent document ID)
- Full document text
- Extracted entity names and IDs
- Requirement IDs, component names, and other structured identifiers parsed from document content

### 3.3 Contextual Chunk Enrichment

**Technique:** Anthropic Contextual Retrieval  
**Applied at:** Indexing time, before embedding

Before embedding each document chunk, prepend a short generated context block that situates the chunk within its document and the broader corpus. This context travels with the chunk into the vector index and significantly improves retrieval accuracy for chunks that would otherwise be ambiguous in isolation.

**Context block format (generated per chunk at index time):**

```
[Document: Authentication Subsystem Design | Type: Design Document | 
Parent: System Architecture Design v2.3 | Implements: REQ-AUTH-001 through REQ-AUTH-018 | 
Section: Token Refresh Flow]

<chunk content>
```

This context is generated once per chunk using a lightweight LLM call during indexing. The cost is a one-time indexing expense that pays dividends in retrieval quality for the lifetime of the index.

**Implementation note:** Generate context blocks using a small, fast model (e.g., Claude Haiku) to minimize indexing cost. The context prompt should be:

```
You are indexing a chunk from a document corpus of engineering and planning documents.
Given the document title, type, and the chunk below, write a single short paragraph 
(2-3 sentences) that describes what this chunk is about and where it fits in the document.
Include the document type, the system or component it covers, and any requirement or 
design IDs mentioned. Output only the context paragraph.
```

### 3.4 Agentic Query Layer

**Frameworks:** LangGraph, LlamaIndex Workflows, or direct tool-calling with Claude  
**Invoked for:** Complex, multi-document reasoning queries

The agentic layer wraps the retrieval system in an LLM reasoning loop. The agent is given a set of retrieval tools and decides how to use them to answer the user's question. For simple queries, the agent may make a single retrieval call. For complex queries, it may plan a multi-step retrieval strategy, refine queries based on intermediate results, and synthesize across many documents.

**Agent Tools:**

| Tool | Description |
|---|---|
| `keyword_search(query, filters)` | BM25 full-text search with optional metadata filters |
| `semantic_search(query, mode)` | LightRAG hybrid/local/global search |
| `get_document(doc_id)` | Retrieve a full document by ID |
| `get_related(doc_id, relation_type)` | Traverse the graph from a known document — get parents, children, implementations, etc. |
| `list_documents(type, status, author)` | List documents matching metadata criteria |

**Agent System Prompt (excerpt):**

```
You are a document retrieval assistant for an engineering project. The corpus contains 
requirements documents, design documents, subdesign documents, planning documents, and 
decision records. Documents are related: requirements are implemented by designs; 
designs have subdesigns; planning documents reference both.

When answering a question:
1. Identify what type of documents are likely to contain the answer.
2. Use keyword_search for precise terms or identifiers.
3. Use semantic_search in hybrid mode for conceptual questions.
4. Use get_related to follow relationships between documents once you have a starting point.
5. Synthesize across all retrieved context before answering.
6. Cite the source document for every claim in your answer.
```

**Query routing logic:**

```
Incoming query
    │
    ├── Short query / looks like identifier / known term?
    │       └── Direct BM25 keyword_search → return results immediately
    │
    └── Natural language / conceptual / multi-part?
            └── Agentic loop → LightRAG hybrid + graph traversal → synthesize
```

### 3.5 Reranking Layer

**Technology:** BGE Reranker, Cohere Rerank, or a cross-encoder of your choice  
**Applied at:** After retrieval, before LLM synthesis

A cross-encoder reranker re-scores retrieved chunks using the full query context before passing them to the LLM. This catches cases where the initial retrieval returns relevant chunks in poor order, and reduces the amount of irrelevant context passed to the LLM.

Reranking adds latency (typically 200–500ms) and should be applied selectively — on agentic queries where synthesis quality matters, but not on direct keyword searches where speed is the priority.

---

## 4. Indexing Pipeline

### 4.1 Document Ingestion

```
New / updated document
    │
    ├── Parse → extract text, metadata, structured IDs (req IDs, component names)
    ├── Chunk → split into ~512 token chunks with 10% overlap
    ├── Contextualize → generate context block per chunk (LLM call)
    ├── Embed → generate vector embeddings with context prepended
    ├── Index (BM25) → add to full-text index with metadata
    ├── Index (Vector) → add embeddings to LightRAG vector store
    └── Index (Graph) → extract entities/relationships → upsert to knowledge graph
```

### 4.2 Relationship Extraction

LightRAG extracts entities and relationships automatically using LLM calls during indexing. To maximize the quality of graph construction for this corpus, the indexing prompt should be customized to recognize domain-specific relationship types:

**Entities to extract:**
- Requirements (IDs, names, functional areas)
- Components and subsystems
- Design decisions and their rationale
- Constraints and assumptions
- Stakeholders and authors
- Planning milestones and dependencies

**Relationships to extract:**
- `implements` — design implements requirement
- `derived-from` — subdesign derived from parent design
- `depends-on` — planning item depends on another
- `supersedes` — newer document supersedes older version
- `references` — document cites or references another
- `constrains` — requirement or decision constrains a design choice
- `resolves` — decision resolves an open question

### 4.3 Incremental Updates

LightRAG supports incremental document ingestion. When a document is updated:

1. Remove the old document's chunks from the vector index by document ID
2. Remove the old document's graph nodes and edges (preserve shared entities referenced by other documents)
3. Re-ingest the updated document through the full pipeline

For new documents, run the full ingestion pipeline. Do not re-index the entire corpus on each update.

---

## 5. Storage & Infrastructure

| Component | Technology | Notes |
|---|---|---|
| Graph store | Neo4j (prod) / NetworkX (dev) | LightRAG supports both |
| Vector store | pgvector or Qdrant | pgvector simplifies ops if Postgres already in use |
| Full-text index | Elasticsearch or Tantivy | Tantivy is embeddable with no external service |
| Object store | S3 or local filesystem | Store original documents and chunk metadata |
| Embedding model | text-embedding-3-small or equivalent | Balance cost vs. quality |
| LLM (indexing) | Claude Haiku | Context generation and entity extraction |
| LLM (query) | Claude Sonnet | Agentic reasoning and synthesis |

---

## 6. Document Taxonomy & Metadata Schema

Every document in the corpus should carry a standardized metadata envelope. This metadata is indexed in both the BM25 and graph indices and enables precise filtering.

```json
{
  "doc_id": "DESIGN-AUTH-001",
  "title": "Authentication Subsystem Design",
  "doc_type": "design",
  "status": "approved",
  "version": "2.1",
  "author": ["J. Smith", "A. Patel"],
  "created": "2024-11-15",
  "updated": "2025-01-22",
  "parent_doc_id": "DESIGN-SYSTEM-001",
  "implements": ["REQ-AUTH-001", "REQ-AUTH-002", "REQ-AUTH-018"],
  "related_docs": ["DESIGN-AUTH-TOKEN-001", "PLAN-Q1-2025"],
  "tags": ["authentication", "security", "token", "session"]
}
```

Enforcing this schema at document creation or import time significantly improves graph construction quality and reduces reliance on LLM-based relationship extraction for structural relationships that are already known.

---

## 7. Query Examples & Expected Behavior

| Query | Mode | Expected Behavior |
|---|---|---|
| `REQ-AUTH-018` | Keyword (BM25) | Returns all documents mentioning this requirement ID |
| `token refresh flow` | Hybrid (LightRAG) | Returns design chunks about token refresh, with related requirement and subdesign docs surfaced via graph |
| `What requirements are not yet addressed in any design?` | Agentic | Agent retrieves all requirements, queries for implementations, diffs the sets, returns gap list |
| `What changed in the authentication design between v1 and v2?` | Agentic | Agent retrieves both versions, compares key sections, summarizes changes |
| `Show me all subdesigns of the authentication design` | Graph (`get_related`) | Graph traversal from DESIGN-AUTH-001 via `derived-from` edges |
| `What assumptions underpin the Q2 planning document?` | Hybrid + Agentic | Retrieves planning doc, identifies assumption references, fetches source documents |

---

## 8. Open Questions & Decisions Required

- [ ] **Storage backend selection** — Neo4j (managed or self-hosted) vs. Postgres with pgvector. Depends on existing infrastructure.
- [ ] **Embedding model** — OpenAI, Cohere, or open-source (BGE, E5)? Affects cost, latency, and data residency.
- [ ] **Metadata enforcement** — Will documents be structured at source, or will metadata be extracted at ingest time? Structured-at-source significantly improves graph quality.
- [ ] **Agentic framework** — LangGraph, LlamaIndex, or custom tool-calling loop? Depends on team familiarity and existing tooling.
- [ ] **Document access control** — Should retrieval results be filtered by user permissions? If so, metadata must carry access level and retrieval must enforce it.
- [ ] **Reranker selection** — Hosted (Cohere) vs. self-hosted (BGE). Latency and cost tradeoff.
- [ ] **UI surface** — How are the two interaction modes (direct search vs. agentic chat) exposed to users? Same interface with query routing, or separate entry points?

---

## 9. Implementation Phases

### Phase 1 — Foundation (Weeks 1–3)
- Set up document ingestion pipeline with metadata schema
- Deploy BM25 full-text index (Elasticsearch or Tantivy)
- Implement direct keyword search interface
- Establish document taxonomy and ID conventions

### Phase 2 — Graph Index (Weeks 4–6)
- Deploy LightRAG with Neo4j or pgvector backend
- Implement contextual chunk enrichment at indexing time
- Index full corpus through LightRAG pipeline
- Validate entity and relationship extraction quality against known document relationships

### Phase 3 — Agentic Layer (Weeks 7–9)
- Define agent tools and system prompt
- Implement query routing logic (keyword vs. agentic)
- Build agentic query loop with iterative retrieval
- Add reranking layer for agentic query results

### Phase 4 — Validation & Tuning (Weeks 10–12)
- Build evaluation set of representative queries with expected answers
- Measure retrieval recall and answer quality across all query types
- Tune chunk size, context block prompts, and entity extraction prompts
- Performance test under realistic load

---

## 10. References

- [LightRAG GitHub](https://github.com/HKUDS/LightRAG) — Primary retrieval framework
- [Microsoft GraphRAG](https://github.com/microsoft/graphrag) — Reference for global synthesis patterns
- [Anthropic Contextual Retrieval](https://www.anthropic.com/news/contextual-retrieval) — Chunk enrichment technique
- [RAPTOR Paper](https://arxiv.org/abs/2401.18059) — Hierarchical summarization reference
- [BGE Reranker](https://github.com/FlagOpen/FlagEmbedding) — Open-source reranking model