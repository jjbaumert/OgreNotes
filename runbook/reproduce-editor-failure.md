# Runbook: Reproduce an Editor Failure

Use this runbook when a user reports a document editing bug (crash, corruption, visual glitch) and you need to identify the root cause.

## Prerequisites

- AWS credentials with DynamoDB read + S3 read access to the ogrenotes tables/buckets
- Environment variables: `DYNAMODB_TABLE_PREFIX`, `S3_BUCKET`, `AWS_REGION`
- The `doc_id` of the affected document (from the URL: `/d/{doc_id}/...`)

## Step 1: Replay the document's edit history

Run the replay harness to apply every stored update one at a time and check for errors:

```bash
cargo run -p ogrenotes-collab --features replay --bin replay -- <doc_id>
```

### What to look for in the output

| Output | Meaning |
|--------|---------|
| `OK` on every line | All updates applied cleanly. The bug is in the frontend editor logic (Step application, view rendering, or selection handling), not in the CRDT layer. |
| `ERROR` on a line | The yrs CRDT rejected that update. Note the `clock` and `user_id`. |
| `PANIC` on a line | The update caused a crash. Note the `clock`, `user_id`, and `ver` (client version). |
| `ver=-` | Update was written before client versioning was added. Cannot determine if a fix applies. |
| `ver=X.Y.Z` | Update was written by client version X.Y.Z. Compare against the version that shipped a fix. |

### Extract a failing update for a unit test

If the replay identifies a failing update at step N:

1. Note the `clock` value from the output
2. The raw `update_bytes` are printed (first 100 bytes) in the error message
3. To get the full bytes, query DynamoDB directly:

```bash
aws dynamodb get-item \
  --table-name <prefix>ogrenotes \
  --key '{"PK":{"S":"DOC#<doc_id>"},"SK":{"S":"UPDATE#<clock>"}}' \
  --query 'Item.update_bytes.B' \
  --output text | base64 -d > failing_update.bin
```

4. Write a unit test in `crates/collab/src/document.rs` that loads the snapshot state from step N-1, then applies the failing update bytes:

```rust
#[test]
fn reproduce_issue_NNN() {
    let snapshot = include_bytes!("fixtures/snapshot_before_crash.bin");
    let mut doc = OgreDoc::from_state_bytes(snapshot).unwrap();
    let failing_update = include_bytes!("fixtures/failing_update.bin");
    // This should either panic (proving the bug) or succeed (if already fixed)
    doc.apply_update(failing_update).unwrap();
}
```

## Step 2: Check the current document state

If the replay completes without errors but the document looks wrong, inspect the final state:

```bash
# Fetch the current document content via the API
curl -s -H "Authorization: Bearer <token>" \
  http://localhost:3000/api/v1/documents/<doc_id>/content \
  -o doc_state.bin
```

Then examine it in a test:

```rust
let bytes = std::fs::read("doc_state.bin").unwrap();
let doc = OgreDoc::from_state_bytes(&bytes).unwrap();
// Inspect the yrs XML structure
let txn = doc.doc().transact();
let fragment = txn.get_xml_fragment("content").unwrap();
println!("Children: {}", fragment.len(&txn));
// ... inspect individual nodes
```

## Step 3: Check for structural corruption

The document normalization layer (`frontend/src/editor/model.rs::normalize_doc`) fixes common corruptions on read. If the bug is a visual glitch that survives page refresh, the corruption is persisted in yrs.

Known corruption patterns:

| Pattern | Cause | Detection |
|---------|-------|-----------|
| Empty `BulletList` with no children | Stale undo positions after concurrent edit | Schema validation rejects empty lists |
| `ListItem` directly under `Doc` | Corrupted undo split the list structure | `ListItem` is not a valid Doc child |
| Duplicated text in a paragraph | Concurrent full-rewrite of same block by two users | Text content longer than expected |
| Paragraph + orphaned list fragment | Undo `ReplaceAround` applied at wrong offset | Two siblings where one should exist |

To check programmatically, use the schema validator on the document model reconstructed from yrs state. The frontend does this via `normalize_doc` in `yrs_bridge::read_doc_from_ydoc`.

## Step 4: Identify the client version

Each `DocUpdate` row includes a `client_version` field (added in v0.1.0). If the failing update has:

- `ver=-` (no version): the update predates versioning. Cannot determine if the fix applies.
- `ver=X.Y.Z`: compare against the changelog to determine if the bug was fixed in a later version.

If all failing updates come from a version that predates a known fix, the issue is already resolved and the document just needs the corrupted state cleaned up.

## Step 5: Clean up a corrupted document

If the document has persistent corruption (orphaned bullets, duplicated text), the normalization layer should fix it on next load. Force a re-normalization by:

1. Loading the document content via the API
2. The `read_doc_from_ydoc` path applies `normalize_doc` automatically
3. The normalized state is synced back to yrs via `sync_model_to_ydoc`
4. A page refresh by any connected client triggers this flow

If normalization doesn't fix the issue, the corruption is deeper than what `normalize_doc` handles. File a bug with:
- The `doc_id`
- The replay output
- The failing update's `clock` and `client_version`
- A screenshot of the visual glitch

## Reference: Key files

| File | Purpose |
|------|---------|
| `crates/collab/src/bin/replay.rs` | Replay harness binary |
| `crates/storage/src/models/document.rs` | `DocUpdate` struct (what's stored per update) |
| `crates/storage/src/repo/doc_repo.rs` | `get_pending_updates()`, `load_snapshot()` |
| `frontend/src/editor/model.rs` | `normalize_doc()` — fixes structural corruption |
| `frontend/src/editor/yrs_bridge.rs` | `sync_model_to_ydoc()`, `read_doc_from_ydoc()` — yrs bridge |
| `frontend/src/editor/plugins.rs` | `HistoryPlugin` — undo/redo (known issue: stale positions) |
| `frontend/src/editor/schema.rs` | `Schema::validate()` — document structure validation |

## Reference: Known bugs

| Bug | Status | Affected versions | Test |
|-----|--------|-------------------|------|
| Undo after concurrent edit corrupts document (stale step positions) | Failing test written, fix pending | All versions | `plugins::tests::undo_list_wrap_after_concurrent_text_edit` |
| Inline mark rule selects text instead of placing cursor | Fixed | Pre-0.1.0 | `input_rules::tests::bold_rule_cursor_after_text_not_selecting` |
| Inline mark rule leaks marks to subsequent text | Fixed | Pre-0.1.0 | `input_rules::tests::bold_rule_clears_stored_marks` |
| Fragment::cut with out-of-bounds positions | Fixed (clamped) | Pre-0.1.0 | Defensive fix, no specific test |
