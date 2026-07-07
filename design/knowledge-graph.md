# Knowledge Graph

> **Distilled pointer doc (#88).** The document-relationship graph's design
> lives in the RAG documents; its storage is implemented in `doc_repo` under the
> `REL#` / `RREL#` sort-key families. This page is the `design/`-level landing
> point criterion #9 expects.

## What it is

A directed graph of typed relationships **between documents** (e.g. references,
derived-from, related-to), used to enrich retrieval: the agentic query layer
(see [`agentic-search.md`](agentic-search.md)) walks the graph to pull in
contextually-linked documents that pure vector/BM25 recall would miss.

## Key decisions (distilled)

- **Stored in the single DynamoDB table, not a separate graph store.** Each
  relationship is written as a pair of items under the document partition: a
  **forward** edge (`SK = REL#<type>#<target_doc_id>`) and a **reverse** edge
  (`SK = RREL#<type>#<source_doc_id>`), created and deleted atomically so the
  graph can be traversed in either direction from one doc's partition. See
  `crates/storage/src/repo/doc_repo.rs` (`create_relationship`,
  `delete_relationship`, `list_relationships`, `list_reverse_relationships`).
- **Reverse-edge cleanup is the caller's responsibility on document delete** —
  deleting a doc purges its own `REL#`/`RREL#` items, but the forward edges
  other docs hold toward it must be cleaned via the reverse-relationship
  listing (`doc_repo` delete path documents this).
- **Relationship extraction** feeds the graph during indexing
  (`rag-architecture-steering.md` §4.2; `rag-implementation-plan.md` §3.1).

## Canonical sources

- [`design/rag-architecture-steering.md`](rag-architecture-steering.md) §4.2
  (relationship extraction), §3 (how the graph factors into retrieval).
- [`design/rag-implementation-plan.md`](rag-implementation-plan.md) §3.1
  (document relationship graph) + §3.2 (contextual chunk enrichment).
- **Code:** `crates/storage/src/repo/doc_repo.rs` — the `REL#` / `RREL#`
  relationship CRUD + traversal.
