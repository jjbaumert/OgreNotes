# OgreNotes -- High-Level Design

## What This Is

OgreNotes is a collaborative document, spreadsheet, and messaging platform built in Rust. It targets teams that need real-time editing, integrated chat, and enterprise-grade access controls -- without vendor lock-in.

This document synthesizes all `design/*.md` files into a single architectural overview. Each section references the detailed design document for deeper specification.

---

## Core Abstractions

### The Thread

Everything in OgreNotes is a **thread**. A thread is either:
- A **document** (rich text with embedded content)
- A **spreadsheet** (cells, formulas, charts)
- A **chat** (standalone messaging channel)

Every thread has an attached message stream. Documents and spreadsheets get a Comments pane; standalone chats are pure message streams. The same API endpoints serve all thread types.

### The Folder

Folders are **tags**, not directories. A thread can belong to multiple folders simultaneously. There is always one version of a thread regardless of which folder you access it from. Every user has system folders: Home, Archive, Pinned, Private, Trash.

### The User

Users belong to a workspace (the organization tenant -- see *The Workspace* below). Each user has a profile, system folders, notification preferences, and an affinity score per contact. Users can be human or bot.

### The Workspace

A **workspace** is the **tenant** -- the organization/company account that owns identity and enterprise policy. It is **not** a folder or a folder subtree (folders are a separate organization axis; see *The Folder*). SAML SSO, SCIM provisioning, and MFA enforcement are all scoped to the workspace (`WORKSPACE#` rows; see Data Model). Every user has a `default_workspace_id` -- solo users get a one-person *"Personal Workspace"* -- and every document inherits its owner's default workspace as `DocumentMeta.workspace_id`.

**Terminology:** a *workspace* is the organization/company tenant boundary. Company-wide link sharing -- "anyone in the company with the link" -- maps directly onto the workspace-member audience; "company" and "workspace" name the same tenant boundary.

**Assumption (one workspace per company).** OgreNotes treats the workspace as the *whole-company* tenant, so "anyone in the workspace" equals "anyone in the company." Modeling sub-company units (e.g. departments) as separate workspaces would make the workspace finer-grained than the whole company and would silently narrow the meaning of company-wide / link sharing. That is a deliberate divergence, not the assumed deployment; revisit the link-sharing and search-discoverability semantics if it is ever adopted.

> Details: [linksharing.md](linksharing.md)

---

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Clients                              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────────┐  │
│  │ Web App  │  │ Mac App  │  │ iOS App  │  │  API Users  │  │
│  │ (Leptos) │  │ (Tauri?) │  │ (native) │  │ (REST/WS)  │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └─────┬──────┘  │
│       └──────────────┴──────────────┴──────────────┘        │
└──────────────────────────┬──────────────────────────────────┘
                           │ HTTPS / WSS
┌──────────────────────────┴──────────────────────────────────┐
│                     API Server (Axum)                        │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌────────────┐  │
│  │   REST   │  │WebSocket │  │   Auth   │  │   Admin    │  │
│  │ Handlers │  │ Handlers │  │Middleware│  │    API     │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └─────┬──────┘  │
│       └──────────────┴──────────────┴──────────────┘        │
└──────┬──────────┬──────────────┬──────────────┬─────────────┘
       │          │              │              │
┌──────┴───┐ ┌────┴────┐  ┌─────┴─────┐  ┌────┴─────┐
│ DynamoDB │ │  Redis  │  │    S3     │  │  Search  │
│ (primary │ │(pubsub, │  │(snapshots,│  │ (index)  │
│  store)  │ │presence,│  │  blobs,   │  │          │
│          │ │  cache) │  │  exports) │  │          │
└──────────┘ └─────────┘  └───────────┘  └──────────┘
```

---

## Crate Architecture

```
ogrenotes/
├── crates/
│   ├── api/          # Axum HTTP + WebSocket server, request routing
│   ├── collab/       # yrs sync engine, room registry, session management
│   ├── documents/    # Document/spreadsheet schema, transforms, validation
│   ├── chat/         # Messaging, mentions, reactions, slash commands
│   ├── auth/         # OAuth 2.0, JWT, SAML SSO, MFA, session management
│   ├── storage/      # S3 presigned URLs, blob management, export generation
│   ├── search/       # Full-text indexing and query
│   ├── notify/       # Notifications, activity feed, email digest, push
│   └── common/       # Shared types, errors, tracing, config
├── frontend/         # Leptos (Rust/WASM) application
└── infra/
    ├── dynamodb/     # Table definitions, GSI configs
    ├── s3/           # Bucket policies, lifecycle rules
    └── docker-compose.yml  # Redis (local dev)
```

---

## Data Model

### DynamoDB Single-Table Design

```
PK                        SK                        Attributes
---------------------------------------------------------------------
USER#<user_id>            PROFILE                   name, email, avatar_url, created_at
USER#<user_id>            SESSION#<session_id>      refresh_token_hash, expires_at,
                                                    created_at, device_info
WORKSPACE#<ws_id>         METADATA                  name, owner_id, plan, created_at
WORKSPACE#<ws_id>         MEMBER#<user_id>          role, joined_at
DOC#<doc_id>              METADATA                  title, doc_type, workspace_id, owner_id,
                                                    created_at, updated_at,
                                                    snapshot_version, snapshot_s3_key
DOC#<doc_id>              SNAPSHOT#<version>         s3_key, size_bytes, crdt_clock  (Phase 2: version history)
DOC#<doc_id>              UPDATE#<clock>             update_bytes, user_id, created_at (Phase 2: CRDT op log)
DOC#<doc_id>              COLLAB#<session_id>        user_id, cursor_pos, last_seen  (Phase 2: presence)
THREAD#<thread_id>        METADATA                  doc_id, created_by, created_at
THREAD#<thread_id>        MSG#<timestamp>#<msg_id>   user_id, content, reactions
FOLDER#<folder_id>        METADATA                  title, color, parent_id, owner_id, folder_type,
                                                    created_at, updated_at,
                                                    inherit_mode (Phase 2: permission inheritance)
FOLDER#<folder_id>        CHILD#<thread_or_folder>   type (thread|folder)
FOLDER#<folder_id>        MEMBER#<user_id>           access_level
```

**GSIs:**

| GSI | PK | SK | Purpose | Phase |
|-----|----|----|---------|-------|
| `GSI1` | `owner_id` | `updated_at` | List user's documents, sorted by recent | MVP |
| `GSI2` | `parent_id` | `title` | List folder children, sorted alphabetically | MVP |
| `GSI3` | `workspace_id` | `updated_at` | List workspace docs/members | Phase 2 (when workspaces are introduced) |
| `GSI4` | `user_id` | `created_at` | List user's workspaces, sessions, activity | Phase 2 |
| `GSI5` | `doc_id` | `updated_at` | Activity feed per document | Phase 2 (notifications) |

MVP ships with GSI1 and GSI2 only. Later GSIs are additive -- DynamoDB supports adding GSIs without downtime. See [mvp-detailed-design.md](mvp-detailed-design.md) "GSI Migration Path" section for details.

### S3 Object Layout

```
ogrenotes-data/
├── workspaces/<ws_id>/
│   ├── docs/<doc_id>/
│   │   ├── snapshots/<version>.bin     # Serialized yrs Y.Doc
│   │   └── exports/<timestamp>.<fmt>   # DOCX, PDF, XLSX
│   └── attachments/<file_id>/<name>    # Uploaded files
├── avatars/<user_id>/<hash>.<ext>      # Profile images
└── backups/<date>/                     # DynamoDB exports
```

### Redis (Ephemeral State)

- Presence: `presence:<doc_id>` -> set of user_ids with TTL
- Cursors: `cursor:<doc_id>:<user_id>` -> position + color with TTL
- Pubsub: `doc:<doc_id>` channel for CRDT update fanout
- Rate limiting: `ratelimit:<user_id>` sliding window counters
- Session cache: `session:<session_id>` -> user context

---

## Real-Time Collaboration

### CRDT Engine

All document state is managed by **yrs** (Rust port of Yjs). The `collab` crate owns the per-document room abstraction:

1. **Room registry** -- `DashMap<DocId, Room>` mapping active documents to live rooms
2. **Client join** -- load latest S3 snapshot + pending DynamoDB ops, bootstrap yrs::Doc, register WebSocket
3. **Update routing** -- apply update to server-side yrs::Doc, persist to DynamoDB op log, fan out via Redis pubsub
4. **Awareness** -- cursor positions and presence via y-sync awareness protocol, broadcast via Redis
5. **Idle compaction** -- after 60s with no edits, snapshot full Y.Doc to S3, prune DynamoDB op log

### Edit Flow

```
Client A types
  -> WebSocket sends yrs update
    -> API server applies to server-side Y.Doc
      -> Raw bytes appended to DynamoDB op log
        -> Published to Redis pubsub channel
          -> Other API instances receive and fan out
            -> Client B's WebSocket receives update
              -> Client merges locally via CRDT
```

### Reconnect Flow

```
Client reconnects
  -> Sends state vector to server
    -> Server loads S3 snapshot + pending DynamoDB ops
      -> Computes diff (only what client is missing)
        -> Sends minimal binary update
          -> Client is in sync
```

**MVP note:** Phase 1 uses REST `PUT /documents/:id/content` for full-state saves (no WebSocket, no op log, no compaction). Phase 2 introduces WebSocket sync with a binary message protocol, `UPDATE#` op log writes, and idle compaction. The REST endpoint remains as a fallback. See [mvp-detailed-design.md](mvp-detailed-design.md) "Phase 2 Transition: REST Save to WebSocket Sync" for the full protocol spec.

> Details: [rich-text-editor.md](rich-text-editor.md)

---

## Document Editor

### Architecture Layers

| Layer | Responsibility | Reference |
|-------|---------------|-----------|
| **Model** | Immutable document tree, typed nodes, marks, schema validation | [rich-text-editor.md](rich-text-editor.md) §1-2 |
| **State** | Document + selection + plugin states, transactions | [rich-text-editor.md](rich-text-editor.md) §3 |
| **Transform** | Steps (insert, delete, mark, split, join, wrap, lift) with position mapping | [rich-text-editor.md](rich-text-editor.md) §3 |
| **View** | contenteditable rendering, DOM mutation observation, input handling, IME | [rich-text-editor.md](rich-text-editor.md) §10 |
| **Extensions** | Nodes, marks, and functionality plugins | [rich-text-editor.md](rich-text-editor.md) §12-15 |

**Schema duality:** The document schema (node types, mark types, content rules) is defined in two locations: the `collab` crate (server-side, for yrs tag mapping and export) and `frontend/editor` (client-side WASM, for editor validation and transforms). These compile to different targets and cannot share a single module. A cross-schema consistency test in the collab crate validates that both sides agree on node types, mark types, tag names, content rules, and mark exclusion. See [mvp-detailed-design.md](mvp-detailed-design.md) "Schema Duality" section.

### Editor Features

These features layer on top of the core editor engine:

| Feature | Description |
|---------|-------------|
| Block Menu | Per-paragraph floating menu (heading, list, insert options) |
| Command Palette | Searchable command palette (`Ctrl+Shift+J`) |
| `@` Menu | Universal inserter (people, documents, tables, images, embeds) |
| Comments | Inline (text selection) + document-level with threading |
| Edit History | Per-section diffs with author attribution, version restoration |
| Document Outline | Auto-generated ToC from headings |
| Typography Themes | 5 document-level font pairings |
| Embedded Spreadsheets | Inline spreadsheet NodeViews |
| Embeds | Interactive components (Kanban, Calendar, etc.) via NodeView |

> Details: [rich-text-editor.md](rich-text-editor.md), [branding.md](branding.md)

---

## Spreadsheet Engine

A custom formula evaluator supporting 400+ functions across categories: math/trig, statistical, text, logical, lookup/reference, date/time, financial, engineering, information, database, array/dynamic, matrix.

Key capabilities:
- Cell types: text, numbers, formulas, dates, @mentions, images, checkboxes, dropdowns
- Charts: pie, line, bar (live-updating from data)
- Sorting, filtering, conditional formatting, data validation (9 rule types)
- Cross-document data references (`REFERENCERANGE`, `REFERENCESHEET`)
- Table mode (hides row/column numbers for clean embedding)
- Import: Excel, CSV, OpenOffice. Export: Excel, PDF.

---

## Chat and Messaging

All threads share a unified message model:

| Type | Description |
|------|-------------|
| Standalone chat rooms | Named channels with member lists |
| 1:1 direct messages | Private two-person conversations |
| Document comments | Message stream attached to every document |
| Inline comments | Threaded conversations anchored to text selections |

Features: @mentions (people, documents, @everyone), emoji reactions, typing indicators, read receipts, slash commands (`/invite`, `/giphy`), file attachments, message forwarding, rich text formatting via parts array.

---

## Authentication and Authorization

### Authentication

| Method | Use Case |
|--------|----------|
| OAuth 2.0 (authorization code + PKCE) | Web/mobile app login |
| Personal access tokens | Developer testing, automation |
| SAML 2.0 SSO | Enterprise IdP integration (Okta, Azure AD, etc.) |
| MFA | Hardware keys, authenticator apps |

Two-token model: short-lived access tokens (JWT, 15 minutes) in the response body, and long-lived refresh tokens (30 days) in HttpOnly cookies. Sessions configurable per platform.

### Authorization

Four permission levels enforced on every API call and WebSocket message:

| Level | Numeric | Can Share | Can Edit | Can Comment | Can View |
|-------|---------|-----------|----------|-------------|----------|
| Full Access | 0 | Yes | Yes | Yes | Yes |
| Can Edit | 1 | No | Yes | Yes | Yes |
| Can Comment | 2 | No | No | Yes | Yes |
| Can View | 3 | No | No | No | Yes |

Permissions flow through folder inheritance (overridable via restricted folders). Link sharing adds company-wide or external access with granular controls.

**Implementation:** Authorization is enforced via permission-checking Axum extractors (`ViewableDoc`, `EditableDoc`, `OwnedDoc`, and equivalents for folders). MVP extractors degenerate to ownership checks; Phase 2 adds ACL row lookups (`FOLDER#/MEMBER#` rows) without changing route handler signatures. See [mvp-detailed-design.md](mvp-detailed-design.md) "Authorization Model" for the per-endpoint permission matrix and refactoring plan.

> Details: [security-concerns.md](security-concerns.md)

---

## Search

Full-text search across documents, spreadsheets, folders, people, chats, comments, edit history, and tasks.

| Feature | Detail |
|---------|--------|
| Typeahead | Results appear as you type |
| Ranking | Previously viewed, recency, title vs body, person-based relevance |
| Filters | Content type, author, date modified, recently opened |
| Operators | `from:`, `by:`, `mention:`, `in:` with autocomplete |
| In-document | `Ctrl+F` with unified results across body, history, comments, chat |

---

## Notifications

Two-tier model:

| Tier | Description |
|------|-------------|
| **Updates feed** (passive) | Browsable activity feed in sidebar; filter by Pinned, Unread, Private, DMs |
| **Push notifications** (active) | @mentions, comment replies, likes, shares, document opens |

Three per-document levels: all activity / direct responses only / muted. Configurable independently for desktop and mobile. Email: individual (capped at 25/day) + daily digest for inactive users. Unread tracking via blue dots with individual dismiss and bulk mark-as-read.

---

## Templates

Templates are regular documents with an `is_template` flag. The Template Library is a filtered view organized into galleries (personal, shared, company-wide).

Variable substitution at copy time via `[[variable_name]]` double-bracket syntax against a JSON values dictionary. Supports nested dot notation (`[[user.name]]`).

---

## Import and Export

| Direction | Formats |
|-----------|---------|
| **Import** | Word (.doc/.docx), Excel (.xls/.xlsx), CSV, OpenOffice, PDF (as images), Markdown, HTML |
| **Export** | DOCX, XLSX, PDF (sync + async), Markdown, HTML, LaTeX |
| **Bulk export** | Async API with delta support; 36,000 docs/hour rate limit |

API accepts HTML or Markdown for document creation (no binary file upload endpoint). PDF export limited to 40,000 cells for spreadsheets; charts excluded.

**Export format roadmap:**

| Format | Phase | Notes |
|--------|-------|-------|
| HTML | MVP | Implemented in `collab/export.rs` |
| Markdown | MVP | Implemented in `collab/export.rs` |
| DOCX | Phase 6 | Implemented (M-6.5) in `collab/export.rs` (`to_docx`) + `import_docx.rs` (`from_docx`), gated by the `docx` feature; async import via `POST /documents/import-job` + the worker |
| XLSX | Phase 3 | Implemented in `collab/export.rs` (gated by `xlsx` feature) |
| PDF | Phase 6 | Implemented (M-6.6) in `collab/export.rs` (`to_pdf`, printpdf) + `import_pdf.rs` (`from_pdf`, text-only via pdf-extract), gated by the `pdf` feature; async import via `POST /documents/import-job` + the worker. Plain-text-only, lossy by design |
| LaTeX | Phase 5 | Low priority |
| Bulk export | Phase 4 | Requires admin console + background workers |

---

## Admin Console

Enterprise administration capabilities:

| Area | Features |
|------|----------|
| **User management** | Invite, deactivate, reactivate, merge, transfer content, read-only mode |
| **Admin roles** | Super Admin + custom permission profiles |
| **Provisioning** | Manual invite, SAML SSO with JIT, SCIM v2.0 |
| **Branding** | Up to 30 corporate colors, document theme defaults |
| **Templates** | Company-wide gallery curation |
| **Security** | Session timeouts per platform, platform restrictions, token revocation |
| **Sharing policies** | Disable public links, external sharing allowlist, domain restrictions |
| **Compliance** | Admin action log, data hold policies, retention policies, quarantine |
| **Events API** | Real-time event stream for SIEM/CASB/audit |

---

## Security

| Layer | Approach |
|-------|----------|
| **Transport** | TLS everywhere (HTTP, WSS, cache, DB); HSTS with long max-age |
| **Authentication** | Short-lived tokens (15-60 min), HttpOnly cookies, PKCE, WebSocket single-use tokens |
| **Authorization** | Document-level ACLs on every request; workspace hierarchy enforcement |
| **Encryption** | AES-256 at rest (DynamoDB, S3); TLS in transit |
| **Input validation** | CRDT update validation before persist; HTML sanitization on render; file MIME + size limits |
| **Infrastructure** | IAM least privilege; S3 block public access; Redis/pubsub in private subnets |
| **Secrets** | Dedicated secrets manager; no secrets in source |
| **Audit** | Append-only logs for access, permissions, sharing events |

> Details: [security-concerns.md](security-concerns.md)

---

## Testing Strategy

### Approach

Three layers of testing, all in Rust:

| Layer | Tools | What It Covers |
|-------|-------|----------------|
| **Unit tests** | `cargo test`, `wasm-bindgen-test` | Pure logic in each crate; no I/O |
| **Property tests** | `proptest`, `test-strategy` | Invariants over randomized inputs |
| **Integration tests** | `axum-test`, `wiremock`, real AWS dev resources | API endpoints, auth flows, persistence roundtrips |

End-to-end browser tests are deferred to Phase 2 (when collaboration UI ships).

### Unit Tests by Crate

| Crate | Focus | Key Tests |
|-------|-------|-----------|
| **common** | ID generation, timestamps | Uniqueness, length, alphabet, microsecond precision, roundtrip |
| **auth** | JWT, OAuth, sessions | Token create/validate, expiry rejection, tampered token rejection, PKCE verification, refresh rotation, reuse detection |
| **collab** | Y.Doc operations, schema, export | Insert/delete/format text, schema content validation, mark exclusion, snapshot roundtrip, HTML/Markdown export (14 export cases) |
| **storage** | Model serialization | Struct <-> DynamoDB AttributeValue roundtrips via serde_dynamo, PK/SK format verification |
| **frontend/editor** | Document model, transforms, commands, input rules, selection, clipboard | Node/Fragment/Mark data structures, all step types (insert, delete, replace, add/remove mark, split, join, wrap, lift), command applicability checks, 17 markdown shortcuts, paste sanitization |

### Property Tests

Property tests validate invariants over randomized inputs using custom strategies for generating arbitrary documents, nodes, marks, positions, and steps.

**Document Model Properties:**

| Property | Description |
|----------|-------------|
| Schema-valid documents remain valid after any valid step | Step application preserves schema invariants |
| Step inversion is a roundtrip | `doc.apply(step).apply(step.invert(doc)) == doc` |
| Position mapping is monotonic | StepMap preserves position ordering |
| Text normalization is idempotent | Normalizing already-normalized text is a no-op |
| Mark sorting is idempotent | Sorting marks twice produces same order as once |
| Fragment size is consistent | Size equals sum of child sizes + 2 per non-leaf child |

**CRDT Properties:**

| Property | Description |
|----------|-------------|
| yrs bridge roundtrip | `editor model -> yrs -> editor model` produces identical document |
| Snapshot roundtrip | `serialize -> deserialize` produces identical Y.Doc state |
| Concurrent updates converge | Two Y.Docs with different update orderings reach identical state |
| Snapshot is deterministic | Same Y.Doc state always produces identical bytes |

**API Properties:**

| Property | Description |
|----------|-------------|
| Document CRUD roundtrip | For any valid title, create then get returns matching data |
| Content PUT/GET roundtrip | For any valid Y.Doc bytes, PUT then GET returns identical bytes |
| Deleted documents invisible | After delete, document never appears in folder listings |

### Integration Tests

Integration tests run against real AWS DynamoDB and S3 (dev resources) with per-run isolation via unique prefixes. Tests clean up after themselves.

| Scope | Tests | Key Scenarios |
|-------|-------|---------------|
| **Auth flow** | 12 | Login redirect, OAuth callback, token exchange, refresh rotation, logout, protected route access, expired/missing token rejection |
| **Document lifecycle** | 22 | CRUD, content roundtrip, soft delete, trash placement, export (HTML/Markdown), invalid input rejection |
| **Folder lifecycle** | 18 | System folder immutability, user folder CRUD, nesting, children management, sorting, deleted doc exclusion |
| **Blobs** | 6 | Presigned URL generation, upload/download roundtrip, URL expiry, document scoping |

### Test Infrastructure

```rust
// Shared test app builder for integration tests
struct TestApp {
    client: axum_test::TestServer,  // Axum test client
    dynamo: aws_sdk_dynamodb::Client,
    s3: aws_sdk_s3::Client,
    table_name: String,
    bucket: String,
    user_token: String,  // Pre-authenticated JWT
}
```

OAuth provider mocked via `wiremock` for auth flow tests. Each test uses a unique ID prefix to avoid collisions.

### Test Execution

```bash
cargo test --workspace                              # All unit tests
cargo test -p collab                                # Single crate
cd frontend && wasm-pack test --headless --chrome    # WASM editor tests
cargo test --test '*' -- --test-threads=1            # Integration tests
PROPTEST_CASES=1000 cargo test -- prop_              # Extended property tests
```

### Coverage Targets

| Crate | Target | Rationale |
|-------|--------|-----------|
| `common` | 95%+ | Small, pure functions |
| `auth` | 90%+ | Security-critical |
| `collab` | 85%+ | Document model correctness is foundational |
| `storage` | 80%+ | Serialization correctness; AWS paths tested in integration |
| `frontend/editor` | 85%+ | Editor model/transforms are core logic |
| `api` | 70%+ | Route wiring covered by integration tests |

> Details: [mvp-detailed-design.md](mvp-detailed-design.md) (full test inventory with every individual test case)

---

## Frontend Architecture

### Recommended: Pure Rust (Leptos + WASM)

The application shell (sidebar, toolbar, file browser, search, sharing dialogs) is built with **Leptos** components. The rich text editor is a "black box" component where Leptos provides the container but does not reconcile its children.

| Concern | Approach |
|---------|----------|
| Framework | Leptos with fine-grained reactivity |
| Editor core | Custom Rust document model + schema + transforms |
| Editor view | Rust via web-sys (contenteditable, MutationObserver, Selection API) or thin JS bridge |
| Collaboration | yrs (WASM bindings) with y-sync over WebSocket |
| Styling | CSS with 4px grid system, self-hosted fonts |
| Theming | 5 document themes + dark mode; CSS custom properties |

> Details: [rich-text-editor.md](rich-text-editor.md) §17, [branding.md](branding.md)

---

## API Design

### URL Conventions

| Resource | Pattern |
|----------|---------|
| Documents | `/d/{id}/{slug}` |
| Folders | `/f/{id}/{slug}` |
| Chats | `/c/{id}/{slug}` |
| User profiles | `/u/{username}` |
| API | `/api/v1/...` |

### Key API Endpoint Groups

| Group | Endpoints |
|-------|-----------|
| **Threads** | CRUD, search, copy, add/remove members, lock, link sharing |
| **Folders** | CRUD, add/remove members, link sharing settings |
| **Messages** | Create, list by thread |
| **Users** | Current user, contacts, update profile |
| **Blobs** | Upload, download (via presigned URLs) |
| **Export** | Sync DOCX/XLSX, async PDF, bulk export |
| **WebSocket** | Real-time updates connection |
| **Admin** | User management, events, quarantine, governance |
| **SCIM** | User/group provisioning (v2.0) |

### API Conventions

- All responses are JSON with **camelCase** keys
- Errors use a consistent shape: `{"error": "<code>", "message": "<human-readable>"}`
- List endpoints use cursor-based pagination: `{"items": [...], "nextCursor": "<opaque>"}`
- Document content endpoints use `application/octet-stream` (binary yrs state)
- Concurrency control via DynamoDB condition expressions (optimistic locking on `snapshot_version`)

> Details: [mvp-detailed-design.md](mvp-detailed-design.md) (Response Format, Pagination, Concurrency Control sections)

### Rate Limits

| Scope | Limit |
|-------|-------|
| Per user | 50 req/min, 750 req/hr |
| Per user (admin) | 100 req/min, 1,500 req/hr |
| Per organization | 600 req/min |
| Bulk export | 36,000 docs/hr |

Redis-backed fixed-window rate limiting ships on `/auth/login`,
`/auth/refresh`, `/search`, and `/sharing`; Phase 4 expands coverage
to the remaining mutation surfaces. Sliding-window counters are a
v2 carry-forward.

---

## Mobile

iOS-first (Android retired). Mobile-first design -- documents are responsive by default.

Key differences from desktop: gray formatting bar above keyboard (replaces block menu), custom spreadsheet keyboards (numeric, formula with autocomplete), tap-and-hold for cell selection, dedicated search operator buttons, Apple Handoff between devices.

Offline mode with full functionality (create, edit, comment, message). Seamless sync on reconnect with unsaved changes indicators.

---

## Presentations (Deferred)

A dedicated presentation/slides editor is deferred. OgreNotes may implement a simplified slide editor in the future.

---

## Build Phases

### Phase 1 -- Core Editor

- Document model, schema, transforms (Rust)
- Rich text editing with contenteditable view layer
- Basic formatting (headings, lists, bold/italic/underline/code/link)
- yrs integration for single-user persistence
- Axum API server with DynamoDB + S3
- OAuth 2.0 authentication
- Basic folder management (create, list, move)

### Phase 2 -- Collaboration

- **Workspace/organization entity** (`WORKSPACE#` rows, GSI3/GSI4, default workspace creation, migration of existing MVP docs into user's default workspace)
- Real-time multi-user editing via yrs + WebSocket
- Cursor presence and awareness
- Comments (inline + document-level)
- Chat (1:1, group, document-attached)
- Notifications (in-app + email)
- Sharing with permission levels
- Block menu (floating per-paragraph menu for heading, list, insert options)
- `@` menu (universal inserter: people, documents, tables, images, embeds)
- Document outline (auto-generated ToC from headings)
- Edit history (per-section diffs with author attribution, version restoration)
- Archive and Pinned system folders

### Phase 3 -- Spreadsheets

- Spreadsheet node type with embedded editing
- Formula engine (~456-function library)
- Cell formatting, sorting, filtering, frozen panes, named ranges
- Conditional formatting (color scales, data bars, icon sets)
- Charts (pie, line, bar)
- Pivot tables, format painter, cell-level comments
- Cross-document live references (REFERENCERANGE / REFERENCESHEET)
- Import/export — XLSX, CSV (DOCX and PDF moved to Phase 6)
- Engine-quality + lifecycle / autocomplete fixes — issues #3, #4, #22

### Phase 4 -- Enterprise

- Admin console
- SCIM provisioning
- SAML SSO
- MFA
- Templates with mail merge
- Audit logging and compliance
- Backup and recovery (scheduled DynamoDB exports to S3 `backups/<date>/`, point-in-time recovery)
- Automated trash cleanup (30-day soft-delete expiry via background worker)
- Redis-backed rate limiting — coverage expansion

> Full-text search (originally listed here) shipped in Phase 3 under
> `crates/search/` (Tantivy BM25 + hybrid mode); Redis-backed rate
> limiting shipped its first wave for `/auth/login`, `/auth/refresh`,
> `/search`, `/sharing` — Phase 4 expands coverage to admin
> mutations, comments, content writes, and WS upgrade.

### Phase 5 -- Polish (closed 2026-05-31)

- Mobile optimization — responsive web + PWA install + offline
  indicator. Native iOS/Android and full offline-CRDT are v2
  carry-forwards.
- Embeds / Live Apps platform — **v1 ships embed-host primitive
  only** (`Embed` block + sandboxed iframe + URL allowlist). Live
  Apps SDK, `.ele` packaging, developer console, and public
  marketplace are v2.
- Typography themes and dark mode
- Command palette
- Import/export — text formats (HTML + Markdown import, bulk-export
  endpoint). DOCX and PDF remain Phase 6 (need the async-worker
  subsystem).
- Bulk operations
- Accessibility (ARIA attributes, keyboard navigation, screen
  reader support, WCAG 2.1 AA)
- Internationalization (i18n harness + en-US + one RTL pilot;
  additional locales are v2)
- Performance budgets (page load time, time-to-interactive, API
  response time SLAs; frontend RUM; WASM bundle CI gate)

### Phase 5.5 -- External Integrations (forward note)

- **Slack** — OAuth, slash commands, outbound webhooks, Slack→
  OgreNote user resolution. Carried
  out of Phase 5 because it's a multi-week platform integration,
  not "polish".
- GitHub / Jenkins / external webhook receivers — wait for Slack
  patterns to land first.

### Phase 6 -- AI Assistant + Async Workers

The RAG / agentic-search track plus the long-tail formats deferred
from Phase 3. Internally split into four sub-phases (6.1–6.4); see
[rag-architecture-steering.md](rag-architecture-steering.md) and
[rag-implementation-plan.md](rag-implementation-plan.md) for the
detailed week-by-week plan.

- **6.1 Foundation** — BM25 keyword search via Tantivy (shipped)
- **6.2 Vector Embeddings + Semantic Search** — Bedrock Titan Embed + Qdrant; `semantic_search` tool (shipped)
- **6.3 Knowledge Graph + Agentic Layer** — DynamoDB adjacency list, Claude tool-use agent loop, `get_related` tool (shipped)
- **6.4 Validation & Tuning** — 50-query eval set, recall@10, per-query latency/cost (shipped; parameter tuning sweep deferred to v2)
- **Async-worker subsystem** — Redis-backed job queue used by 6.3 (long-running agent loops) and the deferred Phase 3 formats (shipped, M-6.4: `crates/worker`, `--mode=worker`, the `ogrenote-worker` ECS service)
- **DOCX / PDF import-export** — both shipped (DOCX M-6.5, PDF M-6.6); the Phase 6 deferred-format track is complete

> Phase 6 was originally numbered "Phase 3" inside the RAG docs; the
> renumbering to 6.1–6.4 (2026-05-05) is purely a project-roadmap
> alignment, not a scope change.

---

## Design Document Index

| Document | Scope |
|----------|-------|
| [rich-text-editor.md](rich-text-editor.md) | Editor engine internals (ProseMirror/TipTap reference) |
| [linksharing.md](linksharing.md) | OgreNotes link-sharing design: workspace-internal visibility toggle (no external/public access) |
| [live-app-blocks.md](live-app-blocks.md) | Internal plugin interface for native structured blocks (Calendar, Kanban, Project Tracker) |
| [security-concerns.md](security-concerns.md) | Security architecture across all layers |
| [branding.md](branding.md) | OgreNotes identity, colors, typography, voice |
| [mvp-detailed-design.md](mvp-detailed-design.md) | Phase 1 detailed design, API, data model, full test inventory |
