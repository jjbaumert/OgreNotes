# Quip import — folder structure (#170) and adjacent work

**Status:** plan, not started. Written 2026-08-01 after the second real-account
demo-gate run; **re-verified against `main` @ `24db1a3` on 2026-08-02** — every
claim below still holds, with the exact gap now pinned to a single line.

Today every imported document lands **flat** in a single per-import folder
(`Quip Import — <date>`, added in #172 as interim containment). The Quip folder
tree is discarded, and the wizard never asks where the import should go.

Note #170 was **closed** when #172 shipped the per-import folder, which was
accepted as interim containment ("I am ok deploying to a temporary folder for the
time being"). This plan is the real hierarchy work; it needs a fresh issue.

This plan covers #170 and the work genuinely coupled to it. It is deliberately
**not** "everything left in the importer" — see *Excluded* at the end.

---

## The gap, precisely (re-verified 2026-08-02)

**One line.** `crates/api/src/worker_mode.rs:1149` writes `ogre_folder_id: None`
when Phase 1's BFS creates each `FOLDER#` row. Nothing else in the codebase ever
writes that field.

Everything on both sides of it is already built:

- `crates/storage/src/repo/import_repo.rs:958` **persists** `ogre_folder_id`
  whenever it is `Some`.
- `crates/api/src/worker_mode.rs:1920` **consumes** it to build the mapping.

So the shape of Unit 1 is: create the folders, and change that `None`.

A test at `worker_mode.rs:3103` already documents the current state — *"Phase 1
writes no `ogre_folder_id`, so every Quip folder resolves to the fallback… once
something populates `ogre_folder_id`, the change is visible here."* Expect that
test to need updating; per repo rules that is a **behaviour change to argue**, not
a mechanical edit. It was written in anticipation of exactly this work.

---

## What already exists (verified, not assumed)

Most of the machinery is built. The gap is narrower than #170's title suggests.

- **`FolderRow`** (`crates/storage/src/models/import_inventory.rs`) already carries
  `quip_folder_id`, `title`, `parent_quip_id`, and an **`ogre_folder_id: Option<String>`**
  slot. Phase 1's BFS populates everything *except* `ogre_folder_id`.
- **`build_folder_mapping`** (`crates/api/src/worker_mode.rs`) already reads the
  `FOLDER#` rows, builds `quip_folder_id → ogre_folder_id`, and falls back to the
  import's `target_folder_id`. **The consumption side is done.** Populate
  `ogre_folder_id` and documents route themselves.
- **`ThreadRow.member_folders[]`** and `first_folder` are already recorded by the
  inventory walk.
- **Multi-folder membership is native**: `DocumentMeta.folder_id ∪
  additional_folder_ids`, and `routes/documents.rs` already chains them when
  listing a document's folders. This is the exact analog of Quip's tag-folders.
- **`POST /{id}/start` already accepts `target_folder_id`** and authorizes it.
  The wizard just always sends Home.
- **`ensure_import_folder`** (#172) is a working precedent for idempotent folder
  creation under a conditional write.

So the remaining work is: **create the folders, record the mapping, wire the
picker** — plus the semantics that make a *re-run* safe.

---

## Unit 1 — Mirror the Quip folder tree

**Files:** `crates/api/src/worker_mode.rs`, `crates/storage/src/repo/import_repo.rs`

Walk the `FOLDER#` rows **parent-before-child**, create one OgreNotes folder per
Quip folder under the import's destination, and record `ogre_folder_id` on the row.
`build_folder_mapping` then places each document without further change.

**The hazards, in the order they will bite:**

1. **Idempotency.** The importer is re-startable and the reaper re-runs crashed
   jobs. Creating folders on every run would spawn duplicate trees. Use
   `ogre_folder_id` as the idempotency key exactly as `ensure_import_folder` uses
   `import_folder_id`: present → reuse, absent → create then record with a
   conditional write. **This is the single most important property in the unit.**
2. **Ordering.** A child cannot be created before its parent. Topologically sort
   the `FOLDER#` rows by `parent_quip_id`.
3. **A malformed graph.** Do not assume the Quip folder graph is a clean tree —
   guard against cycles and against a `parent_quip_id` that names a folder outside
   the selected scope (an unselected parent). Decide and document what happens:
   most likely re-parent to the import root rather than dropping the folder.
4. **Depth and breadth.** A large account can have hundreds of folders. Creating
   them is DynamoDB writes, not Quip calls, so the 50 req/min limit is not the
   constraint — but a partial failure mid-tree must be resumable, which the
   `ogre_folder_id` key gives for free.
5. **Empty folders.** A Quip folder containing only chats (skipped) or only
   inaccessible threads will produce an empty OgreNotes folder. Decide: create it
   anyway (structure fidelity) or prune it (tidiness). Recommend creating it —
   the user asked for their structure, and an empty folder is honest about what
   was there.

**Tests:** a nested tree mirrors with correct parentage; a re-run creates nothing
new; an unselected parent is handled per the documented rule; a cycle terminates.
Mutation-check the idempotency guard — that is the one that turns one bad run into
a mess of duplicate trees.

### New constraint (2026-08-02): the report budget has ONE slot left

Unit 1 will want to report folders it could not create — a forbidden folder, a
name collision, a parent that vanished. That means a new **note kind**, and the
`REPORT` row's budget is **25 notes/kind, 8 distinct kinds, 200 total**. #208
confirmed **7 of 8 kinds are now used**:

`thread_skipped`, `thread_failed`, `image_dropped`, `content_truncated`,
`mentions_degraded`, `live_app_dropped`, `formulas_dropped`

**A 9th kind's notes are discarded outright** — not truncated, dropped. A
roster-driven test (`the_worker_stays_within_the_report_rows_note_kind_budget`)
enforces the ceiling, so this cannot regress silently, but it *will* fail the
build if Unit 1 adds a kind carelessly.

Two ways through, decide deliberately:
1. **Spend the last slot** on `folder_failed`. Defensible — folders become
   user-visible structure in this unit, so a lost one is worth naming.
2. **Piggyback on an existing kind**, as `FOLDERS_FORBIDDEN` already does: it is a
   *counter* whose notes file under `KIND_THREAD_SKIPPED`. That keeps the slot but
   accepts the same defect described below.

**Related known gap, worth fixing inside this unit rather than after:**
`FOLDERS_FORBIDDEN` is the **only unprojected counter** — its notes surface in the
skipped list but its count reaches nothing. Consequence, measured during #208's
review: a run with >25 forbidden folders and no forbidden *threads* reports 25
rather than the true number. That is tolerable today because folders are invisible;
once Unit 1 makes the folder tree the point of the feature, an under-reported
folder failure becomes a real hole. Fold it in.

### Precedent to follow (2026-08-02): `QuipThreadKind`

#230 needed to tell the walker something the HTML could not express, and did it by
plumbing Quip's thread type in as a typed parameter from a **single call site**
(`worker_mode.rs:2362`). If Unit 1 needs comparable context, follow that shape —
and note its documented weakness: one call site with an unchanged convenience
signature means a future caller silently gets the old behaviour. If Unit 1 adds a
similar seam, cover it with a worker-level end-to-end test the way #233 Gap 2 did,
not just a unit test.

---

## Unit 2 — Multi-folder membership

**Files:** `crates/api/src/worker_mode.rs`

A Quip thread can live in several folders. `ThreadRow.member_folders[]` already
records them all, and `first_folder` records the BFS-stable first.

Map to the native model: `DocumentMeta.folder_id` = the mapped `first_folder`,
`additional_folder_ids` = every other mapped member folder. Both fields exist and
are already honoured by the document routes.

Small once Unit 1 lands — it is listed separately because it is a distinct
behavioral claim with its own test (a thread in three folders appears in three
folders after import), and because it is easy to forget when the primary path
looks correct.

---

## Unit 3 — Destination picker in the wizard

**Files:** `frontend/src/components/quip_import/`, `frontend/locales/*`,
possibly `crates/api/src/routes/imports.rs`

Let the user choose where the import lands, instead of always Home.

**Why it was deferred, and what that means for this unit:** the wizard is a modal,
and nesting `FolderPickerDialog` inside it risks focus-trap conflicts — that is the
documented reason Phase 1 skipped it. So do **not** nest a dialog. Prefer a
dedicated step in the wizard's own flow, reusing the picker's *data* rather than
its modal shell.

This codebase has a documented modal-close panic class ("closure invoked
recursively or after being dropped") from synchronous teardown; the wizard already
uses `a11y::defer_close` for that reason. Preserve that discipline.

**Interaction with #172:** the per-import folder stays. The picker chooses the
*parent*; the import still creates its own `Quip Import — <date>` folder beneath it,
so undoing a run remains "delete one folder". If that turns out to feel redundant
once the hierarchy is mirrored, that is a product call to make deliberately — not a
side effect of this unit.

**Tests:** the chosen folder reaches `start` as `target_folder_id`; the default
(Home) still works; the access check still runs against the user's choice.
Note the click-through itself is not unit-testable in this crate (no DOM harness) —
a `frontend-doctor` scenario is the honest coverage, which is also the outstanding
gap on #174.

---

## Unit 4 — Folder links resolve (Phase 2b coupling)

**This is why the units above are worth doing now rather than later.**

A Quip document containing `📁Family` currently imports as a document-mention chip
pointing at a Quip folder id with no OgreNotes counterpart. It can never resolve —
because nothing has ever created the folder it refers to.

Unit 1 populates `ogre_folder_id`, which is exactly the missing half. Phase 2b's
link back-patch can then resolve folder links to real folders, the same way it
resolves thread links to documents.

**Sequence it after Unit 1 and alongside Phase 2b's `LinkRepo` work** rather than
building a parallel mechanism. The `UNRESOLVED#` rows already exist for this.

Worth confirming during the work: whether a folder link and a thread link are
distinguishable at back-patch time, given the corpus finding that Quip wraps both
in `<control>` and both are opaque `quip.com/<id>` URLs (see #179 — the same
ambiguity, resolved worker-side by asking Quip what the id is).

---

## Recommended order

1. **Unit 1** — the keystone. Unblocks Units 2 and 4, and makes Unit 3 meaningful.
2. **Unit 2** — small, immediately after Unit 1.
3. **Unit 3** — independent of 1/2; can run in parallel with a second agent, since
   it touches only the wizard and locales.
4. **Unit 4** — with Phase 2b, not before.

Demo gate for 1–3: import a Quip account with a nested folder structure and a
document filed in two folders. The tree appears, the document appears in both
folders, and re-running the import changes nothing.

**Add to the demo gate (2026-08-02):** re-run the import a *second* time after
manually moving one imported document to a different folder. The moved document
must stay where the user put it. This is the mutable-location constraint below,
and it is the property most likely to be broken by a well-meaning "repair the
tree" implementation.

---

## Testing discipline this importer has earned

Ten fidelity bugs in this importer shared one root cause: **every fixture encoded
an HTML shape Quip never emits**, so tests passed against imaginary markup while
real documents broke. Folder work is less HTML-bound than content work, but two
rules still apply:

- **Never hand-author Quip markup or a Quip folder-graph shape.** If a test needs
  a folder tree, derive its shape from what the inventory walk actually records
  for a real account, not from what a clean tree ought to look like. The malformed
  graph in hazard 3 is not hypothetical — assume it until measured.
- **Add a negative control that passes before *and* after.** For Unit 1 the
  obvious one is: a document the user has already moved does not get moved back.
  The #153 and #233 work both showed that the test which only goes green after the
  fix misses the over-reach failure entirely.

Both the corpus net (`crates/collab/tests/quip_corpus.rs`) and the worker
end-to-end suite (`crates/api/tests/test_quip_content_worker.rs`, extended by #233
Gap 2) are live and should be extended rather than bypassed.

---

## Adjacent, deliberately excluded

- **#161** (report notes show opaque thread ids, not titles) — surfacing work, not
  folder work. Independent.
- **#155 / #156 / #178 / #179 / #183** — importer robustness, report writes, the
  `get_by_email` scan, invisible people, checked-state. All unrelated to folders.
- **Phase 3 identity / Phase 4 comments** — separate phases.
- **Re-import idempotency in general** (Phase 5) — Unit 1 must be re-run safe,
  but the broader "re-import an account that changed" story is Phase 5's.
  **One constraint from the design applies now and must not be violated:** folder
  location is *mutable post-import* — if the user moves a document after importing,
  a later run must respect that and **never force it back to the import target**.
  Unit 1's idempotency key makes this natural; do not add anything that "repairs"
  a document's folder on re-run.
