# Clean-Room Quip — Technical Stack Recommendation

Quip has several distinct subsystems: **real-time collaborative editing**, **structured documents/spreadsheets**, **chat/threads**, **auth/identity**, and a **rich frontend**. Below is the full recommended stack with DynamoDB for key-value persistence and S3 for file/object storage.

---

## Backend (Rust)

| Concern | Recommendation | Notes |
|---|---|---|
| HTTP / REST | **Axum** | Tower middleware ecosystem, integrates cleanly with Tokio |
| Async runtime | **Tokio** | Battle-tested, async-native |
| WebSocket (real-time) | **Axum + tokio-tungstenite** | Built into Axum's upgrade path |
| Auth | **axum-login** + **oauth2** crate | JWT via `jsonwebtoken`; OIDC for SSO |
| Serialization | **serde / serde_json** | camelCase via `#[serde(rename_all = "camelCase")]` |
| Error handling | **thiserror / anyhow** | Typed errors at crate boundaries, anyhow for application layer |
| Observability | **tracing + tracing-subscriber** | Structured logging, spans for async tasks |

---

## Persistence Layer

### DynamoDB (Primary Key-Value Store)

Use DynamoDB for all entities that fit a key-value or single-table design: users, sessions, documents, workspaces, threads, messages, permissions, and presence metadata.

| Concern | Recommendation | Notes |
|---|---|---|
| AWS SDK | **`aws-sdk-dynamodb`** (aws-sdk-rust) | Official, async-native, Tokio-compatible |
| Serialization bridge | **`serde_dynamo`** | Converts Rust structs ↔ DynamoDB `AttributeValue` maps transparently |
| Connection management | SDK-native client (no pool needed) | DynamoDB is HTTP-based; the SDK manages connection reuse internally |

**Single-Table Design sketch:**

```
PK                        SK                        Attributes
---------------------------------------------------------------------
USER#<user_id>            PROFILE                   name, email, avatar_url, created_at
USER#<user_id>            SESSION#<session_id>      expires_at, device_info
WORKSPACE#<ws_id>         METADATA                  name, owner_id, plan, created_at
WORKSPACE#<ws_id>         MEMBER#<user_id>          role, joined_at
DOC#<doc_id>              METADATA                  title, workspace_id, owner_id, created_at, updated_at
DOC#<doc_id>              SNAPSHOT#<version>        s3_key, size_bytes, crdt_clock
DOC#<doc_id>              COLLAB#<session_id>       user_id, cursor_pos, last_seen
THREAD#<thread_id>        METADATA                  doc_id, created_by, created_at
THREAD#<thread_id>        MSG#<timestamp>#<msg_id>  user_id, content, reactions
```

**GSI recommendations:**
- `GSI1`: `workspace_id` → list all docs/members in a workspace
- `GSI2`: `user_id` → list all workspaces/docs a user has access to
- `GSI3`: `doc_id` + `updated_at` → recent activity feed per document

### Redis (Ephemeral / Real-Time State)

Redis handles data that is high-frequency, short-lived, and should never touch DynamoDB:

| Concern | Recommendation | Notes |
|---|---|---|
| Client crate | **`fred`** | Async-native, connection pooling, Cluster support |
| Use cases | Presence, live cursors, pubsub fanout, rate limiting, session cache | TTL-based auto-expiry for presence entries |

> **Decision rule:** If data can be lost on a Redis restart without user-visible data loss, it belongs in Redis. Everything durable belongs in DynamoDB.

---

## Object Storage (S3)

Use S3 for all binary/large-object storage: document CRDT snapshots, file attachments, images, exports, and backups.

| Concern | Recommendation | Notes |
|---|---|---|
| AWS SDK | **`aws-sdk-s3`** (aws-sdk-rust) | Official, async-native |
| Multipart uploads | SDK-native multipart API | For files > 5 MB |
| Presigned URLs | SDK-native presign API | Issue short-lived GET/PUT URLs for client-direct uploads |
| Abstraction crate | **`object_store`** | Useful if you want to swap S3 for MinIO in local dev |

**S3 Bucket / Key layout:**

```
quip-clone-data/
├── workspaces/<workspace_id>/
│   ├── docs/<doc_id>/
│   │   ├── snapshots/<version>.bin        # Serialized yrs Y.Doc state
│   │   └── exports/<timestamp>.<format>   # DOCX, PDF, XLSX exports
│   └── attachments/<file_id>/<filename>   # User-uploaded files
├── avatars/<user_id>/<hash>.<ext>         # Profile images
└── backups/<date>/                        # DynamoDB export backups
```

**Access pattern:**
- Clients **never** get raw AWS credentials. Issue short-lived presigned PUT URLs from the API server for uploads; presigned GET URLs for downloads.
- Set S3 bucket policy to block all public access; all access flows through presigned URLs or a CDN origin.

---

## Real-Time Collaboration Engine

The core technical challenge. Use **`yrs`** (Rust port of Yjs) for all shared document state.

| Concern | Recommendation | Notes |
|---|---|---|
| CRDT library | **`yrs`** | Y.Doc, Y.Text, Y.Map, Y.Array — enough for docs and spreadsheets |
| Sync protocol | **y-sync over WebSocket** | Built into `yrs`; handles awareness (cursors, presence) |
| Persistence strategy | Snapshot Y.Doc state periodically → S3; store snapshot reference (s3_key, version) → DynamoDB | |
| Op log durability | Append raw Yjs update bytes to DynamoDB `DOC#<doc_id> / UPDATE#<clock>` during active sessions; compact into S3 snapshot on idle | |
| Client sync | Frontend uses **Yjs (JS)** with `y-websocket` provider; protocol is binary-compatible with `yrs` | |

---

## WebSocket & Live Collaboration Flow

### Connection Layer

| Concern | Recommendation | Notes |
|---|---|---|
| WebSocket upgrade | **Axum `ws` feature** | Handled via `axum::extract::ws::WebSocketUpgrade` |
| Async WS I/O | **tokio-tungstenite** | Framing, ping/pong, backpressure |
| Sync protocol | **y-sync over WebSocket** | Built into `yrs`; binary, compact, CRDT-native |
| Frontend provider | **`y-websocket`** (Yjs JS) | Binary-compatible with `yrs` server-side; handles reconnect/backoff |

### The `collab` Crate — Session Management

This is the piece that needs the most careful upfront design. The `collab` crate owns the **per-document room abstraction**:

- **Room registry** — an in-process `DashMap<DocId, Room>` (or Tokio `RwLock<HashMap>`) mapping each active document to its room. A room holds the live `yrs::Doc` and the set of currently connected WebSocket handles.
- **Client join/leave** — on WebSocket connect, look up or create the room for the requested `doc_id`; load the latest Y.Doc state from S3 + any pending DynamoDB update log entries to bootstrap it; register the connection.
- **Update routing** — when a client sends a Yjs update, apply it to the server-side `yrs::Doc`, persist the raw bytes to DynamoDB, then fan out to all other connections in the room via Redis pubsub (so updates reach clients connected to other API server instances).
- **Awareness (cursors/presence)** — cursor positions and "who's online" are handled by the y-sync awareness protocol; broadcast via Redis, never written to DynamoDB.
- **Idle compaction** — after a configurable idle window (e.g. 60s with no edits), serialize the full `yrs::Doc` state and write it as a new snapshot to S3; record the S3 key + version in DynamoDB; prune the now-redundant update log entries.

### End-to-End Edit Flow

```
Client A types
  → y-websocket sends binary Yjs update over WebSocket
    → Axum WS handler receives update
      → collab crate applies update to server-side yrs::Doc
        → Raw update bytes appended to DynamoDB (op log)
          → Update published to Redis pubsub channel for doc_id
            → All other API instances subscribed to that channel receive it
              → Fan out to each connected client's WebSocket
                → Client B's y-websocket receives update, Yjs merges locally
```

### Reconnect / Bootstrap Flow

```
Client reconnects after being offline
  → Sends its current Yjs state vector to the server
    → collab crate loads latest S3 snapshot + pending DynamoDB updates
      → Computes diff (only what the client is missing)
        → Sends minimal binary update over WebSocket
          → Client is back in sync
```

---

## Document & Spreadsheet Engine

| Concern | Recommendation |
|---|---|
| Document model | CRDT `Y.Text` + `Y.Map` via `yrs`, wrapped in your own schema |
| Rich text parsing | **`pulldown-cmark`** as baseline; custom AST for Quip-specific block types |
| Spreadsheet formula engine | Custom evaluator or WASM-embedded engine; no mature pure-Rust option yet |
| Export (DOCX/XLSX/PDF) | `docx-rs`, `rust_xlsxwriter`, `printpdf`; upload result to S3, return presigned GET URL |

---

## Frontend

| Concern | Recommendation | Notes |
|---|---|---|
| Framework | **React + TypeScript** | Pragmatic; rich-text ecosystem is mature here |
| Rich text editor | **Tiptap** (ProseMirror-based) | First-class Yjs collaboration extension (`@tiptap/extension-collaboration`) |
| Collaboration state | **Yjs (JS)** + `y-websocket` provider | Binary protocol is compatible with `yrs` on the backend |
| API client | **`fetch`** + **TanStack Query** | REST for CRUD, WebSocket for real-time |
| Auth | **`oidc-client-ts`** | Handles PKCE flow, token refresh |

> **Alternative:** If you want a pure-Rust frontend, **Leptos** compiles to WASM and `yrs` has WASM bindings. The tradeoff is a less mature rich-text editing story — plan extra time.

---

## Workspace Layout

```
quip-clone/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── api/                    # Axum HTTP + WS server
│   ├── collab/                 # yrs sync engine, session management
│   ├── documents/              # doc/spreadsheet schema + ops
│   ├── chat/                   # threads, mentions, reactions
│   ├── auth/                   # JWT, OAuth2, RBAC
│   ├── storage/                # S3 + presigned URL helpers
│   └── common/                 # shared types, errors, tracing
├── frontend/                   # React/TypeScript (or Leptos)
└── infra/
    ├── dynamodb/               # table definitions, GSI configs
    ├── s3/                     # bucket policies, lifecycle rules
    ├── migrations/             # any relational migrations (if added later)
    └── docker-compose.yml      # local: Redis
```

---

## Key Cargo Dependencies

```toml
[dependencies]
# Web framework
axum               = { version = "0.8", features = ["ws", "multipart"] }
tokio              = { version = "1", features = ["full"] }
tower              = "0.5"
tower-http         = { version = "0.6", features = ["cors", "trace"] }

# AWS
aws-config         = "1"
aws-sdk-dynamodb   = "1"
aws-sdk-s3         = "1"
serde_dynamo       = { version = "4", features = ["aws_sdk_dynamodb+1"] }

# Collaboration
yrs                = "0.21"

# Redis
fred               = "9"

# Serialization & errors
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
thiserror          = "2"
anyhow             = "1"

# Auth
jsonwebtoken       = "9"
oauth2             = "4"

# Observability
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# Testing
proptest           = "1"
```

---

## Local Development

Use **actual AWS DynamoDB and S3** for local development. This avoids behavioral differences between emulators and production services. Redis runs locally via Docker.

### AWS Setup

1. Create a dedicated AWS account or use a dev-specific IAM user/role with scoped permissions.
2. Create dev-specific resources with a naming convention to isolate them:
   - DynamoDB tables prefixed with `dev-<username>-` (e.g., `dev-alice-quip-clone`)
   - S3 bucket named `dev-<username>-quip-clone-data`
3. Configure credentials via AWS CLI profiles or environment variables.

```bash
# Option A: Named profile (recommended)
aws configure --profile quip-dev
# Then set in .env:
AWS_PROFILE=quip-dev
AWS_REGION=us-east-1

# Option B: Environment variables
AWS_ACCESS_KEY_ID=<your-dev-key>
AWS_SECRET_ACCESS_KEY=<your-dev-secret>
AWS_REGION=us-east-1
```

The `aws-config` crate picks up credentials automatically from the standard AWS credential chain (env vars, `~/.aws/credentials`, IAM role, etc.).

### Docker (Redis Only)

```yaml
# docker-compose.yml
services:
  redis:
    image: redis:7-alpine
    ports: ["6379:6379"]
```

### Environment Configuration

Use a `.env` file (git-ignored) for local settings:

```bash
# .env
AWS_PROFILE=quip-dev
AWS_REGION=us-east-1
DYNAMODB_TABLE_PREFIX=dev-alice-
S3_BUCKET=dev-alice-quip-clone-data
REDIS_URL=redis://localhost:6379
```

The application should read `DYNAMODB_TABLE_PREFIX` and prepend it to all table names, allowing multiple developers to share one AWS account without collisions. Similarly, `S3_BUCKET` should be configurable per environment.

### Cost Control

- Use **on-demand** (pay-per-request) billing mode for DynamoDB dev tables to avoid provisioned capacity charges during idle periods.
- Set **S3 lifecycle rules** on the dev bucket to expire objects after 30 days.
- Use **AWS Budgets** to set a spending alert on the dev account.
- Tear down dev resources with a script when not actively developing.

---

## Key Design Risks

| Risk | Mitigation |
|---|---|
| DynamoDB hot partitions on busy docs | Use `DOC#<doc_id>` as PK; spread update writes across sort keys by clock/timestamp |
| CRDT op log growing unbounded | Compact into S3 snapshot on doc idle (e.g., 60s no edits); prune old update rows from DynamoDB |
| Presence/cursor latency | Keep entirely in Redis pubsub; never touch DynamoDB for ephemeral state |
| Spreadsheet formula engine | Scope down for v1; implement a basic evaluator and expand incrementally |
| Offline sync / reconnect | `yrs` handles CRDT merge on reconnect; ensure the API serves the latest S3 snapshot + pending DynamoDB updates to bootstrap a reconnecting client |
| S3 presigned URL expiry | Issue short TTLs (15 min) for uploads, longer (1–4 hr) for reads; refresh via API on expiry |
