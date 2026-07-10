// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::path::Path;
use std::sync::{Arc, Mutex};

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::*;
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, TantivyDocument};

// ─── Errors ────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("query parse error: {0}")]
    QueryParse(#[from] tantivy::query::QueryParserError),

    #[error("index writer unavailable")]
    WriterUnavailable,

    #[error("on-disk index schema does not match expected schema — delete the index directory to rebuild")]
    SchemaMismatch,
}

impl SearchError {
    /// Construct a `QueryParse` error from a raw message, for tests in
    /// *other* crates that need to exercise how they handle this variant
    /// without taking a direct dependency on Tantivy's error types. Keeps
    /// the Tantivy substrate encapsulated in this crate. Not part of the
    /// real public API — hence `#[doc(hidden)]`.
    #[doc(hidden)]
    pub fn query_parse_for_test(message: &str) -> Self {
        SearchError::QueryParse(tantivy::query::QueryParserError::SyntaxError(
            message.to_string(),
        ))
    }
}

// ─── Schema handles ────────────────────────────────────────────

#[derive(Clone)]
struct SearchSchema {
    doc_id: Field,
    title: Field,
    body: Field,
    owner_id: Field,
    doc_type: Field,
    folder_id: Field,
    workspace_id: Field,
    updated_at: Field,
    created_at: Field,
}

// ─── Public types ──────────────────────────────────────────────

/// A document to be indexed.
pub struct SearchDocument {
    pub doc_id: String,
    pub title: String,
    pub body: String,
    pub owner_id: String,
    pub doc_type: String,
    pub folder_id: Option<String>,
    pub workspace_id: Option<String>,
    pub updated_at: i64,
    pub created_at: i64,
}

/// Query parameters for search.
pub struct SearchQuery {
    pub text: String,
    pub doc_type: Option<String>,
    pub owner_id: Option<String>,
    pub folder_id: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

/// A single search result.
pub struct SearchHit {
    pub doc_id: String,
    pub title: String,
    pub score: f32,
    pub snippet: String,
    pub doc_type: String,
    pub owner_id: String,
    pub updated_at: i64,
    pub created_at: i64,
}

// ─── SearchIndex ───────────────────────────────────────────────

/// Permission-unaware full-text search index backed by Tantivy.
///
/// Callers are responsible for filtering results through the permission
/// model before returning them to users.
pub struct SearchIndex {
    _index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    schema: SearchSchema,
}

impl SearchIndex {
    /// Open an existing index or create a new one at `path`.
    ///
    /// If an existing index is found, its schema is validated against the
    /// expected schema. A mismatch (e.g., after adding a field) causes an
    /// error — delete the index directory to force a rebuild.
    pub fn open_or_create(path: &Path) -> Result<Self, SearchError> {
        let (schema, fields) = Self::build_schema();
        let index = if path.join("meta.json").exists() {
            let existing = Index::open_in_dir(path)?;
            if existing.schema() != schema {
                return Err(SearchError::SchemaMismatch);
            }
            existing
        } else {
            std::fs::create_dir_all(path).ok();
            Index::create_in_dir(path, schema.clone())?
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let writer = index.writer(50_000_000)?; // 50 MB heap

        Ok(Self {
            _index: index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            schema: fields,
        })
    }

    /// Create an in-memory index (useful for tests).
    pub fn open_in_memory() -> Result<Self, SearchError> {
        let (schema, fields) = Self::build_schema();
        let index = Index::create_in_ram(schema);

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;

        let writer = index.writer(15_000_000)?;

        Ok(Self {
            _index: index,
            reader,
            writer: Arc::new(Mutex::new(writer)),
            schema: fields,
        })
    }

    fn build_schema() -> (Schema, SearchSchema) {
        let mut builder = Schema::builder();

        let doc_id = builder.add_text_field("doc_id", STRING | STORED);
        let title = builder.add_text_field(
            "title",
            TextOptions::default()
                .set_indexing_options(
                    TextFieldIndexing::default()
                        .set_tokenizer("default")
                        .set_index_option(IndexRecordOption::WithFreqsAndPositions),
                )
                .set_stored(),
        );
        let body = builder.add_text_field("body", TEXT | STORED);
        let owner_id = builder.add_text_field("owner_id", STRING | STORED);
        let doc_type = builder.add_text_field("doc_type", STRING | STORED);
        let folder_id = builder.add_text_field("folder_id", STRING | STORED);
        let workspace_id = builder.add_text_field("workspace_id", STRING | STORED);
        let updated_at = builder.add_i64_field("updated_at", INDEXED | STORED | FAST);
        let created_at = builder.add_i64_field("created_at", STORED);

        let schema = builder.build();
        let fields = SearchSchema {
            doc_id,
            title,
            body,
            owner_id,
            doc_type,
            folder_id,
            workspace_id,
            updated_at,
            created_at,
        };
        (schema, fields)
    }

    /// Add or update a document in the index.
    ///
    /// Uses delete-then-add to handle updates (Tantivy doesn't support in-place updates).
    pub fn index_document(&self, search_doc: &SearchDocument) -> Result<(), SearchError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| SearchError::WriterUnavailable)?;

        // Delete any existing document with this ID
        let term = tantivy::Term::from_field_text(self.schema.doc_id, &search_doc.doc_id);
        writer.delete_term(term);

        let mut doc = TantivyDocument::new();
        doc.add_text(self.schema.doc_id, &search_doc.doc_id);
        doc.add_text(self.schema.title, &search_doc.title);
        doc.add_text(self.schema.body, &search_doc.body);
        doc.add_text(self.schema.owner_id, &search_doc.owner_id);
        doc.add_text(self.schema.doc_type, &search_doc.doc_type);
        if let Some(ref fid) = search_doc.folder_id {
            doc.add_text(self.schema.folder_id, fid);
        }
        if let Some(ref wid) = search_doc.workspace_id {
            doc.add_text(self.schema.workspace_id, wid);
        }
        doc.add_i64(self.schema.updated_at, search_doc.updated_at);
        doc.add_i64(self.schema.created_at, search_doc.created_at);

        writer.add_document(doc)?;
        writer.commit()?;

        self.reader.reload()?;

        Ok(())
    }

    /// Remove a document from the index by its ID.
    pub fn delete_document(&self, doc_id: &str) -> Result<(), SearchError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| SearchError::WriterUnavailable)?;

        let term = tantivy::Term::from_field_text(self.schema.doc_id, doc_id);
        writer.delete_term(term);
        writer.commit()?;

        self.reader.reload()?;

        Ok(())
    }

    /// Execute a search query and return ranked results.
    ///
    /// Results are ranked by BM25 with a 2x boost on title matches.
    /// The caller is responsible for permission filtering.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, SearchError> {
        let searcher = self.reader.searcher();
        let f = &self.schema;

        // Build the text query across title (boosted) and body
        let mut parser = QueryParser::for_index(searcher.index(), vec![f.title, f.body]);
        parser.set_field_boost(f.title, 2.0);
        let text_query = parser.parse_query(&query.text)?;

        // Keep a reference to the text query for snippet generation (before
        // it is moved into the BooleanQuery clauses).
        let text_query_ref = parser.parse_query(&query.text)?;

        // Combine text query with optional filters
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = vec![];
        clauses.push((Occur::Must, text_query));

        if let Some(ref dt) = query.doc_type {
            let term = tantivy::Term::from_field_text(f.doc_type, dt);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if let Some(ref oid) = query.owner_id {
            let term = tantivy::Term::from_field_text(f.owner_id, oid);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if let Some(ref fid) = query.folder_id {
            let term = tantivy::Term::from_field_text(f.folder_id, fid);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }

        let combined = BooleanQuery::new(clauses);
        // TopDocs::with_limit asserts limit >= 1. A zero fetch budget can
        // only ever produce an empty page, and an offset+limit that
        // overflows usize has no sane finite answer either — both cases
        // answer with an empty page instead of collecting or panicking.
        let Some(total_needed) = query.offset.checked_add(query.limit).filter(|&n| n > 0) else {
            return Ok(vec![]);
        };
        let top_docs = searcher.search(&combined, &TopDocs::with_limit(total_needed))?;

        let mut hits = Vec::with_capacity(query.limit);
        for (i, (score, doc_address)) in top_docs.into_iter().enumerate() {
            if i < query.offset {
                continue;
            }
            let stored = searcher.doc::<TantivyDocument>(doc_address)?;
            // Pass text_query (not combined) for snippet generation — the
            // SnippetGenerator extracts terms from the query, and filter-only
            // TermQueries on non-body fields produce empty snippets.
            let hit = self.doc_to_hit(&stored, score, &searcher, text_query_ref.as_ref())?;
            hits.push(hit);
        }

        Ok(hits)
    }

    /// Estimate total matching documents for a query (for `totalEstimate` in the response).
    pub fn count(&self, query: &SearchQuery) -> Result<usize, SearchError> {
        let searcher = self.reader.searcher();
        let f = &self.schema;

        let mut parser = QueryParser::for_index(searcher.index(), vec![f.title, f.body]);
        parser.set_field_boost(f.title, 2.0);
        let text_query = parser.parse_query(&query.text)?;

        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = vec![];
        clauses.push((Occur::Must, text_query));

        if let Some(ref dt) = query.doc_type {
            let term = tantivy::Term::from_field_text(f.doc_type, dt);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if let Some(ref oid) = query.owner_id {
            let term = tantivy::Term::from_field_text(f.owner_id, oid);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }
        if let Some(ref fid) = query.folder_id {
            let term = tantivy::Term::from_field_text(f.folder_id, fid);
            clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
        }

        let combined = BooleanQuery::new(clauses);
        let count = searcher.search(&combined, &tantivy::collector::Count)?;
        Ok(count)
    }

    fn doc_to_hit(
        &self,
        stored: &TantivyDocument,
        score: f32,
        searcher: &Searcher,
        query: &dyn tantivy::query::Query,
    ) -> Result<SearchHit, SearchError> {
        let f = &self.schema;

        let doc_id = Self::get_text(stored, f.doc_id);
        let title = Self::get_text(stored, f.title);
        let doc_type = Self::get_text(stored, f.doc_type);
        let owner_id = Self::get_text(stored, f.owner_id);
        let updated_at = Self::get_i64(stored, f.updated_at);
        let created_at = Self::get_i64(stored, f.created_at);

        // Generate snippet from body
        let snippet_generator =
            tantivy::SnippetGenerator::create(searcher, query, f.body)?;
        let snippet = snippet_generator.snippet_from_doc(stored);
        let snippet_html = snippet.to_html();

        Ok(SearchHit {
            doc_id,
            title,
            score,
            snippet: if snippet_html.is_empty() {
                String::new()
            } else {
                snippet_html
            },
            doc_type,
            owner_id,
            updated_at,
            created_at,
        })
    }

    fn get_text(doc: &TantivyDocument, field: Field) -> String {
        doc.get_first(field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    fn get_i64(doc: &TantivyDocument, field: Field) -> i64 {
        doc.get_first(field)
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }
}

// ─── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc(id: &str, title: &str, body: &str) -> SearchDocument {
        SearchDocument {
            doc_id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            owner_id: "user1".to_string(),
            doc_type: "document".to_string(),
            folder_id: Some("folder1".to_string()),
            workspace_id: Some("ws1".to_string()),
            updated_at: 1000000,
            created_at: 900000,
        }
    }

    #[test]
    fn test_index_and_search_by_title() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Authentication Design", "some body text"))
            .unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "Authentication".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");
        assert_eq!(results[0].title, "Authentication Design");
    }

    #[test]
    fn test_search_by_body() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "d1",
            "Project Notes",
            "The token refresh flow handles the edge case where sessions expire",
        ))
        .unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "token refresh".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");
    }

    #[test]
    fn test_delete_document() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "To Delete", "will be removed"))
            .unwrap();

        // Verify it exists
        let results = idx
            .search(&SearchQuery {
                text: "Delete".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 1);

        // Delete and verify gone
        idx.delete_document("d1").unwrap();
        let results = idx
            .search(&SearchQuery {
                text: "Delete".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_update_document() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Original Title", "original body content"))
            .unwrap();

        // Update with new content
        idx.index_document(&sample_doc("d1", "Updated Title", "completely new body"))
            .unwrap();

        // Old content should not match
        let results = idx
            .search(&SearchQuery {
                text: "original".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 0);

        // New content should match
        let results = idx
            .search(&SearchQuery {
                text: "Updated".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Updated Title");
    }

    #[test]
    fn test_filter_by_doc_type() {
        let idx = SearchIndex::open_in_memory().unwrap();

        let mut doc1 = sample_doc("d1", "Shared Title", "common body");
        doc1.doc_type = "document".to_string();
        idx.index_document(&doc1).unwrap();

        let mut doc2 = sample_doc("d2", "Shared Title", "common body");
        doc2.doc_type = "spreadsheet".to_string();
        idx.index_document(&doc2).unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "Shared".to_string(),
                doc_type: Some("spreadsheet".to_string()),
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d2");
    }

    #[test]
    fn test_filter_by_owner() {
        let idx = SearchIndex::open_in_memory().unwrap();

        let mut doc1 = sample_doc("d1", "Report Alpha", "quarterly results");
        doc1.owner_id = "alice".to_string();
        idx.index_document(&doc1).unwrap();

        let mut doc2 = sample_doc("d2", "Report Beta", "quarterly results");
        doc2.owner_id = "bob".to_string();
        idx.index_document(&doc2).unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "quarterly".to_string(),
                doc_type: None,
                owner_id: Some("alice".to_string()),
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");
    }

    #[test]
    fn test_filter_by_folder() {
        let idx = SearchIndex::open_in_memory().unwrap();

        let mut doc1 = sample_doc("d1", "Meeting Notes", "agenda items");
        doc1.folder_id = Some("folder_a".to_string());
        idx.index_document(&doc1).unwrap();

        let mut doc2 = sample_doc("d2", "Meeting Summary", "agenda items");
        doc2.folder_id = Some("folder_b".to_string());
        idx.index_document(&doc2).unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "agenda".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: Some("folder_a".to_string()),
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");
    }

    #[test]
    fn test_snippet_generation() {
        let idx = SearchIndex::open_in_memory().unwrap();
        // Use a longer body so tantivy generates a non-trivial snippet
        idx.index_document(&sample_doc(
            "d1",
            "Design Doc",
            "The authentication system uses JWT tokens for session management. \
             Tokens are refreshed automatically before expiry. \
             The system also supports OAuth2 authorization code flow for third party integrations. \
             Each user can configure multiple authentication providers in their account settings.",
        ))
        .unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "authentication".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        // Snippet should contain highlighted terms
        assert!(
            results[0].snippet.contains("<b>"),
            "Expected highlighted snippet, got: {:?}",
            results[0].snippet
        );
    }

    #[test]
    fn test_count() {
        let idx = SearchIndex::open_in_memory().unwrap();
        for i in 0..5 {
            idx.index_document(&sample_doc(
                &format!("d{i}"),
                &format!("Document {i}"),
                "shared keyword content",
            ))
            .unwrap();
        }

        let count = idx
            .count(&SearchQuery {
                text: "keyword".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 2,
                offset: 0,
            })
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn test_no_results() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Hello World", "some body"))
            .unwrap();

        let results = idx
            .search(&SearchQuery {
                text: "nonexistent".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 0);
    }

    // ─── Review fix #2: None folder/workspace not stored as "" ───

    #[test]
    fn test_none_folder_not_matchable_as_empty_string() {
        let idx = SearchIndex::open_in_memory().unwrap();

        // Index a doc with no folder
        let mut doc = sample_doc("d1", "Orphan Doc", "orphan body");
        doc.folder_id = None;
        idx.index_document(&doc).unwrap();

        // A doc with a real folder
        let mut doc2 = sample_doc("d2", "Filed Doc", "filed body");
        doc2.folder_id = Some("real_folder".to_string());
        idx.index_document(&doc2).unwrap();

        // Filtering by folder_id = "" should match nothing (not the None doc)
        let results = idx
            .search(&SearchQuery {
                text: "body".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: Some("".to_string()),
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 0, "Empty string folder filter should not match None-folder docs");

        // Without filter, both docs should appear
        let results = idx
            .search(&SearchQuery {
                text: "body".to_string(),
                doc_type: None,
                owner_id: None,
                folder_id: None,
                limit: 10,
                offset: 0,
            })
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    // ─── Review fix #5: schema mismatch detection ────────────────

    #[test]
    fn test_schema_mismatch_on_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Create an index with a DIFFERENT schema
        {
            let mut builder = tantivy::schema::Schema::builder();
            builder.add_text_field("some_other_field", tantivy::schema::TEXT);
            let other_schema = builder.build();
            tantivy::Index::create_in_dir(dir.path(), other_schema).unwrap();
        }

        // Opening with our schema should fail with SchemaMismatch
        let result = SearchIndex::open_or_create(dir.path());
        match result {
            Err(SearchError::SchemaMismatch) => {} // expected
            Err(other) => panic!("Expected SchemaMismatch, got: {other:?}"),
            Ok(_) => panic!("Expected SchemaMismatch error, but open_or_create succeeded"),
        }
    }

    // ─── Additional coverage: query construction & boundaries ────

    /// Minimal query builder for the tests below (defaults: no filters,
    /// limit 10, offset 0).
    fn q(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.to_string(),
            doc_type: None,
            owner_id: None,
            folder_id: None,
            limit: 10,
            offset: 0,
        }
    }

    #[test]
    fn test_empty_query_matches_nothing() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Hello World", "some body"))
            .unwrap();

        // Empty and whitespace-only queries parse successfully and match
        // no documents (they must not error or match everything).
        let results = idx.search(&q("")).unwrap();
        assert_eq!(results.len(), 0, "empty query should match nothing");

        let results = idx.search(&q("   ")).unwrap();
        assert_eq!(results.len(), 0, "whitespace query should match nothing");

        let count = idx.count(&q("")).unwrap();
        assert_eq!(count, 0, "count of empty query should be 0");
    }

    #[test]
    fn test_malformed_query_returns_parse_error_not_panic() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Hello", "world"))
            .unwrap();

        // Unbalanced quote — raw user input flows straight into the query
        // parser, so this must surface as a QueryParse error, not a panic.
        // (SearchHit has no Debug impl, so extract the error by matching.)
        let err = match idx.search(&q("\"unbalanced")) {
            Ok(_) => panic!("expected a parse error for unbalanced quote"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SearchError::QueryParse(_)),
            "expected QueryParse, got: {err:?}"
        );

        // count() must fail the same way for the same input.
        let err = idx.count(&q("\"unbalanced")).unwrap_err();
        assert!(
            matches!(err, SearchError::QueryParse(_)),
            "expected QueryParse from count, got: {err:?}"
        );
    }

    #[test]
    fn test_colon_field_syntax_on_unknown_field_is_parse_error() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Meeting", "re: budget review"))
            .unwrap();

        // A user searching text containing `word:` triggers tantivy's
        // field-query syntax; an unknown field name is a parse error.
        // Pinned so any future sanitization layer knows the raw behavior.
        let err = match idx.search(&q("nonexistentfield:budget")) {
            Ok(_) => panic!("expected a parse error for unknown field"),
            Err(e) => e,
        };
        assert!(
            matches!(err, SearchError::QueryParse(_)),
            "expected QueryParse for unknown field, got: {err:?}"
        );
    }

    #[test]
    fn test_query_parse_for_test_helper_produces_query_parse_variant() {
        let err = SearchError::query_parse_for_test("boom");
        assert!(matches!(err, SearchError::QueryParse(_)));
        assert!(
            err.to_string().contains("boom"),
            "message should round-trip, got: {err}"
        );
    }

    // ─── Additional coverage: ranking ─────────────────────────────

    #[test]
    fn test_title_match_ranks_above_body_match() {
        let idx = SearchIndex::open_in_memory().unwrap();

        let mut body_hit = sample_doc(
            "body_hit",
            "Unrelated Heading",
            "the zebra crossed the savanna at dusk",
        );
        body_hit.doc_id = "body_hit".to_string();
        idx.index_document(&body_hit).unwrap();

        idx.index_document(&sample_doc(
            "title_hit",
            "Zebra Migration",
            "notes about seasonal movement patterns",
        ))
        .unwrap();

        let results = idx.search(&q("zebra")).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].doc_id, "title_hit",
            "2x title boost should rank the title match first"
        );
        assert!(
            results[0].score > results[1].score,
            "scores should be strictly ordered: {} vs {}",
            results[0].score,
            results[1].score
        );
    }

    #[test]
    fn test_results_ordered_by_descending_score() {
        let idx = SearchIndex::open_in_memory().unwrap();

        // Vary term frequency / placement so scores differ.
        idx.index_document(&sample_doc("d1", "falcon", "falcon falcon falcon"))
            .unwrap();
        idx.index_document(&sample_doc("d2", "Bird Notes", "a falcon appeared once"))
            .unwrap();
        idx.index_document(&sample_doc(
            "d3",
            "Field Journal",
            "long entry mentioning a falcon among many many other unrelated words \
             spread across a considerably longer body of text than the others",
        ))
        .unwrap();

        let results = idx.search(&q("falcon")).unwrap();
        assert_eq!(results.len(), 3);
        for pair in results.windows(2) {
            assert!(
                pair[0].score >= pair[1].score,
                "results must be sorted by descending score"
            );
        }
    }

    #[test]
    fn test_phrase_query_requires_adjacency() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "adjacent",
            "Doc A",
            "the token refresh flow works",
        ))
        .unwrap();
        idx.index_document(&sample_doc(
            "separated",
            "Doc B",
            "the token that eventually needs a refresh",
        ))
        .unwrap();

        // Quoted phrase should only match the adjacent occurrence.
        let results = idx.search(&q("\"token refresh\"")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "adjacent");

        // Unquoted multi-term query matches both.
        let results = idx.search(&q("token refresh")).unwrap();
        assert_eq!(results.len(), 2);
    }

    // ─── Additional coverage: pagination ──────────────────────────

    #[test]
    fn test_pagination_pages_are_disjoint_and_complete() {
        let idx = SearchIndex::open_in_memory().unwrap();
        for i in 0..5 {
            idx.index_document(&sample_doc(
                &format!("d{i}"),
                &format!("Document {i}"),
                "paginated shared keyword",
            ))
            .unwrap();
        }

        let page = |offset: usize| {
            let mut query = q("paginated");
            query.limit = 2;
            query.offset = offset;
            idx.search(&query).unwrap()
        };

        let p0 = page(0);
        let p1 = page(2);
        let p2 = page(4);
        assert_eq!(p0.len(), 2);
        assert_eq!(p1.len(), 2);
        assert_eq!(p2.len(), 1, "last page holds the remainder");

        let mut ids: Vec<String> = p0
            .iter()
            .chain(p1.iter())
            .chain(p2.iter())
            .map(|h| h.doc_id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            5,
            "pages must be disjoint and together cover all matches"
        );
    }

    #[test]
    fn test_offset_beyond_results_returns_empty() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Solo", "single lonely match"))
            .unwrap();

        let mut query = q("lonely");
        query.limit = 10;
        query.offset = 50;
        let results = idx.search(&query).unwrap();
        assert_eq!(results.len(), 0);
    }

    // ─── Additional coverage: tokenization & unicode ──────────────

    #[test]
    fn test_matching_is_case_insensitive() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "d1",
            "Kubernetes Cluster",
            "The Deployment restarts Pods automatically",
        ))
        .unwrap();

        for term in ["kubernetes", "KUBERNETES", "deployment", "DePlOyMeNt"] {
            let results = idx.search(&q(term)).unwrap();
            assert_eq!(results.len(), 1, "query {term:?} should match");
        }
    }

    #[test]
    fn test_unicode_terms_are_searchable() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "d1",
            "Café Notes",
            "the naïve approach to the café menu über alles",
        ))
        .unwrap();

        let results = idx.search(&q("café")).unwrap();
        assert_eq!(results.len(), 1, "accented term should match itself");

        let results = idx.search(&q("über")).unwrap();
        assert_eq!(results.len(), 1);

        // Pins current behavior: the default tokenizer does NOT fold
        // accents, so the ASCII form does not match. If this test starts
        // failing, accent folding was (perhaps deliberately) introduced.
        let results = idx.search(&q("cafe")).unwrap();
        assert_eq!(
            results.len(),
            0,
            "unaccented form does not match — no accent folding in default tokenizer"
        );
    }

    #[test]
    fn test_long_body_terms_remain_searchable() {
        let idx = SearchIndex::open_in_memory().unwrap();
        // ~5000 words of filler with a unique marker at the very end.
        let mut body = "filler ".repeat(5000);
        body.push_str("xylophone");
        idx.index_document(&sample_doc("d1", "Long Doc", &body)).unwrap();

        let results = idx.search(&q("xylophone")).unwrap();
        assert_eq!(results.len(), 1, "term at end of a long body should match");
    }

    #[test]
    fn test_empty_title_and_body_document_is_indexable() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "", "")).unwrap();

        // It matches no text query...
        let results = idx.search(&q("anything")).unwrap();
        assert_eq!(results.len(), 0);

        // ...and is still deletable without error.
        idx.delete_document("d1").unwrap();
    }

    // ─── Additional coverage: snippets ─────────────────────────────

    #[test]
    fn test_snippet_escapes_html_in_body() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "d1",
            "Injection Doc",
            "the payload <script>alert(1)</script> sits right beside the \
             marker term so the snippet fragment includes the raw markup",
        ))
        .unwrap();

        let results = idx.search(&q("payload")).unwrap();
        assert_eq!(results.len(), 1);
        let snippet = &results[0].snippet;
        assert!(
            snippet.contains("&lt;script&gt;"),
            "raw HTML in the body must be escaped in the snippet, got: {snippet:?}"
        );
        assert!(
            !snippet.contains("<script>"),
            "snippet must not contain unescaped markup, got: {snippet:?}"
        );
        // The generator's own highlight markup is still present.
        assert!(snippet.contains("<b>payload</b>"), "got: {snippet:?}");
    }

    #[test]
    fn test_title_only_match_yields_empty_snippet() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc(
            "d1",
            "Quasar Overview",
            "completely unrelated body text about telescopes",
        ))
        .unwrap();

        let results = idx.search(&q("quasar")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].snippet, "",
            "term appearing only in the title produces an empty body snippet"
        );
    }

    // ─── Additional coverage: hit mapping & filters ────────────────

    #[test]
    fn test_hit_preserves_stored_fields() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Field Check", "round trip body"))
            .unwrap();

        let results = idx.search(&q("round")).unwrap();
        assert_eq!(results.len(), 1);
        let hit = &results[0];
        assert_eq!(hit.doc_id, "d1");
        assert_eq!(hit.title, "Field Check");
        assert_eq!(hit.owner_id, "user1");
        assert_eq!(hit.doc_type, "document");
        assert_eq!(hit.updated_at, 1_000_000);
        assert_eq!(hit.created_at, 900_000);
        assert!(hit.score > 0.0);
    }

    #[test]
    fn test_combined_filters_intersect() {
        let idx = SearchIndex::open_in_memory().unwrap();

        let mut doc = sample_doc("d1", "Budget", "annual budget numbers");
        doc.owner_id = "alice".to_string();
        doc.doc_type = "spreadsheet".to_string();
        idx.index_document(&doc).unwrap();

        let mut doc = sample_doc("d2", "Budget", "annual budget numbers");
        doc.owner_id = "alice".to_string();
        doc.doc_type = "document".to_string();
        idx.index_document(&doc).unwrap();

        let mut doc = sample_doc("d3", "Budget", "annual budget numbers");
        doc.owner_id = "bob".to_string();
        doc.doc_type = "spreadsheet".to_string();
        idx.index_document(&doc).unwrap();

        let mut query = q("budget");
        query.owner_id = Some("alice".to_string());
        query.doc_type = Some("spreadsheet".to_string());
        let results = idx.search(&query).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");

        // count() applies the same filters.
        let count = idx.count(&query).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_count_with_filter_differs_from_unfiltered() {
        let idx = SearchIndex::open_in_memory().unwrap();
        for (id, dt) in [("d1", "document"), ("d2", "document"), ("d3", "spreadsheet")] {
            let mut doc = sample_doc(id, "Countable", "countable content");
            doc.doc_type = dt.to_string();
            idx.index_document(&doc).unwrap();
        }

        assert_eq!(idx.count(&q("countable")).unwrap(), 3);

        let mut query = q("countable");
        query.doc_type = Some("document".to_string());
        assert_eq!(idx.count(&query).unwrap(), 2);
    }

    #[test]
    fn test_delete_nonexistent_document_is_ok() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Keeper", "stays put"))
            .unwrap();

        // Deleting an ID that was never indexed must succeed silently
        // and leave existing documents untouched.
        idx.delete_document("no_such_doc").unwrap();

        let results = idx.search(&q("keeper")).unwrap();
        assert_eq!(results.len(), 1);
    }

    // ─── Additional coverage: on-disk lifecycle ────────────────────

    #[test]
    fn test_open_or_create_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();

        {
            let idx = SearchIndex::open_or_create(dir.path()).unwrap();
            idx.index_document(&sample_doc("d1", "Durable Doc", "survives a restart"))
                .unwrap();
            // Drop releases the writer lock before reopening.
        }

        let idx = SearchIndex::open_or_create(dir.path()).unwrap();
        let results = idx.search(&q("durable")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].doc_id, "d1");
        assert_eq!(results[0].title, "Durable Doc");
    }

    #[test]
    fn test_open_or_create_creates_missing_nested_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");

        let idx = SearchIndex::open_or_create(&nested).unwrap();
        idx.index_document(&sample_doc("d1", "Nested", "created on demand"))
            .unwrap();
        assert_eq!(idx.search(&q("nested")).unwrap().len(), 1);
        assert!(nested.join("meta.json").exists());
    }

    /// Regression: issue #7 — a zero fetch limit must return an empty
    /// result set, not trip tantivy's `TopDocs::with_limit >= 1` assert
    /// and panic the caller.
    #[test]
    fn test_zero_limit_returns_empty_instead_of_panicking() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Hello World", "some body"))
            .unwrap();

        let mut query = q("hello");
        query.limit = 0;
        assert!(idx.search(&query).unwrap().is_empty());
    }

    /// Companion to the zero-limit guard: an `offset + limit` that would
    /// overflow `usize` has no finite answer either — it must yield an
    /// empty page, not a debug-build overflow panic or a release-build
    /// wrapped (undersized) fetch window.
    #[test]
    fn test_offset_limit_overflow_returns_empty() {
        let idx = SearchIndex::open_in_memory().unwrap();
        idx.index_document(&sample_doc("d1", "Hello World", "some body"))
            .unwrap();

        let mut query = q("hello");
        query.offset = usize::MAX;
        query.limit = 10;
        assert!(idx.search(&query).unwrap().is_empty());
    }
}
