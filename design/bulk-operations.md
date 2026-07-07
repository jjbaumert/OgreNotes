# Bulk Operations

Phase 5 M-P7 design doc. Multi-select on the home file browser
plus four batch endpoints (delete, restore, move, share) and the
M-P5 bulk-export endpoint. All synchronous; v1 caps every batch
at 100 ids — beyond that the request rejects, waiting for the
Phase 6 async-worker subsystem.

## Goals

- **One operation, many docs.** The home page lets the user
  check several rows and run delete / move / share / export
  against the selection without N HTTP calls.
- **Partial-failure transparency.** A 100-id batch where 99
  succeed and 1 hits a permission error should surface both
  outcomes; the client gets per-id status, not a single
  pass/fail bit.
- **HTTP semantics.** All-success → 200; partial → 207
  Multi-Status. The response body is the same shape in both
  cases, so the client doesn't need to branch on status code
  just to parse.
- **Per-id authorization.** No bulk authorization shortcut.
  Each id is checked against the same access-level guard the
  single-doc endpoint uses. A user who can delete one doc and
  cannot delete another gets exactly one success entry.

Out of scope for v1:

- > 100 ids per request. The 100 cap protects the synchronous
  request from a slow tail — at p95 ~30 ms per doc, 100 ids
  fits inside a 3-second budget, and beyond that the request
  starts to compete with the per-connection timeout.
- Background / async bulk. Phase 6 async-worker subsystem.
- Cross-workspace bulk. Move and share both restrict to the
  caller's workspace.
- Per-id target folder for restore. All restored docs land in
  the same folder; mirrors the single-doc `/restore` semantics.
- Bulk un-share. v1 has bulk **add** only; bulk member removal
  waits for v2.

## Endpoints

All under `/api/v1/documents/bulk/*`. Authenticated. Per-route
rate-limited via the `bulk_op` bucket (defaults to 10/min/user,
overridable via `RATE_LIMIT_BULK_OP_PER_MIN`).

| Route | Method | Purpose | Required access |
|---|---|---|---|
| `/bulk/delete` | POST | Soft-delete (move to trash) | `Own` per doc |
| `/bulk/restore` | POST | Un-trash into a target folder | `Own` per doc + `Edit` on target folder |
| `/bulk/move` | POST | Move into a target folder | `Edit` per doc + `Edit` on target folder |
| `/bulk/share` | POST | Add a member to many docs | `Own` per doc |
| `/bulk/export` | POST | Zip up to 100 docs into one download | `View` per doc |

### Common request shape

```jsonc
{
  "docIds": ["doc-abc", "doc-def", "doc-ghi"],
  // ... op-specific fields
}
```

Cap: `BULK_OP_MAX = 100`. > 100 ids → **400 Bad Request** with
a message that names the limit. `BULK_EXPORT_MAX = 100` matches
the other ops; the constant is separate so they can diverge if
needed.

### Common response shape

```jsonc
{
  "results": [
    { "docId": "doc-abc", "status": 200 },
    { "docId": "doc-def", "status": 403, "error": "access denied" },
    { "docId": "doc-ghi", "status": 404, "error": "not found" }
  ],
  "succeeded": 1,
  "failed": 2
}
```

- `status` mirrors the single-doc HTTP status for that id: 200
  success, 403 forbidden, 404 not found, 500 unexpected backend
  error.
- `error` is a human-readable string; absent on success.
- `succeeded` / `failed` are convenience counters so the client
  can render a "deleted N of M" toast without re-walking
  `results`.

### Status code

```rust
fn bulk_status(succeeded: usize, total: usize) -> StatusCode {
    if succeeded == total {
        StatusCode::OK            // 200
    } else {
        StatusCode::MULTI_STATUS  // 207
    }
}
```

200 is the all-green path; 207 indicates the body is worth
parsing. Both responses carry the same JSON; a client that
only cares about success can `succeeded == doc_ids.length` and
ignore the rest.

## Per-route specifics

### bulk_delete

```jsonc
POST /documents/bulk/delete
{ "docIds": [...] }
```

For each id: walks the same `try_soft_delete_one` helper that
backs the single-doc DELETE. Failures map to:

- `BulkOpError::NotFound` → status 404
- `BulkOpError::Forbidden` → status 403
- `BulkOpError::Internal(msg)` → status 500 + error message

The handler resolves the caller's trash folder once (one
`user_repo.get_by_id`) and reuses it for every id.

### bulk_restore

```jsonc
POST /documents/bulk/restore
{
  "docIds": [...],
  "targetFolderId": "folder-xyz"
}
```

Up-front: target folder must exist and the caller must have
`Edit` on it. If the target folder fails, the request rejects
with 404 / 403 before any id is touched — saves a per-id check
that would all fail with the same reason. Per-id loop then runs
`try_restore_one`.

### bulk_move

```jsonc
POST /documents/bulk/move
{
  "docIds": [...],
  "destFolderId": "folder-xyz"
}
```

Same up-front-target check as `bulk_restore`. Per-id requires
`Edit` access to the doc (not `Own`) — matches the single-doc
move semantics.

### bulk_share

```jsonc
POST /documents/bulk/share
{
  "docIds": [...],
  "memberId": "user-xyz",
  "accessLevel": "view"  // "view" | "edit"
}
```

Three up-front rejections (apply before any id is processed):

1. `accessLevel == "own"` → 400. Owner transfer is not a bulk
   operation. v1 design point — preventing accidental
   ownership-transfer storms.
2. `memberId == caller.user_id` → 400. No self-shares.
3. Recipient user does not exist → 404.

Per-id loop: requires `Own` access on the doc, then calls into
the same code path that backs `POST /documents/:id/members`.

**Known gap**: bulk_share does **not** emit per-share
Notification rows or send the welcome email that the single-doc
add-member flow does. A "shared N docs with X" aggregate
notification is the intended v1.1 fix. Filed in the Phase 5
deferred-backlog list.

### bulk_export

```jsonc
POST /documents/bulk/export
{
  "docIds": [...],
  "format": "markdown"  // "markdown" | "md" | "html"
}
```

Zips up to 100 docs into a single archive download. Differs
from the other bulk ops:

- Rate-limited via `bulk_export` bucket (defaults 5/min/user),
  not `bulk_op`. The CPU + memory cost is higher.
- Response body is `application/zip`, not JSON, when at least
  one doc exported successfully.
- The archive embeds a `_manifest.json` at the root listing
  every requested id with its per-id status — same shape as
  the JSON `results` array of the other ops, plus a `filename`
  field for successful entries.
- If **zero** docs succeeded (every id 403 or 404), responds
  207 with JSON body — saves the client from unzipping a
  manifest-only archive.

Filename collisions inside the archive are de-duplicated by
appending `-1`, `-2`, etc. Two docs titled "Untitled" get
distinct files in the zip.

## Frontend wiring

`frontend/src/pages/home.rs` owns the selection state:

```rust
let (selected_ids, set_selected_ids) =
    signal::<HashSet<String>>(HashSet::new());
```

Plumbed through `FileBrowser` via two new props:

- `selected_ids: Signal<HashSet<String>>`
- `on_toggle_select: Option<Callback<String>>`

The file-row renders a leading checkbox column only when
`selectable` is true and only on doc rows (folders aren't
selectable). The selection bar is a floating UI element that
appears at the top of the viewport whenever `!selected_ids.
is_empty()`:

- Count label ("3 selected").
- Cancel button — clears the set.
- Delete button — opens a `ConfirmDialog`, then on confirm
  fires `POST /bulk/delete`.

The bar uses `role="region" aria-live="polite"` so the count
announces to screen readers as the user checks rows.

**Known gap**: selection doesn't clear on folder navigation —
if a user checks docs in folder A then descends into folder B,
the bar still claims "3 selected" with 3 rows now invisible.
Documented in the deferred-backlog list; the fix is a one-line
`set_selected_ids.set(HashSet::new())` in the `on_navigate_folder`
handler.

Bulk move and bulk share don't have UI in v1 — the endpoints
exist for API clients and a v1.1 picker UI follow-up. The
selection bar's design has room for a "Move" and "Share" button
when those land.

## Per-id helpers

The four bulk ops share a uniform error type:

```rust
enum BulkOpError {
    NotFound,
    Forbidden,
    Internal(String),
}

async fn try_soft_delete_one(state, doc_id, user_id, trash_folder_id) -> Result<(), BulkOpError>
async fn try_restore_one(state, doc_id, user_id, target_folder_id) -> Result<(), BulkOpError>
async fn try_move_one(state, doc_id, user_id, dest_folder_id) -> Result<(), BulkOpError>
async fn try_share_one(state, doc_id, user_id, member_id, access_level) -> Result<(), BulkOpError>
```

Each helper:

1. Loads the doc meta (`get_verified_doc` or equivalent).
2. Calls the same access-level check the single-doc endpoint uses
   (`check_doc_access(state, id, user_id, AccessLevel::*).await`).
3. Runs the mutation against the same repository call the
   single-doc endpoint hits.
4. Maps `ApiError::NotFound` / `Forbidden` / `Internal` into the
   `BulkOpError` variants.

This shape is the load-bearing design choice: bulk ops are
**N copies of the single-doc handler**, not a separate
optimization path. Any bug fixed in single-doc delete fixes
the bulk variant for free; any new permission check the
single-doc enforces flows through automatically. The cost is
N round trips to DynamoDB per request; the savings is zero
schema duplication.

## Metrics

Each handler emits a counter via the EMF metrics pipeline:

```rust
counter::inc(MetricKey::new(
    "doc.bulk_delete_total",
    &[("succeeded", &succeeded.to_string())],
));
```

`succeeded` is the per-request success count tagged on the
metric so an alarm can detect a regression where bulk ops
silently start returning all-403. Same pattern for
`bulk_restore_total`, `bulk_move_total`, `bulk_share_total`,
`bulk_export_total`.

## Testing

Integration tests live in `crates/api/tests/test_bulk_ops.rs`:

- Auth gate (each route, unauthenticated → 401)
- > 100 ids → 400
- All-success → 200 + counts
- Partial failure → 207 + per-id status
- Restore: bad target folder → 404/403 up-front
- Share: grant-Own rejection, self-share rejection, unknown
  recipient rejection
- Happy paths for each route

Doctor scenarios:

- `bulk-delete` (M-P7 piece D) — end-to-end selection bar →
  confirm dialog → bulk delete → rows gone.

## v2 carry-forwards

- **Async-worker bulk.** > 100 ids becomes a job that the
  client polls for. Needs the Phase 6 async-worker subsystem.
- **Bulk un-share.** Mirror of bulk_share, removes a member
  from many docs.
- **Bulk owner-transfer.** Today's 400-on-Own gate becomes a
  separate `POST /bulk/transfer-ownership` endpoint with extra
  confirmation.
- **Cross-workspace bulk.** Today's ops scope to the caller's
  workspace; cross-workspace move + share needs the multi-
  workspace user model from Phase 6.
- **Aggregate share notifications.** The "shared N docs"
  one-shot notification + email mentioned above.
