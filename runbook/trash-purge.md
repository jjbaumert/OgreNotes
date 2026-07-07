# Trash purge — operational guide

Phase 4 M-E7 item 9 ships a daily worker that hard-purges soft-
deleted documents whose `deleted_at` predates the configured
retention window. This runbook covers what the worker does, how to
enable/disable it, how to dry-run it against real data before
committing to deletions, and how to verify it's working in prod.

## What the worker does, exactly

Every hour the scheduler ticks. On each tick, it checks two gates:

1. Is `trash_cleanup_enabled` set to true in `AppConfig`?
2. Does the current UTC hour match `trash_cleanup_hour_utc`?
3. Has this process already done today's pass (atomic same-day
   guard via an AtomicI64 holding the last-run date)?

If all three pass, the worker:

1. Queries `GSI7-deleted-at` (sparse index keyed on
   `is_deleted_gsi = "deleted"`, range on `deleted_at`) for rows
   where `deleted_at < now - TRASH_RETENTION_DAYS * 86400e6`.
   Capped at 200 docs per tick.
2. For each eligible doc: `doc_repo.hard_delete` (which sweeps
   every row under `PK = DOC#<id>` AND every S3 blob under
   `docs/<id>/`), then `spawn_delete_from_index` for Tantivy +
   Qdrant, then a `SecurityAudit::DocDeleted { hard: true }` row
   keyed on the doc's owner.
3. Logs a tracing event with `scanned`, `purged`, `errors`,
   `dry_run`, `cutoff_usec`.

Errors on a single doc don't abort the sweep — an adversarially-
shaped row mustn't block retention from progressing.

## Configuration knobs

All four are env-overridable in the ECS task definition:

| Variable | Default | Effect |
|---|---|---|
| `TRASH_CLEANUP_ENABLED` | `false` | Opt-in master switch. False = scheduler ticks but does nothing. |
| `TRASH_RETENTION_DAYS` | `30` | Age threshold for eligibility. |
| `TRASH_CLEANUP_HOUR_UTC` | `3` | When the daily pass runs (0-23). |
| `TRASH_CLEANUP_DRY_RUN` | `false` | Log "would purge X" but skip the destructive ops. First-rollout safety valve. |

The scheduler ALWAYS spawns at process start. The gates above
decide whether it does work.

## Enabling for the first time

The worker is off by default in prod. To turn it on:

### 1. Dry-run first

In your prod task definition, set:

    TRASH_CLEANUP_ENABLED=true
    TRASH_CLEANUP_DRY_RUN=true

Deploy. Wait until `TRASH_CLEANUP_HOUR_UTC` passes (default 03:00
UTC). Inspect logs:

    aws logs filter-log-events \
        --log-group-name "/ecs/<prefix>ogrenote" \
        --filter-pattern '"trash_cleanup dry-run: would purge"' \
        --start-time $(date -u -d "1 hour ago" +%s)000

Confirm:
- The count of "would purge" lines matches what you expect
  (rough sanity: total trashed docs older than 30d).
- No log line names a doc you DON'T want to purge.
- The "trash_cleanup pass complete" line shows `purged=0` (dry run).

If the dry-run looks wrong (too many docs, unexpected docs), DON'T
flip dry-run off yet. Investigate first — most likely cause is a
wall-clock skew on the ECS task or a misconfigured retention
window.

### 2. Flip dry-run off

Once the dry-run output looks right:

    TRASH_CLEANUP_DRY_RUN=false

Deploy. The next 03:00 UTC tick will actually purge.

### 3. Verify the first real pass

Same `aws logs` command, but filter for the no-dry-run path:

    aws logs filter-log-events \
        --log-group-name "/ecs/<prefix>ogrenote" \
        --filter-pattern '"trash_cleanup pass complete"' \
        --start-time $(date -u -d "1 hour ago" +%s)000

The log line carries the final counts: `scanned`, `purged`,
`errors`. `errors > 0` means some docs failed to purge — drill in
via the preceding per-doc warning lines.

Cross-check via the SecurityAudit table:

    aws dynamodb query --table-name <table-name> \
        --index-name <none, scan the user-PK> \
        --key-condition-expression "PK = :pk AND begins_with(SK, :prefix)" \
        --expression-attribute-values '{":pk":{"S":"USER#<some-user>"},":prefix":{"S":"SEC_AUDIT#"}}' \
        --filter-expression "action = :a" \
        --expression-attribute-values '{":a":{"S":"docDeleted"}}'

You should see `DocDeleted { hard: true, doc_id: ... }` rows with
`actor_id = "trash_cleanup_worker"` for every doc the worker
purged.

## Disabling temporarily

If you need to pause the worker (e.g. you suspect a bug and want
to investigate before more data is destroyed):

    TRASH_CLEANUP_ENABLED=false

Deploy. The scheduler still ticks but does nothing. No restore is
needed for already-purged docs — they're gone — but no FUTURE
deletions happen.

To resume, flip back to `true`.

## Tuning the retention window

`TRASH_RETENTION_DAYS` is intentionally a runtime config so
adjusting it doesn't require a code deploy.

- **Shortening** (e.g. 30 → 14): the next tick will pick up docs
  with `deleted_at` between 14 and 30 days ago as eligible. This
  is a bulk delete event — consider switching to dry-run for one
  tick first to see the count.

- **Lengthening** (e.g. 30 → 90): no immediate effect; docs that
  would have been purged on the next tick are now safe for another
  60 days. Safe to roll out without dry-run.

## Why an active doc never gets purged

The GSI is sparse on `is_deleted_gsi`. A doc that's never been
soft-deleted doesn't have that attribute set, so the row isn't in
the index and the worker's query won't see it. A restored doc
(`POST /documents/:id/restore`) has the attribute removed,
which drops it back out of the index.

The integration test
`tests/test_trash_cleanup.rs::sweep_ignores_active_non_deleted_docs`
locks this in.

## Failure modes worth knowing about

- **Worker isn't running**: search logs for "trash_cleanup tick
  failed" — that's the per-tick failure log. If no log lines at
  all from the module, check that `trash_cleanup_enabled = true`
  AND the task hasn't crashed before the daily hour fires.

- **Worker runs but purges nothing**: if `scanned > 0` but
  `purged = 0`, the cutoff is excluding everything (retention
  window is wider than your oldest trash). If `scanned = 0`, the
  GSI is empty — either no trash exists older than `cutoff`, OR
  the `is_deleted_gsi` write on `soft_delete` has been broken
  (regression check: look at recent soft-deletes and confirm the
  attribute is populated).

- **One doc keeps failing**: search logs for "trash_cleanup:
  hard_delete failed". The error message names the underlying
  cause — most commonly a stuck S3 multipart upload, a corrupt
  row that fails `doc_meta_from_item`, or an IAM permission gap.
  The worker continues past it on subsequent ticks (it'll keep
  trying), so a transient cause self-heals.

- **GSI is missing**: `GSI7-deleted-at` lands as part of the
  `aws-test-deploy.sh` Phase 2 idempotent-add block on a live
  stack. If a stack predates the GSI and the add was skipped
  (e.g. the table was in `UPDATING` state at deploy time), the
  worker's `list_eligible_for_purge` will fail with
  `ResourceNotFoundException`. Re-run the deploy script when the
  table is ACTIVE.

## Manual purge (out-of-band)

The worker is the official path. For a one-off manual purge of a
specific doc:

1. The user-facing path: the doc's owner hits
   `DELETE /api/v1/documents/<doc-id>` (soft-delete) followed by
   `DELETE /api/v1/documents/<doc-id>/purge` (hard-delete). Two
   audit rows land: `DocDeleted { hard: false }` then
   `DocDeleted { hard: true }`. Owner is the actor.

2. Out-of-band as ops: there is no admin-driven hard-delete API.
   If support needs to purge a doc someone else owns (e.g.
   abuse), the path is to delete via DDB directly (PK = DOC#<id>,
   all SKs under that PK), then S3 (`docs/<id>/`), then Tantivy +
   Qdrant via a one-off script. This bypasses the audit log —
   record the action in your incident ticket.

## v2 carry-forwards

- **Admin "purge stale docs now" trigger**. Currently the only
  way to fire an out-of-cycle sweep is to restart the ECS task at
  the configured hour. A future admin endpoint could call
  `trash_cleanup::sweep` directly (the function is already
  `pub`).
- **Per-workspace retention overrides**. The retention window is
  workspace-agnostic. A regulated industry might want a longer
  window for one workspace.
- **Audit row for the scheduled-job itself**. The worker writes a
  `DocDeleted { hard: true }` per doc but no "I started a sweep"
  meta-row. A future addition could write a single
  `SweepRan { scanned, purged, errors }` row to a dedicated
  ops-audit table for trend analysis.
