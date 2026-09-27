// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Cross-process search reindex requests (#138).
//!
//! The search index is a local Tantivy directory owned by each API
//! process, and in production the job worker runs as a separate ECS
//! service that cannot write it. So a document the worker creates (DOCX,
//! PDF and Quip imports) was never indexed. The worker now appends the
//! new document's id to a Redis stream, and every API process reads that
//! stream and indexes the document into its own index.
//!
//! A stream rather than pub/sub, because pub/sub drops whatever arrives
//! while a subscriber is down — a deploy, a restart, a Redis reconnect.
//! Each API process reads the stream with its own cursor (`XREAD`, not a
//! consumer group: every process must see every entry), and starts that
//! cursor at the beginning of what the stream retains. That start is the
//! catch-up pass: a process that was down, or is new, re-indexes the
//! recent worker-created documents it missed. Indexing is idempotent, so
//! replaying an entry some earlier incarnation already handled is only
//! work, never wrong. The stream is trimmed to roughly
//! [`STREAM_MAX_LEN`] entries, which bounds that work.
//!
//! Embeddings are the exception. The vector store is shared, not
//! per-process, so the catch-up replay only updates the local Tantivy
//! index; embeddings are computed for entries read live, once the replay
//! has caught up. An entry that no API process saw live (every one of
//! them was down when it arrived) is keyword-searchable but has no
//! vectors until the document is next edited.
//!
//! Everything here is advisory. A document that fails to index is still a
//! document; nothing on the import path waits for, or fails on, a reindex.

use std::sync::Arc;
use std::time::Duration;

use fred::prelude::*;
use fred::types::XReadResponse;
use tokio::task::JoinHandle;

use crate::routes::documents::index_document_now;
use crate::state::AppState;

/// Suffix appended to the job stream's name, so each deployment's reindex
/// stream is namespaced the same way its job stream is.
const STREAM_SUFFIX: &str = ":search-reindex";

/// Approximate cap on retained entries (`XADD … MAXLEN ~`). Also the most a
/// process re-indexes on its boot catch-up.
pub const STREAM_MAX_LEN: i64 = 10_000;

/// The one field on each stream entry.
const DOC_ID_FIELD: &str = "doc_id";

/// Entries read per `XREAD`.
const READ_BATCH: u64 = 100;

/// How long one `XREAD` blocks waiting for new entries.
const READ_BLOCK_MS: u64 = 5_000;

/// Pause after a failed read before trying again.
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// The reindex stream's key for a deployment whose job stream is
/// `job_stream_name`.
pub fn stream_key(job_stream_name: &str) -> String {
    format!("{job_stream_name}{STREAM_SUFFIX}")
}

/// The worker's half: asks every API process to index a document.
#[derive(Clone)]
pub struct ReindexPublisher {
    client: Arc<RedisClient>,
    stream: String,
}

impl ReindexPublisher {
    pub fn new(client: Arc<RedisClient>, stream: String) -> Self {
        Self { client, stream }
    }

    /// Append `doc_id` to the reindex stream. Advisory: a failure is logged
    /// and swallowed, because the document itself was created and must not
    /// be reported as a failed import over its search index.
    pub async fn request(&self, doc_id: &str) {
        let result: Result<String, RedisError> = self
            .client
            .xadd(
                self.stream.as_str(),
                false,
                ("MAXLEN", "~", STREAM_MAX_LEN),
                "*",
                vec![(DOC_ID_FIELD, doc_id)],
            )
            .await;
        if let Err(e) = result {
            tracing::warn!(doc_id, error = %e, "search reindex request not published");
        }
    }
}

/// The API's half: read the reindex stream from the start of what it
/// retains, and index every document it names. `client` must be a
/// dedicated connection — `XREAD BLOCK` holds it for the block window.
pub fn spawn_consumer(state: AppState, client: RedisClient, stream: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        tracing::info!(stream, "search reindex consumer started");
        // Where this process has read up to. "0" is the catch-up start.
        let mut cursor = "0".to_string();
        // The id of the newest entry when this process started. Entries up
        // to it are the catch-up replay and skip embedding (module docs).
        // Retried until it reads: guessing "empty" on an error would count
        // the whole replay as live and re-embed all of it.
        let replay_until = loop {
            match newest_entry_id(&client, &stream).await {
                Ok(id) => break id,
                Err(e) => {
                    tracing::warn!(stream, error = %e, "search reindex: could not read the newest entry");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
        };

        loop {
            let batch = match read_batch(&client, &stream, &cursor).await {
                Ok(batch) => batch,
                Err(e) => {
                    tracing::warn!(stream, error = %e, "search reindex read failed");
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            };
            for (id, doc_id) in batch {
                let live = replay_until.as_deref().is_none_or(|last| entry_id_after(&id, last));
                if let Some(doc_id) = doc_id {
                    reindex(&state, &doc_id, live).await;
                }
                cursor = id;
            }
        }
    })
}

/// The newest entry's id, or `None` for an empty stream.
async fn newest_entry_id(client: &RedisClient, stream: &str) -> Result<Option<String>, RedisError> {
    let newest: Vec<(String, std::collections::HashMap<String, String>)> =
        client.xrevrange(stream, "+", "-", Some(1)).await?;
    Ok(newest.into_iter().next().map(|(id, _)| id))
}

/// One `XREAD` after `cursor`: each entry's id and the document id it
/// names (`None` for an entry without one, which is skipped but still
/// advances the cursor).
async fn read_batch(
    client: &RedisClient,
    stream: &str,
    cursor: &str,
) -> Result<Vec<(String, Option<String>)>, RedisError> {
    let resp: XReadResponse<String, String, String, String> = match client
        .xread_map(Some(READ_BATCH), Some(READ_BLOCK_MS), stream, cursor)
        .await
    {
        Ok(r) => r,
        // fred can't convert the NIL a timed-out block returns into a map;
        // that is the normal "nothing new" case (see `JobQueue::consume_next`).
        Err(e) if e.to_string().contains("Cannot convert to map") => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(resp
        .into_values()
        .flatten()
        .map(|(id, mut fields)| (id, fields.remove(DOC_ID_FIELD)))
        .collect())
}

/// Whether stream id `a` sorts after `b`. Ids are `<ms>-<seq>`.
fn entry_id_after(a: &str, b: &str) -> bool {
    fn parts(id: &str) -> (u64, u64) {
        let (ms, seq) = id.split_once('-').unwrap_or((id, "0"));
        (ms.parse().unwrap_or(0), seq.parse().unwrap_or(0))
    }
    parts(a) > parts(b)
}

/// Index one document named on the stream. A document that has since been
/// deleted is skipped — its own delete path removes it from the index.
async fn reindex(state: &AppState, doc_id: &str, embed: bool) {
    match state.doc_repo.get(doc_id).await {
        Ok(Some(meta)) if !meta.is_deleted => index_document_now(state, meta, embed).await,
        Ok(_) => {}
        Err(e) => tracing::warn!(doc_id, error = %e, "search reindex: document lookup failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_key_is_namespaced_by_the_job_stream() {
        assert_eq!(stream_key("ogrenotes-jobs"), "ogrenotes-jobs:search-reindex");
    }

    #[test]
    fn entry_ids_compare_by_time_then_sequence() {
        assert!(entry_id_after("2-0", "1-5"));
        assert!(entry_id_after("1-6", "1-5"));
        assert!(!entry_id_after("1-5", "1-5"));
        assert!(!entry_id_after("1-4", "1-5"));
        // Numeric, not lexicographic.
        assert!(entry_id_after("10-0", "9-0"));
    }
}
