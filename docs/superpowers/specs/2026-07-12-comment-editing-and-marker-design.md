# Comment editing + "edited" marker — design

**Date:** 2026-07-12
**Status:** Approved (design), pending implementation plan
**Scope:** Let comment authors edit their own comments, with a subtle
"edited" marker (timestamp on hover). No stored prior versions.

## Background

OgreNotes models a "comment" as a `Message` inside a `Thread`
(`crates/storage/src/models/thread.rs`). Threads have a `thread_type`
of `Inline`, `Document`, `Chat`, or `DirectMessage`; the same `Message`
struct backs all four. Today messages can be **created**, **replied to**,
and **deleted** (author-only), but **not edited**.

The storage layer is already half-wired for editing: `Message` carries an
unused `updated_at: Option<i64>` field, and the read path
(`message_from_item`) deserializes it. What is missing: a repo writer, a
route handler, the DTO/live-sync wire fields, and the frontend affordance.

### Competitive context

Quip, Google Docs, Notion, and Coda all let the **author** edit their own
comment and show an **"edited" marker**; none expose a per-comment edit
history to end users. This design matches that table-stakes behavior
(the "history" differentiator was explicitly deferred — see the decision
log below).

## Decisions

| Decision | Choice |
|----------|--------|
| What "history" means | **Edited marker only** — no stored prior versions (Option 1) |
| Who can edit | **Author only** (`msg.user_id == user_id`), mirroring `delete_message` |
| Where editing applies | **Comments only** — `thread_type ∈ {Inline, Document}`; chat/DM stay non-editable |
| Marker detail | **"edited" label + timestamp tooltip** (uses `updated_at`) |
| Re-notify on edit | **No** — re-parse mentions for rendering, but fire no new notifications |
| Thread ordering on edit | **Unchanged** — do not bump `thread.updated_at` (drives `GSI5-docid-updated`) |
| Prior versions / history viewer / audit row | **Out of scope** |

## Architecture

### 1. Storage — `crates/storage/src/repo/thread_repo.rs`

Add `ThreadRepo::update_message`. Updates `content`, `parts`, `mentions`,
and `updated_at` on the existing message row. The DynamoDB key
(`PK=THREAD#<thread_id>`, `SK=MSG#<created_at:020>#<message_id>`) is
**unchanged**, so message ordering is preserved. No schema/GSI change.

### 2. API — `crates/api/src/routes/comments.rs`

New handler **`PATCH /threads/{thread_id}/messages/{message_id}`**,
mirroring `delete_message`. Guards, in order:

1. **Doc access** — `check_comment_access` (`AccessLevel::Comment`).
2. **Author-only** — `if msg.user_id != user_id → Forbidden`.
3. **Comment-thread only** — reject unless `thread.thread_type ∈
   {Inline, Document}`; chat/DM → `Forbidden`. Enforces the "comments
   only" scope.
4. **Non-empty content** — reject empty edits (removal is handled by the
   existing delete path).
5. **Rate limit** — reuse `enforce_comments_rate_limit`.

On success: re-run the same content processing `add_message` performs
(re-parse `parts` + `mentions`), set `updated_at = now`, persist via
`update_message`, and fan out a live-sync event (below). Do **not** emit
mention notifications and do **not** bump `thread.updated_at`.

New request DTO `EditMessageRequest { content }` (camelCase, matching the
existing inline DTOs).

### 3. Wire shape + live sync

- Add `updated_at` to `MessageResponse` (currently omitted) and to the
  frontend `MessageItem` (`frontend/src/api/comments.rs`).
- Add a `MessageEdited { message }` variant to `CommentEventPayload`
  in `comments.rs`, fanned out via the existing `fanout_comment_event`
  (local room broadcast + Redis pub/sub). `MessageType::CommentEvent`
  (0x06) is reused.
- **No new frontend event-handling needed**: `document.rs` already bumps
  `comments_dirty` on *any* `CommentEvent`, reloading thread messages, so
  other viewers pick up the edited text + marker automatically.

### 4. Frontend — `frontend/src/components/comment_popup.rs`

- On the author's own `PopupMessage`, add an edit affordance
  (pencil / ⋯) that swaps the rendered text for an inline textarea with
  Save / Cancel, reusing the existing reply-composer styling.
- Add `edit_message` to `frontend/src/api/comments.rs`.
- When `updated_at.is_some()`, render a subtle **"edited"** label beside
  the timestamp; hover tooltip shows the formatted edit time.

## Testing

Backend regression test (per project convention: bug/behavior changes get
regression tests) asserting:

- Author can edit their own comment; `content` + `updated_at` updated.
- Non-author edit → `403 Forbidden`.
- Editing a chat/DM message → `403 Forbidden`.
- Empty content → rejected.
- `thread.updated_at` is unchanged after an edit.

## Out of scope

- Stored prior versions / per-comment history viewer.
- `SecurityAudit` / `AdminAudit` rows (not identity/sharing/destructive
  state).
- Editing chat and DM messages.
- Activity-feed event for edits.
- Mention notifications triggered by an edit.

## Decision log

- **Option 1 (edited marker) chosen over Option 2 (user-visible
  per-comment history).** History is the genuine differentiator (no
  competitor ships it) and the `SecurityAudit` append-only pattern plus
  an `MSGVER#<message_id>#<ts>` sibling-row scheme would make it cheap to
  build later. Deferred to keep this slice small; the `updated_at`
  groundwork does not preclude it.
