# OgreNotes MVP -- Detailed Design

## Goal

Ship a single-user collaborative document editor that proves the core architecture. A user can sign in, create documents with rich text formatting, organize them in folders, and save them durably. Every design decision is made with multi-user collaboration, spreadsheets, chat, and enterprise features in mind -- but none of those ship in Phase 1.

---

## What Ships in MVP

1. OAuth 2.0 login (single provider)
2. Document CRUD (create, open, edit, rename, delete)
3. Rich text editing (headings, lists, bold/italic/underline/code/link, code blocks, horizontal rules, images)
4. yrs-backed persistence (single-user, no real-time sync yet)
5. Folder management (create, rename, delete, move documents)
6. System folders (Home, Private, Trash)
7. Basic file browser (list view with sorting)
8. Blob upload/download (images, file attachments via presigned URLs)
9. Document export (Markdown to clipboard, HTML download)

## What Does NOT Ship (But Is Designed For)

- Multi-user real-time collaboration (Phase 2)
- Workspace/organization entity (Phase 2)
- Comments and chat (Phase 2)
- Notifications (Phase 2)
- Sharing and permissions beyond owner (Phase 2)
- Block menu, `@` menu, document outline, edit history (Phase 2)
- Archive and Pinned system folders (Phase 2)
- Spreadsheets (Phase 3)
- Search (Phase 4)
- Admin console, SCIM, SSO, MFA (Phase 4)
- Templates (Phase 4)
- Mobile optimization (Phase 5)
- Embeds / integrations (Phase 5)
- Command palette, typography themes, dark mode (Phase 5)

---

## Crate Structure

```
ogrenotes/
├── Cargo.toml                     # Workspace root
├── crates/
│   ├── api/                       # Axum HTTP server
│   │   ├── src/
│   │   │   ├── main.rs            # Server entrypoint, router assembly
│   │   │   ├── config.rs          # Environment config (AWS, Redis, secrets)
│   │   │   ├── error.rs           # API error types -> HTTP responses
│   │   │   ├── middleware/
│   │   │   │   ├── auth.rs        # JWT extraction + validation
│   │   │   │   └── rate_limit.rs  # Per-user rate limiting (stub for MVP)
│   │   │   └── routes/
│   │   │       ├── auth.rs        # OAuth login/callback/refresh/logout
│   │   │       ├── documents.rs   # Document CRUD + content
│   │   │       ├── folders.rs     # Folder CRUD + membership
│   │   │       ├── blobs.rs       # Presigned URL generation
│   │   │       └── users.rs       # Current user profile
│   │   └── Cargo.toml
│   │
│   ├── collab/                    # Document engine (yrs wrapper)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── document.rs        # Y.Doc lifecycle: create, load, apply update, snapshot
│   │   │   ├── schema.rs          # Document schema: node types, marks, yrs tag mapping
│   │   │   │                      # NOTE: Mirrors frontend/editor/model.rs NodeType/MarkType.
│   │   │   │                      # See "Schema Duality" section below.
│   │   │   ├── snapshot.rs        # Serialize/deserialize Y.Doc state to/from bytes
│   │   │   └── export.rs          # Y.Doc -> HTML, Y.Doc -> Markdown
│   │   └── Cargo.toml
│   │
│   ├── auth/                      # Authentication
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── oauth.rs           # OAuth 2.0 authorization code + PKCE flow
│   │   │   ├── jwt.rs             # Token creation, validation, refresh
│   │   │   ├── session.rs         # Session creation, lookup, revocation
│   │   │   └── user.rs            # User record creation on first login
│   │   └── Cargo.toml
│   │
│   ├── storage/                   # AWS S3 + DynamoDB operations
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── dynamo.rs          # DynamoDB client wrapper, table operations
│   │   │   ├── s3.rs              # S3 client wrapper, presigned URLs
│   │   │   ├── models/
│   │   │   │   ├── user.rs        # User record
│   │   │   │   ├── document.rs    # Document metadata record
│   │   │   │   ├── folder.rs      # Folder record
│   │   │   │   └── session.rs     # Session record
│   │   │   └── repo/
│   │   │       ├── user_repo.rs   # User CRUD
│   │   │       ├── doc_repo.rs    # Document metadata + snapshot refs
│   │   │       ├── folder_repo.rs # Folder CRUD + children
│   │   │       └── session_repo.rs# Session CRUD
│   │   └── Cargo.toml
│   │
│   └── common/                    # Shared types
│       ├── src/
│       │   ├── lib.rs
│       │   ├── id.rs              # ID generation (nanoid, 21 chars, URL-safe)
│       │   ├── error.rs           # Shared error types
│       │   ├── time.rs            # Timestamp helpers (microsecond precision)
│       │   └── config.rs          # Shared config types
│       └── Cargo.toml
│
├── frontend/                      # Leptos application
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs                # App entrypoint
│   │   ├── app.rs                 # Root component + router
│   │   ├── api/                   # HTTP client functions
│   │   │   ├── client.rs          # Fetch wrapper with auth headers
│   │   │   ├── documents.rs       # Document API calls
│   │   │   ├── folders.rs         # Folder API calls
│   │   │   └── blobs.rs           # Blob/presigned URL calls
│   │   ├── components/
│   │   │   ├── sidebar.rs         # Left navigation panel
│   │   │   ├── file_browser.rs    # Document/folder list view
│   │   │   ├── toolbar.rs         # Document formatting toolbar
│   │   │   ├── editor.rs          # Editor container (black box for contenteditable)
│   │   │   └── login.rs           # OAuth login flow
│   │   ├── editor/                # Rich text editor core (Rust/WASM)
│   │   │   ├── mod.rs
│   │   │   ├── model.rs           # Node, Fragment, Mark, Slice types
│   │   │   ├── schema.rs          # Schema definition, ContentMatch DFA
│   │   │   ├── transform.rs       # Steps, Transform, position mapping
│   │   │   ├── state.rs           # EditorState, Transaction
│   │   │   ├── view.rs            # contenteditable bridge (web-sys)
│   │   │   ├── selection.rs       # TextSelection, NodeSelection, GapCursor
│   │   │   ├── keymap.rs          # Keyboard shortcut handling
│   │   │   ├── input_rules.rs     # Markdown-style shortcuts
│   │   │   ├── clipboard.rs       # Copy/paste handling
│   │   │   ├── commands.rs        # Built-in editing commands
│   │   │   ├── plugins.rs         # Plugin system + decoration set
│   │   │   └── yrs_bridge.rs      # Bidirectional sync: editor model <-> yrs Y.Doc
│   │   └── pages/
│   │       ├── home.rs            # Home / file browser page
│   │       ├── document.rs        # Document editing page
│   │       └── login.rs           # Login page
│   └── style/
│       ├── main.css               # Global styles
│       ├── editor.css             # Editor-specific styles
│       └── variables.css          # CSS custom properties (colors, spacing, fonts)
│
└── infra/
    ├── dynamodb/
    │   └── tables.json            # Table + GSI definitions
    ├── s3/
    │   └── bucket-policy.json     # Block public access policy
    └── docker-compose.yml         # Redis only (DynamoDB + S3 are real AWS)
```

---

## DynamoDB Schema (MVP Subset)

### Table: `ogrenotes` (single table)

Prefix with `DYNAMODB_TABLE_PREFIX` from environment for dev isolation.

```
PK                          SK                          Attributes
--------------------------------------------------------------------------
USER#<user_id>              PROFILE                     name, email, avatar_url,
                                                        created_at, updated_at

USER#<user_id>              SESSION#<session_id>        access_token_hash, refresh_token_hash,
                                                        expires_at, created_at, device_info

DOC#<doc_id>                METADATA                    title, owner_id, doc_type,
                                                        created_at, updated_at,
                                                        snapshot_version, snapshot_s3_key,
                                                        is_deleted, deleted_at

DOC#<doc_id>                UPDATE#<clock>              update_bytes (Binary), user_id,
                                                        created_at
                                                        (CRDT op log; pruned on compaction)

FOLDER#<folder_id>          METADATA                    title, color, parent_id, owner_id,
                                                        folder_type (system|user),
                                                        created_at, updated_at

FOLDER#<folder_id>          CHILD#<child_id>            child_type (doc|folder),
                                                        added_at

FOLDER#<folder_id>          MEMBER#<user_id>            access_level, added_at
                                                        (MVP: owner only)
```

### GSIs (MVP)

| GSI | PK | SK | Purpose |
|-----|----|----|---------|
| **GSI1** | `owner_id` | `updated_at` | List user's documents, sorted by recent |
| **GSI2** | `parent_id` | `title` | List folder children, sorted alphabetically |

### GSI Migration Path

MVP ships with GSI1 (`owner_id -> updated_at`) and GSI2 (`parent_id -> title`). The high-level design requires additional GSIs for workspaces, cross-user queries, and activity feeds. These are added incrementally -- DynamoDB supports adding GSIs to an existing table without downtime or data migration.

**Migration plan:**

| Phase | GSI Added | PK | SK | Notes |
|-------|-----------|----|----|-------|
| MVP | GSI1 | `owner_id` | `updated_at` | Already exists. Stays permanently -- user-scoped doc listing is always needed. |
| MVP | GSI2 | `parent_id` | `title` | Already exists. Stays permanently -- folder child listing is always needed. |
| Phase 2 | GSI3 | `workspace_id` | `updated_at` | Added when `WORKSPACE#` rows are introduced. Requires backfilling `workspace_id` on existing DOC# rows (one-time migration script). |
| Phase 2 | GSI4 | `user_id` | `created_at` | For listing user's workspaces, sessions, and cross-entity activity. |
| Phase 2 | GSI5 | `doc_id` | `updated_at` | For per-document activity feed (edits, comments, shares). |

**Key constraint:** GSI1 and GSI2 are not replaced -- they are supplemented. Existing queries continue to work. New GSIs project onto the same table rows (single-table design) using attributes that MVP already writes or that Phase 2 adds.

**Backfill required for GSI3:** When workspaces ship, existing DOC# METADATA rows need a `workspace_id` attribute added. A migration script assigns all existing docs to the user's default workspace. The GSI backfills automatically once the attribute is present.

### Workspace Introduction (Phase 2)

MVP has no workspace/organization concept. The high-level design's `WORKSPACE#` rows, workspace-scoped GSIs, and multi-tenant queries are introduced in Phase 2 as a prerequisite for sharing and collaboration. The migration path:

1. **Phase 2 adds `WORKSPACE#` rows:**
   ```
   PK                          SK                          Attributes
   --------------------------------------------------------------------------
   WORKSPACE#<ws_id>           METADATA                    name, owner_id, plan, created_at
   WORKSPACE#<ws_id>           MEMBER#<user_id>            role, joined_at
   ```

2. **Default workspace creation:** When a user first upgrades to Phase 2, a migration creates a default `WORKSPACE#` for that user and assigns all their existing documents to it.

3. **DOC# backfill:** A one-time script adds `workspace_id` to every `DOC#<doc_id>/METADATA` row, pointing to the owner's default workspace.

4. **GSI3 activation:** After the backfill, `GSI3` (`workspace_id -> updated_at`) begins serving workspace-scoped queries.

5. **API changes:** Document creation gains an optional `workspace_id` parameter (defaults to user's default workspace). All existing endpoints continue to work -- workspace scoping is additive to owner-based access.

### Snapshot Storage Strategy

The high-level design shows `DOC#<doc_id>/SNAPSHOT#<version>` rows for version history. **MVP does not use SNAPSHOT# rows.** Instead, the latest snapshot reference is stored directly on the `DOC#<doc_id>/METADATA` row as `snapshot_version` and `snapshot_s3_key`.

**Why:** MVP is single-user with no version history UI. Storing the snapshot reference on METADATA avoids an extra DynamoDB query on every document open. The S3 key uses `docs/{doc_id}/snapshots/{version}.bin`, so previous versions remain in S3 but are not referenced (they become orphans until cleanup).

**Phase 2 transition:** When edit history ships (Phase 2), `SNAPSHOT#<version>` rows are added alongside the METADATA reference. Each compaction cycle writes both a new SNAPSHOT# row (for history) and updates the METADATA (for fast latest-version lookup). A backfill script can retroactively create SNAPSHOT# rows from the existing S3 objects if version history for pre-Phase-2 documents is desired.

**S3 orphan cleanup:** MVP's overwrite-style saves leave previous snapshot files in S3. A lifecycle rule or cleanup script should periodically remove unreferenced snapshot files. This is low priority since individual snapshots are small (typically < 10KB).

### Future-Proofing Notes

- `FOLDER#/MEMBER#` rows exist from day one even though MVP is single-user. Phase 2 adds rows for shared users.
- `DOC#/UPDATE#` rows hold the CRDT op log. Same structure supports multi-user in Phase 2.
- `access_level` field uses the 4-level enum (`OWN`, `EDIT`, `COMMENT`, `VIEW`) from day one. MVP only writes `OWN`.
- `doc_type` field supports `document`, `spreadsheet`, `chat` from day one. MVP only creates `document`.
- No `THREAD#/MSG#` rows in MVP. The PK/SK pattern is reserved.
- No `WORKSPACE#` rows in MVP. Introduced in Phase 2 with a migration path (see "Workspace Introduction" above).

---

## S3 Layout (MVP Subset)

```
${S3_BUCKET}/
├── docs/<doc_id>/
│   └── snapshots/<version>.bin      # Serialized yrs Y.Doc state
└── blobs/<doc_id>/<blob_id>/<name>  # User-uploaded images/files
```

### Presigned URL Strategy

| Operation | URL Type | TTL | Conditions |
|-----------|----------|-----|------------|
| Image/file upload | PUT | 15 min | Content-Type + Content-Length limits |
| Image/file download | GET | 4 hours | Scoped to single object |
| Snapshot write | PUT (server-side) | N/A | Server writes directly, never client |
| Snapshot read | GET (server-side) | N/A | Server reads directly, never client |

Clients never receive credentials. All blob access is through presigned URLs issued by the API.

---

## API Endpoints (MVP)

### Authentication

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/auth/login` | Redirect to OAuth provider |
| GET | `/api/v1/auth/callback` | OAuth callback, issue tokens |
| POST | `/api/v1/auth/refresh` | Refresh access token |
| POST | `/api/v1/auth/logout` | Revoke session |

### Users

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/users/me` | Current user profile + system folder IDs |

### Documents

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/documents` | Create document (returns doc_id) |
| GET | `/api/v1/documents/:id` | Get document metadata |
| PATCH | `/api/v1/documents/:id` | Update metadata (title, folder) |
| DELETE | `/api/v1/documents/:id` | Soft delete (move to Trash) |
| GET | `/api/v1/documents/:id/content` | Load Y.Doc state (binary) |
| PUT | `/api/v1/documents/:id/content` | Save Y.Doc state (binary) |
| GET | `/api/v1/documents/:id/export/:format` | Export as `html` or `markdown` |

### Folders

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/folders` | Create folder |
| GET | `/api/v1/folders/:id` | Get folder metadata + children |
| PATCH | `/api/v1/folders/:id` | Update (title, color, parent) |
| DELETE | `/api/v1/folders/:id` | Delete folder |
| POST | `/api/v1/folders/:id/children` | Add document or subfolder |
| DELETE | `/api/v1/folders/:id/children/:child_id` | Remove document or subfolder |

### Blobs

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/v1/documents/:id/blobs` | Request presigned PUT URL for upload |
| GET | `/api/v1/documents/:id/blobs/:blob_id` | Request presigned GET URL for download |

### Response Format

All responses are JSON with camelCase keys. Errors follow a consistent shape:

```json
{
  "error": "not_found",
  "message": "Document not found"
}
```

HTTP status codes: 200 (ok), 201 (created), 204 (no content), 400 (bad request), 401 (unauthorized), 403 (forbidden), 404 (not found), 429 (rate limited), 500 (internal error).

### Pagination

List endpoints (`GET /folders/:id` children) return all results in MVP. Phase 2 adds cursor-based pagination:

```json
{
  "items": [...],
  "nextCursor": "opaque_string_or_null"
}
```

The cursor is an opaque base64-encoded DynamoDB `ExclusiveStartKey`. Clients pass `?cursor=<value>` to get the next page. Default page size is 50; maximum is 200. The `GET /folders/:id` response already returns children sorted by GSI2 (`parent_id -> title`), so the cursor maps naturally to the DynamoDB last-evaluated-key.

MVP can return all children without pagination because folder sizes are small in single-user mode. The response shape should include `nextCursor: null` from day one so clients don't break when pagination activates.

### Concurrency Control

MVP uses optimistic concurrency for document content saves via `snapshot_version`:

- `PUT /documents/:id/content` includes a conditional write: `snapshot_version = :expected_version`. If another save raced ahead, the request fails with 409 Conflict.
- Metadata updates (`PATCH /documents/:id`) do **not** use ETags in MVP. Phase 2 adds `If-Match` / `ETag` headers for metadata when multiple users can edit.

This is already implemented -- `doc_repo.rs` uses a DynamoDB condition expression on `snapshot_version`.

### CORS Policy

MVP CORS configuration (implemented in `api/src/main.rs` via `tower-http`):

| Setting | Value | Rationale |
|---|---|---|
| Allowed origins | `FRONTEND_ORIGIN` env var (e.g., `http://localhost:8080`) | Single frontend origin in MVP |
| Allowed methods | `GET, POST, PUT, PATCH, DELETE, OPTIONS` | All REST methods used by the API |
| Allowed headers | `Authorization, Content-Type` | JWT bearer token + JSON/binary body |
| Expose headers | `Content-Type` | Clients need to read response content type |
| Max age | 3600 seconds | Cache preflight for 1 hour |
| Credentials | true | Required for HttpOnly refresh token cookie |

Phase 2 may add multiple allowed origins if desktop/mobile apps have different origins.

### Input Limits

| Limit | Value | Enforcement |
|---|---|---|
| Document title | 500 characters max | API validation (400 if exceeded) |
| Folder title | 200 characters max | API validation |
| Folder nesting depth | 10 levels max | API validation on create/move |
| Document content size | 10 MB max | API validation (`MAX_CONTENT_SIZE` constant) |
| Blob upload size | 25 MB max | S3 presigned URL `Content-Length` condition |
| Allowed blob MIME types | `image/png, image/jpeg, image/gif, image/webp, image/svg+xml, application/pdf, text/plain, text/csv, application/zip` | S3 presigned URL `Content-Type` condition |
| Folder color | 0-11 | Clamped in storage model |

**SVG safety note:** SVG files can contain JavaScript. Blob download URLs should include `Content-Disposition: attachment` to prevent inline rendering in the browser. This is enforced via the presigned URL response headers.

### Redis in MVP

Redis is configured in the environment (`REDIS_URL`) and included in `docker-compose.yml`, but **no code path uses Redis in MVP**. The `fred` crate is a dependency for Phase 2 readiness. MVP uses DynamoDB for all persistence and has no pubsub, presence, caching, or rate limiting backed by Redis.

Phase 2 activates Redis for: presence tracking, cursor broadcasting, CRDT update pubsub, and session caching. Phase 4 adds Redis-backed rate limiting.

### Authorization Model

MVP enforces a two-layer authorization chain. Phase 2 extends it to a three-layer chain without changing the route handler signatures.

#### MVP Authorization Chain

```
Request
  -> Layer 1: AuthUser extractor (JWT validation -> user_id)
  -> Layer 2: Resource ownership check (owner_id == user_id)
  -> Handler logic
```

Layer 1 is the `AuthUser` Axum extractor in `middleware/auth.rs`. Layer 2 is currently inline in each handler -- `get_verified_doc()` for documents and manual `folder.owner_id != user_id` checks for folders.

#### Phase 2 Authorization Chain

```
Request
  -> Layer 1: AuthUser extractor (JWT validation -> user_id)
  -> Layer 2: Permission resolver (check FOLDER#/MEMBER# ACL rows -> access_level)
  -> Layer 3: Permission guard (required_level <= user's access_level)
  -> Handler logic
```

#### Refactoring Plan for Phase 2

The MVP's `get_verified_doc()` and inline ownership checks are replaced with **permission-checking extractors** that encode the required access level:

```rust
// Phase 2 extractors (replace inline ownership checks)

/// Extracts a document the user can view (access_level >= VIEW).
struct ViewableDoc(DocumentMeta);

/// Extracts a document the user can edit (access_level >= EDIT).
struct EditableDoc(DocumentMeta);

/// Extracts a document the user owns (access_level == OWN).
struct OwnedDoc(DocumentMeta);

/// Same pattern for folders:
struct ViewableFolder(FolderMeta);
struct EditableFolder(FolderMeta);
struct OwnedFolder(FolderMeta);
```

Each extractor:
1. Extracts the resource ID from the URL path
2. Loads the resource metadata from DynamoDB
3. Looks up the user's `FOLDER#/MEMBER#` row (or document ACL) for access_level
4. Returns `403 Forbidden` if the user lacks the required level
5. Returns the resource metadata if authorized

**MVP compatibility:** In MVP, all these extractors degenerate to the ownership check (`owner_id == user_id`). Phase 2 adds ACL row lookups. Route handlers don't change -- they just switch from `get_verified_doc()` to the appropriate extractor.

**Endpoint permission requirements:**

| Endpoint | Required Level | Notes |
|---|---|---|
| `GET /documents/:id` | VIEW | Read metadata |
| `PATCH /documents/:id` | OWN | Only owner can rename/move |
| `DELETE /documents/:id` | OWN | Only owner can delete |
| `GET /documents/:id/content` | VIEW | Read content |
| `PUT /documents/:id/content` | EDIT | Write content |
| `GET /documents/:id/export/:format` | VIEW | Export is a read operation |
| `POST /documents/:id/blobs` | EDIT | Upload requires edit access |
| `GET /documents/:id/blobs/:blob_id` | VIEW | Download is a read operation |
| `GET /folders/:id` | VIEW | List folder contents |
| `PATCH /folders/:id` | OWN | Only owner can modify folder |
| `DELETE /folders/:id` | OWN | Only owner can delete folder |
| `POST /folders/:id/children` | EDIT | Add to folder requires edit |
| `DELETE /folders/:id/children/:child_id` | EDIT | Remove from folder requires edit |

### Future-Proofing Notes

- Document content endpoints use binary (`application/octet-stream`) for yrs state. Phase 2 replaces PUT with WebSocket-based sync; the GET endpoint remains for initial load.
- Folder children endpoints support both `doc` and `folder` child types. The same endpoints work when documents belong to multiple folders in Phase 2.

---

## Authentication Flow (MVP)

### OAuth 2.0 with PKCE

MVP supports a single OAuth provider (GitHub, Google, or similar). The flow:

```
1. User clicks "Sign in"
2. Frontend redirects to /api/v1/auth/login
3. Server generates PKCE code_verifier + code_challenge
4. Server redirects to OAuth provider with code_challenge
5. User authorizes at provider
6. Provider redirects to /api/v1/auth/callback with authorization code
7. Server exchanges code + code_verifier for provider tokens
8. Server creates/updates USER# record in DynamoDB
9. Server creates SESSION# record in DynamoDB
10. Server issues:
    - Short-lived access token (JWT, 15 min) in response body
    - Refresh token in HttpOnly/Secure/SameSite=Strict cookie
11. Frontend stores access token in memory (not localStorage)
12. Frontend uses access token in Authorization header for API calls
13. On 401, frontend calls /api/v1/auth/refresh to get new access token
```

### JWT Claims

```json
{
  "sub": "<user_id>",
  "email": "<email>",
  "iat": 1234567890,
  "exp": 1234568790
}
```

### Session Record

```
PK: USER#<user_id>
SK: SESSION#<session_id>
Attributes: refresh_token_hash, expires_at (30 days), device_info, created_at
```

### Future-Proofing Notes

- JWT claims include `sub` (user ID) only. Phase 4 adds `org_id` and `roles` claims for RBAC.
- Session record structure supports multiple active sessions per user from day one.
- The `auth` crate exposes a trait (`AuthProvider`) that MVP implements for one OAuth provider. Phase 4 adds SAML SSO and MFA as additional implementations.
- Refresh token rotation: each refresh invalidates the old token and issues a new one. Detects token reuse (stolen refresh token).

---

## Rich Text Editor (MVP)

### Document Schema (MVP Nodes)

| Node | Content | Group | MVP | Notes |
|------|---------|-------|-----|-------|
| `doc` | `block+` | top | Yes | Root node |
| `text` | leaf | inline | Yes | Carries marks |
| `paragraph` | `inline*` | block | Yes | Default block |
| `heading` | `inline*` | block | Yes | Levels 1-3 (not 4-6 for MVP) |
| `bulletList` | `listItem+` | block | Yes | |
| `orderedList` | `listItem+` | block | Yes | |
| `listItem` | `paragraph block*` | - | Yes | Defining |
| `taskList` | `taskItem+` | block | Yes | |
| `taskItem` | `paragraph block*` | - | Yes | `checked` attr |
| `codeBlock` | `text*` | block | Yes | `language` attr; no marks inside |
| `blockquote` | `block+` | block | Yes | |
| `horizontalRule` | leaf | block | Yes | |
| `hardBreak` | leaf | inline | Yes | |
| `image` | leaf, atom | block | Yes | `src`, `alt`, `title` attrs |
| `table` | `tableRow+` | block | No | Phase 2 |
| `spreadsheet` | leaf, atom | block | No | Phase 3 (NodeView) |
| `embed` | leaf, atom | block | No | Phase 5 (NodeView) |
| `mention` | leaf, atom | inline | No | Phase 2 |

### Document Schema (MVP Marks)

| Mark | Element | MVP | Notes |
|------|---------|-----|-------|
| `bold` | `<strong>` | Yes | |
| `italic` | `<em>` | Yes | |
| `underline` | `<u>` | Yes | |
| `strike` | `<s>` | Yes | |
| `code` | `<code>` | Yes | Excludes all other marks |
| `link` | `<a>` | Yes | `href`, `target`, `rel` attrs |
| `highlight` | `<mark>` | No | Phase 2 |
| `textStyle` | `<span>` | No | Phase 2 (color, font) |

### Input Rules (MVP)

| Input | Result |
|-------|--------|
| `# ` | Heading 1 |
| `## ` | Heading 2 |
| `### ` | Heading 3 |
| `> ` | Blockquote |
| `* `, `- `, `+ ` | Bullet list |
| `1. ` | Ordered list |
| `[ ] `, `[x] ` | Task list |
| ```` ``` ```` | Code block |
| `---` | Horizontal rule |
| `**text**` | Bold |
| `*text*` | Italic |
| `` `text` `` | Inline code |

### Keyboard Shortcuts (MVP)

Shortcuts follow the Quip interface (see `editor-features.md`). ProseMirror-style alternatives are also bound where they don't conflict.

| Key | Command | Notes |
|-----|---------|-------|
| `Ctrl+B` | toggleBold | |
| `Ctrl+I` | toggleItalic | |
| `Ctrl+U` | toggleUnderline | |
| `Ctrl+Shift+X` | toggleStrike | Quip shortcut |
| `Ctrl+Shift+S` | toggleStrike | ProseMirror alternative |
| `Ctrl+Shift+K` | toggleCode | Quip shortcut |
| `Ctrl+E` | toggleCode | ProseMirror alternative |
| `Ctrl+Alt+0` | setParagraph | |
| `Ctrl+Alt+1-3` | setHeading(level) | |
| `Ctrl+Shift+L` | toggleBulletList | Quip shortcut |
| `Ctrl+A` | Select all | |
| `Enter` | Split block / new list item | In list items, creates a new item |
| `Shift+Enter` | Hard break (`<br>`) | Handled via `insertLineBreak` beforeinput |
| `Tab` | Indent list item | Consumed (preventDefault) even when not applicable |
| `Shift+Tab` | Outdent list item | Consumed (preventDefault) even when not applicable |
| `Ctrl+Backspace` | Delete word backward | |
| `Ctrl+Delete` | Delete word forward | |
| `Backspace` | Join backward / delete char / delete selection | |
| `Delete` | Join forward / delete char / delete selection | |
| `Ctrl+S` | Consumed (no-op) | Prevents browser save dialog |
| `Ctrl+P` | Consumed (no-op) | Prevents browser print dialog |

**Not yet implemented (events consumed but no action):**
`Ctrl+Z` (undo), `Ctrl+Shift+Z` (redo), `Ctrl+V` (paste), `Ctrl+K` (link),
`Ctrl+Shift+7` (ordered list), `Ctrl+Shift+9` (task list),
`Ctrl+Shift+B` (blockquote), `Ctrl+Alt+C` (code block).

### Editor Architecture

```
┌──────────────────────────────────────────────────┐
│                  Leptos Shell                     │
│  ┌────────────┐  ┌───────────────────────────┐   │
│  │  Toolbar   │  │   Editor Container        │   │
│  │ (Leptos    │  │   (NodeRef<Div>)           │   │
│  │  component)│  │                           │   │
│  └──────┬─────┘  │  ┌─────────────────────┐  │   │
│         │        │  │  contenteditable     │  │   │
│         │        │  │  (managed by editor  │  │   │
│         │        │  │   core, not Leptos)  │  │   │
│         │        │  └─────────────────────┘  │   │
│         │        └───────────┬───────────────┘   │
│         └────────────────────┤                    │
│                              │                    │
│  ┌───────────────────────────┴──────────────────┐│
│  │            Editor Core (Rust/WASM)            ││
│  │  ┌────────┐ ┌───────┐ ┌──────────┐ ┌──────┐ ││
│  │  │ Model  │ │ State │ │Transform │ │ View │ ││
│  │  │(schema,│ │(doc,  │ │(steps,   │ │(DOM, │ ││
│  │  │ nodes, │ │select,│ │ mapping) │ │input,│ ││
│  │  │ marks) │ │plugin)│ │          │ │IME)  │ ││
│  │  └────────┘ └───────┘ └──────────┘ └──────┘ ││
│  │  ┌─────────────────────────────────────────┐ ││
│  │  │           yrs Bridge                     │ ││
│  │  │  Editor Model <-> Y.XmlFragment          │ ││
│  │  └─────────────────────────────────────────┘ ││
│  └──────────────────────────────────────────────┘│
└──────────────────────────────────────────────────┘
```

The toolbar is a normal Leptos component that calls commands on the editor core. The editor core owns the contenteditable div and manages it directly via web-sys. Leptos does not reconcile the editor's DOM.

### yrs Bridge (MVP)

In MVP, the yrs bridge handles **local persistence only** (no real-time sync):

1. On document open: load Y.Doc state from API (`GET /documents/:id/content`), apply to editor model
2. On edit: editor model changes are mirrored to the Y.Doc via the bridge
3. On save: serialize Y.Doc state, upload to API (`PUT /documents/:id/content`)
4. Auto-save: debounced (2s after last edit), with dirty flag

The bridge maps between the editor's typed document tree and yrs's `Y.XmlFragment`/`Y.XmlElement`/`Y.XmlText` types. Block nodes become XmlElements; inline content uses XmlText with formatting attributes as marks.

### Schema Duality

The document schema is defined in **two places** that must stay synchronized:

| Location | Purpose | Compilation Target |
|---|---|---|
| `crates/collab/src/schema.rs` | Server-side: yrs tag mapping, export, CRDT operations | Native Rust (server) |
| `frontend/src/editor/model.rs` + `frontend/src/editor/schema.rs` | Client-side: editor model, content validation, transforms | WASM (browser) |

**Why two definitions exist:**

The collab crate runs on the server (native Rust) and maps node/mark types to yrs `XmlElement` tag names for CRDT serialization. The frontend editor runs in the browser (WASM) and needs a richer schema model (`NodeSpec` with `inline_content`, `atom`, `defining`, `isolating`, `allowed_marks`, `default_attrs`, etc.) to drive content validation and editing commands. These concerns are architecturally distinct and compile to different targets, so they cannot share a single Rust module without introducing a shared crate that cross-compiles to both targets.

**What must stay synchronized:**

1. **Node type enum variants** -- both must define the same set of node types (Doc, Paragraph, Heading, BulletList, OrderedList, ListItem, TaskList, TaskItem, Blockquote, CodeBlock, HorizontalRule, HardBreak, Image).
2. **Mark type enum variants** -- both must define the same set (Bold, Italic, Underline, Strike, Code, Link).
3. **Tag names** -- collab's `NodeType::tag_name()` must match the tag names the yrs bridge in `frontend/editor/yrs_bridge.rs` expects.
4. **Content rules** -- collab's `NodeType::valid_children()` must agree with the frontend's `NodeSpec::valid_children`. A document valid on the server must also be valid in the editor, and vice versa.
5. **Mark exclusion** -- collab's `MarkType::excludes_all()` must agree with the frontend's `MarkSpec::exclude_all`.

**Validation mechanism:**

A cross-schema consistency test must be maintained that verifies all five invariants above. This test lives in `crates/collab/tests/` and imports the collab schema, comparing it against a hardcoded expected set that mirrors the frontend definition. When adding a node or mark type to either side, the test fails until both are updated.

Future consideration: if a shared `ogrenotes-schema` crate that compiles to both native and WASM becomes practical, unification is preferred. Until then, the two definitions plus the consistency test are the accepted pattern.

### Future-Proofing Notes

- The schema definition is extensible. Adding `table`, `spreadsheet`, `embed`, `mention` nodes in later phases requires only adding NodeSpec entries and NodeView implementations -- no schema system changes. **Both schema locations must be updated together** (see Schema Duality above).
- The yrs bridge is designed for bidirectional sync from day one. Phase 2 adds WebSocket transport and remote update application, but the bridge logic is the same.
- The plugin system exists in MVP (history, input rules, keymap are all plugins). Phase 2 adds collaboration cursor plugin, comments plugin, etc.
- The command pattern (check applicability vs execute) enables toolbar state (grayed-out buttons) from day one.

---

## Frontend Application (MVP)

### Pages

| Route | Page | Description |
|-------|------|-------------|
| `/login` | Login | OAuth sign-in button |
| `/` | Home | File browser with sidebar |
| `/d/:id/:slug?` | Document | Editor with toolbar |

### Sidebar (MVP)

Collapsible left panel with:

- **Home** button (navigate to file browser)
- **Recent** section (last 10 opened documents, stored in localStorage)
- **Pinned** section (stub -- shows "No pinned items" in MVP)
- **Create** button (new document or new folder)

No search, no chat, no notifications in MVP. The sidebar layout reserves visual space for these sections without implementing them.

### File Browser (MVP)

List view showing the current folder's contents:

| Column | Sortable | Description |
|--------|----------|-------------|
| Title | Yes | Document/folder name (click to open) |
| Updated | Yes | Last modified timestamp |
| Type | No | Icon: document or folder |

Features:
- Breadcrumb navigation at the top
- Click folder to drill in, click document to open editor
- Context menu (right-click): Rename, Move to folder, Delete
- "New Document" and "New Folder" buttons
- Trash view (soft-deleted items with 30-day retention)

### Toolbar (MVP)

Horizontal bar above the editor with formatting buttons. State-aware (buttons highlight when active, gray out when not applicable).

```
[B] [I] [U] [S] [</>] [🔗] | [¶] [H1] [H2] [H3] | [•] [1.] [☐] [>] [—] | [📷]
```

Groups:
1. **Inline marks**: Bold, Italic, Underline, Strike, Code, Link
2. **Block types**: Paragraph, Heading 1-3
3. **Block structures**: Bullet List, Ordered List, Task List, Blockquote, Horizontal Rule
4. **Insert**: Image (triggers file upload)

### Styling (MVP)

CSS custom properties for theming from day one:

```css
:root {
  --color-primary: #2D5F2D;     /* Swamp Green */
  --color-secondary: #5C3D2E;   /* Ogre Brown */
  --color-bg: #F5F0E8;          /* Parchment */
  --color-surface: #FFFFFF;     /* Bone */
  --color-text: #1A1A1A;        /* Ink */
  --color-text-secondary: #6B6B6B; /* Stone */
  --color-border: #E8E4DC;      /* Mist */
  --color-error: #C0392B;       /* Rust */
  --color-link: #2980B9;        /* River */

  --font-ui: 'Inter', system-ui, sans-serif;
  --font-mono: 'JetBrains Mono', ui-monospace, monospace;
  --font-doc-heading: 'Inter', system-ui, sans-serif;
  --font-doc-body: 'Inter', system-ui, sans-serif;

  --space-unit: 4px;
  --radius-sm: 4px;
  --radius-md: 6px;
  --content-max-width: 720px;
  --sidebar-width: 240px;
  --sidebar-collapsed: 48px;
}
```

Document themes (Phase 5) swap the `--font-doc-*` properties. Dark mode (Phase 5) swaps all color properties via a `[data-theme="dark"]` selector.

---

## Document Lifecycle (MVP)

### Create

```
1. User clicks "New Document"
2. Frontend calls POST /api/v1/documents { title: "Untitled" }
3. Server generates doc_id (nanoid, 21 chars, 126 bits entropy)
4. Server creates DOC#<doc_id>/METADATA in DynamoDB
5. Server creates empty Y.Doc, serializes to S3 snapshot
6. Server adds FOLDER#<folder_id>/CHILD#<doc_id> for current folder
7. Server returns { id, title, createdAt }
8. Frontend navigates to /d/<doc_id>/untitled
```

### Open

```
1. Frontend navigates to /d/<doc_id>/<slug>
2. Frontend calls GET /api/v1/documents/<doc_id>/content
3. Server loads latest S3 snapshot + any pending DynamoDB UPDATE# rows
4. Server applies pending updates to reconstruct full Y.Doc state
5. Server returns binary Y.Doc state
6. Frontend initializes editor: Y.Doc -> yrs bridge -> editor model -> contenteditable
7. Frontend calls GET /api/v1/documents/<doc_id> for metadata (title)
```

### Edit + Auto-Save

```
1. User types in editor
2. Editor core produces transaction -> new EditorState
3. yrs bridge mirrors change to Y.Doc
4. Dirty flag set, auto-save timer restarted (2s debounce)
5. On timer fire: serialize Y.Doc state
6. Frontend calls PUT /api/v1/documents/<doc_id>/content with binary state
7. Server writes new S3 snapshot, updates DOC# metadata (snapshot_version, updated_at)
8. Dirty flag cleared
```

### Delete

```
1. User selects "Delete" from context menu
2. Frontend calls DELETE /api/v1/documents/<doc_id>
3. Server sets is_deleted=true, deleted_at=now on DOC# metadata
4. Server moves FOLDER#/CHILD# entry to Trash folder
5. Document no longer appears in folder listings
6. After 30 days: background job permanently deletes DOC# rows and S3 objects
```

### Phase 2 Transition: REST Save to WebSocket Sync

MVP uses `PUT /documents/:id/content` to upload a full Y.Doc snapshot on every auto-save. Phase 2 replaces this with real-time incremental sync over WebSocket. This is the most significant architectural change between phases.

#### Coexistence During Transition

Both mechanisms exist in Phase 2. The WebSocket path is primary; the REST path is a fallback:

| Mechanism | When Used | What It Does |
|---|---|---|
| **WebSocket (primary)** | Active editing session with open connection | Streams yrs updates incrementally as the user types |
| **PUT /content (fallback)** | Connection lost, reconnect failed, explicit manual save | Uploads full Y.Doc state as a snapshot |
| **GET /content** | Initial document open, reconnect | Returns full Y.Doc state (snapshot + pending op log) |

#### WebSocket Protocol

```
1. Client opens WebSocket to /api/v1/documents/:id/ws
2. Client sends single-use auth token in first message
3. Server validates token, loads Y.Doc state (S3 snapshot + UPDATE# rows)
4. Server sends sync-step-1: full state vector
5. Client responds with sync-step-2: diff (what server is missing)
6. Server sends sync-step-2: diff (what client is missing)
7. Both sides are in sync
8. --- Streaming phase ---
9. Client sends yrs update bytes on each edit
10. Server applies update to server-side Y.Doc
11. Server appends UPDATE#<clock> row to DynamoDB
12. Server publishes update to Redis pubsub channel doc:<doc_id>
13. Other API instances receive via pubsub, fan out to their clients
14. Client receives updates from other users, merges via CRDT
```

**WebSocket authentication:** The client first calls `POST /api/v1/documents/:id/ws-token` to get a single-use token (short TTL, 30 seconds). This token is sent as the first WebSocket message. This avoids putting JWTs in WebSocket URL query params (which appear in server logs).

**Message format:** All WebSocket messages are binary frames containing raw yrs update bytes, prefixed by a 1-byte message type:

| Type Byte | Direction | Payload |
|---|---|---|
| `0x00` | Client -> Server | Auth token (UTF-8 string) |
| `0x01` | Bidirectional | yrs sync message (sync-step-1, sync-step-2, or update) |
| `0x02` | Bidirectional | yrs awareness update (cursor position, user info) |
| `0x03` | Server -> Client | Error (UTF-8 JSON: `{"error": "...", "message": "..."}`) |

#### Op Log and Compaction

In Phase 2, every yrs update is persisted as a `DOC#<doc_id>/UPDATE#<clock>` row:

```
PK: DOC#<doc_id>
SK: UPDATE#<lamport_clock>
Attributes: update_bytes (Binary), user_id, created_at
```

**Compaction (idle snapshot):** When a document has no edits for 60 seconds:

1. Server serializes the full Y.Doc state to S3 as a new snapshot
2. Server updates `DOC#<doc_id>/METADATA` with new `snapshot_version` and `snapshot_s3_key`
3. Server deletes all `UPDATE#` rows with clock ≤ the snapshot's clock
4. Next client load reads only the fresh snapshot (no replay needed)

**Why this matters:** Without compaction, the UPDATE# row count grows unboundedly. Load time degrades as more rows must be replayed. The 60-second idle threshold balances write amplification (snapshot is larger than an update) against read performance.

#### MVP Preparation

MVP already has the UPDATE# row schema in DynamoDB but never writes to it. Phase 2 activates this write path. No schema migration is needed -- only code changes in the API's document content handlers and the addition of the WebSocket route.

### Trash Cleanup

Soft-deleted documents remain in DynamoDB and S3 for 30 days. After 30 days they must be permanently deleted. The cleanup mechanism evolves across phases:

| Phase | Mechanism | Details |
|---|---|---|
| MVP | Manual script | Run `cargo run --bin trash-cleanup` on demand. Queries `DOC#` rows where `is_deleted=true` and `deleted_at < now - 30 days`. Deletes DOC# rows, S3 snapshots, and FOLDER#/CHILD# references. |
| Phase 4 | Scheduled background worker | A long-running task (or Lambda) runs daily. Same logic as the manual script but automated and with metrics/alerting. |

The query uses GSI1 (`owner_id -> updated_at`) to find candidate documents, filtered by `is_deleted` and `deleted_at`. This is not index-optimal (scan + filter) but acceptable since the number of deleted documents is small. Phase 4 may add a dedicated GSI (`is_deleted -> deleted_at`) if volume warrants it.

---

## Folder Lifecycle (MVP)

### System Folders

Created automatically on first login:

| Folder | Type | Behavior |
|--------|------|----------|
| Home | system | Default view; cannot delete/rename |
| Private | system | Personal drafts; cannot delete/rename |
| Trash | system | Soft-deleted items; cannot delete/rename |

System folder IDs are stored on the user record and returned by `GET /api/v1/users/me`.

### User Folders

Users can create folders inside Home or inside other user folders. Folders have:
- `title` (string)
- `color` (integer 0-11, default 0)
- `parent_id` (folder_id of parent)

### Document-Folder Relationship

MVP supports one folder per document (simple move semantics). The data model supports multi-folder (tag-based) from day one -- Phase 2 enables "Add to folder" without removing from the current one.

---

## Error Handling Strategy

### Backend

Each crate defines its own error enum with `thiserror`:

```rust
// storage/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("Item not found: {0}")]
    NotFound(String),
    #[error("Condition check failed")]
    ConditionFailed,
    #[error("DynamoDB error: {0}")]
    Dynamo(#[from] aws_sdk_dynamodb::Error),
    #[error("S3 error: {0}")]
    S3(#[from] aws_sdk_s3::Error),
}
```

The `api` crate maps these to HTTP responses via `IntoResponse`:

```rust
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", "...".into()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "...".into()),
            // ...
        };
        (status, Json(ErrorBody { error: error_code, message })).into_response()
    }
}
```

### Frontend

API errors are caught by the fetch wrapper and surfaced as Leptos signals. Components render error states inline:
- 401: redirect to login
- 404: "Document not found" page
- 429: "Slow down" toast
- 500: "Something went wrong" with retry button

---

## Observability (MVP)

### Structured Logging

Every request gets a tracing span with:
- `request_id` (UUID)
- `user_id` (from JWT)
- `method` + `path`
- `status_code`
- `duration_ms`

```rust
tracing::info!(
    request_id = %request_id,
    user_id = %user_id,
    method = %method,
    path = %path,
    status = %status,
    duration_ms = %elapsed,
    "request completed"
);
```

### Metrics (Stub)

MVP logs metrics as structured tracing events. Phase 4 adds Prometheus exposition or CloudWatch metrics.

Key metrics to track from day one:
- Request count by endpoint and status
- Request duration (p50, p95, p99)
- DynamoDB consumed capacity
- S3 operation count
- Active sessions
- Document save latency

---

## Testing Strategy

### Test Dependencies

```toml
[workspace.dependencies]
# Testing
proptest = "1"
test-strategy = "0.4"       # Derive-based proptest strategies
axum-test = "16"             # Axum integration test client
wiremock = "0.6"             # HTTP mock server (for OAuth provider)
claims = "0.7"               # JWT assertion helpers
fake = { version = "3", features = ["derive"] } # Fake data generation
assert_matches = "1.5"
tokio-test = "0.4"
wasm-bindgen-test = "0.3"    # WASM unit tests (frontend/editor)
```

---

### Unit Tests

#### `common` crate

```
tests/
├── id_test.rs
└── time_test.rs
```

| Test | Description |
|------|-------------|
| `id_uniqueness` | Generate 10,000 IDs, assert no duplicates |
| `id_length` | Generated IDs are exactly 21 characters (126 bits entropy) |
| `id_alphabet` | IDs contain only URL-safe characters |
| `timestamp_microsecond_precision` | `now_usec()` returns microseconds since epoch |
| `timestamp_roundtrip` | `usec -> DateTime -> usec` preserves value |
| `timestamp_ordering` | Sequential calls produce monotonically increasing values |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_id_is_url_safe` | For any generated ID, all characters are in `[A-Za-z0-9_-]` |
| `prop_id_length_invariant` | For any generated ID, length is always 21 |

---

#### `auth` crate

```
tests/
├── jwt_test.rs
├── oauth_test.rs
└── session_test.rs
```

**JWT tests:**

| Test | Description |
|------|-------------|
| `create_and_validate_token` | Create JWT, validate it, assert claims match |
| `expired_token_rejected` | Token with past `exp` fails validation |
| `tampered_token_rejected` | Modify payload after signing, assert validation fails |
| `wrong_secret_rejected` | Validate with different secret, assert failure |
| `missing_sub_claim_rejected` | Token without `sub` claim fails validation |
| `token_expiry_is_15_minutes` | Created token's `exp` is ~900s after `iat` |
| `refresh_produces_new_token` | Refresh returns a new access token with fresh `exp` |
| `refresh_rotates_refresh_token` | Refresh invalidates old refresh token, issues new one |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_jwt_roundtrip` | For any `(user_id, email)`, `create -> validate` recovers the same claims |
| `prop_expired_always_rejected` | For any claims with `exp < now`, validation fails |
| `prop_signature_is_deterministic` | Same claims + same secret produce same token |

**OAuth tests:**

| Test | Description |
|------|-------------|
| `pkce_verifier_length` | Code verifier is 43-128 characters (RFC 7636) |
| `pkce_challenge_is_s256` | Challenge is base64url(sha256(verifier)) |
| `authorization_url_contains_required_params` | URL includes `client_id`, `redirect_uri`, `response_type`, `code_challenge`, `state` |
| `token_exchange_sends_verifier` | Exchange request includes `code_verifier` |
| `state_mismatch_rejected` | Callback with wrong `state` param returns error |

**Session tests:**

| Test | Description |
|------|-------------|
| `create_session_stores_hash_not_plaintext` | Session record contains token hash, not raw token |
| `revoke_session_deletes_record` | After revocation, session lookup returns None |
| `expired_session_rejected` | Session past `expires_at` fails lookup |
| `refresh_token_reuse_revokes_all` | Using an already-rotated refresh token revokes the entire session family |

---

#### `collab` crate

```
tests/
├── document_test.rs
├── schema_test.rs
├── snapshot_test.rs
└── export_test.rs
```

**Document tests:**

| Test | Description |
|------|-------------|
| `create_empty_doc` | New Y.Doc has root XmlFragment with empty paragraph |
| `insert_text` | Insert "hello" at position 0, verify text content |
| `insert_text_with_marks` | Insert bold text, verify formatting attributes |
| `delete_range` | Delete characters 2-4, verify remaining content |
| `apply_update_bytes` | Apply a serialized update, verify document state matches |
| `concurrent_inserts_converge` | Two Y.Docs apply each other's updates, verify identical state |
| `update_roundtrip` | Apply update -> encode -> decode -> apply to fresh doc -> states match |

**Schema tests:**

| Test | Description |
|------|-------------|
| `paragraph_accepts_inline` | Paragraph node accepts text, hard break, image |
| `paragraph_rejects_block` | Paragraph node rejects heading, blockquote as children |
| `heading_accepts_inline` | Heading node accepts text with marks |
| `heading_rejects_block` | Heading node rejects nested paragraphs |
| `list_item_requires_paragraph_first` | ListItem content must start with paragraph |
| `code_block_text_only` | CodeBlock accepts only text nodes, no marks |
| `blockquote_accepts_blocks` | Blockquote accepts paragraphs, headings, lists |
| `doc_requires_at_least_one_block` | Empty doc is invalid; minimum is one paragraph |
| `mark_exclusion_code` | Code mark excludes bold, italic, and all other marks |
| `nested_lists` | BulletList > ListItem > BulletList is valid |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_snapshot_roundtrip` | For any valid Y.Doc state, `serialize -> deserialize` produces identical state |
| `prop_update_commutativity` | For updates A and B from same initial state, applying A then B produces same result as B then A |
| `prop_content_match_dfa_accepts_valid` | For any sequence of nodes valid under the content expression, the DFA accepts |
| `prop_content_match_dfa_rejects_invalid` | For any sequence containing a node not in the content expression, the DFA rejects |

**Snapshot tests:**

| Test | Description |
|------|-------------|
| `snapshot_empty_doc` | Serialize empty doc, deserialize, verify structure |
| `snapshot_complex_doc` | Doc with headings, lists, code blocks, images survives roundtrip |
| `snapshot_with_pending_updates` | Snapshot + update log replay produces correct final state |
| `snapshot_size_grows_sublinearly` | 1000-character doc snapshot is under 2KB (compression check) |

**Export tests:**

| Test | Description |
|------|-------------|
| `export_html_paragraph` | Paragraph with text exports as `<p>text</p>` |
| `export_html_heading` | H2 heading exports as `<h2>text</h2>` |
| `export_html_bold_italic` | Bold+italic text exports as `<strong><em>text</em></strong>` |
| `export_html_nested_list` | Nested bullet list produces correct `<ul><li><ul>` structure |
| `export_html_code_block` | Code block exports as `<pre><code class="language-rust">` |
| `export_html_image` | Image node exports as `<img src="..." alt="...">` |
| `export_html_link` | Link mark exports as `<a href="..." rel="...">` |
| `export_html_task_list` | Task list with checked item exports with checkbox attributes |
| `export_markdown_heading` | H1 exports as `# text` |
| `export_markdown_bold` | Bold text exports as `**text**` |
| `export_markdown_bullet_list` | Bullet list exports with `- ` prefix |
| `export_markdown_code_block` | Code block exports with triple backticks and language |
| `export_markdown_link` | Link exports as `[text](url)` |
| `export_html_roundtrip` | Export to HTML, parse HTML back, verify document equality |

---

#### `storage` crate

```
tests/
├── models/
│   ├── user_test.rs
│   ├── document_test.rs
│   ├── folder_test.rs
│   └── session_test.rs
└── repo/
    (integration tests only -- see below)
```

**Model serialization tests:**

| Test | Description |
|------|-------------|
| `user_to_dynamo_roundtrip` | User struct -> DynamoDB AttributeValue map -> User struct matches |
| `user_pk_format` | PK is `USER#<user_id>`, SK is `PROFILE` |
| `document_to_dynamo_roundtrip` | Document metadata roundtrips through serde_dynamo |
| `document_pk_format` | PK is `DOC#<doc_id>`, SK is `METADATA` |
| `document_soft_delete_fields` | Deleted document has `is_deleted=true` and `deleted_at` set |
| `folder_to_dynamo_roundtrip` | Folder metadata roundtrips correctly |
| `folder_child_doc_format` | Child record has PK `FOLDER#<id>`, SK `CHILD#<doc_id>`, `child_type=doc` |
| `folder_child_folder_format` | Child record has `child_type=folder` |
| `folder_color_range` | Color value clamped to 0-11 |
| `session_pk_format` | PK is `USER#<user_id>`, SK is `SESSION#<session_id>` |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_user_serde_roundtrip` | For any valid User, `to_dynamo -> from_dynamo` is identity |
| `prop_document_serde_roundtrip` | For any valid Document, roundtrip is identity |
| `prop_folder_serde_roundtrip` | For any valid Folder, roundtrip is identity |
| `prop_folder_color_always_valid` | For any u8 input, color is clamped to 0..=11 |

---

#### `frontend/editor` (WASM unit tests)

Run with `wasm-pack test --headless --firefox` (or `--chrome` if ChromeDriver version matches).

Unit tests are inline `#[cfg(test)]` modules within each editor source file.
Browser integration tests use `wasm-bindgen-test` in a single file:

```
src/editor/
├── model.rs         # model unit tests
├── schema.rs        # schema validation tests
├── transform.rs     # step/transform tests
├── commands.rs      # command unit tests
├── input_rules.rs   # input rule unit tests
├── selection.rs     # selection tests
├── state.rs         # transaction/split/join tests
├── position.rs      # position resolution tests
└── view.rs          # view rendering tests

tests/
└── browser.rs       # 111 browser integration tests (wasm-bindgen-test)
```

Current totals: **298 unit tests** + **111 browser integration tests**.

**Model tests:**

| Test | Description |
|------|-------------|
| `text_node_creation` | Create text node with content and marks |
| `text_node_merge` | Adjacent text nodes with same marks merge |
| `text_node_no_merge_different_marks` | Adjacent nodes with different marks stay separate |
| `fragment_size_calculation` | Fragment size equals sum of child sizes + boundaries |
| `fragment_cut` | Cutting fragment produces correct sub-fragment |
| `node_child_access` | Access child by index returns correct node |
| `position_resolution` | Resolve position returns correct depth, parent, offset |
| `slice_open_depth` | Slice from middle of nested structure has correct openStart/openEnd |
| `mark_ordering` | Marks are stored in schema-defined canonical order |
| `mark_equality` | Marks with same type and attrs are equal |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_fragment_size_is_consistent` | For any fragment, `size` equals the sum of content sizes of all children plus 2 per non-text child |
| `prop_position_resolution_depth` | For any valid position in a document, `resolve(pos).depth` is ≥ 0 and ≤ max nesting |
| `prop_text_normalization` | For any sequence of text insertions, the result never has adjacent text nodes with identical marks |
| `prop_mark_sort_is_idempotent` | Sorting marks twice produces the same order as sorting once |

**Transform tests:**

| Test | Description |
|------|-------------|
| `insert_text_at_start` | Insert text at position 1, verify document |
| `insert_text_at_end` | Insert text at end of paragraph |
| `insert_text_mid_word` | Insert text in middle of existing text |
| `delete_single_char` | Delete one character, verify positions shift |
| `delete_across_nodes` | Delete spanning two paragraphs, verify join |
| `replace_range_with_text` | Replace characters 3-7 with new text |
| `add_mark_to_range` | Apply bold to characters 2-5, verify mark array |
| `remove_mark_from_range` | Remove bold from subset of bolded text |
| `split_paragraph` | Split at cursor, verify two paragraphs result |
| `join_paragraphs` | Join two adjacent paragraphs into one |
| `wrap_in_blockquote` | Wrap paragraph in blockquote, verify structure |
| `lift_from_blockquote` | Lift paragraph out of blockquote |
| `set_block_type_heading` | Change paragraph to heading level 2 |
| `step_inversion` | For each step type, `step.invert(doc).apply(result)` recovers original |
| `step_map_positions` | After insert step, positions after the insert shift by the right amount |

**Property tests:**

| Property | Description |
|----------|-------------|
| `prop_insert_delete_roundtrip` | For any (doc, pos, text), insert then delete at same range recovers original |
| `prop_step_inversion` | For any valid step on any valid doc, `apply(step).apply(step.invert)` is identity |
| `prop_step_map_consistency` | For any step, mapped positions via StepMap agree with actual positions in the resulting doc |
| `prop_add_remove_mark_roundtrip` | For any (doc, from, to, mark), add then remove recovers original |
| `prop_split_join_roundtrip` | For any splittable position, split then join recovers original |

**Command tests:**

| Test | Description |
|------|-------------|
| `toggle_bold_on` | With no bold, toggleBold applies bold to selection |
| `toggle_bold_off` | With bold active, toggleBold removes bold |
| `toggle_bold_check_only` | `toggleBold(state, None)` returns true without modifying state |
| `toggle_bold_in_code_block` | `toggleBold` returns false (not applicable) inside code block |
| `set_heading_from_paragraph` | `setHeading(1)` converts paragraph to h1 |
| `set_heading_preserves_inline` | Heading conversion preserves bold/italic text |
| `set_paragraph_from_heading` | `setParagraph` converts h2 back to paragraph |
| `toggle_bullet_list` | Wraps paragraphs in bullet list |
| `toggle_bullet_list_off` | Unwraps list items back to paragraphs |
| `indent_list_item` | Tab sinks list item one level (cursor stays in indented item) |
| `indent_first_item_consumes_tab` | Tab on first list item is a no-op but prevents focus escape |
| `outdent_list_item` | Shift+Tab lifts list item one level (cursor stays in lifted item) |
| `outdent_top_level_consumes_tab` | Shift+Tab at top level is a no-op but prevents focus escape |
| `split_list_item` | Enter in middle of list item creates two list items (not two paragraphs in one item) |
| `split_empty_list_item_exits` | Enter on empty list item lifts out of list (not yet implemented) |
| `insert_hard_break` | Shift+Enter inserts `<br>` without splitting block (via `insertLineBreak` beforeinput) |
| `join_forward` | Delete at end of block merges with next block (cursor stays at join point) |
| `delete_word_backward` | Ctrl+Backspace deletes word before cursor (skips whitespace, then word chars) |
| `delete_word_forward` | Ctrl+Delete deletes word after cursor (skips word chars, then whitespace) |
| `delete_selection` | With selection, delete removes selected content |
| `select_all` | selectAll selects from start to end of document |

**Input rule tests:**

| Test | Description |
|------|-------------|
| `hash_space_creates_h1` | Typing `# ` at line start converts to heading 1 |
| `double_hash_creates_h2` | `## ` creates heading 2 |
| `triple_hash_creates_h3` | `### ` creates heading 3 |
| `hash_mid_line_no_effect` | `# ` in middle of text does not convert |
| `asterisk_space_creates_bullet` | `* ` creates bullet list |
| `dash_space_creates_bullet` | `- ` creates bullet list |
| `number_dot_creates_ordered` | `1. ` creates ordered list |
| `bracket_space_creates_task` | `[ ] ` creates unchecked task item |
| `checked_bracket_creates_task` | `[x] ` creates checked task item |
| `gt_space_creates_blockquote` | `> ` wraps in blockquote |
| `triple_backtick_creates_code` | ` ``` ` creates code block |
| `triple_backtick_with_lang` | ` ```rust ` creates code block with language attr |
| `triple_dash_creates_hr` | `---` replaces paragraph with horizontal rule + new empty paragraph (cursor in new paragraph) |
| `triple_underscore_creates_hr` | `___` creates horizontal rule (same behavior as `---`) |
| `double_asterisk_bolds` | `**text**` applies bold mark |
| `single_asterisk_italicizes` | `*text*` applies italic mark |
| `backtick_creates_code` | `` `text` `` applies code mark |
| `double_underscore_bolds` | `__text__` applies bold mark |
| `single_underscore_italicizes` | `_text_` applies italic mark |
| `hash_mid_line_no_effect` | `# ` in middle of text does not convert |
| `input_rule_undoable` | After rule fires, Ctrl+Z reverts to the typed text (not yet implemented) |

**Selection tests:**

| Test | Description |
|------|-------------|
| `cursor_at_start` | TextSelection at position 1 has empty=true, from=to=1 |
| `range_selection` | TextSelection from 2 to 5 has empty=false |
| `node_selection_image` | Selecting image node creates NodeSelection |
| `node_selection_hr` | Selecting horizontal rule creates NodeSelection |
| `selection_after_insert` | Selection maps correctly after text insertion before it |
| `gap_cursor_after_code_block` | GapCursor is valid after a code block at end of doc |

**Clipboard tests:**

| Test | Description |
|------|-------------|
| `copy_plain_text` | Copying "hello" produces text/plain "hello" |
| `copy_bold_text` | Copying bold text produces HTML `<strong>hello</strong>` |
| `copy_nested_list` | Copying nested list produces valid HTML structure |
| `paste_plain_text` | Pasting plain text inserts at cursor |
| `paste_html_bold` | Pasting `<strong>text</strong>` creates bold text node |
| `paste_html_strips_scripts` | Pasting `<script>` tags does not create script nodes |
| `paste_html_unknown_elements` | Pasting `<custom-el>text</custom-el>` extracts text only |
| `paste_adjusts_to_context` | Pasting a heading inside a list item converts to paragraph |

---

### Integration Tests

Integration tests run against real AWS DynamoDB and S3 (dev resources). Each test uses a unique prefix or document ID to avoid collisions. Tests clean up after themselves.

```
tests/integration/
├── auth_flow_test.rs
├── document_lifecycle_test.rs
├── folder_lifecycle_test.rs
├── blob_test.rs
└── helpers/
    ├── mod.rs              # Test app builder, auth helpers
    ├── test_app.rs         # Spin up Axum with test config
    └── fixtures.rs         # Sample documents, users
```

**Test infrastructure:**

```rust
// helpers/test_app.rs
struct TestApp {
    client: axum_test::TestServer,
    dynamo: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table_name: String,
    bucket: String,
    user_token: String,  // Pre-authenticated JWT for test user
}

impl TestApp {
    async fn new() -> Self { /* ... */ }
    async fn cleanup(&self) { /* delete all items with test prefix */ }
    fn auth_header(&self) -> (String, String) {
        ("Authorization".into(), format!("Bearer {}", self.user_token))
    }
}
```

#### Auth Flow Tests

| Test | Description |
|------|-------------|
| `login_redirects_to_provider` | `GET /auth/login` returns 302 with correct OAuth URL params (client_id, redirect_uri, code_challenge, state) |
| `callback_exchanges_code` | `GET /auth/callback?code=...&state=...` with mocked OAuth provider returns access token in body and refresh token in Set-Cookie |
| `callback_invalid_state_returns_400` | Callback with wrong `state` param returns 400 |
| `callback_creates_user_on_first_login` | After callback, USER# record exists in DynamoDB |
| `callback_creates_session` | After callback, SESSION# record exists in DynamoDB |
| `refresh_returns_new_access_token` | `POST /auth/refresh` with valid cookie returns new JWT with later `exp` |
| `refresh_rotates_refresh_token` | After refresh, old refresh token no longer works |
| `refresh_without_cookie_returns_401` | `POST /auth/refresh` without cookie returns 401 |
| `logout_revokes_session` | `POST /auth/logout` deletes SESSION# record; subsequent refresh fails |
| `protected_route_without_token_returns_401` | `GET /users/me` without Authorization header returns 401 |
| `protected_route_with_expired_token_returns_401` | Request with expired JWT returns 401 |
| `protected_route_with_valid_token_returns_200` | Request with valid JWT returns user data |

#### Document Lifecycle Tests

| Test | Description |
|------|-------------|
| `create_document` | `POST /documents` returns 201 with `id`, `title`, `createdAt` |
| `create_document_default_title` | Created without title gets "Untitled" |
| `create_document_custom_title` | Created with `{ title: "My Doc" }` uses that title |
| `create_document_creates_s3_snapshot` | After create, S3 object exists at `docs/<id>/snapshots/1.bin` |
| `create_document_adds_to_folder` | After create with `folderId`, FOLDER#/CHILD# record exists |
| `get_document` | `GET /documents/:id` returns metadata matching created document |
| `get_document_not_found` | `GET /documents/nonexistent` returns 404 |
| `get_document_deleted_returns_404` | Soft-deleted document returns 404 |
| `get_document_wrong_user_returns_404` | Another user's document returns 404 (MVP: no sharing) |
| `update_document_title` | `PATCH /documents/:id { title: "New" }` updates title |
| `update_document_updates_timestamp` | After PATCH, `updatedAt` is later than `createdAt` |
| `delete_document` | `DELETE /documents/:id` sets `is_deleted=true` |
| `delete_document_moves_to_trash` | After DELETE, document appears in Trash folder children |
| `delete_document_removes_from_original_folder` | After DELETE, document no longer in original folder |
| `delete_document_idempotent` | DELETE on already-deleted document returns 204 |
| `get_content_empty_doc` | `GET /documents/:id/content` for new doc returns valid Y.Doc bytes |
| `put_content_and_get_content_roundtrip` | PUT content bytes, GET them back, verify identical |
| `put_content_updates_snapshot_version` | After PUT, metadata `snapshot_version` increments |
| `put_content_invalid_bytes_returns_400` | PUT with random bytes (not valid Y.Doc) returns 400 |
| `export_html` | `GET /documents/:id/export/html` returns valid HTML with correct formatting |
| `export_markdown` | `GET /documents/:id/export/markdown` returns correct Markdown |
| `export_not_found_returns_404` | Export on nonexistent document returns 404 |

#### Document Content Tests (Deeper)

| Test | Description |
|------|-------------|
| `save_paragraph_text` | Create doc, PUT content with paragraph text, GET back, verify text |
| `save_heading` | PUT content with H1 heading, export as HTML, verify `<h1>` |
| `save_formatted_text` | PUT content with bold+italic text, export, verify marks |
| `save_bullet_list` | PUT content with nested bullet list, export, verify structure |
| `save_task_list_checked` | PUT content with checked task, export, verify checked attribute |
| `save_code_block_with_language` | PUT content with Rust code block, export, verify language class |
| `save_image_node` | PUT content with image, export, verify `<img>` with src/alt |
| `save_multiple_edits` | PUT content three times, GET returns latest version only |

#### Folder Lifecycle Tests

| Test | Description |
|------|-------------|
| `system_folders_created_on_first_login` | After first auth, USER# record has `home_folder_id`, `private_folder_id`, `trash_folder_id` |
| `system_folders_exist` | GET each system folder returns 200 with correct `folder_type=system` |
| `system_folders_cannot_be_deleted` | DELETE on system folder returns 403 |
| `system_folders_cannot_be_renamed` | PATCH title on system folder returns 403 |
| `create_folder` | `POST /folders { title, parentId }` returns 201 with folder ID |
| `create_folder_default_color` | Created without color gets color 0 |
| `create_folder_with_color` | Created with color 4 (Blue) returns color 4 |
| `create_nested_folder` | Create folder inside folder, verify parent-child relationship |
| `get_folder_with_children` | Folder with docs and subfolders returns all children |
| `get_folder_not_found` | `GET /folders/nonexistent` returns 404 |
| `update_folder_title` | `PATCH /folders/:id { title: "New" }` updates title |
| `update_folder_color` | `PATCH /folders/:id { color: 3 }` updates color |
| `update_folder_move_parent` | `PATCH /folders/:id { parentId: "new_parent" }` moves folder |
| `delete_folder` | `DELETE /folders/:id` removes folder |
| `delete_folder_with_children` | DELETE folder moves contained documents to Trash |
| `add_document_to_folder` | `POST /folders/:id/children { childId, childType: "doc" }` creates CHILD# record |
| `remove_document_from_folder` | `DELETE /folders/:id/children/:childId` removes CHILD# record |
| `folder_children_sorted_by_title` | GET folder returns children alphabetically by title (via GSI2) |
| `folder_listing_excludes_deleted_docs` | Soft-deleted documents do not appear in folder children |

#### Blob Tests

| Test | Description |
|------|-------------|
| `request_upload_url` | `POST /documents/:id/blobs { filename, contentType }` returns presigned PUT URL |
| `upload_url_is_valid_s3` | Returned URL starts with expected S3 bucket hostname |
| `upload_url_expires` | URL contains `X-Amz-Expires` parameter ≤ 900 (15 min) |
| `upload_and_download_roundtrip` | Upload file via presigned PUT, request download URL, GET file, verify content matches |
| `download_url_for_nonexistent_blob_returns_404` | Request download for fake blob_id returns 404 |
| `upload_url_scoped_to_document` | Presigned URL path contains the document ID |

---

### Property Tests (Cross-Cutting)

These property tests validate invariants that span multiple modules. They use `proptest` with custom strategies for generating arbitrary documents, operations, and state.

#### Document Model Properties

```rust
// Custom strategies
fn arb_text() -> impl Strategy<Value = String> { "[a-zA-Z0-9 ]{1,100}" }
fn arb_mark() -> impl Strategy<Value = Mark> { /* bold, italic, code, link */ }
fn arb_inline_node() -> impl Strategy<Value = Node> { /* text w/ marks, hard break */ }
fn arb_block_node() -> impl Strategy<Value = Node> { /* paragraph, heading, list, blockquote */ }
fn arb_document() -> impl Strategy<Value = Node> { /* doc with 1-10 random blocks */ }
fn arb_position(doc: &Node) -> impl Strategy<Value = usize> { 0..=doc.content.size }
fn arb_step(doc: &Node) -> impl Strategy<Value = Step> { /* valid step for this doc */ }
```

| Property | Crate | Description |
|----------|-------|-------------|
| `prop_document_always_valid` | `collab` | Any document produced by the schema builder passes schema validation |
| `prop_fragment_size_equals_content` | `frontend/editor` | Fragment.size == sum of child content sizes + 2 per non-leaf child |
| `prop_resolve_pos_in_bounds` | `frontend/editor` | For any valid position, resolve succeeds and depth ≥ 0 |
| `prop_step_preserves_validity` | `frontend/editor` | Applying any valid step to a valid document produces a valid document |
| `prop_step_inversion_roundtrip` | `frontend/editor` | `doc.apply(step).apply(step.invert(doc)) == doc` |
| `prop_step_map_monotonic` | `frontend/editor` | StepMap maps positions monotonically (order preserved) |
| `prop_mapping_composition` | `frontend/editor` | `map(a).map(b) == map(a.compose(b))` for sequential steps |
| `prop_yrs_bridge_roundtrip` | `collab` | `editor_model -> yrs -> editor_model` produces identical document |
| `prop_yrs_snapshot_deterministic` | `collab` | Same Y.Doc state always produces identical snapshot bytes |
| `prop_export_html_is_valid` | `collab` | For any valid document, exported HTML parses without errors |
| `prop_mark_sort_stable` | `frontend/editor` | Sorting marks is idempotent and produces canonical order |
| `prop_text_normalize_idempotent` | `frontend/editor` | Normalizing text nodes is idempotent |
| `prop_delete_insert_preserves_length` | `frontend/editor` | After delete(n) then insert(n chars), document size is unchanged |
| `prop_concurrent_yrs_updates_converge` | `collab` | Two Y.Docs with different update orderings converge to identical state |

#### API Property Tests

| Property | Description |
|----------|-------------|
| `prop_document_crud_roundtrip` | For any valid title, create then get returns matching title |
| `prop_folder_crud_roundtrip` | For any valid (title, color), create then get returns matching values |
| `prop_content_put_get_roundtrip` | For any valid Y.Doc bytes, PUT then GET returns identical bytes |
| `prop_folder_color_clamped` | For any integer color value, created folder's color is in 0..=11 |
| `prop_deleted_document_not_in_listing` | After delete, document never appears in any folder's children listing |

---

### Test Execution

```bash
# All unit tests
cargo test --workspace

# Unit tests for specific crate
cargo test -p collab
cargo test -p auth

# WASM editor tests (requires headless browser)
cd frontend && wasm-pack test --headless --firefox

# Integration tests (requires AWS dev resources + Redis)
cargo test --test '*' -- --test-threads=1

# Property tests only (longer running)
cargo test --workspace -- prop_

# Property tests with more cases (CI)
PROPTEST_CASES=1000 cargo test --workspace -- prop_
```

### CI Configuration

```yaml
# Property tests run with higher case count in CI
env:
  PROPTEST_CASES: 500
  # Integration tests use dedicated CI AWS resources
  DYNAMODB_TABLE_PREFIX: ci-${{ github.run_id }}-
  S3_BUCKET: ci-ogrenotes-test
```

### Test Coverage Targets (MVP)

| Crate | Target | Rationale |
|-------|--------|-----------|
| `common` | 95%+ | Small, pure functions |
| `auth` | 90%+ | Security-critical; must cover all token states |
| `collab` | 85%+ | Document model correctness is foundational |
| `storage` | 80%+ | Serialization correctness; AWS calls tested in integration |
| `frontend/editor` | 85%+ | Editor model/transform are core logic |
| `api` | 70%+ | Route wiring tested via integration tests |

---

## Development Workflow

### Local Setup

```bash
# Prerequisites: Rust toolchain, wasm-pack, trunk (or leptos CLI)

# Start Redis
docker compose up -d

# Configure AWS (dev account)
cp .env.example .env
# Edit .env with AWS profile, region, table prefix, bucket name

# Create DynamoDB table + S3 bucket (one-time)
cargo run --bin setup-dev

# Run backend
cargo run -p api

# Run frontend (separate terminal)
cd frontend && trunk serve
```

### Environment Variables

```bash
# AWS
AWS_PROFILE=ogrenotes-dev
AWS_REGION=us-east-1
DYNAMODB_TABLE_PREFIX=dev-<username>-
S3_BUCKET=dev-<username>-ogrenotes-data

# Redis
REDIS_URL=redis://localhost:6379

# Auth
OAUTH_CLIENT_ID=<from-provider>
OAUTH_CLIENT_SECRET=<from-provider>
OAUTH_REDIRECT_URI=http://localhost:8080/api/v1/auth/callback
JWT_SECRET=<random-dev-secret>

# Server
API_PORT=3000
FRONTEND_ORIGIN=http://localhost:8080
```

### Deployment (MVP)

MVP deployment is minimal -- single-instance, no orchestration:

| Component | MVP Deployment | Production (Phase 4+) |
|---|---|---|
| API server | Single EC2 instance or ECS task | Auto-scaling ECS service behind ALB |
| Frontend | Static files served by API or S3 + CloudFront | S3 + CloudFront CDN |
| DynamoDB | On-demand capacity, single table | On-demand or provisioned with auto-scaling |
| S3 | Single bucket, default encryption | Versioned bucket with lifecycle rules |
| Redis | Not used in MVP | ElastiCache cluster in private subnet |
| CI/CD | GitHub Actions: `cargo test --workspace` + `wasm-pack test` | Full pipeline: test, build, deploy to staging, smoke test, promote to prod |
| Monitoring | Structured tracing logs to stdout | CloudWatch Logs + Metrics, or Datadog/Grafana |
| Secrets | `.env` file (local dev) | AWS Secrets Manager or SSM Parameter Store |

MVP does not require a load balancer, auto-scaling, or multi-AZ setup. A single process handles all requests. The stateless API design (no in-memory document state in MVP) means horizontal scaling in Phase 2+ is straightforward -- add instances behind an ALB.

Production deployment design is deferred to Phase 4 (Enterprise). The key prerequisite is that Phase 2's WebSocket support requires sticky sessions or a shared room registry (via Redis), which changes the deployment model.

---

## Key Dependencies (Cargo.toml)

```toml
[workspace.dependencies]
# Web framework
axum = { version = "0.8", features = ["ws", "multipart"] }
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }

# AWS
aws-config = "1"
aws-sdk-dynamodb = "1"
aws-sdk-s3 = "1"
serde_dynamo = { version = "4", features = ["aws_sdk_dynamodb+1"] }

# CRDT
yrs = "0.21"

# Redis
fred = "9"

# Auth
jsonwebtoken = "9"
oauth2 = "4"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Errors
thiserror = "2"
anyhow = "1"

# Observability
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# IDs
nanoid = "0.4"

# Testing
axum-test = "16"

# Frontend (in frontend/Cargo.toml)
leptos = { version = "0.7", features = ["csr"] }
web-sys = { version = "0.3", features = [
    "HtmlElement", "HtmlDivElement", "Document", "Window",
    "Selection", "Range", "MutationObserver", "MutationRecord",
    "KeyboardEvent", "InputEvent", "CompositionEvent",
    "ClipboardEvent", "DataTransfer", "DragEvent",
    "Element", "Node", "Text", "NodeList",
] }
wasm-bindgen = "0.2"
js-sys = "0.3"
gloo = "0.11"
```

---

## Decisions That Affect Later Phases

| Decision | MVP Impact | Later Impact |
|----------|------------|--------------|
| **yrs from day one** | Slightly more complex than raw JSON persistence | Phase 2 multi-user sync works without data migration |
| **Single DynamoDB table** | More complex queries but simpler infra | No table-per-entity sprawl; GSI additions are non-breaking |
| **Access level on every folder membership** | Only `OWN` in MVP | Phase 2 sharing adds `EDIT`/`COMMENT`/`VIEW` rows without schema change |
| **Doc type field** | Only `document` in MVP | Phase 3 `spreadsheet` and Phase 2 `chat` use the same metadata pattern |
| **Presigned URLs for blobs** | Clients never touch S3 directly | Same pattern scales to any file size; no credential management |
| **JWT with minimal claims** | Simple auth | Phase 4 adds org/role claims; old JWTs still validate (missing claims = deny) |
| **CSS custom properties** | One theme | Phase 5 dark mode and document themes swap variables, no CSS rewrite |
| **Editor plugin system** | History + keymap + input rules | Phase 2 adds collab cursor, comments, decorations as plugins |
| **Command pattern** | Toolbar state works | Phase 2 command palette lists all registered commands automatically |
| **Binary content endpoint** | Simple save/load | Phase 2 WebSocket replaces save; GET stays for initial load + reconnect |
