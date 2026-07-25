# Quip Import — Phase 1 (Inventory) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a user scopes a connected Quip import (root folders + target OgreNotes folder) and clicks Continue, a background worker walks the selected Quip folder tree, persists a resumable per-thread inventory to the DynamoDB manifest, and the wizard shows live progress + a thread-count/time estimate.

**Architecture:** The API's `POST /imports/quip/{id}/start` authorizes the target folder, records the scope on the `META` row, and enqueues a **token-free** `StartQuipImport { import_id, owner_id }` trigger onto the existing Redis-Streams job queue. The worker handler re-reads the token from the `TokenStore`, constructs a **per-import** `QuipClient` (its own 45/min throttle), BFS-walks the selected roots via `/1/folders/`, fetches thread metadata via `/1/threads/`, and writes `FOLDER#`/`THREAD#` manifest rows with **conditional (insert-if-absent)** writes so a reclaimed/retried run resumes without clobbering advanced threads. A DynamoDB `runner_claim` lease on `META` prevents two workers from inventorying the same import concurrently. `GET /imports/quip/{id}` returns `{status, phase, progress}` which the wizard polls.

**Tech Stack:** Rust (axum, aws-sdk-dynamodb hand-built items, reqwest via `QuipClient`, Redis-Streams `JobQueue`), Leptos 0.7 CSR frontend, wiremock (Quip HTTP fakes), dynamodb-local + Redis for integration tests.

## Global Constraints

- **The Quip token never leaves the `TokenStore`.** It must never appear in a job envelope, a DynamoDB row, an S3 object, a log line, an error message, or any `Debug` output. The `StartQuipImport` job carries only `import_id` + `owner_id`; the worker re-reads the token from `TokenStore::get`. (design §Security & privacy; §"Enqueue path".)
- **Per-import `QuipClient`.** The worker constructs a fresh `QuipClient::new(None)` per `StartQuipImport` job so concurrent imports do not share one 45/min budget. Do NOT reuse `AppState.quip_client` in the worker path. (Resolves post-merge review Finding 4.)
- **Manifest rows are owner-gated and token-free.** Every new row (`FOLDER#`, `THREAD#`) carries `owner_id`; no row carries a token/secret (mirror the existing `import_item_has_no_token_field` guard).
- **Idempotent resume via conditional writes.** Per-thread checkpoints use `attribute_not_exists(SK)` inserts so a re-run never downgrades a thread that has advanced past `Pending` (Phase 2+ safety). Inventory itself is naturally re-runnable.
- **Raw `String` identifiers throughout** (`identifier_strategy = "string-grandfathered"`). No newtypes on IDs.
- **Additive/flagged changes only.** New `Job` enum variant, new manifest SK shapes, new routes, new DTOs — all additive. The `Job` enum's serde wire tags are a cross-target contract (pinned by worker tests); a new variant tag `startQuipImport` is additive.
- **Wizard modal-close discipline:** use `a11y::defer_close` / focus-trap patterns already in `components/quip_import/mod.rs`; never synchronously tear down a `<Show>` in an `on:click` (avoids the "modal-close panic" class).

---

## Design decisions taken (flag at review)

1. **`start` returns `202` immediately, not a synchronous estimate.** The design table shows `POST /{id}/start → { estimate }`, but computing an estimate requires the full folder-tree BFS, which can be many throttled Quip calls. Doing that inside the HTTP handler risks request timeouts and duplicates the worker's work. Instead: `start` enqueues and returns `202 { import_id, status }`; the **worker** computes `total` (thread count) during inventory and writes it to `META`; `GET /{id}` surfaces `progress { done, total, stage }`, and the wizard renders the time estimate as `total / 45 min` client-side. Net effect matches the design's intent (user sees an estimate) without a slow synchronous walk.

2. **Phase-1 `runner_claim` is a DynamoDB lease on `META`** (`{ instance_id, heartbeat_ms }`), net-new (the current worker has no heartbeat — only the Redis PEL/XAUTOCLAIM reaper). It guards against two workers (original + reaper-reclaimed) inventorying concurrently. A claim is acquired conditionally (absent or `heartbeat_ms` older than `CLAIM_STALE_MS`), heart-beaten during the walk, and cleared on terminal state.

---

## File Structure

**Created:**
- `crates/storage/src/models/import_inventory.rs` — `FolderRow`, `ThreadRow`, `ThreadState`, `RunnerClaim` models + their SK helpers.
- `crates/quip-import/src/inventory.rs` — the pure BFS walker (`walk_inventory`) + `Inventory` result type, plus the `QuipThread` DTO's home if not in `client.rs`.
- `crates/api/tests/test_quip_start.rs` — integration tests for `POST /{id}/start` + `GET /{id}`.
- `crates/api/tests/test_quip_inventory_worker.rs` — integration tests for the worker inventory handler (real Dynamo + wiremock Quip).

**Modified:**
- `crates/storage/src/models/mod.rs` — `pub mod import_inventory;`.
- `crates/storage/src/repo/import_repo.rs` — add `put_folder`, `put_thread`, `list_threads`, `count_threads_by_state`, `set_inventory_total`, `set_phase`, `claim_runner`, `heartbeat_runner`, `clear_runner_claim`.
- `crates/quip-import/src/client.rs` — add `QuipClient::threads(ids)` + `QuipThread` DTO.
- `crates/quip-import/src/lib.rs` — `pub mod inventory;` + re-exports.
- `crates/worker/src/lib.rs` — add `Job::StartQuipImport { import_id, owner_id }` (`:59-84`); extend `owner_of` (`:116-123`).
- `crates/api/src/worker_mode.rs` — extend `WorkerCtx` (`:60-70`) with `import_repo` + `quip_token_store`; add the `execute_start_quip_import` handler + dispatch arm in `execute` (`:307-357`).
- `crates/api/src/routes/imports.rs` — add `start` + `get_status` handlers; register routes in `router()` (`:30-32`).
- `crates/api/src/state.rs` — no new fields (worker builds its own `QuipClient`; `import_repo` + `quip_token_store` already on `AppState`, thread them into `WorkerCtx`).
- `crates/api/src/main.rs` — pass `import_repo` + `quip_token_store` into `WorkerCtx::new` (worker-mode branch).
- `frontend/src/api/imports.rs` — add `start` + `get_status` client fns + DTOs.
- `frontend/src/components/quip_import/mod.rs` — wire the Continue button → `start` → progress-polling view.
- `frontend/locales/en-US/main.ftl` (+ the 5 other locales) — Phase-1 strings; remove the `quip-import-continue-hint` "coming soon" copy.

---

## Task 1: Storage — inventory manifest rows + `ImportRepo` methods

**Files:**
- Create: `crates/storage/src/models/import_inventory.rs`
- Modify: `crates/storage/src/models/mod.rs` (add `pub mod import_inventory;`)
- Modify: `crates/storage/src/repo/import_repo.rs`
- Test: unit tests in both files; `crates/storage/tests/test_import_repo.rs` (extend, if it exists) for the live-Dynamo methods.

**Interfaces:**
- Consumes: `crate::dynamo::DynamoClient` (`get_item`, `put_item`, `put_item_conditional`, `query`, `update_item_conditional`), `crate::repo::{RepoError, get_s, get_n}`.
- Produces (used by Task 3 worker + Task 4 API):
  - `ThreadState` enum: `Pending | ContentDone | CommentsDone | Skipped`.
  - `FolderRow { quip_folder_id, owner_id, title, parent_quip_id: Option<String>, ogre_folder_id: Option<String> }`, `FolderRow::sk(&self) -> String` = `format!("FOLDER#{}", self.quip_folder_id)`.
  - `ThreadRow { quip_thread_id, owner_id, title, thread_type: String, updated_usec: i64, member_folders: Vec<String>, first_folder: String, state: ThreadState, ogre_doc_id: Option<String> }`, `ThreadRow::sk(&self) -> String` = `format!("THREAD#{}", self.quip_thread_id)`.
  - `ImportRepo::put_folder(&self, import_id, &FolderRow) -> Result<(), RepoError>`
  - `ImportRepo::put_thread(&self, import_id, &ThreadRow) -> Result<(), RepoError>` — **conditional `attribute_not_exists(SK)`**, mapping the conditional-failure error to `Ok(())` (already inventoried → leave as-is).
  - `ImportRepo::list_threads(&self, import_id) -> Result<Vec<ThreadRow>, RepoError>` — `query(pk, "THREAD#")`.
  - `ImportRepo::count_threads_by_state(&self, import_id) -> Result<(usize /*total*/, usize /*done_past_pending*/), RepoError>`.
  - `ImportRepo::set_inventory_total(&self, import_id, total: usize) -> Result<(), RepoError>` (writes `inventory_total` on `META`).
  - `ImportRepo::set_phase(&self, import_id, phase: u8) -> Result<(), RepoError>`.
  - `ImportRepo::claim_runner(&self, import_id, instance_id: &str, now_ms: i64, stale_ms: i64) -> Result<bool, RepoError>` — conditional claim; `Ok(true)` acquired, `Ok(false)` held by a live runner.
  - `ImportRepo::heartbeat_runner(&self, import_id, instance_id: &str, now_ms: i64) -> Result<(), RepoError>`.
  - `ImportRepo::clear_runner_claim(&self, import_id) -> Result<(), RepoError>`.

- [ ] **Step 1: Write the failing model test** (`crates/storage/src/models/import_inventory.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_state_serializes_lowercase_and_round_trips() {
        for (s, tok) in [
            (ThreadState::Pending, "pending"),
            (ThreadState::ContentDone, "contentdone"),
            (ThreadState::CommentsDone, "commentsdone"),
            (ThreadState::Skipped, "skipped"),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(j.trim_matches('"'), tok);
            let back: ThreadState = serde_json::from_str(&j).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn row_sk_formats() {
        let f = FolderRow { quip_folder_id: "qf1".into(), owner_id: "u1".into(),
            title: "Root".into(), parent_quip_id: None, ogre_folder_id: None };
        assert_eq!(f.sk(), "FOLDER#qf1");
        let t = ThreadRow { quip_thread_id: "qt1".into(), owner_id: "u1".into(),
            title: "Doc".into(), thread_type: "document".into(), updated_usec: 5,
            member_folders: vec!["qf1".into()], first_folder: "qf1".into(),
            state: ThreadState::Pending, ogre_doc_id: None };
        assert_eq!(t.sk(), "THREAD#qt1");
    }
}
```

- [ ] **Step 2: Run it — expect FAIL** (`ThreadState`/`FolderRow`/`ThreadRow` undefined)

Run: `cargo test -p ogrenotes-storage import_inventory:: -- --nocapture`
Expected: FAIL to compile — "cannot find type `ThreadState`".

- [ ] **Step 3: Write the models** (`crates/storage/src/models/import_inventory.rs`)

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Inventory rows for the Quip import manifest (Phase 1). All rows share
//! the import partition `PK = IMPORT#<import_id>`; none carries a token.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Pending,
    ContentDone,
    CommentsDone,
    Skipped,
}

/// One folder discovered during inventory BFS. SK = `FOLDER#<quip_folder_id>`.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    pub quip_folder_id: String,
    pub owner_id: String,
    pub title: String,
    pub parent_quip_id: Option<String>,
    pub ogre_folder_id: Option<String>,
}

impl FolderRow {
    pub fn sk(&self) -> String {
        format!("FOLDER#{}", self.quip_folder_id)
    }
}

/// One thread (doc/spreadsheet/chat) discovered during inventory. SK =
/// `THREAD#<quip_thread_id>`. This is the per-thread resumability unit.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub quip_thread_id: String,
    pub owner_id: String,
    pub title: String,
    pub thread_type: String,
    pub updated_usec: i64,
    /// Every folder the thread appears in (multi-folder membership).
    pub member_folders: Vec<String>,
    /// First folder it was encountered in during BFS (stable ordering).
    pub first_folder: String,
    pub state: ThreadState,
    pub ogre_doc_id: Option<String>,
}

impl ThreadRow {
    pub fn sk(&self) -> String {
        format!("THREAD#{}", self.quip_thread_id)
    }
}
```

Also add `pub mod import_inventory;` to `crates/storage/src/models/mod.rs`.

- [ ] **Step 4: Run the model test — expect PASS**

Run: `cargo test -p ogrenotes-storage import_inventory::`
Expected: PASS.

- [ ] **Step 5: Write the failing repo round-trip test** (append to `crates/storage/src/repo/import_repo.rs` `tests` module)

```rust
    use crate::models::import_inventory::{FolderRow, ThreadRow, ThreadState};

    // Pure item (de)serialization round-trips — no live Dynamo needed.
    #[test]
    fn folder_row_item_round_trips() {
        let f = FolderRow { quip_folder_id: "qf1".into(), owner_id: "u1".into(),
            title: "Root".into(), parent_quip_id: Some("qp".into()),
            ogre_folder_id: Some("of1".into()) };
        let back = folder_from_item(&folder_to_item(&f)).expect("from_item");
        assert_eq!(back, f);
    }

    #[test]
    fn thread_row_item_round_trips_and_has_no_token() {
        let t = ThreadRow { quip_thread_id: "qt1".into(), owner_id: "u1".into(),
            title: "Doc".into(), thread_type: "document".into(), updated_usec: 42,
            member_folders: vec!["qf1".into(), "qf2".into()], first_folder: "qf1".into(),
            state: ThreadState::Pending, ogre_doc_id: None };
        let item = thread_to_item(&t);
        assert!(!item.contains_key("token") && !item.contains_key("secret"));
        assert_eq!(thread_from_item(&item).expect("from_item"), t);
    }
```

- [ ] **Step 6: Run it — expect FAIL** (`folder_to_item` etc. undefined)

Run: `cargo test -p ogrenotes-storage import_repo::tests::folder_row_item_round_trips`
Expected: FAIL — "cannot find function `folder_to_item`".

- [ ] **Step 7: Implement the item mappers + repo methods** (`crates/storage/src/repo/import_repo.rs`)

Add near the existing `import_to_item`/`import_from_item` (mirror their hand-built `AttributeValue` style; use `get_s`/`get_n`; store `member_folders` as `AttributeValue::L` of `S`; store `updated_usec` as `N`; `state` as `S` via serde like `status_from_item` does):

```rust
use crate::models::import_inventory::{FolderRow, ThreadRow, ThreadState};

fn folder_to_item(f: &FolderRow) -> HashMap<String, AttributeValue> { /* mirror import_to_item; sparse Options */ }
fn folder_from_item(item: &HashMap<String, AttributeValue>) -> Result<FolderRow, RepoError> { /* mirror import_from_item */ }
fn thread_state_to_str(s: ThreadState) -> &'static str {
    match s { ThreadState::Pending => "pending", ThreadState::ContentDone => "contentdone",
              ThreadState::CommentsDone => "commentsdone", ThreadState::Skipped => "skipped" }
}
fn thread_state_from_item(item: &HashMap<String, AttributeValue>) -> Result<ThreadState, RepoError> {
    let raw = get_s(item, "state")?;
    serde_json::from_str(&format!("\"{raw}\"")).map_err(|e| RepoError::MissingField(format!("state: {e}")))
}
fn thread_to_item(t: &ThreadRow) -> HashMap<String, AttributeValue> { /* member_folders as L(S); state via thread_state_to_str */ }
fn thread_from_item(item: &HashMap<String, AttributeValue>) -> Result<ThreadRow, RepoError> { /* ... */ }
```

Then the `impl ImportRepo` methods:

```rust
pub async fn put_folder(&self, import_id: &str, f: &FolderRow) -> Result<(), RepoError> {
    let mut item = folder_to_item(f);
    item.insert("PK".into(), AttributeValue::S(format!("IMPORT#{import_id}")));
    item.insert("SK".into(), AttributeValue::S(f.sk()));
    self.db.put_item(item).await.map_err(|e| RepoError::Dynamo(e.to_string()))
}

/// Insert-if-absent: a re-run must never downgrade a thread that has
/// advanced past `Pending` (Phase 2+). A conditional-check failure means
/// the row already exists — treat as success, leave it as-is.
pub async fn put_thread(&self, import_id: &str, t: &ThreadRow) -> Result<(), RepoError> {
    let mut item = thread_to_item(t);
    item.insert("PK".into(), AttributeValue::S(format!("IMPORT#{import_id}")));
    item.insert("SK".into(), AttributeValue::S(t.sk()));
    match self.db.put_item_conditional(item, "attribute_not_exists(SK)").await {
        Ok(()) => Ok(()),
        Err(e) if is_conditional_check_failure(&e) => Ok(()),
        Err(e) => Err(RepoError::Dynamo(e.to_string())),
    }
}

pub async fn list_threads(&self, import_id: &str) -> Result<Vec<ThreadRow>, RepoError> {
    let items = self.db.query(&format!("IMPORT#{import_id}"), Some("THREAD#")).await
        .map_err(|e| RepoError::Dynamo(e.to_string()))?;
    items.iter().map(thread_from_item).collect()
}

pub async fn count_threads_by_state(&self, import_id: &str) -> Result<(usize, usize), RepoError> {
    let rows = self.list_threads(import_id).await?;
    let total = rows.len();
    let done = rows.iter().filter(|r| r.state != ThreadState::Pending).count();
    Ok((total, done))
}
```

For `is_conditional_check_failure`, follow whatever the repo already does for conditional errors (check `crate::dynamo` / existing repos for a helper; if none, match the SDK error string for `ConditionalCheckFailedException`). Add `set_inventory_total`, `set_phase` (both `update_item` SET like `set_status`), and the runner-claim methods:

```rust
/// Acquire the inventory lease. Succeeds if no claim exists or the
/// existing claim's heartbeat is older than `stale_ms`. Uses a
/// conditional update so two workers cannot both acquire.
pub async fn claim_runner(&self, import_id: &str, instance_id: &str, now_ms: i64, stale_ms: i64)
    -> Result<bool, RepoError>
{
    let pk = format!("IMPORT#{import_id}");
    let mut values = HashMap::new();
    values.insert(":inst".into(), AttributeValue::S(instance_id.to_string()));
    values.insert(":now".into(), AttributeValue::N(now_ms.to_string()));
    values.insert(":stale".into(), AttributeValue::N((now_ms - stale_ms).to_string()));
    // condition: no claim, OR same instance, OR heartbeat older than cutoff.
    let cond = "attribute_not_exists(runner_instance) OR runner_instance = :inst OR runner_heartbeat_ms < :stale";
    match self.db.update_item_conditional(
        &pk, ImportRecord::sk(),
        "SET runner_instance = :inst, runner_heartbeat_ms = :now",
        values, None, cond,
    ).await {
        Ok(()) => Ok(true),
        Err(e) if is_conditional_check_failure(&e) => Ok(false),
        Err(e) => Err(RepoError::Dynamo(e.to_string())),
    }
}
```

`heartbeat_runner` = `update_item_conditional` SET `runner_heartbeat_ms = :now` with condition `runner_instance = :inst` (drop the heartbeat if we lost the lease). `clear_runner_claim` = `update_item` REMOVE `runner_instance, runner_heartbeat_ms`.

> Note: verify `DynamoClient::put_item` takes just the item map and `update_item_conditional(pk, sk, expr, values, names, condition)` — adapt arg order to the real signatures at `crates/storage/src/dynamo.rs:48,287`.

- [ ] **Step 8: Run the repo unit tests — expect PASS**

Run: `cargo test -p ogrenotes-storage import_repo::`
Expected: PASS (item round-trips + no-token guard).

- [ ] **Step 9: Add a live-Dynamo integration test for conditional resume** (`crates/storage/tests/test_import_repo.rs` — follow the file's existing `require_infra!`/local-Dynamo harness if present; otherwise gate the same way sibling storage tests do)

```rust
#[tokio::test]
async fn put_thread_is_insert_if_absent() {
    // ... set up ImportRepo against dynamodb-local, create a META record ...
    let t0 = pending_thread("qt1", ThreadState::Pending);
    repo.put_thread("imp1", &t0).await.unwrap();
    // Simulate Phase 2 advancing the row:
    let mut advanced = t0.clone();
    advanced.state = ThreadState::ContentDone;
    // A second inventory run tries to (re)insert the Pending version:
    repo.put_thread("imp1", &advanced).await.unwrap();      // seed advanced
    repo.put_thread("imp1", &t0).await.unwrap();            // re-run, Pending
    let rows = repo.list_threads("imp1").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, ThreadState::ContentDone, "re-run must not downgrade");
}
```

- [ ] **Step 10: Run it — expect PASS** (skip cleanly if infra gate unmet)

Run: `cargo test -p ogrenotes-storage --test test_import_repo put_thread_is_insert_if_absent`
Expected: PASS (or ignored if `require_infra!` unmet — then run against local Dynamo per the repo's test README).

- [ ] **Step 11: Commit**

```bash
git add crates/storage/src/models/import_inventory.rs crates/storage/src/models/mod.rs \
        crates/storage/src/repo/import_repo.rs crates/storage/tests/test_import_repo.rs
git commit -m "feat(storage): Quip import inventory rows + resumable ImportRepo methods"
```

---

## Task 2: quip-import — `/1/threads/` endpoint + pure BFS inventory walker

**Files:**
- Modify: `crates/quip-import/src/client.rs` (add `QuipThread` DTO + `QuipClient::threads`)
- Create: `crates/quip-import/src/inventory.rs`
- Modify: `crates/quip-import/src/lib.rs` (`pub mod inventory;`)
- Test: wiremock test in `client.rs`; pure-function tests in `inventory.rs`.

**Interfaces:**
- Consumes: `QuipClient::folders(&token, &ids) -> Vec<QuipFolder>` (exists), `QuipFolder { id, title, children: Vec<QuipFolderChild{thread_id, folder_id}> }` (exists), `QuipToken`, `QuipError`.
- Produces (used by Task 3):
  - `QuipThread { id: String, title: String, thread_type: String, updated_usec: i64 }` (from `/1/threads/`).
  - `QuipClient::threads(&self, t: &QuipToken, ids: &[String]) -> Result<Vec<QuipThread>, QuipError>`.
  - `Inventory { folders: Vec<InvFolder>, threads: Vec<InvThread> }` where
    `InvFolder { quip_folder_id, title, parent_quip_id: Option<String> }` and
    `InvThread { quip_thread_id, member_folders: Vec<String>, first_folder: String }`.
  - `async fn walk_inventory<F, Fut>(roots: &[String], fetch_folders: F) -> Result<Inventory, QuipError>`
    where `F: Fn(Vec<String>) -> Fut`, `Fut: Future<Output = Result<Vec<QuipFolder>, QuipError>>` — the BFS is decoupled from the network via this fetch closure so it is unit-testable with an in-memory fixture. Thread **metadata** (title/type/updated_usec) is fetched separately by the caller (Task 3) via `QuipClient::threads`; the walker only discovers thread IDs + membership.

- [ ] **Step 1: Write the failing `threads` wiremock test** (`crates/quip-import/src/client.rs` tests)

```rust
    #[tokio::test]
    async fn threads_joins_ids_and_parses_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/1/threads/"))
            .and(header("authorization", "Bearer tok-t"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "t1": {"thread": {"id":"t1","title":"Doc A","type":"document","updated_usec": 111}},
                "t2": {"thread": {"id":"t2","title":"Sheet","type":"spreadsheet","updated_usec": 222}}
            })))
            .mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        let mut ts = c.threads(&QuipToken::new("tok-t".into()),
                               &["t1".into(), "t2".into()]).await.unwrap();
        ts.sort_by(|a,b| a.id.cmp(&b.id));
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].id, "t1");
        assert_eq!(ts[0].thread_type, "document");
        assert_eq!(ts[1].updated_usec, 222);
    }
```

- [ ] **Step 2: Run it — expect FAIL** (`threads` undefined)

Run: `cargo test -p ogrenotes-quip-import threads_joins_ids`
Expected: FAIL — "no method named `threads`".

- [ ] **Step 3: Implement `QuipThread` + `threads`** (`crates/quip-import/src/client.rs`, mirroring `folders`)

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct QuipThread {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "type")]
    pub thread_type: String,
    #[serde(default)]
    pub updated_usec: i64,
}

#[derive(Debug, Deserialize)]
struct ThreadEnvelope { thread: QuipThread }

impl QuipClient {
    /// `GET /1/threads/?ids=<comma-joined>`.
    pub async fn threads(&self, t: &QuipToken, ids: &[String]) -> Result<Vec<QuipThread>, QuipError> {
        if ids.is_empty() { return Ok(Vec::new()); }
        self.throttle.acquire().await;
        let resp = self.http.get(format!("{}/1/threads/", self.base))
            .bearer_auth(t.expose())
            .query(&[("ids", ids.join(","))])
            .send().await?;
        let body: std::collections::HashMap<String, ThreadEnvelope> =
            self.observe_and_check(resp).await?.json_body().await?;
        Ok(body.into_values().map(|e| e.thread).collect())
    }
}
```

- [ ] **Step 4: Run it — expect PASS**

Run: `cargo test -p ogrenotes-quip-import threads_joins_ids`
Expected: PASS.

- [ ] **Step 5: Write failing BFS walker tests** (`crates/quip-import/src/inventory.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{QuipFolder, QuipFolderChild};
    use std::collections::HashMap;

    fn child_thread(id: &str) -> QuipFolderChild { QuipFolderChild { thread_id: Some(id.into()), folder_id: None } }
    fn child_folder(id: &str) -> QuipFolderChild { QuipFolderChild { thread_id: None, folder_id: Some(id.into()) } }

    // Fixture: root -> [thread t1, subfolder f2]; f2 -> [thread t1 (shared), t2].
    fn fixture() -> HashMap<String, QuipFolder> {
        HashMap::from([
            ("root".into(), QuipFolder { id: "root".into(), title: "Root".into(),
                children: vec![child_thread("t1"), child_folder("f2")] }),
            ("f2".into(), QuipFolder { id: "f2".into(), title: "Sub".into(),
                children: vec![child_thread("t1"), child_thread("t2")] }),
        ])
    }

    async fn fetch(ids: Vec<String>, fx: &HashMap<String, QuipFolder>) -> Result<Vec<QuipFolder>, crate::client::QuipError> {
        Ok(ids.iter().filter_map(|id| fx.get(id).cloned()).collect())
    }

    #[tokio::test]
    async fn bfs_discovers_all_and_dedups_shared_thread() {
        let fx = fixture();
        let inv = walk_inventory(&["root".into()], |ids| fetch(ids, &fx)).await.unwrap();
        assert_eq!(inv.folders.len(), 2, "root + f2");
        let t1 = inv.threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
        assert_eq!(t1.first_folder, "root");
        let mut mf = t1.member_folders.clone(); mf.sort();
        assert_eq!(mf, vec!["f2".to_string(), "root".to_string()], "shared thread lists both folders once");
        assert_eq!(inv.threads.iter().filter(|t| t.quip_thread_id == "t1").count(), 1, "no duplicate rows");
    }

    #[tokio::test]
    async fn bfs_terminates_on_cycle() {
        // f_a -> f_b -> f_a (cycle). Must not infinite-loop.
        let fx = HashMap::from([
            ("a".to_string(), QuipFolder { id: "a".into(), title: "A".into(), children: vec![child_folder("b")] }),
            ("b".to_string(), QuipFolder { id: "b".into(), title: "B".into(), children: vec![child_folder("a")] }),
        ]);
        let inv = walk_inventory(&["a".into()], |ids| fetch(ids, &fx)).await.unwrap();
        assert_eq!(inv.folders.len(), 2);
    }
}
```

- [ ] **Step 6: Run — expect FAIL** (`walk_inventory` undefined)

Run: `cargo test -p ogrenotes-quip-import inventory::`
Expected: FAIL to compile.

- [ ] **Step 7: Implement the walker** (`crates/quip-import/src/inventory.rs`)

```rust
//! Pure BFS over a selected Quip folder tree. Decoupled from the network
//! via a `fetch_folders` closure so it is unit-testable with an in-memory
//! fixture. Discovers folders + thread IDs with multi-folder membership;
//! thread *metadata* is fetched separately by the caller.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;

use crate::client::{QuipError, QuipFolder};

#[derive(Debug, Clone, PartialEq)]
pub struct InvFolder { pub quip_folder_id: String, pub title: String, pub parent_quip_id: Option<String> }

#[derive(Debug, Clone, PartialEq)]
pub struct InvThread { pub quip_thread_id: String, pub member_folders: Vec<String>, pub first_folder: String }

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory { pub folders: Vec<InvFolder>, pub threads: Vec<InvThread> }

/// BFS from `roots`, fetching folders in batches via `fetch_folders`.
/// `visited` guards cycles; threads are deduped, accumulating
/// `member_folders` (first encounter sets `first_folder`).
pub async fn walk_inventory<F, Fut>(roots: &[String], fetch_folders: F) -> Result<Inventory, QuipError>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<QuipFolder>, QuipError>>,
{
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut folders: Vec<InvFolder> = Vec::new();
    // thread id -> (member_folders in insertion order, first_folder)
    let mut threads: HashMap<String, InvThread> = HashMap::new();
    let mut thread_order: Vec<String> = Vec::new();

    while !queue.is_empty() {
        // Batch this BFS level (Quip /1/folders/ takes multiple ids).
        let batch: Vec<String> = queue.drain(..).filter(|id| visited.insert(id.clone())).collect();
        if batch.is_empty() { continue; }
        for folder in fetch_folders(batch).await? {
            folders.push(InvFolder { quip_folder_id: folder.id.clone(), title: folder.title.clone(), parent_quip_id: None });
            for child in &folder.children {
                if let Some(sub) = &child.folder_id {
                    if !visited.contains(sub) { queue.push_back(sub.clone()); }
                }
                if let Some(tid) = &child.thread_id {
                    let entry = threads.entry(tid.clone()).or_insert_with(|| {
                        thread_order.push(tid.clone());
                        InvThread { quip_thread_id: tid.clone(), member_folders: Vec::new(), first_folder: folder.id.clone() }
                    });
                    if !entry.member_folders.contains(&folder.id) { entry.member_folders.push(folder.id.clone()); }
                }
            }
        }
    }
    let threads = thread_order.into_iter().map(|id| threads.remove(&id).unwrap()).collect();
    Ok(Inventory { folders, threads })
}
```

> `parent_quip_id` is left `None` here (children reference parents, not vice-versa; Phase 1 doesn't need the reverse edge for the demo). If a later phase needs it, thread it through the queue as `(id, parent)` — noted, not built.

Add `pub mod inventory;` and `pub use inventory::{Inventory, InvFolder, InvThread, walk_inventory};` to `crates/quip-import/src/lib.rs`; re-export `QuipThread` alongside the existing client exports.

- [ ] **Step 8: Run walker tests — expect PASS**

Run: `cargo test -p ogrenotes-quip-import inventory::`
Expected: PASS (dedup + cycle).

- [ ] **Step 9: Commit**

```bash
git add crates/quip-import/src/client.rs crates/quip-import/src/inventory.rs crates/quip-import/src/lib.rs
git commit -m "feat(quip-import): /1/threads endpoint + pure BFS inventory walker"
```

---

## Task 3: worker — `StartQuipImport` job + inventory handler + resume

**Files:**
- Modify: `crates/worker/src/lib.rs` (`Job` enum `:59-84`, `owner_of` `:116-123`, add wire-tag test)
- Modify: `crates/api/src/worker_mode.rs` (`WorkerCtx` `:60-70`, `execute` dispatch `:307-357`, new handler)
- Modify: `crates/api/src/main.rs` (build `WorkerCtx` with the two new deps)
- Test: `crates/worker/src/lib.rs` unit (wire tag + owner); `crates/api/tests/test_quip_inventory_worker.rs` integration.

**Interfaces:**
- Consumes: `ImportRepo` (Task 1 methods), `TokenStore::get` (`crates/quip-import`), `QuipClient::{folders,threads}` (Task 2), `walk_inventory` (Task 2), `FolderRow`/`ThreadRow`/`ThreadState` (Task 1).
- Produces (used by the run loop + tests):
  - `Job::StartQuipImport { import_id: String, owner_id: String }` (serde tag `startQuipImport`).
  - `WorkerCtx` gains `import_repo: Arc<ImportRepo>` and `quip_token_store: Arc<dyn TokenStore>`.
  - `pub async fn execute_start_quip_import(ctx: &WorkerCtx, import_id: &str, owner_id: &str) -> Result<(), String>` (pub for the test seam, mirroring `execute_import_docx`).

- [ ] **Step 1: Write the failing wire-tag + owner test** (`crates/worker/src/lib.rs` tests, next to the existing tag pins ~`:721`)

```rust
    #[test]
    fn start_quip_import_wire_tag_and_owner() {
        let job = Job::StartQuipImport { import_id: "imp1".into(), owner_id: "u1".into() };
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains(r#""type":"startQuipImport""#), "wire tag pinned: {json}");
        let back: Job = serde_json::from_str(&json).unwrap();
        assert_eq!(back, job);
        // owner_of must return the owner so poll-auth (#85) doesn't silently
        // fall through to ownerless.
        assert_eq!(owner_of(&job), Some("u1"));
    }
```

- [ ] **Step 2: Run — expect FAIL** (variant undefined)

Run: `cargo test -p ogrenotes-worker start_quip_import_wire_tag`
Expected: FAIL to compile — "no variant `StartQuipImport`".

- [ ] **Step 3: Add the variant + extend `owner_of`** (`crates/worker/src/lib.rs`)

```rust
// in the Job enum (:59-84), add:
    /// Token-free trigger for a checkpointed Quip import (Phase 1+). The
    /// token is NEVER carried here — the worker re-reads it from the
    /// TokenStore keyed by import_id. See design §"Enqueue path".
    StartQuipImport { import_id: String, owner_id: String },
```

```rust
// owner_of (:116-123):
fn owner_of(payload: &Job) -> Option<&str> {
    match payload {
        Job::ImportDocx { owner_id, .. } | Job::ImportPdf { owner_id, .. } => Some(owner_id.as_str()),
        Job::StartQuipImport { owner_id, .. } => Some(owner_id.as_str()),
        Job::Noop { .. } => None,
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p ogrenotes-worker start_quip_import_wire_tag`
Expected: PASS.

- [ ] **Step 5: Extend `WorkerCtx` + write the failing handler integration test** (`crates/api/tests/test_quip_inventory_worker.rs`)

Use the existing worker-mode test harness (`worker_ctx(&app)` builder pattern from `crates/api/tests/test_worker_mode.rs`, `require_infra!` gate) plus a wiremock Quip server; seed the token into the in-memory `TokenStore` on the test `AppState`. The handler is driven directly (the `pub` seam).

```rust
mod common;
use ogrenotes_quip_import::QuipToken;

#[tokio::test]
async fn inventory_walk_persists_folders_and_threads_and_total() {
    common::require_infra!();
    let server = quip_fixture_server().await; // /1/folders/ + /1/threads/ fixtures: root->[t1,f2], f2->[t1,t2]
    let app = common::TestApp::new_with_quip_base(server.uri()).await; // Phase-0 dev seam
    let import_id = seed_scoping_import(&app, "owner1", &["root"]).await; // META Scoping + selected_roots
    app.state.quip_token_store.put(&import_id, &QuipToken::new("tok".into())).await.unwrap();

    let ctx = common::worker_ctx(&app);
    ogrenotes_api::worker_mode::execute_start_quip_import(&ctx, &import_id, "owner1").await.unwrap();

    let threads = app.state.import_repo.list_threads(&import_id).await.unwrap();
    let ids: std::collections::BTreeSet<_> = threads.iter().map(|t| t.quip_thread_id.clone()).collect();
    assert_eq!(ids, ["t1", "t2"].iter().map(|s| s.to_string()).collect());
    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.phase, 1);
    // total surfaced for the estimate:
    let (total, done) = app.state.import_repo.count_threads_by_state(&import_id).await.unwrap();
    assert_eq!((total, done), (2, 0));
}

#[tokio::test]
async fn inventory_is_idempotent_on_rerun() {
    common::require_infra!();
    // ... same setup ... run execute_start_quip_import twice; assert thread count stays 2,
    // and a thread pre-advanced to ContentDone between runs stays ContentDone.
}

#[tokio::test]
async fn inventory_token_rejected_sets_status() {
    common::require_infra!();
    // wiremock returns 401 on /1/folders/; assert the handler Errs AND the
    // META status becomes TokenRejected (not a generic Failed).
}
```

- [ ] **Step 6: Run — expect FAIL** (`execute_start_quip_import` / ctx fields undefined)

Run: `cargo test -p ogrenotes-api --test test_quip_inventory_worker`
Expected: FAIL to compile.

- [ ] **Step 7: Implement `WorkerCtx` fields + the handler + dispatch arm** (`crates/api/src/worker_mode.rs`)

Extend `WorkerCtx` (`:60-70`) and `WorkerCtx::new`:

```rust
pub struct WorkerCtx {
    pub doc_repo: Arc<DocumentRepo>,
    pub folder_repo: Arc<FolderRepo>,
    pub s3: Arc<S3Client>,
    pub import_repo: Arc<ImportRepo>,
    pub quip_token_store: Arc<dyn TokenStore>,
}
```

Handler (mirror `execute_import_docx`'s pub-seam style; **construct a per-import `QuipClient`**; heartbeat the claim):

```rust
const CLAIM_STALE_MS: i64 = 90_000; // > REAPER_MIN_IDLE_MS so the DB lease outlives one reaper cycle

/// Phase 1 inventory: claim the import, re-read the token, BFS-walk the
/// selected roots, persist FOLDER#/THREAD# rows (insert-if-absent →
/// resumable), record the thread total, advance to phase 1. Token-free
/// job trigger; the token is read from the store here and never logged.
pub async fn execute_start_quip_import(ctx: &WorkerCtx, import_id: &str, owner_id: &str) -> Result<(), String> {
    let instance = worker_instance_id(); // reuse the consumer name / hostname+pid
    let now_ms = ogrenotes_common::time::now_usec() / 1000;
    if !ctx.import_repo.claim_runner(import_id, &instance, now_ms, CLAIM_STALE_MS).await
        .map_err(|e| format!("claim: {e}"))? {
        // Another live runner owns this import — nothing to do (not an error).
        return Ok(());
    }

    let record = ctx.import_repo.get(import_id).await.map_err(|e| e.to_string())?
        .ok_or_else(|| format!("import {import_id} not found"))?;
    if record.owner_id != owner_id { return Err("owner mismatch".into()); }

    let token = match ctx.quip_token_store.get(import_id).await.map_err(|e| format!("token store: {e}"))? {
        Some(t) => t,
        None => { ctx.import_repo.set_status(import_id, ImportStatus::TokenRejected).await.ok(); 
                  return Err("no token in store".into()); }
    };

    let client = ogrenotes_quip_import::QuipClient::new(None); // per-import throttle
    ctx.import_repo.set_status(import_id, ImportStatus::Running).await.map_err(|e| e.to_string())?;

    // BFS. Heartbeat between fetches so the reaper doesn't reclaim.
    let inv = match ogrenotes_quip_import::walk_inventory(&record.selected_roots, |ids| {
        let (client, token) = (&client, &token);
        async move { client.folders(token, &ids).await }
    }).await {
        Ok(inv) => inv,
        Err(e) => { mark_quip_failure(ctx, import_id, &e).await; return Err(format!("inventory walk failed")); }
    };
    ctx.import_repo.heartbeat_runner(import_id, &instance, ogrenotes_common::time::now_usec()/1000).await.ok();

    // Persist folders.
    for f in &inv.folders {
        ctx.import_repo.put_folder(import_id, &FolderRow {
            quip_folder_id: f.quip_folder_id.clone(), owner_id: owner_id.to_string(),
            title: f.title.clone(), parent_quip_id: f.parent_quip_id.clone(), ogre_folder_id: None,
        }).await.map_err(|e| e.to_string())?;
    }

    // Fetch thread metadata in id-batches, then persist THREAD# rows (insert-if-absent).
    let meta = fetch_thread_meta(&client, &token, &inv).await
        .map_err(|e| { /* mark_quip_failure */ format!("thread meta") })?;
    for t in &inv.threads {
        let m = meta.get(&t.quip_thread_id);
        ctx.import_repo.put_thread(import_id, &ThreadRow {
            quip_thread_id: t.quip_thread_id.clone(), owner_id: owner_id.to_string(),
            title: m.map(|m| m.title.clone()).unwrap_or_default(),
            thread_type: m.map(|m| m.thread_type.clone()).unwrap_or_default(),
            updated_usec: m.map(|m| m.updated_usec).unwrap_or(0),
            member_folders: t.member_folders.clone(), first_folder: t.first_folder.clone(),
            state: ThreadState::Pending, ogre_doc_id: None,
        }).await.map_err(|e| e.to_string())?;
    }

    let (total, _) = ctx.import_repo.count_threads_by_state(import_id).await.map_err(|e| e.to_string())?;
    ctx.import_repo.set_inventory_total(import_id, total).await.map_err(|e| e.to_string())?;
    ctx.import_repo.set_phase(import_id, 1).await.map_err(|e| e.to_string())?;
    ctx.import_repo.clear_runner_claim(import_id).await.ok();
    Ok(())
}
```

Where `fetch_thread_meta` chunks `inv.threads` ids (e.g. 100 per call) through `client.threads(&token, &chunk)` and collects into a `HashMap<String, QuipThread>`, and `mark_quip_failure` maps `QuipError::Unauthorized` → `set_status(TokenRejected)` else leaves the job to retry/DLQ (do NOT set `Failed` on a transient `RateLimited`; the reaper/retry will resume). Add `worker_instance_id()` (reuse the consumer id already available in the loop, or `format!("{}-{}", hostname, pid)`).

Dispatch arm in `execute` (`:307-357`):

```rust
        Job::StartQuipImport { import_id, owner_id } => {
            execute_start_quip_import(ctx, import_id, owner_id).await?;
            Ok(Some(serde_json::json!({ "importId": import_id }).to_string()))
        }
```

Update `crates/api/src/main.rs` where `WorkerCtx::new(...)` is built (worker-mode branch, ~`worker_mode.rs:124`) to pass `import_repo` + `quip_token_store` (both already constructed for `AppState`; hoist them or rebuild the same way `main.rs` does for the API path).

- [ ] **Step 8: Run the worker integration tests — expect PASS** (against local infra)

Run: `cargo test -p ogrenotes-api --test test_quip_inventory_worker`
Expected: PASS (persist + total + idempotent + token-rejected); ignored if `require_infra!` unmet.

- [ ] **Step 9: Full-crate compile + existing worker tests still green**

Run: `cargo test -p ogrenotes-worker && cargo build -p ogrenotes-api`
Expected: PASS / Finished (the `Job` match is exhaustive everywhere — check `routes/jobs.rs` doesn't need a new arm; it only enqueues Noop, so no change).

- [ ] **Step 10: Commit**

```bash
git add crates/worker/src/lib.rs crates/api/src/worker_mode.rs crates/api/src/main.rs \
        crates/api/tests/test_quip_inventory_worker.rs
git commit -m "feat(worker): StartQuipImport trigger + resumable inventory handler"
```

---

## Task 4: api — `POST /imports/quip/{id}/start` + `GET /imports/quip/{id}`

**Files:**
- Modify: `crates/api/src/routes/imports.rs` (routes + handlers)
- Test: `crates/api/tests/test_quip_start.rs`

**Interfaces:**
- Consumes: `AuthUser` (owner), `state.import_repo`, `state.job_producer: Option<Arc<dyn JobProducer>>` (enqueue `Job::StartQuipImport`), `super::folders::check_folder_access(&state, &folder_id, &user_id, AccessLevel::Edit)`, `state.import_repo.count_threads_by_state`.
- Produces (wire contract for Task 5):
  - `POST /quip/{id}/start` body `{ selectedRootFolderIds: [String], targetFolderId: String }` → `202 { importId, status }`.
  - `GET /quip/{id}` → `{ status, phase, progress: { done, total, stage } }`.

- [ ] **Step 1: Write failing route tests** (`crates/api/tests/test_quip_start.rs`)

```rust
mod common;
use axum::http::StatusCode;

#[tokio::test]
async fn start_authorizes_target_folder_persists_scope_and_enqueues() {
    let app = common::TestApp::new().await;           // Phase-0 harness w/ fake JobProducer
    let (user, token) = app.dev_user("owner1").await;
    let folder = app.create_folder(&user, "Dest").await;
    let import_id = common::seed_scoping_import(&app, &user, &[]).await;

    let res = app.post_json(&format!("/api/v1/imports/quip/{import_id}/start"), &token,
        serde_json::json!({ "selectedRootFolderIds": ["root"], "targetFolderId": folder }))
        .await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);

    let rec = app.state.import_repo.get(&import_id).await.unwrap().unwrap();
    assert_eq!(rec.selected_roots, vec!["root".to_string()]);
    assert_eq!(rec.target_folder_id.as_deref(), Some(folder.as_str()));
    // enqueued exactly one StartQuipImport for this owner:
    assert_eq!(app.fake_producer_jobs().await.len(), 1);
}

#[tokio::test]
async fn start_rejects_unauthorized_target_folder() {
    let app = common::TestApp::new().await;
    let (owner, otok) = app.dev_user("owner1").await;
    let (other, _)   = app.dev_user("other").await;
    let their_folder = app.create_folder(&other, "Theirs").await;
    let import_id = common::seed_scoping_import(&app, &owner, &[]).await;
    let res = app.post_json(&format!("/api/v1/imports/quip/{import_id}/start"), &otok,
        serde_json::json!({ "selectedRootFolderIds": ["root"], "targetFolderId": their_folder })).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "check_folder_access hides unauthorized folders as 404");
}

#[tokio::test]
async fn get_status_returns_progress_and_is_owner_gated() {
    let app = common::TestApp::new().await;
    let (owner, otok) = app.dev_user("owner1").await;
    let (_other, xtok) = app.dev_user("other").await;
    let import_id = common::seed_scoping_import(&app, &owner, &[]).await;
    // owner sees it:
    let res = app.get(&format!("/api/v1/imports/quip/{import_id}"), &otok).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await;
    assert_eq!(body["progress"]["total"], 0);
    // a different user gets 404 (no existence disclosure):
    let res = app.get(&format!("/api/v1/imports/quip/{import_id}"), &xtok).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run — expect FAIL** (routes 404 / handlers undefined)

Run: `cargo test -p ogrenotes-api --test test_quip_start`
Expected: FAIL (routes not registered).

- [ ] **Step 3: Implement handlers + register routes** (`crates/api/src/routes/imports.rs`)

```rust
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/quip/connect", post(connect))
        .route("/quip/{id}/start", post(start))
        .route("/quip/{id}", get(get_status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest { selected_root_folder_ids: Vec<String>, target_folder_id: String }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartResponse { import_id: String, status: String }

async fn start(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(import_id): Path<String>,
    Json(req): Json<StartRequest>,
) -> Result<(StatusCode, Json<StartResponse>), ApiError> {
    // Owner-gate the import row.
    let record = state.import_repo.get(&import_id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .filter(|r| r.owner_id == user_id)
        .ok_or_else(|| ApiError::NotFound("import not found".into()))?;
    // Authorize the destination folder (hides unauthorized as 404, rejects Trash/system).
    super::folders::check_folder_access(&state, &req.target_folder_id, &user_id, AccessLevel::Edit).await?;
    // Persist scope on META. (Add ImportRepo::set_scope, or reuse update_item.)
    state.import_repo.set_scope(&import_id, &req.selected_root_folder_ids, &req.target_folder_id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    // Enqueue the token-free trigger.
    let producer = state.job_producer.as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("job queue unavailable".into()))?;
    producer.enqueue(ogrenotes_worker::Job::StartQuipImport {
        import_id: import_id.clone(), owner_id: user_id,
    }).await.map_err(|e| ApiError::ServiceUnavailable(format!("enqueue failed: {e}")))?;

    Ok((StatusCode::ACCEPTED, Json(StartResponse { import_id, status: "running".into() })))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse { status: String, phase: u8, progress: Progress }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress { done: usize, total: usize, stage: String }

async fn get_status(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Path(import_id): Path<String>,
) -> Result<Json<StatusResponse>, ApiError> {
    let record = state.import_repo.get(&import_id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .filter(|r| r.owner_id == user_id)
        .ok_or_else(|| ApiError::NotFound("import not found".into()))?;
    let (total, done) = state.import_repo.count_threads_by_state(&import_id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let stage = if record.phase >= 1 { "inventory" } else { "scoping" };
    Ok(Json(StatusResponse {
        status: serde_json::to_string(&record.status).unwrap().trim_matches('"').to_string(),
        phase: record.phase,
        progress: Progress { done, total, stage: stage.into() },
    }))
}
```

Add `ImportRepo::set_scope(import_id, roots: &[String], target: &str)` (a single `update_item` SET of `selected_roots` (L of S) + `target_folder_id` (S) + `updated_at`) in Task 1's file if not already present — or fold it into Task 1 (preferred; add it there and reference here). Add the needed imports (`Path`, `get`, `AccessLevel`, `ogrenotes_worker::Job`).

- [ ] **Step 4: Run route tests — expect PASS**

Run: `cargo test -p ogrenotes-api --test test_quip_start`
Expected: PASS.

- [ ] **Step 5: Confirm the connect flow test still passes** (regression)

Run: `cargo test -p ogrenotes-api --test test_quip_connect`
Expected: PASS (router change is additive).

- [ ] **Step 6: Commit**

```bash
git add crates/api/src/routes/imports.rs crates/api/tests/test_quip_start.rs
git commit -m "feat(api): POST /imports/quip/{id}/start + GET /imports/quip/{id} status"
```

---

## Task 5: frontend — wire Continue → start → live progress

**Files:**
- Modify: `frontend/src/api/imports.rs` (add `start`, `get_status` + DTOs)
- Modify: `frontend/src/components/quip_import/mod.rs` (Continue button + progress view)
- Modify: `frontend/locales/en-US/main.ftl` + the 5 sibling locales
- Test: `frontend/` builds for wasm32; extend `scripts/frontend-doctor/probe-quip-wizard.mjs` scenario.

**Interfaces:**
- Consumes: `client::api_post`, a `client::api_get` (confirm the helper name in `frontend/src/api/client.rs`), the wire contract from Task 4.
- Produces: `start(import_id, selected_root_ids: &[String], target_folder_id: &str) -> Result<StartResponse, ApiClientError>`, `get_status(import_id) -> Result<StatusResponse, ApiClientError>`.

- [ ] **Step 1: Add client fns + DTOs** (`frontend/src/api/imports.rs`)

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartRequest<'a> { selected_root_folder_ids: &'a [String], target_folder_id: &'a str }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse { pub import_id: String, pub status: String }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress { pub done: usize, pub total: usize, pub stage: String }

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse { pub status: String, pub phase: u8, pub progress: Progress }

pub async fn start(import_id: &str, selected_root_folder_ids: &[String], target_folder_id: &str)
    -> Result<StartResponse, ApiClientError>
{
    client::api_post(&format!("/imports/quip/{import_id}/start"),
        &StartRequest { selected_root_folder_ids, target_folder_id }).await
}

pub async fn get_status(import_id: &str) -> Result<StatusResponse, ApiClientError> {
    client::api_get(&format!("/imports/quip/{import_id}")).await
}
```

> Confirm `client::api_get` exists (grep `frontend/src/api/client.rs`); if the getter has a different name, use it.

- [ ] **Step 2: Wire the Continue button + progress view** (`frontend/src/components/quip_import/mod.rs`)

Replace the disabled/"coming soon" Continue affordance (the `quip-import-continue-hint` block) with an active button that, on click: collects the checked root ids from the `selected` signal + a target-folder choice (Phase 1 minimal: default to the user's Home/root folder — reuse whatever "New doc" uses for the default parent; a full target-folder picker is a later polish), calls `imports::start`, and switches the wizard into a **progress step** that polls `imports::get_status` on an interval (Leptos `set_interval` / an async resource with a signal tick) until `status` is terminal or `stage == "inventory"` with `done == total && total > 0`. Render `done / total` + an estimate line (`total / 45` → minutes). Use `a11y::defer_close` on any close.

Follow the existing wizard's `LocalResource`/`Action` + `set_error` idiom; do not block the modal thread. Keep the token cleared (Phase 0 already clears it after connect).

- [ ] **Step 3: Update i18n** — in each `frontend/locales/*/main.ftl`, remove `quip-import-continue-hint`, and add:

```
quip-import-starting = Starting import…
quip-import-progress-heading = Importing from Quip
quip-import-progress-count = { $done } of { $total } items
quip-import-estimate = About { $minutes } min at Quip's rate limit
quip-import-inventory-done = Found { $total } items to import
```

(Translate for de/es/fr/it/ar or copy English as a placeholder consistent with how the Phase-0 keys were seeded.)

- [ ] **Step 4: Build the frontend for wasm32**

Run: `cd frontend && cargo build --target wasm32-unknown-unknown` (or `trunk build`)
Expected: Finished — no wasm-only breakage. (Per repo guidance, `cargo check` natively is not sufficient for wasm-gated code.)

- [ ] **Step 5: Extend the doctor probe** (`scripts/frontend-doctor/probe-quip-wizard.mjs`)

Add a scenario (behind a mock or against a local stack with a wiremock Quip): connect → check a root → Continue → assert the progress view renders `x of y items`. Keep it resilient (skip gracefully if no local stack). This mirrors the Phase-0 probe already in the file.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/imports.rs frontend/src/components/quip_import/mod.rs \
        frontend/locales scripts/frontend-doctor/probe-quip-wizard.mjs
git commit -m "feat(frontend): Quip import Continue → start + live inventory progress"
```

---

## Self-Review

**Spec coverage (design §Pipeline Phase 1 + §Build order 1):**
- "Inventory + ImportRepo" → Tasks 1–3 (rows + repo + walker + handler). ✓
- "BFS selected folders → FOLDER#/THREAD# rows" → Task 2 walker + Task 3 persistence. ✓
- "Dedup / tag membership (member_folders, first_folder)" → Task 2 `walk_inventory` dedup test + Task 1 `ThreadRow.member_folders`. ✓
- "scope UI + estimate" → Task 4 `start`/`get_status` + Task 5 progress view (estimate = total/45, decision #1). ✓
- "worker trigger + claim + resume" → Task 3 `StartQuipImport` + `claim_runner`/heartbeat + insert-if-absent resume. ✓
- "Demo: scoped inventory persisted + resumable" → Task 3 integration tests (persist + idempotent rerun). ✓
- Security spine (token never in envelope/row/log) → Global Constraints + Task 3 token-free trigger + Task 1 no-token guard. ✓

**Placeholder scan:** No "TBD"/"add error handling"-style steps; every code step has real code or an explicit, bounded "mirror X / confirm signature at file:line" instruction. Two spots deliberately defer sub-features with a named reason (folder `parent_quip_id` reverse edge; full target-folder picker) — flagged, not silent.

**Type consistency:** `ThreadState`, `FolderRow`, `ThreadRow` defined in Task 1 and consumed with the same field names in Tasks 3–4. `walk_inventory`/`Inventory`/`InvThread` defined in Task 2, consumed in Task 3. `Job::StartQuipImport { import_id, owner_id }` defined in Task 3, enqueued with the same shape in Task 4. Wire DTOs (`StartRequest`/`StartResponse`/`Progress`/`StatusResponse`) match camelCase across Task 4 (server) and Task 5 (client).

**Open items to confirm during execution (not blockers):**
- Exact `DynamoClient::put_item` / `update_item_conditional` signatures (`crates/storage/src/dynamo.rs:48,287`) and whether a `ConditionalCheckFailedException` helper already exists.
- `client::api_get` helper name in `frontend/src/api/client.rs`.
- The worker-mode `WorkerCtx` construction site in `main.rs` (thread the two new deps).

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-25-quip-import-phase1.md`.
