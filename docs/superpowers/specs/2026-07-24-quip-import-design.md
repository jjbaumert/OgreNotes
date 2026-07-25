# Quip Import — design

**Date:** 2026-07-24 (rev. 2 — token store + link index)
**Area:** new `crates/quip-import` + `crates/api` (routes + worker runner) +
`crates/storage` (import-state + link-index tables) + `crates/collab` (Quip
HTML walker) + `frontend/` (import wizard + optional backlinks panel).
Backend cargo workspace; frontend outside it.
**Status:** DESIGN — awaiting disposition. No code until approved.

## Provenance & scope

Implements the "Quip Import (Documents, Structure, Comments, People)"
request, incorporating the reviewer's rev-2 steers: (a) a secure transient
token store is acceptable if it eases the design; (b) collect unresolved
links and back-patch them at the end; (c) persist all links in DynamoDB as a
durable index that also powers a "referenced by / backlinks" capability. This
is the disposition gate; no implementation plan yet.

## Disposed decisions

1. **Token lifetime (revised):** the Quip PAT is held in a **secure,
   transient, per-import store** (recommended: an SSM SecureString parameter
   `/{prefix}import/{import_id}/quip-token`, KMS-backed, or a KMS-envelope-
   encrypted Dynamo item — see Open Item E), created at `connect`, deleted on
   any terminal state, with a sweeper deleting stale entries as a backstop.
   The worker also holds a `Secret<String>` copy in memory (zeroize-on-drop,
   `Debug=[redacted]`). It is never placed in a Redis job envelope, never in
   a manifest row, never logged. Consequence: the import can be **worker-
   hosted** and **auto-resumes after a restart** by re-reading the store — no
   client re-prompt.
2. **Unmatched comment authors:** preserved as author metadata on the comment
   (`imported_author {name, email?, quip_user_id}`, badged external) — no
   synthetic user accounts. Matched authors link to the real `user_id`.
3. **Changed thread on re-run:** never overwrite. Changed threads are listed
   in the report; the user may opt in to re-import a chosen one as a NEW doc,
   mention-linked from the prior. No in-place content replacement.
4. **Spreadsheets:** native `DocType::Spreadsheet` via
   `import_spreadsheet::from_xlsx`; raw `.xlsx` retained as a doc blob only as
   a fidelity fallback on data loss.

## Architecture

Browsers can't call `platform.quip.com` (CORS), so all Quip fetching is
server-side. With a secure token store (decision 1) the long, rate-limited,
resumable import lives in the **worker process**, not the API — long jobs
belong there, off the request path.

The existing Redis-Streams queue is *atomic* (crash ⇒ full redo/DLQ, 60 s
reaper) and serializes its envelope to Redis + DLQ, so it cannot host a
multi-hour checkpointed job nor carry the token. Instead:

- The Redis queue carries only a **token-free trigger** `StartQuipImport
  { import_id, owner_id }` to wake a worker promptly.
- Durable state is a **DynamoDB manifest** (below). A dedicated import loop in
  the worker **claims** an import (conditional write of `runner_claim
  {instance_id, heartbeat_ms}`) and heartbeats while running; a
  `claim_stale`-style sweep reclaims imports whose heartbeat went cold (crash/
  redeploy). On (re)claim the worker re-reads the token from the secure store
  and resumes from the last per-thread checkpoint.

```
Browser wizard ──HTTPS──▶ API (routes/imports.rs) ──▶ secure token store
  token+scope             │  enqueue trigger              (SSM SecureString)
  progress / identity     ▼                                     ▲
                    Redis trigger ──▶ Worker import loop ────────┘ (read at run)
                                      │  QuipClient (throttled)
                                      │  converter (crates/quip-import)
                                      ▼  DocRepo/ThreadRepo/S3 writes
                    DynamoDB: manifest + link index    S3: imports/{id}/ staging
```

## Component layout

- **`crates/quip-import`** (new) — `QuipClient` (endpoints + throttle), the
  Quip-HTML→OgreNotes walker, manifest/link types, the phase state machine.
  Repo/S3/secret access via traits for unit tests with fakes.
- **`crates/api/src/routes/imports.rs`** (new) — wizard REST surface; writes
  the token to the secure store; enqueues the trigger.
- **`crates/api/src/worker_mode.rs`** — add the import loop + `StartQuipImport`
  dispatch alongside the existing atomic handlers.
- **`crates/storage`** — new `ImportRepo` (manifest) and `LinkRepo` (durable
  link index).
- **`crates/collab`** — the converter (new Quip walker; not the lossy
  `from_html`) producing a `yrs::Doc` → `snapshot::doc_to_bytes`.
- **`frontend/src/components/quip_import/`** (new) — the wizard; plus an
  optional `referenced_by` panel (Open Item F).

## REST surface (`/api/v1/imports/quip`, owner-gated)

Token appears only in the `connect` request body; never in any response or
manifest row.

| Method / path | Purpose |
|---|---|
| `POST /connect` | `{ token }` → validate via Quip `/1/users/current`; create manifest `META (status=Scoping)`; write token to the secure store keyed by `import_id`; return `{ import_id, quip_profile, root_folders[] }`. |
| `POST /{id}/start` | `{ selected_root_folder_ids[], target_folder_id?｜create, changed_thread_optins?[] }`; authorize target folder in the request auth context; enqueue `StartQuipImport`; return `{ estimate }`. |
| `GET /{id}` | `{ status, phase, progress{done,total,stage}, pending_identity_confirmation }`. |
| `GET /{id}/identities` | proposed identity table; runner blocks here until confirmed. |
| `POST /{id}/confirm-identities` | `{ overrides:[{quip_user_id, action}] }` → unblock. |
| `GET /{id}/report` | final structured report. |
| `POST /{id}/cancel` | cooperative cancel; secure token deleted. |

(No `/token` re-prompt endpoint — auto-resume from the store. A 401/403 from
Quip mid-run marks the import `TokenRejected` and surfaces a re-connect
prompt, which is the only path that re-collects a token.)

## DynamoDB manifest (`ImportRepo`) — `PK = IMPORT#<import_id>`

All rows carry `owner_id`; owner-gated. TTL expires the partition 30 days
after completion. **No row contains the token.**

| SK | Item | Key fields |
|---|---|---|
| `META` | import record | `owner_id`, `status`, `phase`, `target_folder_id`, `selected_roots[]`, `quip_user_id`, `runner_claim{instance_id,heartbeat_ms}`, `report_summary` |
| `FOLDER#<quip_folder_id>` | inventory folder | `title`, `parent_quip_id?`, `ogre_folder_id` |
| `THREAD#<quip_thread_id>` | per-thread checkpoint (resumability unit) | `title`, `type`, `updated_usec`, `member_folders[]`, `state (Pending｜ContentDone｜CommentsDone｜Skipped{reason})`, `ogre_doc_id?`, `content_s3_key?`, `first_folder` |
| `SECMAP#<quip_thread_id>[#<n>]` | section→block map | `{ quip_section_id: ogre_block_id }` (chunked <400 KB) |
| `UNRESOLVED#<source_quip_thread_id>` | pending back-patch links | `[ {source_block_id, kind: doc｜section, target_quip_thread_id, target_quip_section_id?} ]` |
| `IDMAP` | identity map | `{ quip_user_id: {name,email?,resolution} }` |
| `REPORT` | accumulating report | counters + bounded list of named losses/fallbacks |

Rationale for Dynamo-index + S3-payload split: Dynamo's 400 KB item cap makes
it wrong for raw HTML/message-pages/blobs (→ S3 under `imports/{id}/…`,
reusing `S3Client::put_object` and the existing `imports/` prefix). Per-thread
items give idempotent conditional-write checkpointing with no
read-modify-write races.

## Link handling: two-pass back-patch + durable index

**Why two passes:** when converting doc A we may encounter a `quip.com` link
to thread B before B has been imported (no `ogre_doc_id`, no `SECMAP` yet). So
the converter emits a **placeholder** and records the edge in
`UNRESOLVED#<A>`; a back-patch pass after all threads reach `ContentDone`
resolves every edge (both endpoints now known), rewriting A's content and
recording the settled edge in the durable index.

**Placeholder representation:** an unresolved intra-import link is stored as
plain text of the original Quip URL wrapped so the back-patch can find it
(e.g. a `DocMention` with `doc_id=""` + a `pending_quip_thread` attr, hidden
from resolve until patched); on patch it becomes a real `DocMention`
(`doc_id`, `url={origin}/d/{doc_id}[#b=<blockId>]`, `target_block_id` from the
target's `SECMAP`). External URLs pass through untouched at pass 1.

**Durable link index (`LinkRepo`) — a permanent OgreNotes capability, not
import-only.** A separate table of resolved reference edges, keyed for
**reverse lookup** so any doc can answer "what references me?":

- `PK = LINKTGT#<target_doc_id>`, `SK = SRC#<source_doc_id>#<source_block_id>`
- attrs: `target_block_id?`, `origin (quip_import｜mention)`, `created_at`,
  and a denormalized `source_title` for cheap rendering.
- A forward view (`PK = LINKSRC#<source_doc_id>`) supports edge cleanup when a
  source doc is deleted or a mention is removed.

The import populates this index during back-patch. **Proposed:** the editor's
`DocMention` create/convert/resolve paths (`commands.rs`,
`mention_overlay.rs`) also upsert/remove edges here, so backlinks stay correct
for user-authored mentions — making the index a first-class feature seeded,
not defined, by the import. This subsumes the reviewer's "store all links in
DynamoDB" and enables the "referenced by" surface (Open Item F).

## Pipeline (each phase checkpointed)

- **Phase 0 Connect & scope** — `connect` validates + returns roots + stores
  token; wizard scope checklist + target-folder picker; `start` estimates
  thread count and projects time from the 45/min throttle, enqueues trigger.
- **Phase 1 Inventory** — BFS selected folders → `FOLDER#`/`THREAD#` rows.
  **Dedup / tag membership:** import once at `first_folder`; represent the
  other memberships as multi-`FolderChild` (one doc, several folders — the
  native analog of Quip tag-folders). Diverges from the request's "mentions
  at other locations," which has no home in the folder model. See Open Item D.
- **Phase 2 Content** — per doc thread: fetch `/2` HTML → stage S3 → walk →
  assign `blockId` per block + persist `SECMAP` → fetch blobs → `put_object`
  under `blobs/{doc_id}/…` + set `Image.src` → emit placeholders + record
  `UNRESOLVED#` for intra-import links → create doc via the
  `persist_imported_document` path (+ explicit search index) preserving Quip
  timestamps → checkpoint `ContentDone`. Spreadsheet threads → native
  Spreadsheet doc; embedded grids → threshold (Open Item A); chat threads →
  skip, note. **Back-patch pass** after all `ContentDone`: resolve every
  `UNRESOLVED#` edge → rewrite content → write `LinkRepo` edges.
- **Phase 3 People (front-loaded identity pre-pass, gated)** — page every
  thread's `/1/messages` once → stage pages to S3, collect distinct
  `author_id` + `created_usec` + section refs → resolve via `/1/users/` batch
  → build `IDMAP` → `status=AwaitingIdentityConfirm`, BLOCK. Wizard shows the
  mapping table (email-matched via `get_by_email`, lowercased + plus-stripped
  for compare, preserved for display; unmatched-with-email; anonymous). On
  confirm, proceed. (Open Item B.)
- **Phase 4 Comments** — replay staged pages oldest-first: inline annotations
  → `ThreadType::Inline` `block_id` from `SECMAP` (unmapped → document-level +
  "context lost" marker + original highlighted text); conversation-pane →
  one `ThreadType::Document` thread chronological. Written via
  `ThreadRepo::create_thread`/`add_message` DIRECTLY (not REST routes, which
  restamp author+now) to preserve original author (`user_id` if matched, else
  `imported_author` metadata) + `created_at`; attachments via blob fetch.
  Checkpoint `CommentsDone`.
- **Phase 5 Report** — counts per phase; imported/skipped(+reason);
  anchored/context-lost; matched/placeholder/anonymous; every fallback.
  Nothing dropped silently.

## Identity map semantics

Compare key `email.trim().lowercase()` with `+tag` stripped for matching only
(display keeps original). `matched` → real `user_id`, no metadata.
`placeholder`/`anonymous` → sentinel `user_id="quip:<quip_user_id>"` +
`imported_author` metadata, rendered with an external badge, never resolved as
a real user. `skip` → omitted, counted. Only additive schema change to an
existing table: `Message.imported_author: Option<ImportedAuthor>` + comment
rendering support.

## Idempotent re-run

Dedup index = the import's `THREAD#` rows keyed by owner + `quip_thread_id`
(Open Item C — avoids a DocumentMeta change). Unchanged `updated_usec` →
skip (counted); newer → listed "changed since import," untouched;
`changed_thread_optins[]` re-imports chosen ones as NEW mention-linked docs.
Never overwrites (decision 3).

## Throttle

One `QuipClient` per import; all requests funnel through it. Token bucket at
**45 req/min** (10% under 50), refill from `X-Ratelimit-Reset`, pre-emptive
sleep as `X-Ratelimit-Remaining`→0. On 503: exp backoff + full jitter (1 s
base, 60 s cap) floored at `X-Ratelimit-Reset`; checkpoints mean a stall
loses nothing. Waits are cancellable and heartbeat `runner_claim` so the
sweeper won't reclaim a throttled job. New code (`governor` or hand-rolled),
independent of the request-path limiter.

## Security & privacy

- Token: transiently in the secure store (KMS-backed), memory copy is
  `Secret<String>` (zeroize, redacted Debug), deleted on every terminal state
  + swept as backstop; never in Redis/manifest/S3/logs. Runner `tracing` logs
  only `import_id`/`owner_id`/counts. Emails appear only in owner-scoped
  identity responses + comment metadata, never in logs.
- All writes via normal repo create paths; block IDs server-assigned; the
  importer explicitly indexes for search (deviation from `worker_mode`, which
  defers indexing).
- Per-doc transactionality: doc + its comments land completely or the
  `THREAD#` checkpoint stays short of `CommentsDone` and is retried.

## Open items (dispose at review)

- **A — embedded-spreadsheet inline threshold.** Proposed: ≤ 20 rows × 12
  cols AND ≤ 200 non-empty cells → inline native `Table`; larger → separate
  linked Spreadsheet doc. Standalone spreadsheet threads always own doc.
- **B — identity pre-pass front-loaded** (messages fetched once, gate before
  any comment write). Proposed as above.
- **C — dedup index location.** Proposed: the import `THREAD#` rows (no
  DocumentMeta change). Alt: add `quip_thread_id` to DocumentMeta (queryable
  standalone, but a schema change).
- **D — multi-folder membership vs mentions** for tag-folders. Proposed
  multi-`FolderChild`. Needs a quick check that the sidebar renders a
  multi-parent doc sanely; fallback: canonical folder only + report note.
- **E — secure token store choice.** Proposed SSM SecureString (project-
  idiomatic, KMS-backed, separate IAM) + delete-on-terminal + stale sweeper.
  Alt: KMS-envelope-encrypted Dynamo item with native TTL (one store, harder
  delete guarantees satisfied by TTL). Both add a runtime AWS SDK path not
  present today.
- **F — backlinks scope.** The `LinkRepo` index is in scope (import needs it
  for back-patch + it's the reviewer's ask). Building the "Referenced by" UI
  now (reuse `relationship_panel` + a `GET /documents/:id/backlinks`
  endpoint) vs. laying only the index this milestone — dispose. Also dispose:
  wire the editor mention paths to feed `LinkRepo` now (keeps backlinks live
  for user mentions) or defer.

## Build order (each phase demoable on a real account)

0. `quip-import` skeleton + `QuipClient`/throttle + `/1/users/current` +
   `Secret` + secure store + `connect`. Demo: paste token → profile + roots.
1. Inventory + `ImportRepo` + scope UI + estimate; worker trigger + claim +
   resume. Demo: scoped inventory persisted + resumable.
2. Quip walker (tables/lists/checklists/code/images/marks + section capture +
   blockId) with fixtures; content + blobs; `UNRESOLVED` + back-patch;
   `LinkRepo` writes. Demo: real docs faithful, images + intra-import links
   work.
3. Identity pre-pass + mapping UI + confirm gate.
4. Comments (inline + conversation) preserving author/timestamp +
   `imported_author` rendering.
5. Report UI + idempotent re-run + changed-thread opt-in. (Optionally: the
   "Referenced by" panel, per Open Item F.)

## Testing

Converter fixtures (headings/lists/checklists/tables/code+lang/images/marks/
section→block/link-rewrite matrix); identity normalization; manifest resume
(503 storm + mid-phase restart: no refetch of `ContentDone`; exact-boundary
`max_created_usec` paging); throttle (≤45/min, header-honoring, backoff+
jitter); link index (edge upsert/reverse-lookup/cleanup on delete); security
(no `Secret` ever serialized; token absent from every Dynamo item, S3 object,
captured log).

## Out of scope

Live/incremental sync; non-spreadsheet Quip live-apps beyond best-effort HTML;
importing Quip ACLs (imported docs owned by the importer).

## Reference (consult, don't copy)

`github.com/quip/quip-api` — `samples/baqup` (rate-limit cautionary tale) +
`quip.py` endpoint shapes.
