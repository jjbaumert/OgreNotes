# Open Failures — Remediation Plan

**Scope:** the seven issues filed during the Quip Phase 2a / durable-image work (#138, #140, #141, #142, #144, #145, #146), sequenced by what unblocks real use.

**Shape:** five work units. Unit 1 is specified in enough detail to execute immediately. Units 2–5 get their own implementation plans when picked up — writing them now would be guessing at code that Unit 1 may reshape.

---

## Sequencing rationale

The single most important finding while grounding this plan: **#141 and #142 are not independent.** Both need the same machinery, and building them separately means building it twice.

Verified in the code:

- `crates/quip-import/src/client.rs:261` maps **both 401 and 403** to `QuipError::Unauthorized`.
- `ThreadState` (`crates/storage/src/models/import_inventory.rs:10`) is `Pending | ContentDone | CommentsDone | Skipped` — there is **no per-thread failure state**, and `Skipped` carries no reason.
- **No `REPORT` row exists.** The design specifies one (`REPORT` = "counters + bounded list of named losses/fallbacks") but schedules it for Phase 5. Today every per-thread outcome — skipped chats, dropped images, the flat-folder fallback — is a `tracing` event the user never sees.

#141's fix is "a 403 on one thread skips that thread." #142's fix is "a deterministic failure on one thread skips that thread." Those are the same mechanism with different triggers. And neither is safe to ship without a way to tell the user *which* threads were skipped and why — otherwise a migration silently drops documents, which is worse than halting.

So Unit 1 is all three together.

| Unit | Issues | Blocks | Size |
|---|---|---|---|
| **1. Per-thread failure handling** | #141, #142, + minimal `REPORT` | **A real migration** | M |
| **2. Recovery robustness** | #144 | Fleet reliability under crash recovery | S |
| **3. Editor correctness** | #145, #146 | Nothing; user-visible bugs today | S |
| **4. Image reach** | #140 | Templates gallery being usable | M (design decision first) |
| **5. Search indexing** | #138 | Imported docs being findable | L (architectural) |

Units 2–5 are mutually independent and can be done in any order, or in parallel by different people. Only Unit 1 has an ordering constraint (it must come before a real migration).

---

## Unit 1 — Per-thread failure handling (#141, #142, minimal `REPORT`)

**Goal:** one bad thread costs you that thread and a line in a report, not the entire import — and never a misleading diagnosis.

**Why first:** #141 is the highest-likelihood failure of a real migration. A single access-restricted Quip document halts the import and reports the user's token as expired. The user reconnects a perfectly valid token, re-runs, hits the same thread, and halts again — a permanent wedge pointing at the wrong cause. Any Quip account with one restricted doc hits this on the first try.

### Task 1 — Split `Unauthorized` into `Unauthorized` (401) and `Forbidden` (403)

**Files:** `crates/quip-import/src/client.rs`

`observe_and_check` currently collapses both statuses. Split them:

```rust
if status.as_u16() == 401 { return Err(QuipError::Unauthorized); }
if status.as_u16() == 403 { return Err(QuipError::Forbidden); }
```

This is a **public-API change** to a `pub` enum — flag it per CLAUDE.md, and update every existing `match` on `QuipError` (the compiler will find them; do not add a catch-all arm that would silently swallow the new variant).

**Tests:** wiremock cases asserting 401 → `Unauthorized` and 403 → `Forbidden`, and that neither error's `Display` contains the token (extend the existing `unauthorized_and_rate_limited_map` guard).

**Note:** `connect` (`routes/imports.rs`) maps `Unauthorized` → 400 "invalid Quip token". Decide whether a 403 at connect time should say something different — a 403 from `/1/users/current` genuinely does mean the token lacks access, so mapping `Forbidden` the same way there is defensible. Say which you chose and why.

### Task 2 — Add a per-thread failure state with a reason

**Files:** `crates/storage/src/models/import_inventory.rs`, `crates/storage/src/repo/import_repo.rs`

`ThreadState` gains `Failed`, and both `Failed` and `Skipped` gain a reason. The existing `set_thread_skipped` takes no reason today; chat-skips are already invisible for this same lack.

Two shapes to choose between — **decide deliberately and record it**:

- **(a)** `ThreadState::Failed` + a separate `skip_reason: Option<String>` attribute on the `THREAD#` row. Keeps `ThreadState` a plain lowercase-serde enum (the current convention), no serialization change to the existing variants.
- **(b)** `ThreadState::Failed { reason }` / `Skipped { reason }` as data-carrying variants, matching the *design*'s `Skipped{reason}`. Changes the serialized shape of an existing variant — a wire change to flag.

(a) is the smaller change and preserves the shipped serialization; (b) matches the design doc. Phase 2a already deviated to (a)'s spirit by putting the chat-skip reason in the report — so (a) is the consistent choice unless you want to converge on the design.

**Tests:** round-trip both states with and without a reason; confirm existing rows decode unchanged (backward compatibility — imports may be mid-flight across a deploy).

### Task 3 — Minimal `REPORT` row

**Files:** `crates/storage/src/models/import_inventory.rs`, `crates/storage/src/repo/import_repo.rs`

The design specifies `REPORT` as "counters + bounded list of named losses/fallbacks." Build exactly that — **not** the Phase 5 report UI:

```rust
pub struct ReportRow {
    pub owner_id: String,
    pub counters: BTreeMap<String, u64>,   // e.g. threads_imported, threads_skipped, images_dropped
    pub notes: Vec<ReportNote>,            // BOUNDED — see below
}
pub struct ReportNote { pub quip_thread_id: String, pub kind: String, pub detail: String }
```

**The bound is load-bearing.** DynamoDB's 400 KB item cap applies here exactly as it did to `SECMAP#`/`UNRESOLVED#`. A 10k-thread import where every thread fails would blow the item. Cap `notes` at a constant (say 200), keep counting in `counters` past the cap, and record that truncation happened so the report can say "and 9,800 more." Do **not** chunk this one — a bounded list is the right shape for something a human reads.

Provide `append_report_note` and `bump_report_counter` as idempotent-ish upserts. Note the read-modify-write hazard: the content pass is single-writer per import (the runner lease guarantees it), so a plain read-modify-write is acceptable — **say so in a comment**, because it is only safe because of the lease.

**Tests:** counters accumulate; notes truncate at the cap while counters keep counting; the row round-trips; no token/secret key (mirror the existing guards).

### Task 4 — Wire the disposition into the content pass

**Files:** `crates/api/src/worker_mode.rs`

This is the behavior change everything else supports.

`ThreadImportError` gains a per-thread-fatal disposition. The mapping becomes:

| Condition | Disposition |
|---|---|
| `Unauthorized` (401) — credential is dead | **Run-terminal.** `set_status(TokenRejected)`, stop. Unchanged. |
| `Forbidden` (403) on a thread or blob | **Thread-skip.** `set_thread_skipped(reason)`, report note, **continue to the next thread.** |
| Transient (`RateLimited`/`Http`/`Api`/`Parse`) | Retry as today — but count attempts per thread. |
| Same thread failed > N times (N = 3) | **Thread-fail.** `set_thread_failed(reason)`, report note, **continue.** |
| Storage/lease failure | **Run-terminal.** Unchanged — these are not per-thread conditions. |

The per-thread attempt counter needs somewhere to live. Simplest: an `attempts` attribute on the `THREAD#` row, incremented before the work and read on entry — which also survives process restarts, unlike an in-memory count. Note this interacts with the job-level `MAX_RETRIES`: a thread that fails 3 times should be marked `Failed` and skipped *without* failing the job, so the pass continues rather than dead-lettering.

**Preserve:** resumability (`ContentDone` threads still skipped with zero Quip calls), the reserved-`doc_id` idempotency from Phase 2a's fixes, and the lease heartbeat.

**Tests (integration, `require_infra!` + wiremock):**
- A 403 on one thread skips it and **the other threads still import** (assert both a skipped row *and* documents for the rest).
- A thread failing deterministically N+1 times is marked `Failed` and the pass **completes** for the others.
- A 401 still halts the whole run (regression — the terminal path must not be weakened).
- The report row names the skipped/failed threads with reasons.
- Mutation-check the headline: revert the continue-on-thread-failure and confirm the "others still import" test fails.

### Task 5 — Surface it in the wizard

**Files:** `crates/api/src/routes/imports.rs`, `frontend/src/components/quip_import/`, `frontend/locales/*`

`GET /imports/quip/{id}` returns the counters and (bounded) notes. The wizard's completion state shows "Imported N documents, skipped M" with the reasons expandable, instead of today's bare "Imported N items."

This is what makes the whole unit worth doing: without it, a skipped thread is indistinguishable from a thread that never existed.

**Verification:** wasm32 build; the poll-loop generation guard and terminal-status handling must survive untouched.

### Unit 1 demo gate

Run an import against a Quip account containing at least one document you cannot access. It completes, imports everything else, and tells you which document it skipped and why.

---

## Unit 2 — Recovery robustness (#144)

`reaper_loop` runs reclaimed jobs **inline and serially**, so one long crash-recovery blocks all reaping for its duration. Pre-existing, but Phase 2a's `HeldByLiveRunner` fix converted the reaper's inline work from a fast no-op into a potentially hours-long import.

**Direction:** make the reaper a pure detect-and-requeue loop — hand reclaimed entries to the normal consumer pool rather than executing them. Failing that, spawn each reclaimed job onto its own task bounded by the existing concurrency cap.

**Size:** S. Self-contained in `worker_mode.rs`. Needs a test that a long reclaimed job doesn't stall reaping of a second stale entry.

---

## Unit 3 — Editor correctness (#145, #146)

Two independent bugs in the same subsystem, both small, both user-visible today.

- **#145** — forward-delete never merges the next list item. `join_forward`'s next-block probe advances `+1`, assuming one open token; inside a list the next textblock is two deep. **A test added in #148 pins seam-resolution and block-end resolution to agree**, so the fix must move both together — read it first.
- **#146** — `delete_word_backward` can panic: it computes a *model* offset but indexes into `text_content()`, which excludes inline atoms. `delete_word_forward` already guards this; backward only checks `offset == 0`.

**Do the boundary sweep** (this repo's convention, and #148 proved it works — it found a corruption bug and a second broken handler beyond the reported symptom). Enumerate the mutation entry points first, then classify each; don't sweep the ones that come to mind.

**Size:** S each. Could be one PR or two; two is cleaner since the root causes are unrelated.

---

## Unit 4 — Image reach (#140)

Copied documents and template instantiations render images blank, because blob *authorization* is keyed to the document being viewed (`blobs/{doc_id}/…` prefix) rather than the document that owns the blob.

**Needs a decision before any code:**

- **(a) Copy the S3 objects on document copy.** `copy_document` already owns the doc bytes and has an S3 client — copy each referenced blob under the new doc's prefix and rewrite the references. Smallest change; keeps the guard trivially sound.
- **(b) Authorize on "can the caller read the document named in the key."** More general — it would also enable cross-document paste — but it **moves an access-control boundary** and deserves a `security-auditor` pass.

(a) is recommended for the immediate fix. Note the templates gallery is the acute case: it exists to be copied, and has effectively been shipping broken images since before durable references (a copy used to inherit a URL that had usually already expired).

**Size:** M for (a). Needs a decision first — flag it rather than picking silently.

---

## Unit 5 — Search indexing (#138)

Documents created by the worker — DOCX, PDF, **and Quip imports** — are never indexed, so a user imports documents and cannot find them by searching their contents.

Structural, not an oversight: the search index is a **local Tantivy directory** with a single-process writer, and in production the worker is a **separate ECS service**. It physically cannot write the API's index.

**Options, all real work:**
1. Worker calls an internal API endpoint to index after each document (new internal surface + auth story).
2. Move indexing behind a shared service or store both processes can write.
3. An API-side reconciliation pass that indexes documents missing from the index.

**Size:** L. This is an architecture decision, not a bug fix, and it affects every worker-created document — not just Quip. Worth its own design pass rather than being folded into import work.

---

## What this plan deliberately excludes

- **#133** (inline grid block) — a feature, not a failure. Quip imports embedded grids as separate Spreadsheet documents plus a `DocMention` in the interim, which is the documented behavior.
- **#61 / #60 / #58** (doc-vs-code drift audits), **#45** (mermaid polish), **#35** (search_users scan) — predate this work and aren't regressions from it.
- **Phase 2b** (link back-patch + `LinkRepo`) — that's the next *feature* phase, not a failure. It's what makes intra-Quip links resolve instead of rendering as broken chips. Worth noting the sequencing question: Unit 1 makes a migration survivable, Phase 2b makes its output correct. If the goal is a usable migration, Unit 1 then Phase 2b.

---

## Recommended order

1. **Unit 1** — before any real migration attempt.
2. **Unit 3** — small, independent, fixes bugs users hit today.
3. **Unit 2** — small; do it while the worker code is fresh.
4. **Unit 4** — after the (a)/(b) decision.
5. **Unit 5** — schedule its own design pass.

Units 2–4 can run in parallel with each other or with Phase 2b if more than one person is working.
