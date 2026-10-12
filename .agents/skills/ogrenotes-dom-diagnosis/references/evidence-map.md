# Evidence and source map

Paths below are relative to the OgreNotes repository root, not the installed skill directory. Confirm symbols against the inspected commit before relying on storage ordering or writer behavior. Use `rg` for symbols when code has moved. Read only the relevant branch of this map.

## Stored state and retained history

| Evidence | Repository paths / symbols | What it establishes and does not establish |
|---|---|---|
| Scanner contract and output | `crates/collab/src/bin/scan_document_dom/{main,args,cloud,inspect}.rs`; operator guide `scripts/document-dom-scanner.md` | Scope, budgets, read stability, schema paths, byte hashes and statuses for this tool revision. Do not invent missing output fields. |
| Schema and validation | `crates/collab/src/schema.rs`, `crates/collab/src/validate.rs`, `crates/collab/src/blocks/validate_writes.rs`; mirrored `frontend/src/editor/schema.rs` | Distinguish decode failure, invalid child structure, incomplete CRDT dependencies, and traversal limits. The scanner additionally examines raw roots/content and node kinds. Attribute semantics and block-ID uniqueness are not checked; separate evidence is needed for those claims. Rendering/export success is not full structural validation. |
| Current metadata | `crates/storage/src/models/document.rs`, `crates/storage/src/repo/doc_repo.rs` | `DOC#<id>` / `METADATA`, `snapshot_version`, `snapshot_s3_key`; use the recorded pointer, not a guessed newest key. |
| Snapshot inventory | `crates/storage/src/models/snapshot.rs`, `crates/storage/src/repo/snapshot_repo.rs` | `DOC#<id>` / zero-padded `SNAPSHOT#<version>`, `s3_key`, `size_bytes`, `user_id`, `created_at`. History rows are best effort; absent rows are not proof of invalid bytes. |
| S3 snapshots | `DocRepo::{create,save_snapshot,save_snapshot_conditional,load_snapshot}`; `crates/storage/src/s3.rs` | Historical naming includes `docs/<id>/snapshots/<version>.bin`. Follow actual pointers and inventory; version alone is not byte identity. Keys can be reused by historical writers, so record byte hash and S3 identity. |
| Retained operation log | `DocUpdate` in `models/document.rs`; `DocRepo::{append_update,get_pending_updates,delete_updates_before}` | `UPDATE#<clock>` rows carry authenticated sender `user_id`, `created_at`, optional `client_version`; payload is inline `update_bytes` or `update_s3_key`. Follow repository precedence if both occur. S3 payload names historically use `docs/<id>/updates/<clock>.bin`. |
| Compaction and retention | `crates/api/src/compaction.rs`; `crates/api/src/routes/ws.rs` last-client handling | Snapshots are followed by pruning older update rows. An S3 update blob may disappear before its row during cleanup. A racing missing object needs an incomplete-read explanation, not an invented corrupt author. |
| Legacy replay helper | `crates/collab/src/bin/replay.rs` | Useful to understand replay ordering/provenance, but its success is not the scanner's validity proof: inspect schema checks, unresolved dependencies, and missing-snapshot behavior before using it. |

There is no assumed second durable operation log behind the `UPDATE#` rows. Check Yrs `pending_update().missing` and `pending_ds()` through the scanner before declaring replay complete.

Order retained updates as the scanner/storage contract does. Wall-clock timestamps are correlation aids, not a total causal order. Applying a merged update can reveal earlier content carried by that update; its sender and the Yrs internal client ID do not uniquely identify the original human or component that authored every node.

`--history` examines retained S3 historical objects as well as recorded snapshots. An orphan object may be an uncommitted/rejected candidate; its existence does not prove it was ever the document's active state. S3 listing is not a promise to recover deleted object versions. Missing history, overwritten keys, or pruned update rows bound what can be concluded.

Do not infer a complete transition chain by joining snapshot versions and today's pending rows without verifying their coverage. A bad earliest retained snapshot only establishes “invalid by this retained point.” To prove a producing mechanism, prefer a verified valid replay state (the starting snapshot or a later state after dependencies resolve) plus retained causal updates that reproduce the invalid path under the relevant writer/version.

## Writer and provenance mapping

| Candidate mechanism | Source to inspect | Correlation to seek |
|---|---|---|
| Browser edit or paste | `frontend/src/editor/{view,commands,transform,yrs_bridge}.rs`; `frontend/src/collab/ws_client.rs` | Exact transaction shape, UTF-16/model boundaries, schema command, serialized update and source build. A similar source bug is a hypothesis until the retained input reproduces it. |
| WebSocket ingestion | `frontend/src/api/documents.rs` WS-token request; `crates/api/src/routes/ws.rs`; `crates/collab/src/{room,document}.rs` | Token-associated user/doc/client version, update clock, apply/save errors, and subsequent peer/snapshot behavior. Never print the token itself. |
| REST content replacement, restore or copy | `crates/api/src/routes/documents.rs`; `DocRepo` snapshot methods | Request/operation identity, snapshot pointer/version, conditional-write outcome, source object identity. A restore may copy pre-existing invalid bytes. |
| Imported document | `crates/api/src/{routes/imports,worker_mode}.rs`; `crates/collab/src/import*.rs` | Source type, import/run/thread-to-document mapping, converter version, staging bytes when retained, and destination snapshot identity. |
| Quip manifest/checkpoints | `crates/storage/src/models/{import,import_inventory}.rs`; `crates/storage/src/repo/import_repo.rs` | `IMPORT#<id>` / `META` records owner/status/phase/source user/target folder/timestamps. `THREAD#<quip_thread_id>` records `ogre_doc_id`, thread type, state, attempts/reason, `updated_usec`. Inspect current run-generation fields rather than mixing attempts. `SECMAP#<thread>#<chunk>` maps source section IDs to block IDs; it is not writer attribution. |
| Import staging | `worker_mode.rs` `quip_staging_prefix`, `import_one_thread`, `cleanup_quip_staging` | Historical prefixes include `imports/<import>/runs/<run>/threads/` and legacy `imports/<import>/threads/`. Staged HTML is diagnostic material that may be deleted after completion. Its absence is not a successful-conversion claim or permission to fetch Quip again. |
| Compactor/backfill | `crates/api/src/compaction.rs`, `crates/api/src/bin/backfill_snapshots.rs` | Snapshot writer may only serialize an already-invalid tree. Never run compaction/backfill as a diagnostic read. |

## CloudWatch and deployed-version evidence

For existing read-only log access conventions, inspect `scripts/aws-test-logs.sh` and `.claude/agents/aws-diagnostic.md`. Client bridge telemetry maps through `crates/api/src/routes/client_telemetry.rs`; counters and sampled logs are corroboration, not retained update bytes.

Resolve the actual deployment's region/log group from existing deployment configuration or session evidence; do not invent a production account or log group. Query a bounded time window and relevant document/import/run/request IDs. Follow the configured log retention and pagination. Capture query scope, event timestamps and identifiers; absence of an event is not proof of absence of the operation.

Useful source search anchors: `client_version`, `clock`, `append_update`, `save_snapshot`, `SNAPSHOT# row write failed`, import/run identifiers, and `delete_updates_before`. Correlate logs to retained byte identities where possible. A request timestamp or authenticated user alone cannot establish which bytes produced an invalid node. Match client-version strings to deployment/build artifacts only when that mapping exists; do not equate a package version with a unique Git commit.

Avoid automatic retries that broaden time windows or scan all documents. On unstable reads, a small authorized repeat of the same scope can distinguish a race from a repeatable finding; retain both reports. After repeated instability or a resource limit, report the uncertainty and the specific additional access/window needed.
