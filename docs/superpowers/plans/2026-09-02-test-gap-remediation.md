# Test Gap Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Work through the 2026-09-02 cross-crate test-coverage survey from most to least important: first make CI enforce the tests that already exist, then fix the confirmed defects the survey surfaced with a regression test for each, then close the highest-value api audit and permission gaps.

**Architecture:** Three phases in this plan, each independently shippable. Phase 1 is workflow-only (no Rust changes). Phase 2 adds a shared `test_support` module to `crates/storage` so every storage repo can be tested against a replaying HTTP client without DynamoDB Local, then fixes four storage defects with request-shape tests. Phase 3 adds three `SecurityAuditAction` variants and closes six api handler gaps with integration tests in the existing `crates/api/tests/` harness. Phases 4–6 (collab/frontend structural checkers, mermaid goldens, frontend keypress sweep, remaining small-crate defects) are listed at the end as a prioritized backlog for separate plans.

**Tech Stack:** Rust workspace (axum, aws-sdk-dynamodb, aws-smithy-runtime test-util `StaticReplayClient`, tokio, proptest), GitHub Actions, Playwright doctor (`scripts/frontend-doctor/doctor.js`).

**Status (2026-09-02):** Tasks 1–16 done on branch `test-gap-remediation` (15 commits). Task 17 verification in progress; push + PR pending. Survey item D5 (scan truncation flag) was re-read and found correct, so Task 10 pins it instead of changing it.

**Spec:** The survey artifact "OgreNotes Test Gap Survey" (https://claude.ai/code/artifact/dbc8825f-fecc-48c2-bfad-64b32167e0cb). Finding ids in this plan (C1, D1, A1, S1, …) refer to that page.

## Global Constraints

- Existing tests are immutable. Add new tests; never edit an existing test body. The one exception in this plan is the `require_redis!` macro in `crates/worker/tests/integration_queue.rs` (Task 3), which is test *infrastructure* gating, not a behavioral contract; the change is additive.
- Identifiers stay raw `String` (`identifier_strategy = "string-grandfathered"`). No newtypes.
- Do not edit `design/`, `framework/`, or `runbook/`.
- New `SecurityAuditAction` variants are additive wire-shape changes to the audit `detail` JSON. Each one is flagged in its task and must be added to `action_tag_round_trips_for_every_variant` coverage via a *new* test, not by editing the existing one.
- Never `git add -A` or `git add .` in this repo. Stage files by name.
- `git push` is denied to the agent. Ask the user to run pushes with `! git push ...`.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1`.
- Work on branch `test-gap-remediation` off `main`. The user's uncommitted `.dockerignore`, `.claude/settings.json`, and two plan-doc edits stay uncommitted and untouched.
- Local integration infra: `docker compose up -d` brings up DynamoDB Local (:8000), MinIO (:9000), Redis (:6379). `crates/api/tests` and `crates/storage/tests` panic in CI without it and skip locally without it, so run them with infra up before claiming green.

---

## Phase 1 — CI enforcement (C1, C2, C4, C6)

### Task 1: Run the non-api `tests/` directories in CI

**Files:**
- Modify: `.github/workflows/ci.yml:68-73` (unit job steps) and `.github/workflows/ci.yml:168-169` (integration job step)

**Interfaces:**
- Consumes: nothing.
- Produces: two new workflow steps that later tasks rely on being present (Task 3 sets `OGRE_REQUIRE_REDIS` on the unit-job step).

- [x] **Step 1: Confirm the current local baseline for the pure suites**

Run:
```bash
cd /home/kender/projects/rust/ogre
cargo test -p ogrenotes-collab -p ogrenotes-highlight -p ogrenotes-search -p ogrenotes-worker --tests --locked --features ogrenotes-collab/xlsx,ogrenotes-collab/docx,ogrenotes-collab/pdf 2>&1 | grep -E "^test result|Running"
```
Expected: every `test result:` line is `ok`. The worker `integration_queue` binary reports its 9 tests as passing (REDIS_URL is set in `.env`; if redis is down, `docker compose up -d redis` first).

- [x] **Step 2: Add the unit-job step**

In `.github/workflows/ci.yml`, after the `Workspace lib tests` step and before `Frontend lib tests`, insert:

```yaml
      # The `tests/` directories of the non-api crates were never run in
      # CI (only `--lib` above and the api crate's suite below), so 78
      # passing-locally tests had no enforcement: collab import_fuzz +
      # quip_corpus, highlight partition/dispatch/css-sync, search props,
      # worker integration_queue. collab's xlsx/docx/pdf importers are
      # off by default and only compile in the --workspace run because
      # the api crate's defaults unify them on, so name them here.
      # storage's tests/ needs DynamoDB Local and runs in the
      # integration job instead. REDIS_URL points at the service
      # container above; OGRE_REQUIRE_REDIS makes the worker suite
      # panic instead of skipping green if it is unreachable.
      - name: Workspace integration-target tests (non-api)
        env:
          REDIS_URL: redis://127.0.0.1:6379
          OGRE_REQUIRE_REDIS: "1"
        run: |
          cargo test --locked --tests \
            -p ogrenotes-collab -p ogrenotes-highlight \
            -p ogrenotes-search -p ogrenotes-worker \
            --features ogrenotes-collab/xlsx,ogrenotes-collab/docx,ogrenotes-collab/pdf
```

- [x] **Step 3: Add the integration-job step**

In `.github/workflows/ci.yml`, directly after the `API integration tests` step (`run: cargo test -p ogrenotes-api --locked --no-fail-fast`), insert:

```yaml
      # storage's tests/test_import_repo.rs (25 tests, incl. the import
      # runner-lease contract) needs DynamoDB Local and was never run
      # in CI. Its require_infra! panics when CI is set and infra is
      # down, so a missing service fails loud here.
      - name: Storage integration tests
        run: cargo test -p ogrenotes-storage --tests --locked --no-fail-fast
```

- [x] **Step 4: Verify the storage suite actually passes with infra up**

Run:
```bash
cd /home/kender/projects/rust/ogre
docker compose up -d && sleep 5
CI=1 cargo test -p ogrenotes-storage --tests --locked 2>&1 | grep -E "^test result|panicked|Running"
```
Expected: `Running tests/test_import_repo.rs` followed by `test result: ok. 25 passed`. (Without `CI=1` the suite skips silently when infra is absent, which is why the earlier "passes locally" reading was vacuous.)

- [x] **Step 5: Validate the YAML**

Run:
```bash
cd /home/kender/projects/rust/ogre
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('ok')"
```
Expected: `ok`.

- [x] **Step 6: Commit**

```bash
git checkout -b test-gap-remediation
git add .github/workflows/ci.yml
git commit -m "ci: run the non-api tests/ directories (78 previously unenforced tests)

collab import_fuzz + quip_corpus, highlight, search props, and worker
integration_queue now run in the unit job (redis service present);
storage test_import_repo runs in the integration job with DDB Local.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 2: Compile every workspace target in CI

**Files:**
- Modify: `.github/workflows/ci.yml` (unit job, after the step added in Task 1)

- [x] **Step 1: Confirm the workspace compiles with all targets locally**

Run:
```bash
cd /home/kender/projects/rust/ogre
cargo check --workspace --all-targets --locked 2>&1 | grep -E "^(error|warning: unused)|Finished" | head
```
Expected: a `Finished` line and no `error` lines. If an `error` appears, fix the target it names before continuing (that is exactly the rot this task exists to catch); report it in the final summary.

- [x] **Step 2: Add the step**

Insert after the `Workspace integration-target tests (non-api)` step:

```yaml
      # Nothing else compiles bins, examples, or benches: mermaid_cli,
      # the mermaid gallery examples, collab's replay bin, and the
      # yrs_ops benches could rot without a red check. `check` rather
      # than `clippy -D warnings` so pre-existing warnings don't block.
      - name: Workspace check (all targets)
        run: cargo check --workspace --all-targets --locked
```

- [x] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: cargo check --all-targets so bins, examples, and benches can't rot silently

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 3: Make the worker queue suite fail loud without redis

**Files:**
- Modify: `crates/worker/tests/integration_queue.rs:20-30` (the `require_redis!` macro only)

**Interfaces:**
- Consumes: `OGRE_REQUIRE_REDIS` env var set by the Task 1 workflow step.

- [x] **Step 1: Replace the macro body**

Change:
```rust
macro_rules! require_redis {
    () => {
        match std::env::var("REDIS_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("REDIS_URL not set; skipping integration test");
                return;
            }
        }
    };
}
```
to:
```rust
/// Locally, skips (with a stderr note) when `REDIS_URL` is unset. When
/// `OGRE_REQUIRE_REDIS` is set (CI does this), panics instead — the
/// whole queue-durability contract lives in this file, and a silently
/// green run with redis missing is exactly the false signal CI must not
/// produce.
macro_rules! require_redis {
    () => {
        match std::env::var("REDIS_URL") {
            Ok(url) => url,
            Err(_) => {
                if std::env::var("OGRE_REQUIRE_REDIS").is_ok() {
                    panic!(
                        "REDIS_URL not set but OGRE_REQUIRE_REDIS is: the queue \
                         integration suite must run in this environment"
                    );
                }
                eprintln!("REDIS_URL not set; skipping integration test");
                return;
            }
        }
    };
}
```

- [x] **Step 2: Verify both branches**

Run:
```bash
cd /home/kender/projects/rust/ogre
env -u REDIS_URL OGRE_REQUIRE_REDIS=1 cargo test -p ogrenotes-worker --test integration_queue --locked 2>&1 | grep -E "panicked|test result" | head -3
env -u REDIS_URL cargo test -p ogrenotes-worker --test integration_queue --locked 2>&1 | grep -E "test result"
```
Expected: first command shows `panicked ... REDIS_URL not set but OGRE_REQUIRE_REDIS is` and `FAILED`; second shows `ok. 9 passed` (skipping green, as before).

- [x] **Step 3: Commit**

```bash
git add crates/worker/tests/integration_queue.rs
git commit -m "test(worker): OGRE_REQUIRE_REDIS makes the queue suite panic instead of skipping green

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 4: Fail the doctor run on any page error and wire the orphaned scenarios

**Files:**
- Modify: `scripts/frontend-doctor/doctor.js:~7860` (exit path)
- Modify: `.github/workflows/playwright.yml` (append scenario steps after `deck-present`, before `# ─── Teardown`)

**Interfaces:**
- Consumes: the existing step-chaining convention `if: always() && (steps.<prev>.outcome == 'success' || steps.<prev>.outcome == 'failure')`.

- [x] **Step 1: Read the exit path and the scenario preconditions**

Run:
```bash
cd /home/kender/projects/rust/ogre
sed -n 7840,7870p scripts/frontend-doctor/doctor.js
grep -n "ADMIN_EMAILS\|QDRANT\|EMBEDDINGS\|worker" .github/workflows/playwright.yml | head
for s in admin-console mfa-flow semantic-search collab-sync import-job-round-trip type-past-atom code-block-enter calendar-block kanban-block kanban-drag kanban-wip-limit kanban-column-reorder kanban-card-metadata comment-popup menu-export-downloads; do echo "-- $s"; grep -n "scenario === \"$s\"" -A 3 scripts/frontend-doctor/doctor.js | head -4; done
```
Decide per scenario: wire it only if the CI server (started in playwright.yml with `--features dev-login,xlsx,docx`) satisfies its preconditions. Expected exclusions: `admin-console` and `mfa-flow` (need `ADMIN_EMAILS` / MFA config), `semantic-search` (needs qdrant + embeddings), `import-job-round-trip` (needs the worker process). Record the excluded ones and the reason in the commit message.

- [x] **Step 2: Add the global pageerror gate**

In `doctor.js`, immediately before `process.exit(ok ? 0 : 1);`, add:

```js
  // Any uncaught page error (a WASM panic surfaces as one) fails the
  // run, not just the four scenarios that opted in via `panicRe`. The
  // "closure invoked recursively or after being dropped" class was
  // captured-and-ignored in the other fifty. Allowlist by scenario
  // name only when a scenario knowingly provokes an error.
  const PAGEERROR_ALLOWLIST = new Set([]);
  if (!PAGEERROR_ALLOWLIST.has(scenario)) {
    for (const tag of Object.keys(collector)) {
      const errs = (collector[tag] && collector[tag].errors) || [];
      if (errs.length > 0) {
        console.error(`[doctor] ${errs.length} page error(s) on ${tag}; failing run`);
        for (const e of errs) console.error(`  - ${e.message}`);
        ok = false;
      }
    }
  }
```
If `ok` is declared `const`, change that declaration to `let`.

- [x] **Step 3: Append the scenario steps**

For each wired scenario, append a step after `deck-present` following the exact existing shape (Chromium default; each step's `if:` names the previous step's id so the chain keeps running after a failure):

```yaml
      - name: Run type-past-atom scenario
        id: type-past-atom
        if: always() && (steps.deck-present.outcome == 'success' || steps.deck-present.outcome == 'failure')
        working-directory: scripts/frontend-doctor
        run: |
          node doctor.js \
            --scenario type-past-atom \
            --base-url http://127.0.0.1:3000 \
            --out ../../artifacts/doctor/type-past-atom
```
Chain order: type-past-atom → code-block-enter → calendar-block → kanban-block → kanban-drag → kanban-wip-limit → kanban-column-reorder → kanban-card-metadata → comment-popup → menu-export-downloads → collab-sync. For `collab-sync`, doctor.js sets `needsDocId`; pass whatever flag the scenario reads (check `grep -n needsDocId -A 6 doctor.js`) or omit if it creates its own doc.

- [x] **Step 4: Validate**

Run:
```bash
cd /home/kender/projects/rust/ogre
node --check scripts/frontend-doctor/doctor.js && echo js-ok
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/playwright.yml')); print('yaml-ok')"
```
Expected: `js-ok` and `yaml-ok`. The workflow itself is nightly/dispatch-only; after the user pushes, validate with `gh workflow run playwright.yml --ref test-gap-remediation` and `gh run watch --exit-status`.

- [x] **Step 5: Commit**

```bash
git add scripts/frontend-doctor/doctor.js .github/workflows/playwright.yml
git commit -m "ci(doctor): fail on any page error; wire 11 orphaned scenarios

Excluded (preconditions CI's server does not meet): admin-console,
mfa-flow, semantic-search, import-job-round-trip.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

---

## Phase 2 — storage defects with request-shape regression tests (S6, D1, D2, D3, D4, S1)

### Task 5: Shared `test_support` module for storage

**Files:**
- Create: `crates/storage/src/test_support.rs`
- Modify: `crates/storage/src/lib.rs` (add `#[cfg(test)] pub(crate) mod test_support;`)

**Interfaces:**
- Produces:
  - `pub(crate) fn offline_dynamo() -> DynamoClient` (never sends; for input-guard tests)
  - `pub(crate) fn replaying_dynamo(responses: Vec<&str>) -> (DynamoClient, StaticReplayClient)` (canned JSON bodies, records requests)
  - `pub(crate) fn request_body(replay: &StaticReplayClient, idx: usize) -> String`
  - `pub(crate) fn request_target(replay: &StaticReplayClient, idx: usize) -> String` (the `x-amz-target` header, e.g. `DynamoDB_20120810.Query`)

- [x] **Step 1: Write the module**

```rust
// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Test-only builders for repo tests that must not touch DynamoDB Local.
//!
//! `offline_dynamo` never sends (input-guard tests bail before IO).
//! `replaying_dynamo` answers each SDK call with the next canned JSON
//! body and records the request actually put on the wire, so a test can
//! assert request *shape* (ConditionExpression, Limit, ExclusiveStartKey)
//! — the only way to pin behaviour DynamoDB Local cannot distinguish.

use crate::dynamo::DynamoClient;
use aws_smithy_runtime::client::http::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;

pub(crate) fn offline_dynamo() -> DynamoClient {
    let conf = aws_sdk_dynamodb::Config::builder()
        .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
        .build();
    DynamoClient::new(aws_sdk_dynamodb::Client::from_conf(conf), "test-table".to_string())
}

pub(crate) fn replaying_dynamo(responses: Vec<&str>) -> (DynamoClient, StaticReplayClient) {
    let replay = StaticReplayClient::new(
        responses
            .into_iter()
            .map(|body| {
                ReplayEvent::new(
                    http::Request::builder()
                        .uri("http://localhost/")
                        .body(SdkBody::from("{}"))
                        .unwrap(),
                    http::Response::builder()
                        .status(200)
                        .body(SdkBody::from(body))
                        .unwrap(),
                )
            })
            .collect(),
    );
    let conf = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://localhost:9999")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "k", "s", None, None, "test",
        ))
        .http_client(replay.clone())
        .behavior_version_latest()
        .build();
    let db = DynamoClient::new(
        aws_sdk_dynamodb::Client::from_conf(conf),
        "test-table".to_string(),
    );
    (db, replay)
}

/// Body of the `idx`-th request the SDK emitted, as UTF-8.
pub(crate) fn request_body(replay: &StaticReplayClient, idx: usize) -> String {
    let reqs: Vec<_> = replay.actual_requests().collect();
    let req = reqs.get(idx).unwrap_or_else(|| panic!("no request #{idx}; {} recorded", reqs.len()));
    String::from_utf8(req.body().bytes().expect("in-memory body").to_vec()).expect("utf-8 body")
}

/// `x-amz-target` of the `idx`-th request, e.g. `DynamoDB_20120810.Query`.
pub(crate) fn request_target(replay: &StaticReplayClient, idx: usize) -> String {
    let reqs: Vec<_> = replay.actual_requests().collect();
    let req = reqs.get(idx).unwrap_or_else(|| panic!("no request #{idx}; {} recorded", reqs.len()));
    req.headers().get("x-amz-target").unwrap_or_default().to_string()
}

/// A replayed `ConditionalCheckFailedException` body (HTTP 400 is set by
/// the caller via `replaying_dynamo_with_status`).
pub(crate) const CONDITIONAL_CHECK_FAILED: &str =
    r#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"The conditional request failed"}"#;

/// Like `replaying_dynamo` but each response carries its own HTTP status,
/// for error-path tests.
pub(crate) fn replaying_dynamo_with_status(
    responses: Vec<(u16, &str)>,
) -> (DynamoClient, StaticReplayClient) {
    let replay = StaticReplayClient::new(
        responses
            .into_iter()
            .map(|(status, body)| {
                ReplayEvent::new(
                    http::Request::builder()
                        .uri("http://localhost/")
                        .body(SdkBody::from("{}"))
                        .unwrap(),
                    http::Response::builder()
                        .status(status)
                        .body(SdkBody::from(body))
                        .unwrap(),
                )
            })
            .collect(),
    );
    let conf = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://localhost:9999")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "k", "s", None, None, "test",
        ))
        .http_client(replay.clone())
        .behavior_version_latest()
        .build();
    let db = DynamoClient::new(
        aws_sdk_dynamodb::Client::from_conf(conf),
        "test-table".to_string(),
    );
    (db, replay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replaying_dynamo_records_the_request_it_answered() {
        let (db, replay) = replaying_dynamo(vec![r#"{"Items":[]}"#]);
        let items = db.query("PK1", None).await.expect("query");
        assert!(items.is_empty());
        assert_eq!(request_target(&replay, 0), "DynamoDB_20120810.Query");
        assert!(request_body(&replay, 0).contains(r#""TableName":"test-table""#));
    }
}
```

- [x] **Step 2: Register the module**

In `crates/storage/src/lib.rs` add after `pub mod s3;`:
```rust
#[cfg(test)]
pub(crate) mod test_support;
```

- [x] **Step 3: Run the self-test**

Run: `cargo test -p ogrenotes-storage --lib test_support --locked`
Expected: `1 passed`.

- [x] **Step 4: Commit**

```bash
git add crates/storage/src/test_support.rs crates/storage/src/lib.rs
git commit -m "test(storage): shared offline/replaying DynamoClient builders for repo tests

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 6: `list_unread_since` pages past filtered-out rows (D1)

**Files:**
- Modify: `crates/storage/src/repo/notification_repo.rs:98-140`
- Test: same file, new `#[cfg(test)] mod paging_tests` at the bottom

**Interfaces:**
- Consumes: `test_support::{replaying_dynamo, request_body}`.

- [x] **Step 1: Write the failing test**

Append to `notification_repo.rs`:

```rust
#[cfg(test)]
mod paging_tests {
    use super::*;
    use crate::test_support::{replaying_dynamo, request_body};

    fn unread_item(created_at: i64, id: &str) -> String {
        format!(
            r#"{{"PK":{{"S":"USER#u1"}},"SK":{{"S":"NOTIF#{created_at:020}#{id}"}},"notif_id":{{"S":"{id}"}},"user_id":{{"S":"u1"}},"notif_type":{{"S":"mention"}},"actor_id":{{"S":"u2"}},"actor_name":{{"S":"Bob"}},"is_read":{{"BOOL":false}},"created_at":{{"N":"{created_at}"}}}}"#
        )
    }

    /// DynamoDB applies `Limit` to items *read*, then `FilterExpression`.
    /// A user whose `limit` newest rows are all read used to get an empty
    /// unread list while unread rows existed on the next page. The loop
    /// must follow `LastEvaluatedKey` until it has `limit` unread rows or
    /// the range is exhausted.
    #[tokio::test]
    async fn list_unread_since_pages_past_filtered_out_rows() {
        let (db, replay) = replaying_dynamo(vec![
            // Page 1: every row was read → filtered to nothing, but more exist.
            r#"{"Items":[],"LastEvaluatedKey":{"PK":{"S":"USER#u1"},"SK":{"S":"NOTIF#00000000000000000900#r9"}}}"#,
            // Page 2: one unread row.
            &format!(r#"{{"Items":[{}]}}"#, unread_item(800, "n8")),
        ]);
        let repo = NotificationRepo::new(db);

        let rows = repo.list_unread_since("u1", 0, 5).await.expect("list");

        assert_eq!(rows.len(), 1, "the unread row on page 2 must be returned");
        assert_eq!(rows[0].notif_id, "n8");
        let second = request_body(&replay, 1);
        assert!(
            second.contains(r#""ExclusiveStartKey""#),
            "page 2 must resume from page 1's LastEvaluatedKey: {second}"
        );
    }

    /// Once `limit` unread rows are in hand the loop stops even if the
    /// server offers another page.
    #[tokio::test]
    async fn list_unread_since_stops_at_limit() {
        let (db, replay) = replaying_dynamo(vec![&format!(
            r#"{{"Items":[{},{}],"LastEvaluatedKey":{{"PK":{{"S":"USER#u1"}},"SK":{{"S":"NOTIF#00000000000000000700#n7"}}}}}}"#,
            unread_item(900, "n9"),
            unread_item(800, "n8")
        )]);
        let repo = NotificationRepo::new(db);

        let rows = repo.list_unread_since("u1", 0, 2).await.expect("list");

        assert_eq!(rows.len(), 2);
        assert_eq!(replay.actual_requests().count(), 1, "no second page once limit is met");
    }
}
```
Check the exact attribute names `notif_from_item` requires (`grep -n "get_s(item" crates/storage/src/repo/notification_repo.rs`) and adjust `unread_item` so the row decodes.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-storage --lib paging_tests --locked`
Expected: `list_unread_since_pages_past_filtered_out_rows` FAILS with `the unread row on page 2 must be returned` (0 rows). The second test may pass already; that is fine.

- [x] **Step 3: Implement the continuation loop**

Replace the body of `list_unread_since` from `let result = self` through the final `.collect()` with:

```rust
        let mut out: Vec<Notification> = Vec::new();
        let mut last_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let remaining = limit.saturating_sub(out.len());
            if remaining == 0 {
                break;
            }
            let mut builder = self
                .db
                .inner()
                .query()
                .table_name(self.db.table_name())
                .key_condition_expression("PK = :pk AND SK BETWEEN :start AND :end")
                .filter_expression("is_read = :false")
                .expression_attribute_values(":pk", AttributeValue::S(pk.clone()))
                .expression_attribute_values(":start", AttributeValue::S(sk_start.clone()))
                .expression_attribute_values(":end", AttributeValue::S(sk_end.clone()))
                .expression_attribute_values(":false", AttributeValue::Bool(false))
                .scan_index_forward(false)
                // `Limit` bounds rows *read* before the filter, so it is a
                // page size, not a result cap — hence the loop.
                .limit(remaining as i32);
            if let Some(start) = last_key.take() {
                builder = builder.set_exclusive_start_key(Some(start));
            }

            let result = builder
                .send()
                .await
                .map_err(|e| RepoError::Dynamo(e.into_service_error().to_string()))?;

            for item in result.items.unwrap_or_default().iter() {
                if out.len() >= limit {
                    break;
                }
                out.push(notif_from_item(item, user_id)?);
            }

            match result.last_evaluated_key {
                Some(key) if out.len() < limit => last_key = Some(key),
                _ => break,
            }
        }

        Ok(out)
```
Also fix the doc comment: replace the sentence beginning "`limit` is applied after filtering" with "`limit` caps the unread rows returned; because DynamoDB applies `Limit` before `FilterExpression`, the query follows `LastEvaluatedKey` until it has `limit` unread rows or the range is exhausted."

- [x] **Step 4: Run the tests**

Run: `cargo test -p ogrenotes-storage --lib notification_repo --locked`
Expected: all pass, including the pre-existing ones.

- [x] **Step 5: Commit**

```bash
git add crates/storage/src/repo/notification_repo.rs
git commit -m "fix(storage): list_unread_since pages past filtered-out rows

DynamoDB applies Limit before FilterExpression; a user whose newest
rows were all read got an empty digest while unread rows existed.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 7: `FolderRepo::update` refuses to resurrect a deleted row (D2)

**Files:**
- Modify: `crates/storage/src/repo/folder_repo.rs:69-133`
- Modify: `crates/api/src/routes/folders.rs:323-333` (map `RepoError::NotFound` to 404)
- Test: `folder_repo.rs`, new `#[cfg(test)] mod guard_tests`

- [x] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod guard_tests {
    use super::*;
    use crate::test_support::{
        replaying_dynamo, replaying_dynamo_with_status, request_body, CONDITIONAL_CHECK_FAILED,
    };

    /// A bare `update_item` upserts. A rename racing a delete used to leave
    /// a row holding only `updated_at` (+ title), which `folder_from_item`
    /// then rejects on every later read — a permanently unreadable folder.
    /// Mirrors `DocRepo::update_metadata`'s guard.
    #[tokio::test]
    async fn update_sends_attribute_exists_guard() {
        let (db, replay) = replaying_dynamo(vec!["{}"]);
        let repo = FolderRepo::new(db);

        repo.update("f1", Some("Renamed"), None, None, None, 42)
            .await
            .expect("update");

        let body = request_body(&replay, 0);
        assert!(
            body.contains(r#""ConditionExpression":"attribute_exists(PK)""#),
            "update must be guarded: {body}"
        );
    }

    #[tokio::test]
    async fn update_of_a_deleted_folder_is_not_found() {
        let (db, _replay) = replaying_dynamo_with_status(vec![(400, CONDITIONAL_CHECK_FAILED)]);
        let repo = FolderRepo::new(db);

        let err = repo
            .update("f1", Some("Renamed"), None, None, None, 42)
            .await
            .expect_err("guard must refuse");
        assert!(matches!(err, RepoError::NotFound(_)), "got {err:?}");
    }
}
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-storage --lib guard_tests --locked`
Expected: both FAIL (no ConditionExpression in body; error is `Dynamo(..)` not `NotFound`).

- [x] **Step 3: Implement**

Replace the tail of `update` (from `self.db` / `.update_item(&pk, Folder::sk(), ...)` to the end of the fn) with:

```rust
        // Guard with attribute_exists(PK): a bare update_item upserts, so a
        // folder deleted between the caller's ownership check and this write
        // would otherwise resurrect a partial row that `folder_from_item`
        // can never decode again. Mirrors DocRepo::update_metadata.
        match self
            .db
            .inner()
            .update_item()
            .table_name(self.db.table_name())
            .key("PK", AttributeValue::S(pk))
            .key("SK", AttributeValue::S(Folder::sk().to_string()))
            .update_expression(&update_expr)
            .condition_expression("attribute_exists(PK)")
            .set_expression_attribute_values(Some(values))
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(e) => {
                let svc = e.into_service_error();
                if svc.is_conditional_check_failed_exception() {
                    Err(RepoError::NotFound(format!("folder {folder_id} no longer exists")))
                } else {
                    Err(RepoError::Dynamo(svc.to_string()))
                }
            }
        }
```

In `crates/api/src/routes/folders.rs` change the `.map_err(|e| ApiError::Internal(e.to_string()))?;` directly after the `.update(` call to:

```rust
        .map_err(|e| match e {
            ogrenotes_storage::repo::RepoError::NotFound(_) => {
                ApiError::NotFound("Folder not found".to_string())
            }
            other => ApiError::Internal(other.to_string()),
        })?;
```

- [x] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-storage --lib folder_repo --locked && cargo check -p ogrenotes-api --locked`
Expected: all pass; api compiles.

- [x] **Step 5: Commit**

```bash
git add crates/storage/src/repo/folder_repo.rs crates/api/src/routes/folders.rs
git commit -m "fix(storage): guard FolderRepo::update with attribute_exists so a delete race can't resurrect a partial row

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 8: `get_secmap` returns only the requested thread's rows (D3)

**Files:**
- Modify: `crates/storage/src/repo/import_repo.rs:253-266`
- Test: new proptest in `crates/storage/src/models/import_inventory.rs` plus a replay test in `import_repo.rs`'s existing `#[cfg(test)]` module (new fn only)

- [x] **Step 1: Write the failing tests**

In `crates/storage/src/repo/import_repo.rs`, inside the existing `mod tests` (append a new fn; do not touch existing ones):

```rust
    /// `SECMAP#<thread>#` is a prefix query, so a thread id that itself
    /// contains `#` (e.g. `abc#0`) is returned by the query for thread
    /// `abc`. The row carries `quip_thread_id`; filter on it rather than
    /// trusting the prefix — `list_unresolved` already regroups by the
    /// attribute for the same reason.
    #[tokio::test]
    async fn get_secmap_ignores_rows_from_a_prefix_colliding_thread() {
        let (repo, _replay) = replaying_repo(vec![
            r#"{"Items":[
              {"PK":{"S":"IMPORT#imp1"},"SK":{"S":"SECMAP#abc#0"},"quip_thread_id":{"S":"abc"},"chunk":{"N":"0"},"owner_id":{"S":"u1"},"entries":{"L":[{"L":[{"S":"s1"},{"S":"b1"}]}]}},
              {"PK":{"S":"IMPORT#imp1"},"SK":{"S":"SECMAP#abc#0#0"},"quip_thread_id":{"S":"abc#0"},"chunk":{"N":"0"},"owner_id":{"S":"u1"},"entries":{"L":[{"L":[{"S":"s2"},{"S":"b2"}]}]}}
            ]}"#,
        ]);
        let entries = repo.get_secmap("imp1", "abc").await.expect("get_secmap");
        assert_eq!(entries, vec![("s1".to_string(), "b1".to_string())]);
    }
```
Check how `secmap_from_item` encodes `entries` (`grep -n "fn secmap_from_item" -A 25 crates/storage/src/repo/import_repo.rs`) and match the JSON to it.

In `crates/storage/src/models/import_inventory.rs` add a new test module:

```rust
#[cfg(test)]
mod secmap_prefix_props {
    use super::*;
    use proptest::prelude::*;

    fn prefix_for(thread: &str) -> String {
        format!("SECMAP#{thread}#")
    }

    proptest! {
        /// Documents (does not fix) that the SK prefix alone cannot
        /// separate thread ids when one is a `#`-extension of another.
        /// `ImportRepo::get_secmap` therefore filters on the attribute.
        #[test]
        fn a_hash_extended_thread_id_shares_the_prefix(base in "[a-zA-Z0-9]{1,12}", ext in "[a-zA-Z0-9]{1,4}") {
            let row = SecMapRow {
                quip_thread_id: format!("{base}#{ext}"),
                chunk: 0,
                owner_id: "u1".into(),
                entries: vec![],
            };
            prop_assert!(row.sk().starts_with(&prefix_for(&base)));
        }
    }
}
```

- [x] **Step 2: Run to verify the repo test fails**

Run: `cargo test -p ogrenotes-storage --lib get_secmap_ignores --locked`
Expected: FAIL with both entries returned.

- [x] **Step 3: Implement**

In `get_secmap`, change:
```rust
        let mut rows: Vec<SecMapRow> = items.iter().map(secmap_from_item).collect::<Result<_, _>>()?;
```
to:
```rust
        let mut rows: Vec<SecMapRow> = items
            .iter()
            .map(secmap_from_item)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // The SK prefix cannot separate `abc` from `abc#0`; the row's
            // own attribute can.
            .filter(|r| r.quip_thread_id == quip_thread_id)
            .collect();
```

- [x] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-storage --lib secmap --locked`
Expected: all pass.

- [x] **Step 5: Commit**

```bash
git add crates/storage/src/repo/import_repo.rs crates/storage/src/models/import_inventory.rs
git commit -m "fix(storage): get_secmap filters on quip_thread_id, not just the SK prefix

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 9: `get_by_ids` surfaces persistent UnprocessedKeys (D4)

**Files:**
- Modify: `crates/storage/src/repo/user_repo.rs:100-147`
- Test: `user_repo.rs`, new `#[cfg(test)] mod batch_tests`

- [x] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::test_support::replaying_dynamo;

    fn throttled_page() -> String {
        r#"{"Responses":{"test-table":[]},"UnprocessedKeys":{"test-table":{"Keys":[{"PK":{"S":"USER#u1"},"SK":{"S":"PROFILE"}}]}}}"#.to_string()
    }

    /// After the retry budget is spent the old code `break`ed and returned
    /// a short map — indistinguishable from "that user does not exist".
    /// Share dialogs then rendered real members as absent.
    #[tokio::test]
    async fn get_by_ids_errors_when_keys_stay_unprocessed() {
        let pages: Vec<String> = (0..6).map(|_| throttled_page()).collect();
        let (db, _replay) = replaying_dynamo(pages.iter().map(String::as_str).collect());
        let repo = UserRepo::new(db);

        let err = repo
            .get_by_ids(&["u1".to_string()])
            .await
            .expect_err("persistent UnprocessedKeys must not look like absence");
        assert!(matches!(err, RepoError::Dynamo(ref m) if m.contains("unprocessed")), "got {err:?}");
    }
}
```
Confirm the `get_by_ids` parameter type (`grep -n "pub async fn get_by_ids" crates/storage/src/repo/user_repo.rs`) and adjust the argument.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-storage --lib batch_tests --locked`
Expected: FAIL (`Ok` with an empty map).

- [x] **Step 3: Implement**

Replace the `match resp.unprocessed_keys { ... }` block with:

```rust
                match resp.unprocessed_keys {
                    Some(unprocessed) if !unprocessed.is_empty() => {
                        if attempt >= 5 {
                            return Err(RepoError::Dynamo(format!(
                                "batch_get_item left {} key group(s) unprocessed after {attempt} retries",
                                unprocessed.len()
                            )));
                        }
                        attempt += 1;
                        // Linear backoff so a throttled table gets a chance
                        // to recover instead of five back-to-back retries.
                        tokio::time::sleep(std::time::Duration::from_millis(50 * attempt as u64)).await;
                        request_items = unprocessed;
                    }
                    _ => break,
                }
```
Check that `tokio` with the `time` feature is a dependency of `ogrenotes-storage` (`grep -n "^tokio" crates/storage/Cargo.toml`). If not, add `tokio = { workspace = true, features = ["time"] }`.

- [x] **Step 4: Run tests**

Run: `cargo test -p ogrenotes-storage --lib user_repo --locked`
Expected: all pass.

- [x] **Step 5: Commit**

```bash
git add crates/storage/src/repo/user_repo.rs crates/storage/Cargo.toml
git commit -m "fix(storage): get_by_ids returns an error instead of a short map when keys stay unprocessed

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 10: Pin `dynamo.rs` pagination arithmetic (S1)

**Files:**
- Test: `crates/storage/src/dynamo.rs`, new `#[cfg(test)] mod tests` at the bottom

- [x] **Step 1: Write the tests**

```rust
#[cfg(test)]
mod tests {
    use crate::test_support::{replaying_dynamo, request_body};

    fn page(n: usize, start: usize, more: bool) -> String {
        let items: Vec<String> = (start..start + n)
            .map(|i| format!(r#"{{"PK":{{"S":"P"}},"SK":{{"S":"S{i:04}"}}}}"#))
            .collect();
        let lek = if more {
            format!(r#","LastEvaluatedKey":{{"PK":{{"S":"P"}},"SK":{{"S":"S{:04}"}}}}"#, start + n - 1)
        } else {
            String::new()
        };
        format!(r#"{{"Items":[{}]{}}}"#, items.join(","), lek)
    }

    /// `limit` is a cap on the total across pages: three pages of 40 with
    /// a cap of 50 yields exactly 50, and the second request asks DynamoDB
    /// for only the 10 still needed.
    #[tokio::test]
    async fn query_index_caps_total_across_pages() {
        let (db, replay) = replaying_dynamo(vec![
            &page(40, 0, true),
            &page(40, 40, true),
            &page(40, 80, true),
        ]);
        let items = db
            .query_index("gsi", "gsi_pk", "v", None, None, true, Some(50))
            .await
            .expect("query_index");
        assert_eq!(items.len(), 50);
        assert_eq!(replay.actual_requests().count(), 2, "third page must not be fetched");
        assert!(request_body(&replay, 0).contains(r#""Limit":50"#));
        assert!(request_body(&replay, 1).contains(r#""Limit":10"#));
        assert!(request_body(&replay, 1).contains(r#""ExclusiveStartKey""#));
    }

    #[tokio::test]
    async fn query_index_with_zero_limit_makes_no_request() {
        let (db, replay) = replaying_dynamo(vec![]);
        let items = db
            .query_index("gsi", "gsi_pk", "v", None, None, true, Some(0))
            .await
            .expect("query_index");
        assert!(items.is_empty());
        assert_eq!(replay.actual_requests().count(), 0);
    }

    #[tokio::test]
    async fn query_concatenates_all_pages_in_order() {
        let (db, replay) = replaying_dynamo(vec![&page(2, 0, true), &page(2, 2, false)]);
        let items = db.query("P", None).await.expect("query");
        let sks: Vec<String> = items
            .iter()
            .map(|i| i["SK"].as_s().unwrap().clone())
            .collect();
        assert_eq!(sks, vec!["S0000", "S0001", "S0002", "S0003"]);
        assert_eq!(replay.actual_requests().count(), 2);
    }

    #[tokio::test]
    async fn scan_with_filter_reports_truncation_only_when_rows_remain() {
        // Exact fill, no continuation → not truncated.
        let (db, _r) = replaying_dynamo(vec![&page(3, 0, false)]);
        let (items, truncated) = db.scan_with_filter("a", "v", 3).await.expect("scan");
        assert_eq!(items.len(), 3);
        assert!(!truncated);

        // Over-full page → truncated, capped.
        let (db, _r) = replaying_dynamo(vec![&page(5, 0, false)]);
        let (items, truncated) = db.scan_with_filter("a", "v", 3).await.expect("scan");
        assert_eq!(items.len(), 3);
        assert!(truncated);

        // Exact fill but server offers another page → truncated.
        let (db, r) = replaying_dynamo(vec![&page(3, 0, true)]);
        let (items, truncated) = db.scan_with_filter("a", "v", 3).await.expect("scan");
        assert_eq!(items.len(), 3);
        assert!(truncated);
        assert_eq!(r.actual_requests().count(), 1, "cap reached: must not fetch the next page");
    }
}
```

- [x] **Step 2: Run**

Run: `cargo test -p ogrenotes-storage --lib dynamo::tests --locked`
Expected: all four pass. If `query_index_caps_total_across_pages` fails on the `"Limit":10` assertion, the production arithmetic is wrong; report rather than loosen the test.

- [x] **Step 3: Commit**

```bash
git add crates/storage/src/dynamo.rs
git commit -m "test(storage): pin DynamoClient pagination caps, ordering, and scan truncation

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

---

## Phase 3 — api audit and permission defects (D6, D7, D8, D9, D10, D11)

All integration tests below need `docker compose up -d`. Run each new test with `cargo test -p ogrenotes-api --test <file> <name> --locked`.

### Task 11: Three new `SecurityAuditAction` variants

**Files:**
- Modify: `crates/storage/src/models/security_audit.rs` (enum, `as_str`, and the `detail` payload fn — read the whole `impl SecurityAuditAction` first)
- Test: same file, new `#[test] fn restore_and_workspace_membership_actions_round_trip_through_storage`

**Interfaces:**
- Produces:
  - `SecurityAuditAction::DocRestored { doc_id: String, from_version: u64, to_version: u64 }` → tag `"docRestored"`
  - `SecurityAuditAction::WorkspaceMemberAdded { workspace_id: String, target: String, role: String }` → tag `"workspaceMemberAdded"`
  - `SecurityAuditAction::WorkspaceMemberRemoved { workspace_id: String, target: String }` → tag `"workspaceMemberRemoved"`

Wire-shape note: additive variants in the audit `detail` JSON; `GET /admin/audit` re-emits `detail` verbatim, so the new camelCase keys (`docId`, `fromVersion`, `toVersion`, `workspaceId`, `target`, `role`) become public. Flagged here deliberately.

- [x] **Step 1: Write the failing round-trip test**

Look at `workspace_identity_actions_round_trip_through_storage` (line ~704) and write the sibling with the same mechanics:

```rust
    #[test]
    fn restore_and_workspace_membership_actions_round_trip_through_storage() {
        for action in [
            SecurityAuditAction::DocRestored {
                doc_id: "d1".into(),
                from_version: 7,
                to_version: 8,
            },
            SecurityAuditAction::WorkspaceMemberAdded {
                workspace_id: "ws1".into(),
                target: "u2".into(),
                role: "member".into(),
            },
            SecurityAuditAction::WorkspaceMemberRemoved {
                workspace_id: "ws1".into(),
                target: "u2".into(),
            },
        ] {
            let tag = action.as_str();
            let detail = action.detail_json().to_string();
            let back = SecurityAuditAction::from_storage(tag, &detail).expect("round trip");
            assert_eq!(back, action);
        }
        assert_eq!(
            SecurityAuditAction::DocRestored { doc_id: "d".into(), from_version: 1, to_version: 2 }.as_str(),
            "docRestored"
        );
    }
```
Replace `detail_json()` / `from_storage(tag, detail)` with the actual method names used by the existing round-trip test.

- [x] **Step 2: Run to verify it fails to compile**

Run: `cargo test -p ogrenotes-storage --lib security_audit --locked`
Expected: compile error, no such variant.

- [x] **Step 3: Add the variants**

After `WorkspaceScimTokenRevoked { token_id: String },` add:

```rust
    /// A document was restored to an earlier version
    /// (`POST /documents/{id}/versions/{v}/restore`). Destructive: the
    /// pending UPDATE# rows newer than the target are discarded, so
    /// collaborators' unflushed edits vanish. `user_id` PK is the doc
    /// owner (subject); `actor_id` is the restorer. Mirrors `DocDeleted`.
    DocRestored { doc_id: String, from_version: u64, to_version: u64 },
    /// A workspace admin added a member. Workspace membership is the
    /// widest grant in the system (it feeds link-sharing audience), so
    /// it is audited like `ShareGranted`. `user_id` PK is the added
    /// member (subject); `actor_id` is the admin.
    WorkspaceMemberAdded { workspace_id: String, target: String, role: String },
    /// A workspace admin removed a member. Subject = removed member.
    WorkspaceMemberRemoved { workspace_id: String, target: String },
```
Add to `as_str`:
```rust
            Self::DocRestored { .. } => "docRestored",
            Self::WorkspaceMemberAdded { .. } => "workspaceMemberAdded",
            Self::WorkspaceMemberRemoved { .. } => "workspaceMemberRemoved",
```
Add matching arms to the detail-payload fn and the tag→variant parser, copying the shape of `DocCompacted` and `WorkspaceScimTokenRevoked`.

- [x] **Step 4: Run the model tests**

Run: `cargo test -p ogrenotes-storage --lib security_audit --locked`
Expected: all pass, including the existing `action_tag_round_trips_for_every_variant` (if it enumerates variants by hand and now fails on exhaustiveness, that is a compile error in a `match` — add the arms it needs; do not delete any existing assertion).

- [x] **Step 5: Commit**

```bash
git add crates/storage/src/models/security_audit.rs
git commit -m "feat(storage): DocRestored and WorkspaceMemberAdded/Removed security-audit actions

Additive wire-shape change: new camelCase detail keys surface in
GET /admin/audit.

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 12: `restore_version` emits `DocRestored` (D6)

**Files:**
- Modify: `crates/api/src/routes/history.rs:275-300`
- Test: `crates/api/tests/test_security_audit_writers.rs` (append)

- [x] **Step 1: Write the failing test**

Append to `test_security_audit_writers.rs`. Reuse its `wait_for_audit_row` and follow `test_history.rs::test_restore_version_reverts_live_content` for the two-version setup (copy its `make_doc_bytes` helper into this file if it is private there):

```rust
/// A restore discards collaborators' unflushed UPDATE# rows — the one
/// endpoint that silently throws away someone else's work — and had no
/// SecurityAudit row while every sibling destructive path (delete, lock,
/// compact) did.
#[tokio::test]
async fn restore_version_writes_doc_restored_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (owner_id, token) = app.create_user("restore-audit@test.com").await;
    let doc_id = app.create_doc(&token, "Restore Audit", None).await;

    for body in ["Version one", "Version two"] {
        let (status, _) = app
            .bytes_request(
                Method::PUT,
                &format!("/api/v1/documents/{doc_id}/content"),
                Some(&token),
                make_doc_bytes(body),
                "application/octet-stream",
            )
            .await;
        assert_eq!(status, 204);
    }
    let meta = app.state.doc_repo.get(&doc_id).await.unwrap().unwrap();
    let from = meta.snapshot_version;
    let target = from - 1;

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/documents/{doc_id}/versions/{target}/restore"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &owner_id, |a| {
        matches!(a, SecurityAuditAction::DocRestored { doc_id: d, .. } if d == &doc_id)
    })
    .await;
    match row.action {
        SecurityAuditAction::DocRestored { from_version, to_version, .. } => {
            assert_eq!(from_version, target);
            assert_eq!(to_version, from + 1);
        }
        other => panic!("unexpected action {other:?}"),
    }
    assert_eq!(row.actor_id, owner_id);

    app.cleanup().await;
}
```
Check how `test_history.rs` produces content bytes (`grep -n "fn make_doc_bytes" -A 12 crates/api/tests/test_history.rs`) and whether each PUT bumps `snapshot_version`; if a PUT does not create a version, use the same sequence that test uses.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers restore_version_writes --locked`
Expected: FAIL with `expected SecurityAudit row ... within 200ms`.

- [x] **Step 3: Implement**

In `restore_version`, directly after the `let _ = state.doc_repo.delete_updates_before(&id, now_usec()).await;` line, add:

```rust
    // Durable SecurityAudit row: a restore discards every collaborator's
    // unflushed edits newer than the target, so it is destructive doc
    // state like delete/lock/compact. Subject = owner, actor = restorer.
    crate::routes::audit::record_security_event_by_actor(
        &state,
        &meta.owner_id,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::DocRestored {
            doc_id: id.clone(),
            from_version: version,
            to_version: new_version,
        },
    );
```
`meta` is moved into `spawn_index_document_from_bytes` later in the fn; clone `meta.owner_id` before that call if the borrow checker complains.

- [x] **Step 4: Run**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers restore_version_writes --locked && cargo test -p ogrenotes-api --test test_history --locked`
Expected: pass; history suite still green.

- [x] **Step 5: Commit**

```bash
git add crates/api/src/routes/history.rs crates/api/tests/test_security_audit_writers.rs
git commit -m "fix(api): restore_version writes a DocRestored security-audit row

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 13: Workspace membership changes are audited (D8)

**Files:**
- Modify: `crates/api/src/routes/workspaces.rs:276-332`
- Test: `crates/api/tests/test_security_audit_writers.rs` (append two tests)

- [x] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn workspace_add_member_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, admin_token) = app.create_user("ws-add-admin@test.com").await;
    let (bob_id, _) = app.create_user("ws-add-bob@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&admin_token),
            Some(serde_json::json!({ "name": "Audited Members" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/workspaces/{ws_id}/members"),
            Some(&admin_token),
            Some(serde_json::json!({ "userId": bob_id, "role": "member" })),
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(a, SecurityAuditAction::WorkspaceMemberAdded { workspace_id, target, role }
            if workspace_id == &ws_id && target == &bob_id && role == "member")
    })
    .await;
    assert_eq!(row.actor_id, admin_id);
    assert_eq!(row.user_id, bob_id, "row is keyed on the member (subject)");

    app.cleanup().await;
}

#[tokio::test]
async fn workspace_remove_member_writes_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, admin_token) = app.create_user("ws-rm-admin@test.com").await;
    let (bob_id, _) = app.create_user("ws-rm-bob@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&admin_token),
            Some(serde_json::json!({ "name": "Audited Removal" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();
    app.json_request(
        Method::POST,
        &format!("/api/v1/workspaces/{ws_id}/members"),
        Some(&admin_token),
        Some(serde_json::json!({ "userId": bob_id, "role": "member" })),
    )
    .await;

    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/workspaces/{ws_id}/members/{bob_id}"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &bob_id, |a| {
        matches!(a, SecurityAuditAction::WorkspaceMemberRemoved { workspace_id, target }
            if workspace_id == &ws_id && target == &bob_id)
    })
    .await;
    assert_eq!(row.actor_id, admin_id);

    app.cleanup().await;
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers workspace_ --locked`
Expected: the two new tests FAIL on the audit-row wait.

- [x] **Step 3: Implement**

In `add_member`, the `member` struct moves `id` and `body.user_id`; capture what the audit needs first. Replace from `let member = WorkspaceMember {` to `Ok(StatusCode::NO_CONTENT)` with:

```rust
    let target = body.user_id.clone();
    let role_label = serde_json::to_value(&body.role)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{:?}", body.role).to_lowercase());
    let member = WorkspaceMember {
        workspace_id: id.clone(),
        user_id: body.user_id,
        role: body.role,
        joined_at: now_usec(),
    };

    state
        .workspace_repo
        .add_member(&member)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Workspace membership is the widest grant in the system (it feeds
    // link-sharing audience); audit it like ShareGranted. Subject = the
    // added member, actor = the admin.
    crate::routes::audit::record_security_event_by_actor(
        &state,
        &target,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::WorkspaceMemberAdded {
            workspace_id: id,
            target: target.clone(),
            role: role_label,
        },
    );

    Ok(StatusCode::NO_CONTENT)
```
In `remove_member`, after the `remove_member(&id, &target_user_id)` call succeeds and before `Ok(StatusCode::NO_CONTENT)`:

```rust
    crate::routes::audit::record_security_event_by_actor(
        &state,
        &target_user_id,
        &user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::WorkspaceMemberRemoved {
            workspace_id: id,
            target: target_user_id.clone(),
        },
    );
```
Check how `WorkspaceRole` serializes (`grep -n "serde" -B2 -A6 crates/storage/src/models/mod.rs | sed -n '/WorkspaceRole/,+8p'`) so `role_label` yields `"member"` / `"admin"`.

- [x] **Step 4: Run**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers workspace_ --locked && cargo test -p ogrenotes-api --test test_workspaces --locked`
Expected: pass.

- [x] **Step 5: Commit**

```bash
git add crates/api/src/routes/workspaces.rs crates/api/tests/test_security_audit_writers.rs
git commit -m "fix(api): audit workspace member add/remove as security events

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 14: Admin disable revokes sessions with a `SessionRevoked` row and the victim's bearer stops working (D9)

**Files:**
- Modify: `crates/api/src/routes/admin.rs:297-310`
- Test: `crates/api/tests/test_security_audit_writers.rs` (append)

- [x] **Step 1: Write the failing test**

```rust
/// SCIM deprovision writes `SessionRevoked` at the same semantic point
/// and tests it; the admin path called `delete_all_for_user` and wrote
/// nothing. Also pins that the victim's live bearer is dead *through the
/// admin endpoint* (the existing auth test flips the row directly).
#[tokio::test]
async fn admin_disable_revokes_sessions_and_writes_security_audit_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (admin_id, _) = app.create_user("admin-revoke@test.com").await;
    let _ = app.state.user_repo.set_admin(&admin_id, true).await;
    let (_, admin_token) = app.create_user("admin-revoke@test.com").await;
    let (victim_id, victim_token) = app.create_user("victim-revoke@test.com").await;

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&victim_token), None)
        .await;
    assert_eq!(status, 200, "victim's token works before disable");

    let (status, _) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/admin/users/{victim_id}/disable"),
            Some(&admin_token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let row = wait_for_audit_row(&app, &victim_id, |a| {
        matches!(a, SecurityAuditAction::SessionRevoked { reason } if reason == "admin_disable")
    })
    .await;
    assert_eq!(row.actor_id, admin_id);

    let (status, _) = app
        .json_request(Method::GET, "/api/v1/users/me", Some(&victim_token), None)
        .await;
    assert_eq!(status, 401, "disabled user's bearer must be rejected");

    app.cleanup().await;
}
```
If the bearer is a stateless JWT that stays valid until the refresh, the last assertion may see 403 (disabled) rather than 401; check what `test_auth.rs:418` asserts for a disabled row and use that code.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers admin_disable_revokes --locked`
Expected: FAIL on the audit-row wait.

- [x] **Step 3: Implement**

In `disable_user`, after `let _ = state.session_repo.delete_all_for_user(&id).await;` add:

```rust
    // Mirror the SCIM deprovision path: the session kill is a security
    // event on the victim's account (subject = victim, actor = admin),
    // separate from the AdminAudit row that records the admin's action.
    crate::routes::audit::record_security_event_by_actor(
        &state,
        &id,
        &auth.user_id,
        ogrenotes_storage::models::security_audit::SecurityAuditAction::SessionRevoked {
            reason: "admin_disable".to_string(),
        },
    );
```

- [x] **Step 4: Run**

Run: `cargo test -p ogrenotes-api --test test_security_audit_writers admin_disable --locked && cargo test -p ogrenotes-api --test test_admin --locked`
Expected: pass.

- [x] **Step 5: Commit**

```bash
git add crates/api/src/routes/admin.rs crates/api/tests/test_security_audit_writers.rs
git commit -m "fix(api): admin disable_user writes SessionRevoked{admin_disable}

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 15: Chat messages share the comments write budget (D7)

**Files:**
- Modify: `crates/api/src/routes/chat.rs:447-465`
- Test: `crates/api/tests/test_rate_limits.rs` (append)

- [x] **Step 1: Write the failing test**

Test config sets `rate_limit_comments_per_min: 5`. Follow `comments_rate_limit_fires_across_thread_and_messages` for the window alignment helper:

```rust
/// Chat `send_message` writes to the same thread_repo as comments but was
/// uncapped. It now draws from the shared "comments" budget (5/min in the
/// test config) so a chatty user can't amplify DDB writes by switching
/// surfaces.
#[tokio::test]
async fn chat_send_message_is_rate_limited_per_user() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token_a) = app.create_user("ratelimit-chat-a@test.com").await;
    let (bob_id, _) = app.create_user("ratelimit-chat-b@test.com").await;
    let (status, chat) = app
        .json_request(
            Method::POST,
            "/api/v1/chats",
            Some(&token_a),
            Some(serde_json::json!({ "chatType": "chat", "title": "RL", "memberIds": [bob_id] })),
        )
        .await;
    assert_eq!(status, 201);
    let chat_id = chat["id"].as_str().unwrap().to_string();

    common::align_rate_limit_window().await;
    for i in 0..5 {
        let (status, _) = app
            .json_request(
                Method::POST,
                &format!("/api/v1/chats/{chat_id}/messages"),
                Some(&token_a),
                Some(serde_json::json!({ "content": format!("msg {i}") })),
            )
            .await;
        assert!(status < 400, "iter {i}: under-cap message must succeed (status {status})");
    }

    let (status, json) = app
        .json_request(
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/messages"),
            Some(&token_a),
            Some(serde_json::json!({ "content": "one too many" })),
        )
        .await;
    assert_eq!(status, 429, "6th chat message must be rate limited: {json}");

    app.cleanup().await;
}
```

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-api --test test_rate_limits chat_send_message --locked`
Expected: FAIL, 6th message returns 201/204.

- [x] **Step 3: Implement**

In `send_message`, directly after `check_chat_member(&thread, &user_id)?;` add:

```rust
    // Shares the comments budget: both surfaces write to thread_repo and
    // a per-surface cap could be gamed by alternating between them.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "comments",
        &user_id,
        state.config.rate_limit_comments_per_min,
        60,
    )
    .await?;
```

- [x] **Step 4: Run**

Run: `cargo test -p ogrenotes-api --test test_rate_limits --locked && cargo test -p ogrenotes-api --test test_chat --locked`
Expected: pass. If `test_chat.rs` sends more than 5 messages in one test from one user, it will now 429; that is a real behavior change to surface to the user, not a reason to raise the test config.

- [x] **Step 5: Commit**

```bash
git add crates/api/src/routes/chat.rs crates/api/tests/test_rate_limits.rs
git commit -m "fix(api): chat send_message draws from the comments rate-limit budget

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 16: Deleting a non-empty folder is refused; rate-limiter helpers pinned (D10, D11)

**Files:**
- Modify: `crates/api/src/routes/folders.rs:345-375`
- Modify: `crates/api/src/middleware/rate_limit.rs:61` and add `#[cfg(test)] mod tests`
- Test: `crates/api/tests/test_folders.rs` (append)

- [x] **Step 1: Write the failing folder test**

```rust
/// `delete_folder` dropped the folder row and left every child edge in
/// place: documents inside became unreachable from the tree without
/// being trashed. The repo's own doc-comment says "children should be
/// moved first"; the handler now enforces it with 409.
#[tokio::test]
async fn test_delete_folder_with_children_is_refused() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let token = app.create_user_token("alice@test.com").await;
    let folder_id = app.create_folder(&token, "Full", None).await;
    let doc_id = app.create_doc(&token, "Inside", Some(&folder_id)).await;

    let (status, json) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/folders/{folder_id}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 409, "non-empty folder must not be deleted: {json}");

    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/documents/{doc_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200, "the document is untouched");

    let (status, _) = app
        .json_request(Method::GET, &format!("/api/v1/folders/{folder_id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200, "the folder is untouched");

    app.cleanup().await;
}
```
Check `create_doc`'s third parameter (`grep -n "pub async fn create_doc" -A 8 crates/api/tests/common/mod.rs`) is a folder id; if it is not, add the doc to the folder via `POST /api/v1/folders/{id}/children` with the body shape `test_folders.rs` already uses.

- [x] **Step 2: Run to verify it fails**

Run: `cargo test -p ogrenotes-api --test test_folders test_delete_folder_with_children --locked`
Expected: FAIL, status 204.

- [x] **Step 3: Implement the folder guard**

In `delete_folder`, after the `FolderType::System` check and before `state.folder_repo.delete(&id)`, add:

```rust
    // Children are edges under FOLDER#<id>/CHILD#…; deleting the parent
    // row alone strands every child outside the tree (unreachable, not
    // trashed). Callers move or trash children first.
    let children = state
        .folder_repo
        .list_children(&id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !children.is_empty() {
        return Err(ApiError::Conflict(
            "Folder is not empty; move or delete its contents first".to_string(),
        ));
    }
```
Check `list_children`'s signature (`sed -n 189,203p crates/storage/src/repo/folder_repo.rs`) and pass any extra arguments it needs.

- [x] **Step 4: Fix the latent clamp panic and pin the pure helpers**

In `rate_limit.rs` change:
```rust
    let secs_until_next = (window_secs - (now % window_secs)).clamp(1, window_secs - 1);
```
to:
```rust
    // `clamp(min, max)` panics when min > max; a 1-second window would
    // hit that. Every current caller passes 60, but make the arithmetic
    // total rather than rely on it.
    let secs_until_next = (window_secs - (now % window_secs)).clamp(1, window_secs.saturating_sub(1).max(1));
```
Append:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn ip_identifier_takes_first_xff_hop_and_trims() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", " 203.0.113.9 , 10.0.0.1".parse().unwrap());
        assert_eq!(ip_identifier(&h), "203.0.113.9");
    }

    #[test]
    fn ip_identifier_defaults_to_unknown_without_header() {
        assert_eq!(ip_identifier(&HeaderMap::new()), "unknown");
    }

    #[test]
    fn scope_label_covers_every_enforce_call_site() {
        // Every scope string passed to `enforce` in the crate. A new
        // caller that forgets to add its scope here lands in "other"
        // and its metrics vanish into one bucket.
        for scope in [
            "auth_login", "auth_refresh", "search", "sharing", "admin_mut",
            "scim_request", "mfa_verify", "comments", "content_write", "import",
            "bulk_op", "bulk_export", "ws_upgrade", "dev_login", "client_telemetry",
            "rum", "saml_acs",
        ] {
            assert_ne!(scope_label(scope), "other", "scope {scope} must have a label");
        }
        assert_eq!(scope_label("doc-abc123"), "other");
    }
}
```
Add `secs_until_next` coverage only if the arithmetic is in a pure fn; if it is inline in an async fn that needs redis, leave it (the `.max(1)` guard is total by construction).

- [x] **Step 5: Run**

Run:
```bash
cargo test -p ogrenotes-api --lib rate_limit --locked
cargo test -p ogrenotes-api --test test_folders --locked
```
Expected: all pass, including the three pre-existing folder delete tests (they delete empty folders).

- [x] **Step 6: Commit**

```bash
git add crates/api/src/routes/folders.rs crates/api/src/middleware/rate_limit.rs crates/api/tests/test_folders.rs
git commit -m "fix(api): refuse to delete a non-empty folder; make rate-limit window arithmetic total

Claude-Session: https://claude.ai/code/session_01HaaK47kBbTD8DA5THaLzy1"
```

### Task 17: Full verification and handoff

- [ ] **Step 1: Run everything CI will run**

```bash
cd /home/kender/projects/rust/ogre
docker compose up -d
cargo test --workspace --lib --locked 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c
cargo test --locked --tests -p ogrenotes-collab -p ogrenotes-highlight -p ogrenotes-search -p ogrenotes-worker --features ogrenotes-collab/xlsx,ogrenotes-collab/docx,ogrenotes-collab/pdf 2>&1 | grep -E "^test result|FAILED"
CI=1 cargo test -p ogrenotes-storage --tests --locked 2>&1 | grep -E "^test result|FAILED"
cargo check --workspace --all-targets --locked 2>&1 | tail -1
cargo test -p ogrenotes-api --locked --no-fail-fast 2>&1 | grep -E "^test result|FAILED" | sort | uniq -c
```
Expected: no `FAILED` lines anywhere.

- [ ] **Step 2: Hand off**

Ask the user to push with `! git push -u origin test-gap-remediation`, then open the PR with `gh pr create`. Report the four excluded doctor scenarios and the three additive audit variants explicitly in the PR body.

---

## Backlog — separate plans, in priority order

These are out of scope for this plan and should each get their own plan document. Ids refer to the survey.

1. **Structural schema checkers (C3, K1, F1, D20, D21).** Promote `assert_valid_tree` to `pub fn schema_violations(&Doc)` in collab and proptest every importer; call `Schema::validate` after every transaction in a frontend proptest; make the duality test read `frontend/src/editor/schema.rs` the way `themes.rs:81` reads the presentation themes; widen `needs_normalize`; wrap the template-picker and find-bar closes in `a11y::defer`. Ends the orphan-container class generically.
2. **Mermaid output pinning (M1, M2, M3, D12, D13, D22).** 22-file golden set, quick-xml well-formedness in the fuzz net, hoisted NaN/inf check, unique marker ids per family, control-char stripping in `escape_xml`, nested-cluster fuzzing.
3. **Remaining api gaps (A1–A10, D24).** Lock-then-upgrade WS downgrade, `show_conversation` gate, cross-instance Update fan-out, three unasserted workspace audit writers, non-admin negatives, single dismiss, seed `--force`/`--dry-run`, `TestApp::workspace_with_members` and `new_with_rate_limits` fixtures.
4. **Small-crate defects (D14–D19, D23).** Mail-merge mark loss, email cap zero + refund, Quip 429/backoff, worker enqueue idempotency + retry loss window, embed_many retry, search metadata-field injection, `AppConfig::from_env` parsers.
5. **Frontend sweep (F2–F10, C8).** Block × keypress table-driven test, dialog lifecycle harness, atom-size for all 10 atoms, WS opcode parity, yrs tag round-trip for 28 types, spreadsheet function smoke table, API client transport seam, locale key parity in the lib target.
6. **Storage follow-ups (S2–S5).** Reader/writer key-encoder agreement, `list_all_meta` cursor round-trip, aggregated-error ConditionalCheckFailed idiom, refresh-token reuse revoke-all assertion.
