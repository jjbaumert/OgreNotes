# Quip Import — design

**Date:** 2026-07-24
**Area:** new `crates/quip-import` + `crates/api` (routes + an in-process runner)
+ `crates/storage` (import-state tables) + `crates/collab` (a Quip HTML
walker) + `frontend/` (import wizard). Backend cargo workspace; frontend is
outside it.
**Status:** DESIGN — awaiting disposition. No code until this is approved.

## Provenance & scope

Implements the "Quip Import (Documents, Structure, Comments, People)"
feature request. This document resolves the architecture and every item the
request marked "propose"; it is the disposition gate the request asked for
("Design document first … Stop for disposition before code"). It does not
contain an implementation plan — that follows on approval.

## Disposed decisions (confirmed 2026-07-24)

1. **Token lifetime:** the Quip PAT lives ONLY in the running import task's
   memory (a redacting/zeroizing wrapper), never in Redis, DynamoDB, S3, or
   logs. A process restart pauses the import in a `NeedsToken` state; the
   user re-pastes the token to resume from the last checkpoint.
2. **Unmatched comment authors:** preserved as author *metadata on the
   comment* (name + email + Quip user id, badged external) — no synthetic
   user accounts. Matched authors link to the real `user_id`.
3. **Changed thread on re-run:** never overwrite. Already-imported threads
   that changed in Quip are listed in the report; the user may opt in to
   re-import a chosen one as a NEW doc (mention-linked from the prior). No
   in-place content replacement, ever.
4. **Spreadsheets:** import as native OgreNotes `DocType::Spreadsheet` docs
   via the existing `import_spreadsheet::from_xlsx` path. Raw `.xlsx` is
   retained as a doc blob only as a fidelity fallback when conversion loses
   data.

## The architecture-forcing constraint

Browsers cannot call `platform.quip.com` (no CORS for arbitrary origins), so
the token and all Quip fetching are server-side. Decision 1 forbids
persisting the token. The existing Redis-Streams job queue
(`crates/worker`) serializes the whole job envelope into Redis and, on
failure, the dead-letter queue — putting a PAT there would persist it. That
queue is also *atomic* (crash ⇒ full redo or DLQ after 3 tries) with no
checkpoint model.

**Therefore the import runs as an API-process-hosted task, not a Redis
job.** The API instance that receives the token from the client's HTTPS
request spawns the runner and holds the PAT in that instance's memory. All
durable state (the manifest) lives in DynamoDB + S3 and contains NO token.
If the hosting instance restarts, the task dies; a sweeper marks the import
`NeedsToken`; the client re-supplies the token to any instance, which claims
the import and resumes from the checkpoint. This is the only topology that
satisfies "token in memory only, resumable" — a cross-process hand-off would
require persisting or network-relaying the secret.

```
Browser wizard ──HTTPS──▶ API instance ──▶ ImportRunner (tokio task)
   token+scope            │  in-mem token   │   ├─ QuipClient (throttled)
   progress polling       │  (AppState map) │   ├─ converter (crates/quip-import)
   identity confirm       │                 │   └─ writes via DocRepo/ThreadRepo/S3
                          ▼                 ▼
                   DynamoDB import tables   S3 staging (imports/{id}/…)
                   (manifest, NO token)     (raw HTML, message pages, blobs)
```

## Component layout

- **`crates/quip-import`** (new) — pure-ish domain crate: the `QuipClient`
  (endpoint wrappers + throttle), the Quip-HTML→OgreNotes converter/walker,
  the manifest types, and the phase state machine. No Axum, no direct AWS;
  takes repo/S3 handles by trait so it's unit-testable with fakes.
- **`crates/api/src/routes/imports.rs`** (new) — the wizard's REST surface
  and the in-process runner registry on `AppState`.
- **`crates/storage`** — new `ImportRepo` over new DynamoDB item types.
- **`crates/collab`** — the converter lives here or in `quip-import`; it
  produces a `yrs::Doc` (content fragment) the same shape `import::from_html`
  produces, then `snapshot::doc_to_bytes`. (It does NOT reuse `from_html`
  as-is — that drops tables/images/links/marks and assigns no block IDs.)
- **`frontend/src/components/quip_import/`** (new) — the wizard.

## REST surface (`/api/v1/imports/quip`)

All owner-gated. Token appears only in request bodies of `connect`/`token`,
never in any response or stored row.

| Method / path | Purpose |
|---|---|
| `POST /connect` | Body `{ token }`. Validate via Quip `GET /1/users/current`. Create an import record (`status=Scoping`), stash the token in the in-memory registry keyed by `import_id` (idle-TTL). Return `{ import_id, quip_profile, root_folders[] }` for the scope checklist. |
| `POST /{id}/start` | Body `{ selected_root_folder_ids[], target_folder_id?｜create, changed_thread_optins?[] }`. Authorizes the target folder in the request's auth context (like `documents::import_job`). Spawns the runner. Returns `{ estimate: {thread_count, projected_minutes} }`. |
| `GET /{id}` | Poll: `{ status, phase, progress{done,total,stage}, needs_token: bool, pending_identity_confirmation: bool }`. |
| `GET /{id}/identities` | The proposed identity table (matched / unmatched-with-email / anonymous) — served once the identity pre-pass completes; the runner *blocks here* until confirmed. |
| `POST /{id}/confirm-identities` | Body `{ overrides: [{quip_user_id, action: attribute(user_id)｜placeholder｜skip}] }`. Unblocks the runner into comment writing. |
| `POST /{id}/token` | Re-supply the token to resume a `NeedsToken` import; re-claims + restarts the runner from checkpoint. |
| `GET /{id}/report` | The final structured report (also embedded in `status=Succeeded`). |
| `POST /{id}/cancel` | Cooperative cancel; leaves completed docs, marks the rest skipped-cancelled in the report. |

**In-memory token registry:** `AppState.quip_tokens:
Arc<DashMap<ImportId, TokenEntry>>` where `TokenEntry { secret:
Secret<String>, last_used: Instant }`. `Secret<String>` is a small
zeroize-on-drop, `Debug=[redacted]` newtype (the `secrecy`/`zeroize` crates —
neither is currently a dependency; adding them is part of this feature). A
periodic sweep evicts idle entries and, when an import's runner is gone but
its Dynamo status is `Running` with a stale heartbeat, flips it to
`NeedsToken`.

## DynamoDB manifest schema (`ImportRepo`)

One partition per import: `PK = IMPORT#<import_id>`. All rows carry
`owner_id`; reads are owner-gated. A TTL attribute expires the whole
partition 30 days after completion. **No row ever contains the token.**

| SK | Item | Key fields |
|---|---|---|
| `META` | import record | `owner_id`, `status` (Scoping｜Running｜NeedsToken｜AwaitingIdentityConfirm｜Succeeded｜Failed｜Cancelled), `phase` (0–5), `target_folder_id`, `selected_roots[]`, `quip_user_id`, `runner_claim` `{instance_id, heartbeat_ms}`, `created_at`, `report_summary` (counts) |
| `FOLDER#<quip_folder_id>` | inventory folder | `title`, `parent_quip_id?`, `ogre_folder_id` (assigned) |
| `THREAD#<quip_thread_id>` | per-thread checkpoint (the resumability unit) | `title`, `type`, `updated_usec`, `member_folders[]`, `state` (Pending｜ContentDone｜CommentsDone｜Skipped{reason}), `ogre_doc_id?`, `content_s3_key?`, `first_folder` (canonical) |
| `SECMAP#<quip_thread_id>` | section→block map | `map: { quip_section_id: ogre_block_id }` (chunked to stay under 400 KB; large docs split across `SECMAP#<thread>#<n>`) |
| `IDMAP` | identity map | `authors: { quip_user_id: {name, email?, resolution: matched(user_id)｜placeholder｜anonymous｜skip} }` |
| `REPORT` | accumulating report | append-only counters + a bounded list of named losses/fallbacks |

**Why Dynamo for the index and S3 for payloads:** Dynamo items cap at
400 KB, so raw thread HTML, fetched message pages, and blobs stage to S3
under `imports/{import_id}/…` (reusing `S3Client::put_object` and the
`imports/` prefix `documents::import_job` already writes to). Dynamo holds
only cursors, statuses, and the two maps. Per-thread items give clean,
idempotent, conditional-write checkpointing with no read-modify-write races
(the failure mode of a single JSON blob manifest).

**Section→block persistence (called out because comment anchoring and future
link rewriting both depend on it):** during Phase 2 the converter assigns
each block a `blockId` (server-side `schema::generate_block_id`,
`[A-Za-z0-9]{10}` — a valid `#b=` anchor target) and records
`quip_section_id → blockId` into `SECMAP#…` *before* serializing the doc, in
the same checkpoint that flips the thread to `ContentDone`. Phase 4 reads it
to anchor inline comments; the link-rewrite step reads it to build `#b=`
anchors. It survives for the import's lifetime + report retention.

## Pipeline (each phase checkpointed in the manifest)

- **Phase 0 Connect & scope** — `POST /connect` validates the token and
  returns the Quip root folders; the wizard shows a scope checklist (default
  private + shared) and a target-folder picker; `POST /start` walks folder
  metadata to estimate thread count and projects time from the throttle rate,
  then spawns the runner.
- **Phase 1 Inventory** — BFS the selected folders; write `FOLDER#…` and
  `THREAD#…` rows (title, type, `updated_usec`, `member_folders`). **Dedup /
  tag membership:** a thread is imported ONCE, at its first-encountered
  folder (`first_folder`). *Proposed representation of the extra
  memberships:* add the single imported doc as a `FolderChild` under each
  other folder too (multi-folder membership — one doc, no duplicated
  content), which is the exact OgreNotes-native analog of Quip's tag-like
  folders. **This diverges from the request's "document mentions at the other
  locations"**, which doesn't fit the folder model (a folder holds
  doc/folder children, not inline mention content). See Open Item D.
- **Phase 2 Content** — per document thread: fetch `/2/threads/{id}/html`
  (paginated) → stage to S3 → run the Quip walker →
    - map every section id to a fresh `blockId` (persist `SECMAP`),
    - fetch each referenced blob (`GET /1/blob/…`) → `put_object` under
      `blobs/{ogre_doc_id}/{blob_id}/…` → set the `Image` node `src`,
    - defer link rewriting to a second pass (targets may not have doc ids
      until their own thread is imported): after all threads have
      `ogre_doc_id`s, rewrite intra-import `quip.com` thread URLs →
      `DocMention` nodes (`doc_id` set; `url={origin}/d/{doc_id}`), and
      section-fragment URLs → `#b=<blockId>` anchors via `SECMAP`
      (`target_block_id` set) — the exact node/attr shape and URL form
      `parse_ogre_doc_url` already consumes. External URLs pass through.
    - Spreadsheet threads → native Spreadsheet doc (decision 4). Embedded
      grids → *proposed threshold* (Open Item A). Chat-type threads → skip
      content, note in report.
    - create the doc via the `worker_mode::persist_imported_document` path
      (`DocRepo::create` + `FolderRepo::add_child`), preserving Quip
      created/updated as document metadata; checkpoint `ContentDone`.
- **Phase 3 People (identity pre-pass + gated confirm)** — *Proposed
  ordering (Open Item B):* run a message-metadata scan at the FRONT of the
  comment work: page every selected thread's `/1/messages` once, stage the
  pages to S3, collect the distinct `author_id` set + each message's
  `created_usec`/section refs into the manifest. Resolve the author set via
  `/1/users/` batch, build `IDMAP`, set `status=AwaitingIdentityConfirm`, and
  BLOCK. The wizard shows the mapping table (email-matched via
  `UserRepo::get_by_email`, lowercased + plus-addressing stripped for compare
  but preserved for display; unmatched-with-email; anonymous). On
  `confirm-identities` the runner proceeds. Automation proposes, human
  disposes.
- **Phase 4 Comments** — replay the staged message pages oldest-first, per
  thread, splitting the two populations:
    - inline annotations → `ThreadType::Inline`, `block_id` from `SECMAP`;
      unmapped section → document-level thread with a "context lost" marker +
      the original highlighted text if recoverable.
    - conversation-pane messages → one `ThreadType::Document` discussion,
      chronological.
    - written via `ThreadRepo::create_thread` / `add_message` DIRECTLY (not
      the REST routes, which stamp caller id + now) to preserve original
      author + `created_at`. Author id = matched `user_id`; unmatched →
      the new author-metadata field (decision 2). Attachments via blob fetch.
      Checkpoint `CommentsDone` per thread.
- **Phase 5 Report & verify** — counts per phase; imported vs skipped
  (with reasons); comments anchored vs context-lost; identities matched vs
  placeholder vs anonymous; every fallback taken. Nothing dropped silently.

## Identity map semantics (detail)

- Compare key: `email.trim().to_lowercase()`, with `+tag` stripped from the
  local part for MATCHING only (display keeps the original). `get_by_email`
  already lowercases.
- `matched` → `Message.user_id = <ogre user_id>`, no author metadata.
- `placeholder` (email present, no match, or user chose placeholder) →
  `user_id = "quip:<quip_user_id>"` sentinel + `imported_author {name,
  email, quip_user_id}` metadata; frontend renders the metadata with an
  external badge and never tries to resolve the sentinel as a real user.
- `anonymous` (no email visible) → same, `email=None`.
- `skip` (user opts out) → comments from that author are omitted, counted in
  the report.

New storage: an `imported_author: Option<ImportedAuthor>` field on `Message`
(and `Thread.created_by` may hold the sentinel). Frontend comment rendering
learns to show `imported_author` when present. This is additive and the only
schema change to an existing table.

## Idempotent re-run semantics (detail)

- Every imported doc stores `quip_thread_id` + `quip_updated_usec` in its
  DocumentMeta (a new sparse field, or the import's Dynamo `THREAD#` row keyed
  by owner+quip_thread_id serves as the dedup index — proposed: the latter,
  to avoid a DocumentMeta schema change; Open Item C).
- Re-run against the same target: threads whose `quip_thread_id` already
  imported and whose `updated_usec` is unchanged → skipped (counted).
- Changed (`updated_usec` newer) → listed in the report as "changed since
  import"; NOT touched. `start` accepts `changed_thread_optins[]` to
  re-import specific ones as NEW docs (mention-linked from the prior via a
  `DocMention`), never overwriting (decision 3).

## Throttle design

- One `QuipClient` per running import; ALL Quip requests funnel through it.
- Token-bucket pinned at **45 req/min** (10% under the 50/min limit), refill
  computed from `X-Ratelimit-Reset`; before each request, consult
  `X-Ratelimit-Remaining` and pre-emptively sleep when it approaches 0.
- On HTTP 503 "Over Rate Limit": exponential backoff with full jitter
  (base 1 s, cap 60 s), honoring `X-Ratelimit-Reset` as a floor; the thread's
  checkpoint means a backoff/stall never loses progress.
- On 401/403: pause `NeedsToken`, drop the in-memory token, surface to the
  wizard for re-auth; resume from checkpoint.
- Rate-limit waits are cooperative-cancellable and heartbeat the Dynamo
  `runner_claim` so the sweeper doesn't reclaim a legitimately-throttled job.
- No worker-side limiter exists today; this is new (a small `governor` or
  hand-rolled bucket inside `quip-import`, independent of the request-path
  `middleware::rate_limit`).

## Security & privacy

- Token: `Secret<String>`, zeroized on drop, `Debug=[redacted]`; never
  serialized into Dynamo/Redis/S3/logs. `tracing` call sites in the runner
  log only `import_id`/`owner_id`/counts — never the token or user emails
  (emails appear only in the identity table response and comment metadata,
  both owner-scoped, and never in logs at any verbosity).
- All writes go through the normal repo create paths, so block IDs (now
  server-assigned), indexing, and edit-history behave as authored. (Caveat:
  the worker-style create path does NOT auto-index for search; the importer
  will explicitly call `spawn_index_document_from_bytes` so imported docs are
  searchable immediately — a deliberate deviation from `worker_mode`.)
- Per-document transactionality: a doc + its comments land completely or the
  thread stays `Pending`/`ContentDone` and is retried; the `THREAD#`
  checkpoint is the transaction boundary.

## Open items proposed here (dispose at review)

- **A — embedded-spreadsheet inline threshold.** Proposed: a grid with
  ≤ 20 rows × ≤ 12 cols AND ≤ 200 non-empty cells renders inline as a native
  `Table` block in the parent doc; anything larger becomes a separate
  `DocType::Spreadsheet` doc, `DocMention`-linked. Standalone spreadsheet
  threads are always their own Spreadsheet doc.
- **B — identity pre-pass ordering** (message-scan front-loaded, above).
  Proposed as described so messages are fetched once and the identity gate
  precedes any comment write.
- **C — dedup index location.** Proposed: the import's `THREAD#` Dynamo rows
  (keyed by owner + `quip_thread_id`) are the re-run dedup index, avoiding a
  DocumentMeta schema change. Alternative: add `quip_thread_id` to
  DocumentMeta (queryable independent of an import record, but a schema
  change).
- **D — multi-folder membership vs mentions** for tag-like Quip folders.
  Proposed: multi-`FolderChild` membership (native, no duplication). Diverges
  from the request's "DocMention at other locations." **Needs confirmation
  that the OgreNotes sidebar/UI renders a doc that is a child of multiple
  folders sanely** — a design-review verification item; fallback is
  "canonical location only, cross-membership noted in the report."

## Build order (each phase demonstrable against a real Quip account)

0. `crates/quip-import` skeleton: `QuipClient` + throttle + `/1/users/current`
   validation; `Secret` wrapper; `connect` endpoint + token registry.
   Demo: paste token, see profile + roots.
1. Inventory + manifest (`ImportRepo`, Dynamo tables) + scope UI + estimate.
   Demo: scoped inventory persisted, resumable.
2. The Quip HTML walker (tables/lists/checklists/code/images/marks +
   section-id capture + blockId assignment) with fixture tests; content
   import + blob fetch; link-rewrite second pass. Demo: real docs imported
   faithfully with working image + intra-import links.
3. Identity pre-pass + mapping table UI + confirm gate. Demo: mapping shown,
   overrides honored.
4. Comment import (inline-anchored + conversation) with preserved
   author/timestamp + `imported_author` rendering. Demo: comments land
   anchored and attributed.
5. Report UI + idempotent re-run + changed-thread opt-in. Demo: re-run is a
   no-op except opted-in changes; report names every outcome.

## Testing

- Converter fixtures: headings, ordered/bulleted/checklists, tables, code
  (with language), images, inline marks, section-id→block mapping, and the
  link-rewrite matrix (intra-import thread URL, section-fragment URL,
  external URL).
- Identity normalization (case, plus-addressing, missing email).
- Manifest resume: simulate a 503 storm and a mid-phase restart; assert no
  refetch of `ContentDone` threads and exact-boundary message paging
  (`max_created_usec` on the last message of a full page).
- Throttle: never exceeds 45/min under load; honors headers; backoff+jitter
  on injected 503s.
- Security: property test that no code path serializes a `Secret` and that
  the token never appears in a Dynamo item, an S3 object, or a captured log.

## Out of scope

- Live/incremental sync (this is a one-time import; re-run is manual).
- Quip live-app/embedded non-spreadsheet widgets beyond best-effort HTML.
- Importing Quip access-control/sharing (imported docs are owned by the
  importing user).

## Reference (consult, do not copy)

`github.com/quip/quip-api` — `samples/baqup` (full-account export; its
rate-limit troubles motivate the throttle) and `quip.py` for endpoint shapes.
