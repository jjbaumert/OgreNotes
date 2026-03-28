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

> Details: [quip-clean-room.md](quip-clean-room.md), [chat-messaging.md](chat-messaging.md)

### The Folder

Folders are **tags**, not directories. A thread can belong to multiple folders simultaneously. There is always one version of a thread regardless of which folder you access it from. Every user has system folders: Home, Archive, Pinned, Private, Trash.

> Details: [folder-file-management.md](folder-file-management.md)

### The User

Users belong to an organization. Each user has a profile, system folders, notification preferences, and an affinity score per contact. Users can be human or bot.

> Details: [admin-user-management.md](admin-user-management.md)

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

> Details: [technical-stack.md](technical-stack.md)

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

> Details: [technical-stack.md](technical-stack.md)

---

## Data Model

### DynamoDB Single-Table Design

```
PK                        SK                        Attributes
---------------------------------------------------------------------
USER#<user_id>            PROFILE                   name, email, avatar_url, created_at
USER#<user_id>            SESSION#<session_id>      expires_at, device_info
WORKSPACE#<ws_id>         METADATA                  name, owner_id, plan, created_at
WORKSPACE#<ws_id>         MEMBER#<user_id>          role, joined_at
DOC#<doc_id>              METADATA                  title, type, workspace_id, owner_id, created_at
DOC#<doc_id>              SNAPSHOT#<version>         s3_key, size_bytes, crdt_clock
DOC#<doc_id>              COLLAB#<session_id>        user_id, cursor_pos, last_seen
THREAD#<thread_id>        METADATA                  doc_id, created_by, created_at
THREAD#<thread_id>        MSG#<timestamp>#<msg_id>   user_id, content, reactions
FOLDER#<folder_id>        METADATA                  title, color, parent_id, inherit_mode
FOLDER#<folder_id>        CHILD#<thread_or_folder>   type (thread|folder)
FOLDER#<folder_id>        MEMBER#<user_id>           access_level
```

**GSIs:**
- `GSI1`: workspace_id -> docs/members
- `GSI2`: user_id -> workspaces/docs
- `GSI3`: doc_id + updated_at -> activity feed

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

> Details: [technical-stack.md](technical-stack.md)

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

> Details: [technical-stack.md](technical-stack.md), [rich-text-editor.md](rich-text-editor.md)

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

### Quip-Specific Editor Features

These features layer on top of the core editor engine:

| Feature | Description | Reference |
|---------|-------------|-----------|
| Block Menu | Per-paragraph floating menu (heading, list, insert options) | [editor-features.md](editor-features.md) |
| Command Palette | Searchable command palette (`Ctrl+Shift+J`) | [editor-features.md](editor-features.md) |
| `@` Menu | Universal inserter (people, documents, tables, images, embeds) | [editor-features.md](editor-features.md) |
| Comments | Inline (text selection) + document-level with threading | [editor-features.md](editor-features.md), [chat-messaging.md](chat-messaging.md) |
| Edit History | Per-section diffs with author attribution, version restoration | [editor-features.md](editor-features.md) |
| Document Outline | Auto-generated ToC from headings | [editor-features.md](editor-features.md) |
| Typography Themes | 5 document-level font pairings | [branding.md](branding.md) |
| Embedded Spreadsheets | Inline spreadsheet NodeViews | [spreadsheet-editor.md](spreadsheet-editor.md) |
| Embeds | Interactive components (Kanban, Calendar, etc.) via NodeView | [integrations.md](integrations.md) |

> Details: [rich-text-editor.md](rich-text-editor.md), [editor-features.md](editor-features.md)

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

> Details: [spreadsheet-editor.md](spreadsheet-editor.md)

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

> Details: [chat-messaging.md](chat-messaging.md)

---

## Authentication and Authorization

### Authentication

| Method | Use Case |
|--------|----------|
| OAuth 2.0 (authorization code + PKCE) | Web/mobile app login |
| Personal access tokens | Developer testing, automation |
| SAML 2.0 SSO | Enterprise IdP integration (Okta, Azure AD, etc.) |
| MFA | Hardware keys, authenticator apps |

Tokens expire every 30 days with refresh. Sessions configurable per platform.

### Authorization

Four permission levels enforced on every API call and WebSocket message:

| Level | Numeric | Can Share | Can Edit | Can Comment | Can View |
|-------|---------|-----------|----------|-------------|----------|
| Full Access | 0 | Yes | Yes | Yes | Yes |
| Can Edit | 1 | No | Yes | Yes | Yes |
| Can Comment | 2 | No | No | Yes | Yes |
| Can View | 3 | No | No | No | Yes |

Permissions flow through folder inheritance (overridable via restricted folders). Link sharing adds company-wide or external access with granular controls.

> Details: [authentication.md](authentication.md), [sharing-permissions.md](sharing-permissions.md), [security-concerns.md](security-concerns.md)

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

> Details: [search.md](search.md)

---

## Notifications

Two-tier model:

| Tier | Description |
|------|-------------|
| **Updates feed** (passive) | Browsable activity feed in sidebar; filter by Pinned, Unread, Private, DMs |
| **Push notifications** (active) | @mentions, comment replies, likes, shares, document opens |

Three per-document levels: all activity / direct responses only / muted. Configurable independently for desktop and mobile. Email: individual (capped at 25/day) + daily digest for inactive users. Unread tracking via blue dots with individual dismiss and bulk mark-as-read.

> Details: [notifications.md](notifications.md)

---

## Templates

Templates are regular documents with an `is_template` flag. The Template Library is a filtered view organized into galleries (personal, shared, company-wide).

Variable substitution at copy time via `[[variable_name]]` double-bracket syntax against a JSON values dictionary. Supports nested dot notation (`[[user.name]]`).

> Details: [templates.md](templates.md)

---

## Import and Export

| Direction | Formats |
|-----------|---------|
| **Import** | Word (.doc/.docx), Excel (.xls/.xlsx), CSV, OpenOffice, PDF (as images), Markdown, HTML |
| **Export** | DOCX, XLSX, PDF (sync + async), Markdown, HTML, LaTeX |
| **Bulk export** | Async API with delta support; 36,000 docs/hour rate limit |

API accepts HTML or Markdown for document creation (no binary file upload endpoint). PDF export limited to 40,000 cells for spreadsheets; charts excluded.

> Details: [import-export.md](import-export.md)

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

> Details: [admin-user-management.md](admin-user-management.md)

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

> Details: [rich-text-editor.md](rich-text-editor.md) §17, [technical-stack.md](technical-stack.md), [branding.md](branding.md)

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

### Rate Limits

| Scope | Limit |
|-------|-------|
| Per user | 50 req/min, 750 req/hr |
| Per user (admin) | 100 req/min, 1,500 req/hr |
| Per organization | 600 req/min |
| Bulk export | 36,000 docs/hr |

> Details: [quip-clean-room.md](quip-clean-room.md)

---

## Mobile

iOS-first (Android retired). Mobile-first design -- documents are responsive by default.

Key differences from desktop: gray formatting bar above keyboard (replaces block menu), custom spreadsheet keyboards (numeric, formula with autocomplete), tap-and-hold for cell selection, dedicated search operator buttons, Apple Handoff between devices.

Offline mode with full functionality (create, edit, comment, message). Seamless sync on reconnect with unsaved changes indicators.

> Details: [mobile.md](mobile.md)

---

## Presentations (Deferred)

Quip Slides was retired January 2021. OgreNotes may implement a simplified slide editor in the future. The feature set is documented for reference.

> Details: [presentation-editor.md](presentation-editor.md)

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

- Real-time multi-user editing via yrs + WebSocket
- Cursor presence and awareness
- Comments (inline + document-level)
- Chat (1:1, group, document-attached)
- Notifications (in-app + email)
- Sharing with permission levels

### Phase 3 -- Spreadsheets

- Spreadsheet node type with embedded editing
- Formula engine (start with core functions, expand incrementally)
- Cell formatting, sorting, filtering
- Charts (pie, line, bar)
- Import/export (Excel, CSV)

### Phase 4 -- Enterprise

- Admin console
- SCIM provisioning
- SAML SSO
- MFA
- Search (full-text indexing)
- Templates with mail merge
- Audit logging and compliance

### Phase 5 -- Polish

- Mobile optimization
- Embeds / Live Apps platform
- Integrations (Slack, external services)
- Typography themes and dark mode
- Command palette
- Import/export for all formats
- Bulk operations

---

## Design Document Index

| Document | Scope |
|----------|-------|
| [quip-clean-room.md](quip-clean-room.md) | API reference and data model overview |
| [technical-stack.md](technical-stack.md) | Technology choices, crate architecture, infrastructure |
| [rich-text-editor.md](rich-text-editor.md) | Editor engine internals (ProseMirror/TipTap reference) |
| [editor-features.md](editor-features.md) | Application-level editor UI and features |
| [spreadsheet-editor.md](spreadsheet-editor.md) | Spreadsheet engine, 400+ functions, UI |
| [presentation-editor.md](presentation-editor.md) | Slides editor (retired, deferred) |
| [chat-messaging.md](chat-messaging.md) | Chat, messaging, comments, reactions |
| [folder-file-management.md](folder-file-management.md) | Folder model, file browser, organization |
| [search.md](search.md) | Full-text search, ranking, filters |
| [notifications.md](notifications.md) | Activity feed, push, email, unread tracking |
| [sharing-permissions.md](sharing-permissions.md) | Permission levels, link sharing, external access |
| [authentication.md](authentication.md) | OAuth 2.0, SSO, MFA, sessions |
| [admin-user-management.md](admin-user-management.md) | Admin console, user provisioning, SCIM, compliance |
| [import-export.md](import-export.md) | Format support, sync/async export, bulk operations |
| [templates.md](templates.md) | Template library, mail merge, automation |
| [integrations.md](integrations.md) | Salesforce, Slack, Live Apps, webhooks |
| [mobile.md](mobile.md) | iOS experience, offline mode, custom keyboards |
| [security-concerns.md](security-concerns.md) | Security architecture across all layers |
| [branding.md](branding.md) | OgreNotes identity, colors, typography, voice |
| [mvp-detailed-design.md](mvp-detailed-design.md) | Phase 1 detailed design, API, data model, full test inventory |
