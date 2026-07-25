# Quip Import — Phase 0 (Connect & Scope) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Phase 0 of the Quip importer (design:
`docs/superpowers/specs/2026-07-24-quip-import-design.md`): paste a Quip
personal access token → the token is validated, held securely, and the user's
Quip profile + root folders come back for the scope step. Demoable end to end
against a real Quip account; no content import yet.

**Architecture:** A new `crates/quip-import` domain crate holds a throttled
`QuipClient` (reqwest) and a `Secret` token type. A `TokenStore` trait keeps
the token out of every durable store (SSM SecureString in prod; an in-process
map in dev, since the local stack has no SSM). `ImportRepo` (DynamoDB) creates
the import record — never the token. `POST /api/v1/imports/quip/connect`
validates the token via the QuipClient, stores it, and returns the profile +
roots. A token-entry wizard step calls it.

**Tech stack:** Rust (workspace crates), reqwest+rustls, aws-sdk-ssm,
secrecy/zeroize; Leptos 0.7 CSR frontend.

## Global Constraints

- **The token is a secret.** It appears only in the `connect` request body and
  in `Secret<String>` in memory. It is NEVER serialized into a Dynamo item, an
  S3 object, a Redis value, a log line (any level), or a `Debug`/`Display`
  output. Every type that holds it uses `secrecy::Secret` (zeroize-on-drop,
  `Debug=[redacted]`). A test asserts this.
- **Frontend is outside the workspace** — `cd frontend/`; editor/WASM code
  verified with `cargo check` AND `cargo build --target wasm32-unknown-unknown`.
- **reqwest for the Quip client uses rustls**, not OpenSSL:
  `reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }`.
- **Quip base URL is injectable** on `QuipClient` (default
  `https://platform.quip.com`) so tests point it at a mock server.
- **Throttle target = 45 req/min** (10% under Quip's 50/min).
- **Storage items are hand-built `AttributeValue` maps** (repo convention; no
  `serde_dynamo`). PK/SK per the design manifest (`IMPORT#<id>` / `META`).
- **Do not `git add -A`** — stage exact paths. Line numbers below are from
  exploration on 2026-07-24; anchor by content if drifted.
- **New crate + new AWS SDK dep + `AppState::new` signature change** are
  deliberate, flagged here (not incidental).

## File Structure

**New crate `crates/quip-import`:** `Cargo.toml`, `src/lib.rs`, `src/secret.rs`
(Secret token type), `src/throttle.rs` (pure token-bucket + async gate),
`src/client.rs` (`QuipClient` + DTOs + `QuipError`), `src/token_store.rs`
(`TokenStore` trait + `InMemoryTokenStore` + `SsmTokenStore`).

**`crates/storage`:** `src/models/import.rs` (`ImportRecord`), `src/repo/import_repo.rs`.

**`crates/api`:** `src/routes/imports.rs` (new), edits to `src/routes/mod.rs`,
`src/state.rs`, `src/main.rs`, `Cargo.toml`.

**Root:** `Cargo.toml` (workspace member + dep).

**Frontend:** `src/api/imports.rs` (new), `src/api/mod.rs`,
`src/components/quip_import/mod.rs` (wizard, token step), `src/components/mod.rs`,
`src/components/app_shell.rs` (ShellCtx field + mount + entry point).

---

## Task 1: `quip-import` crate skeleton + `Secret` token type

**Files:**
- Create: `crates/quip-import/Cargo.toml`, `crates/quip-import/src/lib.rs`, `crates/quip-import/src/secret.rs`
- Modify: root `Cargo.toml` (`members` + `[workspace.dependencies]`)

**Interfaces:**
- Produces: crate `ogrenotes-quip-import`; `pub use secret::QuipToken` where
  `QuipToken(secrecy::Secret<String>)` with `fn expose(&self) -> &str` (via
  `ExposeSecret`) and a `Debug` that prints `QuipToken([redacted])`. Consumed
  by every later task that carries the token.

- [ ] **Step 1: Add the workspace member + deps**

Root `Cargo.toml`: add `"crates/quip-import"` to `members`; add to
`[workspace.dependencies]`:
```toml
ogrenotes-quip-import = { path = "crates/quip-import" }
aws-sdk-ssm = "1"
secrecy = "0.8"
```
(reqwest/wiremock are added in the crate's own Cargo.toml since they're not
workspace deps yet — or promote to workspace; match how `reqwest` is declared
in `crates/api/Cargo.toml`, i.e. crate-local is fine.)

- [ ] **Step 2: Crate Cargo.toml**

`crates/quip-import/Cargo.toml` (mirror `crates/common/Cargo.toml`'s inherited
package fields):
```toml
[package]
name = "ogrenotes-quip-import"
version.workspace = true
edition.workspace = true
license.workspace = true
description.workspace = true
publish.workspace = true

[dependencies]
ogrenotes-common = { workspace = true }
ogrenotes-storage = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
secrecy = { workspace = true }
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
aws-sdk-ssm = { workspace = true }
async-trait = { workspace = true }

[dev-dependencies]
wiremock = "0.6"
tokio = { workspace = true, features = ["macros", "rt", "test-util", "time"] }
```
(Confirm `async-trait`, `tokio`, `tracing` are workspace deps — they are used
across crates; if a name/feature differs, match the real workspace declaration.)

- [ ] **Step 3: Write the failing Secret test**

`crates/quip-import/src/secret.rs`:
```rust
use secrecy::{ExposeSecret, Secret};

/// A Quip personal access token. Wraps `secrecy::Secret` so it zeroizes on
/// drop and never appears in Debug/logs. The ONLY way to read it is `expose`.
pub struct QuipToken(Secret<String>);

impl QuipToken {
    pub fn new(raw: String) -> Self { Self(Secret::new(raw)) }
    pub fn expose(&self) -> &str { self.0.expose_secret() }
}

impl std::fmt::Debug for QuipToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("QuipToken([redacted])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn debug_is_redacted_and_expose_returns_value() {
        let t = QuipToken::new("secret-abc123".into());
        assert_eq!(format!("{t:?}"), "QuipToken([redacted])");
        assert!(!format!("{t:?}").contains("abc123"));
        assert_eq!(t.expose(), "secret-abc123");
    }
}
```

`crates/quip-import/src/lib.rs`:
```rust
//! Quip import — client, throttle, token store, converter (design:
//! docs/superpowers/specs/2026-07-24-document-mentions… quip-import-design.md).
pub mod secret;
pub use secret::QuipToken;
```

- [ ] **Step 4: Build + test**

Run: `cargo test -p ogrenotes-quip-import secret`
Expected: builds (deps resolve) and the redaction test passes. `cargo build -p ogrenotes-quip-import` clean.

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/quip-import/Cargo.toml crates/quip-import/src/lib.rs crates/quip-import/src/secret.rs
git commit -m "feat(quip-import): crate skeleton + redacting QuipToken"
```

---

## Task 2: The throttle (pure token-bucket + async gate)

**Files:**
- Create: `crates/quip-import/src/throttle.rs`
- Modify: `crates/quip-import/src/lib.rs` (`pub mod throttle;`)

**Interfaces:**
- Produces:
  - `RateState { tokens: f64, last_refill_ms: i64, reset_at_ms: Option<i64>, remaining_hint: Option<u32> }`
  - `pub fn plan_delay(state: &mut RateState, now_ms: i64, rate_per_min: u32) -> u64` — pure: mutates the bucket for one request at `now_ms`, returns milliseconds to sleep before sending (0 if a token is available). Honors `remaining_hint`→0 and `reset_at_ms`.
  - `pub fn backoff_ms(attempt: u32, reset_at_ms: Option<i64>, now_ms: i64, rng01: f64) -> u64` — pure: exp backoff (base 1000, cap 60000) with full jitter, floored at `reset_at_ms - now_ms`.
  - `pub struct Throttle` — async wrapper owning a `Mutex<RateState>` + a clock fn; `async fn acquire(&self)` sleeps `plan_delay` then returns; `fn observe_headers(&self, remaining, reset_at_ms)`; `async fn backoff(&self, attempt)`. Consumed by `QuipClient`.

- [ ] **Step 1: Failing pure tests**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_gates_to_the_rate_over_a_minute() {
        // 45/min: the 46th request within the same minute must be delayed.
        let mut s = RateState::full(45, 0);
        for _ in 0..45 { assert_eq!(plan_delay(&mut s, 0, 45), 0); }
        let d = plan_delay(&mut s, 0, 45);
        assert!(d > 0 && d <= 60_000, "46th within the minute delays, got {d}");
    }

    #[test]
    fn remaining_hint_zero_waits_for_reset() {
        let mut s = RateState::full(45, 0);
        s.remaining_hint = Some(0);
        s.reset_at_ms = Some(5_000);
        assert_eq!(plan_delay(&mut s, 1_000, 45), 4_000);
    }

    #[test]
    fn backoff_is_bounded_jittered_and_reset_floored() {
        assert!(backoff_ms(0, None, 0, 0.0) <= 1_000);
        assert!(backoff_ms(10, None, 0, 1.0) <= 60_000);      // capped
        assert!(backoff_ms(0, Some(30_000), 0, 0.0) >= 30_000); // reset floor
    }
}
```
Run: `cargo test -p ogrenotes-quip-import throttle` → RED (unimplemented).

- [ ] **Step 2: Implement** the pure functions + `RateState::full(rate, now_ms)`
+ the `Throttle` async wrapper (a `tokio::sync::Mutex<RateState>` + a
`clock: fn() -> i64` defaulting to `now_ms_wall()`; `acquire` computes
`plan_delay`, drops the lock, `tokio::time::sleep`s; `backoff` sleeps
`backoff_ms` using a `rng01` from a cheap source — derive jitter from
`now_ms % 1000 / 1000.0` to avoid adding an rng dep, or use `fastrand` if it's
already a dep — confirm). `now_ms_wall()` = `now_usec()/1000`.

- [ ] **Step 3: GREEN** — `cargo test -p ogrenotes-quip-import throttle` passes.
- [ ] **Step 4: Commit**
```bash
git add crates/quip-import/src/throttle.rs crates/quip-import/src/lib.rs
git commit -m "feat(quip-import): rate throttle (token bucket + header-aware backoff)"
```

---

## Task 3: `QuipClient` (reqwest + throttle + endpoints)

**Files:**
- Create: `crates/quip-import/src/client.rs`
- Modify: `crates/quip-import/src/lib.rs`

**Interfaces:**
- Consumes: `QuipToken` (Task 1), `Throttle` (Task 2).
- Produces:
  - `pub struct QuipClient { http: reqwest::Client, base: String, throttle: Throttle }`, `pub fn new(base: Option<String>) -> Self` (default base, 30 s timeout).
  - `pub async fn current_user(&self, t: &QuipToken) -> Result<QuipUser, QuipError>` (`GET /1/users/current`).
  - `pub async fn folders(&self, t: &QuipToken, ids: &[String]) -> Result<Vec<QuipFolder>, QuipError>` (`GET /1/folders/?ids=…`).
  - DTOs `QuipUser { id, name, emails: Vec<String>, private_folder_id, shared_folder_ids: Vec<String>, ... }`, `QuipFolder { id, title, children: Vec<QuipFolderChild> }` (serde, tolerant `#[serde(default)]`).
  - `pub enum QuipError { Http, RateLimited{retry_after_ms}, Unauthorized, Api{status,message}, Parse }` (thiserror; `Unauthorized` for 401/403, `RateLimited` for 503-over-limit). The token is NEVER in an error message.

- [ ] **Step 1: Failing wiremock test**

Point the client at a mock server; assert bearer auth, header parsing, and the
401/503 mappings:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, header}};

    #[tokio::test]
    async fn current_user_sends_bearer_and_parses() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/1/users/current"))
            .and(header("authorization", "Bearer tok-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id":"u1","name":"Ada","emails":["ada@example.com"],
                "private_folder_id":"pf","shared_folder_ids":["sf1"]
            })))
            .mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        let u = c.current_user(&QuipToken::new("tok-1".into())).await.unwrap();
        assert_eq!(u.id, "u1");
        assert_eq!(u.emails, vec!["ada@example.com"]);
    }

    #[tokio::test]
    async fn unauthorized_and_rate_limited_map() {
        let server = MockServer::start().await;
        Mock::given(path("/1/users/current"))
            .respond_with(ResponseTemplate::new(401)).mount(&server).await;
        let c = QuipClient::new(Some(server.uri()));
        assert!(matches!(c.current_user(&QuipToken::new("x".into())).await,
            Err(QuipError::Unauthorized)));
        // token never leaks into the error text:
        let e = c.current_user(&QuipToken::new("SEEKRET".into())).await.unwrap_err();
        assert!(!format!("{e}").contains("SEEKRET"));
    }
}
```
Run: `cargo test -p ogrenotes-quip-import client` → RED.

- [ ] **Step 2: Implement** the client mirroring `crates/api/src/claude.rs`'s
reqwest idiom but with `.bearer_auth(t.expose())`, a 30 s timeout on the
builder, and — new to this codebase — reading `resp.headers()` for
`x-ratelimit-remaining`/`x-ratelimit-reset` into `throttle.observe_headers(...)`
BEFORE consuming the body. Wrap each call in `throttle.acquire().await`; on 503
map to `RateLimited`, on 401/403 `Unauthorized`, other non-2xx `Api`. Do NOT
put the token in any error. `folders` builds `?ids=<comma-joined>`.

- [ ] **Step 3: GREEN + build**
Run: `cargo test -p ogrenotes-quip-import` and `cargo build -p ogrenotes-quip-import`.

- [ ] **Step 4: Commit**
```bash
git add crates/quip-import/src/client.rs crates/quip-import/src/lib.rs
git commit -m "feat(quip-import): throttled QuipClient (current_user, folders)"
```

---

## Task 4: `TokenStore` (trait + in-memory + SSM)

**Files:**
- Create: `crates/quip-import/src/token_store.rs`
- Modify: `crates/quip-import/src/lib.rs`

**Interfaces:**
- Produces:
  - `#[async_trait] pub trait TokenStore: Send + Sync { async fn put(&self, import_id: &str, token: &QuipToken) -> Result<(), TokenStoreError>; async fn get(&self, import_id: &str) -> Result<Option<QuipToken>, TokenStoreError>; async fn delete(&self, import_id: &str) -> Result<(), TokenStoreError>; }`
  - `pub struct InMemoryTokenStore` (a `DashMap<String, Secret<String>>` — dev/local; single-process).
  - `pub struct SsmTokenStore { client: aws_sdk_ssm::Client, prefix: String }` — `put` = `put_parameter(Type=SecureString, Overwrite=true, Name=<prefix>import/<id>/quip-token)`, `get` = `get_parameter(WithDecryption=true)`, `delete` = `delete_parameter` (ignore not-found).
  - Consumed by the connect route + (Phase 1+) the worker.

- [ ] **Step 1: Failing test on the in-memory impl**
```rust
#[tokio::test]
async fn in_memory_round_trip_and_delete() {
    let s = InMemoryTokenStore::new();
    assert!(s.get("i1").await.unwrap().is_none());
    s.put("i1", &QuipToken::new("tok".into())).await.unwrap();
    assert_eq!(s.get("i1").await.unwrap().unwrap().expose(), "tok");
    s.delete("i1").await.unwrap();
    assert!(s.get("i1").await.unwrap().is_none());
}
```
(The SSM impl is exercised only against real AWS — the local stack has no SSM;
mark its methods thin and covered by the trait contract + a deployed smoke
check, not a local unit test. State this in the report.)
Run: `cargo test -p ogrenotes-quip-import token_store` → RED.

- [ ] **Step 2: Implement** both impls (add `dashmap` to the crate deps if not
present — confirm; it's used elsewhere in the workspace). SSM parameter name:
`format!("{prefix}import/{import_id}/quip-token")` where `prefix` comes from
config (e.g. `/{stack_prefix}ogrenote/`). Never log the value.

- [ ] **Step 3: GREEN + commit**
```bash
git add crates/quip-import/src/token_store.rs crates/quip-import/src/lib.rs crates/quip-import/Cargo.toml
git commit -m "feat(quip-import): TokenStore trait + in-memory (dev) and SSM (prod) impls"
```

---

## Task 5: `ImportRepo` + `ImportRecord` model (the `META` item)

**Files:**
- Create: `crates/storage/src/models/import.rs`, `crates/storage/src/repo/import_repo.rs`
- Modify: `crates/storage/src/models/mod.rs`, `crates/storage/src/repo/mod.rs`

**Interfaces:**
- Consumes: `DynamoClient`, `RepoError`, `get_s`/`get_n` helpers (repo mod), `new_id`/`now_usec`.
- Produces:
  - `ImportRecord { import_id, owner_id, status: ImportStatus, phase: u8, quip_user_id: Option<String>, target_folder_id: Option<String>, selected_roots: Vec<String>, created_at, updated_at }`; `pk()` = `format!("IMPORT#{import_id}")`, `sk()` = `"META"`.
  - `enum ImportStatus { Scoping, Running, AwaitingIdentityConfirm, TokenRejected, Succeeded, Failed, Cancelled }` (serde lowercase).
  - `ImportRepo { db: DynamoClient }` with `new(db)`, `create(&ImportRecord) -> Result<(), RepoError>` (conditional `attribute_not_exists(PK)`), `get(import_id) -> Result<Option<ImportRecord>, RepoError>`, `set_status(import_id, ImportStatus) -> Result<(), RepoError>`.

- [ ] **Step 1: Failing integration test** (infra-gated like existing repo tests)

`crates/storage/tests/…` or inline `#[cfg(test)]` following the repo's own test
convention (`require_infra!`/dynamodb-local). Create → get round-trip; status
update; the token is not a field (compile-enforced — there is no token field).
Run: `cargo test -p ogrenotes-storage import_repo` → RED.

- [ ] **Step 2: Implement** the model (mirror `models/folder.rs`: derives,
`pk`/`sk`, sparse optionals) + repo (mirror `repo/folder_repo.rs`: hand-built
`AttributeValue` map via an `import_to_item` fn, `put_item_conditional`,
`get_item`, `update_item` for status). Register both mods.

- [ ] **Step 3: GREEN + commit**
```bash
git add crates/storage/src/models/import.rs crates/storage/src/models/mod.rs crates/storage/src/repo/import_repo.rs crates/storage/src/repo/mod.rs
git commit -m "feat(storage): ImportRepo + ImportRecord (no token field, by design)"
```

---

## Task 6: Wire `AppState` (SSM client, token store, ImportRepo, QuipClient)

**Files:**
- Modify: `crates/api/src/state.rs`, `crates/api/src/main.rs`, `crates/api/Cargo.toml`

**Interfaces:**
- Produces on `AppState` (all `Arc`/`Clone`): `import_repo: Arc<ImportRepo>`,
  `quip_token_store: Arc<dyn ogrenotes_quip_import::TokenStore>`,
  `quip_client: Arc<QuipClient>`. Consumed by `routes/imports.rs`.

- [ ] **Step 1** Add deps to `crates/api/Cargo.toml`:
`ogrenotes-quip-import = { workspace = true }`, `aws-sdk-ssm = { workspace = true }`.

- [ ] **Step 2** In `main.rs` (~line 66, after `aws_config`) build
`let ssm_client = aws_sdk_ssm::Client::new(&aws_config);` and select the token
store: when SSM is usable (deployed — gate on a config flag, e.g.
`config.quip_token_store == "ssm"` or `!config.dev_mode`) →
`Arc::new(SsmTokenStore::new(ssm_client, config.ssm_prefix.clone()))`, else
`Arc::new(InMemoryTokenStore::new())`. Build `quip_client =
Arc::new(QuipClient::new(None))`. Pass them into `AppState::new` (extend its
signature — the single call site is here; add params after `job_producer`).

- [ ] **Step 3** In `state.rs` add the three fields, build `import_repo` inside
`AppState::new` (`Arc::new(ImportRepo::new(dynamo.clone()))` BEFORE `dynamo` is
moved into the last repo), and accept the passed-in `quip_token_store` +
`quip_client`. Update the `Self { … }`.

- [ ] **Step 4** Build: `cargo build -p ogrenotes-api`. Expected clean (one
call-site signature change).

- [ ] **Step 5: Commit**
```bash
git add crates/api/Cargo.toml crates/api/src/state.rs crates/api/src/main.rs
git commit -m "feat(api): wire ImportRepo + Quip token store + QuipClient into AppState"
```

---

## Task 7: `POST /api/v1/imports/quip/connect`

**Files:**
- Create: `crates/api/src/routes/imports.rs`
- Modify: `crates/api/src/routes/mod.rs`

**Interfaces:**
- Consumes: `AuthUser`, `ApiError`, rate-limit `enforce`, `state.quip_client`,
  `state.quip_token_store`, `state.import_repo`.
- Produces: `POST /connect` — body `{ token }`; validate via
  `quip_client.current_user`; create an `ImportRecord (Scoping)`;
  `quip_token_store.put(import_id, token)`; return `{ importId, quipProfile{ id, name }, rootFolders: [{ id, title }] }`. Maps `QuipError::Unauthorized` → `ApiError::BadRequest("invalid Quip token")` (NOT 401 — that's OUR auth), `RateLimited`/`Http` → `ServiceUnavailable`.

- [ ] **Step 1: Failing integration test** (infra-gated), QuipClient pointed at
a wiremock server via a test-constructed `AppState` (or a test that injects a
`QuipClient` with a mock base): `POST /connect` with a valid mock token → 201 +
importId + roots; the created `ImportRecord` has no token; `token_store.get`
returns it. Follow `crates/api/tests/` harness (`require_infra!`, `TestApp`).
Confirm how tests build/override `AppState.quip_client` (add a test hook or a
constructor that takes a base URL). Run → RED.

- [ ] **Step 2: Implement** `router()` (`.route("/quip/connect", post(connect))`)
and the handler (mirror `documents::create_from_text`): rate-limit
(`"quip_connect"`, a small per-min cap), `current_user`, `new_id()` import id,
`ImportRepo::create`, `token_store.put`, fetch roots via `folders(&token,
&[private_folder_id, ...shared_folder_ids])`, return the DTO. Register
`pub mod imports;` + `.nest("/api/v1/imports", imports::router())` in
`routes/mod.rs`.

- [ ] **Step 3: GREEN + regression**
Run: `cargo test -p ogrenotes-api --test <import test>` and `cargo build --workspace`.

- [ ] **Step 4: Commit**
```bash
git add crates/api/src/routes/imports.rs crates/api/src/routes/mod.rs crates/api/tests/…
git commit -m "feat(api): POST /imports/quip/connect — validate token, stash securely, return roots"
```

---

## Task 8: Wizard token-entry step (frontend)

**Files:**
- Create: `frontend/src/api/imports.rs`, `frontend/src/components/quip_import/mod.rs`
- Modify: `frontend/src/api/mod.rs`, `frontend/src/components/mod.rs`, `frontend/src/components/app_shell.rs`
- Modify: `frontend/locales/{en-US,ar,es,it,fr,de}/main.ftl` (labels)

**Interfaces:**
- Consumes: `api_post` (`api/client.rs`); `ShellCtx` mount pattern.
- Produces: `api::imports::connect(token) -> Result<ConnectResponse, ApiClientError>`
  (DTOs camelCase mirroring the backend); `QuipImportWizard(visible, on_close)`
  component whose first step is a token field + Connect button that calls
  `connect` and renders the returned profile + root-folder checklist (scope
  step is stubbed for Phase 1). ShellCtx gains `quip_import_open: RwSignal<bool>`.

- [ ] **Step 1** `api/imports.rs`:
```rust
use serde::{Deserialize, Serialize};
use crate::api::client::{self, ApiClientError};

#[derive(Serialize)] #[serde(rename_all = "camelCase")]
struct ConnectRequest<'a> { token: &'a str }

#[derive(Debug, Clone, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct QuipProfile { pub id: String, pub name: String }
#[derive(Debug, Clone, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct RootFolder { pub id: String, pub title: String }
#[derive(Debug, Clone, Deserialize)] #[serde(rename_all = "camelCase")]
pub struct ConnectResponse {
    pub import_id: String,
    pub quip_profile: QuipProfile,
    pub root_folders: Vec<RootFolder>,
}

pub async fn connect(token: &str) -> Result<ConnectResponse, ApiClientError> {
    client::api_post("/imports/quip/connect", &ConnectRequest { token }).await
}
```
Register `pub mod imports;` in `api/mod.rs`.

- [ ] **Step 2** `QuipImportWizard` (mirror `template_picker_modal.rs`:
`#[component] pub fn QuipImportWizard(visible: ReadSignal<bool>, on_close: Callback<()>)`;
focus-trap; `<Show when=visible>`). Step 1: a password-type `<input>` for the
token, a Connect button that `spawn_local`s `api::imports::connect`, on Ok
stores `import_id` + shows profile + a checkbox list of `root_folders`
(default all checked) with a disabled "Continue" (wired in Phase 1); on Err
shows an inline error. The token input is never logged and cleared on close.
i18n keys `quip-import-*` in all six catalogs.

- [ ] **Step 3** Mount in `app_shell.rs`: add `quip_import_open: RwSignal<bool>`
to `ShellCtx` (init `RwSignal::new(false)` — stays `Copy`), mount
`<QuipImportWizard visible=ctx.quip_import_open.read_only() on_close=… />` next
to the template-picker modal, and add an entry point (an "Import from Quip"
item wherever "New/Import" surfaces live — mirror `on_templates`).

- [ ] **Step 4** Build: `cd frontend && cargo check && cargo build --target wasm32-unknown-unknown`. (No native render test harness; behavior is manual + Phase-0 demo.)

- [ ] **Step 5: Commit**
```bash
git add frontend/src/api/imports.rs frontend/src/api/mod.rs frontend/src/components/quip_import/mod.rs frontend/src/components/mod.rs frontend/src/components/app_shell.rs frontend/locales/*/main.ftl
git commit -m "feat(editor): Quip import wizard — token-entry + connect step"
```

---

## Task 9: Phase-0 verification (demo)

**Files:** none.
- [ ] Local: bring up the stack (`verify` skill), open the wizard, paste a
  REAL Quip personal access token, Connect → assert the profile + root folders
  render; verify (server logs) no token string appears at any level; verify the
  `IMPORT#…/META` Dynamo item exists with no token attribute; with the
  in-memory store, confirm `connect` works single-instance.
- [ ] Security check: grep the running server's captured logs for the token
  substring → absent. Confirm `git grep` shows no `Debug`/format of a
  `QuipToken`/`Secret` anywhere.
- [ ] Record outcomes; failures become findings.

## Self-Review Notes

- **Scope:** Phase 0 = connect + scope-data only; no inventory/content/comments
  (Phases 1–5). Each task is independently testable; the crate builds before
  the API wires it.
- **Security threading:** the token type (`QuipToken`/`Secret`) flows request →
  `current_user` → `TokenStore`, and is absent from `ImportRecord` (compile-
  enforced), logs (asserted), and errors (asserted in Task 3). Task 9 verifies
  end to end.
- **Flagged confirm-against-file:** `async-trait`/`dashmap`/`fastrand`
  workspace-dep names + features; the `AppState::new` param list + the dev-vs-
  SSM selection config flag; the `crates/api/tests` harness hook for injecting a
  mock-base `QuipClient`; the exact `require_infra!`/repo-test idiom.
- **Deliberate cross-cutting changes:** new workspace crate; new `aws-sdk-ssm`
  dep; `AppState::new` signature (one call site). All intentional, called out.
- **Deferred to later phases (not gaps):** the worker runner + Redis trigger +
  manifest checkpointing (Phase 1); everything content/identity/comments/
  report/re-run (Phases 2–5). Phase 0's token store already has the SSM impl so
  the worker can read it later.
