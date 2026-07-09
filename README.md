# OgreNotes

A collaborative document editor built in Rust. Real-time editing via CRDTs (yrs), rich text and spreadsheet support, full-text and semantic search, and an AI-powered document assistant.

## Architecture

```
frontend/          Leptos 0.7 (Rust/WASM) — rich text editor, spreadsheet, file browser
crates/
  api/             Axum HTTP + WebSocket server, route handlers, Claude API client
  collab/          CRDT engine (yrs), real-time sync, Redis pub/sub, document export
  search/          Tantivy BM25 full-text search index
  embeddings/      Vector embedding pipeline (Bedrock Titan + Qdrant)
  storage/         DynamoDB + S3 repositories, data models
  auth/            OAuth2, JWT, session management
  notify/          Email notification delivery (preferences, templates, background send)
  worker/          Async job queue (Redis streams) — background imports & long-running jobs
  common/          Shared config, ID generation, time utilities
```

**Infrastructure:** DynamoDB (single-table), S3 (snapshots/blobs), Redis (pub/sub + presence), Tantivy (in-process search), Qdrant (vector search, optional), Claude API (AI assistant, optional).

## What's Built

### Core Platform
- Rich text editor with collaborative editing (yrs CRDT, WebSocket sync)
- Spreadsheet editor with formula engine and charting
- Document, spreadsheet, and chat types
- Folder hierarchy with system folders (Home, Trash)
- OAuth2 authentication (GitHub + Google) with JWT sessions
- Document sharing with access levels (Own, Edit, Comment, View)
- Folder sharing with inheritance (including Restricted mode)
- Link sharing for workspace members
- Comments, threads, @mentions
- Document version history with diff and restore
- Notifications (document opened, comments, sharing)
- Workspaces with member management
- Import/export (HTML, Markdown, CSV, XLSX)
- Blob upload/download with presigned S3 URLs
- Document templates with placeholder / mail-merge fill (basic placeholder substitution)
- Third-party embeds (static provider allowlist, sandboxed iframe render)
- Kanban board and calendar views

### Enterprise
- MFA / TOTP enrollment and challenge
- SAML SSO (per-workspace configuration)
- SCIM user provisioning (token-authenticated)
- Audit logging — admin-mutation and security-event trails, with a retention worker
- Admin console: user management (list, disable/enable, promote/demote) with `ADMIN_EMAILS` bootstrap, plus audit-log and metrics views

### Search (Phase 1 + 2)
- BM25 keyword search via Tantivy (title 2x boost, body, snippets)
- Vector semantic search via Bedrock Titan Embed + Qdrant
- Hybrid search mode with Reciprocal Rank Fusion
- Permission-filtered results (post-query check_doc_access)
- Frontend search dialog (Ctrl+K)

### AI Assistant (Phase 6)
- `POST /api/v1/ask` with SSE streaming responses
- Claude tool-use agent loop (max 5 rounds)
- Tools: keyword_search, semantic_search, get_document, get_related
- Document relationship graph (DynamoDB adjacency list)
- Relationship types: implements, derived-from, depends-on, references, supersedes
- Prompt injection mitigation (data boundary markers, system prompt guardrails)

### Frontend
- Leptos 0.7 WASM SPA
- Rich text editing with block menu, selection toolbar, formatting toolbar
- Spreadsheet grid with formula bar and chart rendering
- Desktop-style menu bar (Document, Edit, View, Insert, Format)
- Collaborative cursors and presence
- Comment highlights and conversation pane
- Document outline (table of contents)
- File browser with sorting
- Search dialog (Ctrl+K) with debounced hybrid search
- AI assistant dialog with streaming (SSE) responses
- Document relationship panel (add/remove/typeahead)
- In-document find and replace
- Template picker
- Admin console pages (users, audit, metrics)
- Responsive mobile layout with touch gestures and drawer navigation
- Share dialog
- Notification bell

## What's Not Built Yet
- Contextual chunk enrichment (LLM-generated context headers at embedding time) — the chunker currently prepends a static title header only
- Backfill script for re-embedding existing documents — imported docs aren't indexed into Qdrant until first edited (a v1 limitation)
- Per-workspace embed domain allowlist — embeds ship with a static provider allowlist; per-workspace configuration is stubbed

## Local Development

### Prerequisites

- Rust (edition 2024)
- [Trunk](https://trunkrs.dev/) for frontend builds
- Docker for local services

### Start Local Services

`docker-compose.yml` provides the full local backend — DynamoDB Local
(`:8000`), MinIO (`:9000`, S3-compatible), Redis (`:6379`), and MailHog
(`:1025` SMTP / `:8025` web UI):

```bash
docker compose up -d
```

For vector/semantic search, also run Qdrant (optional):

```bash
docker run -d --name qdrant -p 6333:6333 -p 6334:6334 qdrant/qdrant
```

### Configure Environment

All local env vars live in `scripts/local-dev.env`. Source it with `set -a`
so the values are **exported** to the child process — a plain `source`
leaves them unexported and the server silently falls back to real AWS and
placeholder OAuth:

```bash
set -a && source scripts/local-dev.env && set +a
```

What that file bakes in (and why it matters):

- **AWS creds are `minioadmin/minioadmin`.** MinIO validates credentials and
  rejects anything else (`InvalidAccessKeyId`); DynamoDB Local ignores them,
  so the same pair works for both.
- **`AWS_ENDPOINT_URL_DYNAMODB` / `AWS_ENDPOINT_URL_S3`** point the AWS SDK at
  the local containers. Without them every call goes to real AWS.
- **`DEV_MODE=true`** enables the `/auth/dev-login` shortcut — log in locally
  with any email, no real OAuth needed.

Edit the file to enable optional Google OAuth, Qdrant, or the AI assistant
(Anthropic) — see its inline comments.

### Initialize and Run

```bash
cargo run --bin setup_dev   # first run (and after `docker compose down`) — creates the table + bucket
cargo run                   # starts the API on :3000
```

### Start the Frontend

```bash
cd frontend
trunk serve
```

Opens at `http://localhost:8080`. API calls proxy to `localhost:3000`.

### Run Tests

```bash
# Unit tests (no Docker needed)
cargo test --workspace --lib

# Integration tests (requires Docker services running)
cargo test --workspace

# Frontend tests
cargo test --bin ogrenotes-frontend --target x86_64-unknown-linux-gnu \
  --manifest-path frontend/Cargo.toml
```

## Environment Variables

### Required

| Variable | Description |
|----------|-------------|
| `DYNAMODB_TABLE_PREFIX` | Prefix for the DynamoDB table name (e.g., `dev-` creates `dev-ogrenotes`) |
| `S3_BUCKET` | S3 bucket for document snapshots and blob storage |
| `OAUTH_CLIENT_ID` | GitHub OAuth application client ID |
| `OAUTH_CLIENT_SECRET` | GitHub OAuth application client secret |
| `OAUTH_REDIRECT_URI` | GitHub OAuth callback URL (e.g., `https://your-domain/api/v1/auth/callback`) |
| `JWT_SECRET` | Secret for signing JWTs (min 32 bytes) |

### Google OAuth (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `GOOGLE_CLIENT_ID` | *(disabled)* | Google OAuth client ID (from [Google Cloud Console](https://console.cloud.google.com/apis/credentials)). When not set, the Google login button returns an error. |
| `GOOGLE_CLIENT_SECRET` | *(disabled)* | Google OAuth client secret |

Google OAuth callback URL: `https://your-domain/api/v1/auth/callback/google` (set this in the Google Cloud Console as an authorized redirect URI).

Both GitHub and Google login resolve to the same user if the email matches — a user who logs in with GitHub and later with Google (same email) will access the same account.

### Optional

| Variable | Default | Description |
|----------|---------|-------------|
| `AWS_REGION` | `us-east-1` | AWS region |
| `REDIS_URL` | `redis://localhost:6379` | Redis connection URL |
| `API_PORT` | `3000` | HTTP server port |
| `FRONTEND_ORIGIN` | `http://localhost:8080` | CORS origin for the frontend |
| `SEARCH_INDEX_PATH` | `/tmp/ogrenotes-search-index` | Path for the Tantivy search index on disk |
| `DEV_MODE` | `false` | Enable dev-only endpoints (dev-login). **Must be false in production.** |
| `EMBED_YOUTUBE_NOCOOKIE` | `true` | Rewrite YouTube embeds to `youtube-nocookie.com` (privacy-enhanced — no cookies until the viewer plays). Set `false` for the standard `youtube.com` host. |

### Vector Search (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `QDRANT_URL` | *(disabled)* | Qdrant gRPC URL (e.g., `http://localhost:6334`). When not set, search uses keyword-only mode. |
| `EMBEDDING_MODEL_ID` | `amazon.titan-embed-text-v2:0` | Bedrock embedding model ID |
| `EMBEDDING_DIMENSIONS` | `1024` | Embedding vector dimensions |

When `QDRANT_URL` is set, the server connects to Qdrant on startup, creates an `ogrenotes` collection, and begins embedding documents on create/update. Search defaults to hybrid mode (BM25 + vector). Requires valid AWS credentials with `bedrock:InvokeModel` permission.

### AI Assistant (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `ANTHROPIC_API_KEY` | *(disabled)* | Anthropic API key for Claude. When not set, `POST /api/v1/ask` returns 503. |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | Claude model to use for the agentic query loop |

### Admin (Optional)

| Variable | Default | Description |
|----------|---------|-------------|
| `ADMIN_EMAILS` | *(empty)* | Comma-separated emails that are auto-promoted to admin on login (e.g., `alice@example.com,bob@example.com`). Takes effect on the user's next login. |

When a user whose email matches `ADMIN_EMAILS` logs in, their account is automatically granted admin privileges. Admins can then manage other users via the `/api/v1/admin/*` endpoints — list users, disable/enable accounts, and promote/demote other admins. Disabled users are blocked from logging in and have all active sessions revoked.

## AWS Deployment

See `design/aws.md` for the full deployment architecture. Minimal starting point:

| Component | Spec |
|-----------|------|
| Compute | 2 ECS Fargate tasks (512MB / 0.25 vCPU each) |
| Load Balancer | ALB with sticky sessions (WebSocket support) |
| Cache | ElastiCache Redis cache.t3.micro, single shard |
| Database | DynamoDB on-demand, single table, PITR enabled |
| Storage | S3 bucket with Intelligent-Tiering |
| Vector Search | Qdrant on ECS Fargate (optional, ~$15/month) |
| Networking | VPC with public/private subnets, VPC endpoints for DynamoDB/S3 |

Estimated cost at zero traffic: ~$50-80/month base + ~$50/month for search/AI features.

## API Endpoints

| Path | Description |
|------|-------------|
| `/api/v1/auth` | OAuth login (`/login/{github\|google}`), callbacks, token refresh, logout, dev-login |
| `/api/v1/users` | Current user profile, user search |
| `/api/v1/documents` | Document CRUD, content (Y.Doc binary), export, import, blobs, link settings |
| `/api/v1/documents/{id}/ws` | WebSocket for real-time collaboration |
| `/api/v1/documents/{id}/comments` | Inline comment threads |
| `/api/v1/documents/{id}/versions` | Version history, diff, restore |
| `/api/v1/documents/{id}/sharing` | Document-level sharing |
| `/api/v1/documents/{id}/activity` | Activity log |
| `/api/v1/documents/{id}/relationships` | Document relationship graph (CRUD) |
| `/api/v1/threads` | Comment thread operations |
| `/api/v1/chats` | Chat rooms and direct messages |
| `/api/v1/notifications` | Notification feed, mark read |
| `/api/v1/folders` | Folder CRUD, folder sharing |
| `/api/v1/workspaces` | Workspace management, members |
| `/api/v1/search` | Full-text + semantic search (`?mode=keyword\|semantic\|hybrid`) |
| `/api/v1/ask` | AI document assistant (SSE streaming) |
| `/api/v1/admin/users` | Admin: list, view, disable/enable, promote/demote users |

## Project status & governance

OgreNotes is a personal project, maintained by its author at his own
discretion. It is shared publicly so the code can be read, learned from,
and reused — not as a product with a support commitment.

- **No roadmap or support guarantees.** Direction is set solely by the
  maintainer. There is no SLA, and no promise that any request will be
  acted on.
- **Issues and pull requests may be closed without action.** Bug reports
  are welcome and appreciated, but triage is best-effort. PRs are not
  guaranteed to be reviewed or merged; please open an issue to discuss
  before investing significant work.
- **The maintainer retains final say** over scope, design, and what does
  and does not get merged.

If you'd like to build on OgreNotes in a different direction, the MIT
license makes forking easy and explicitly permitted.

## License

Licensed under the [MIT License](LICENSE). Copyright (c) 2026 Joel Baumert.

You are free to use, copy, modify, and distribute this software, including
for commercial purposes, provided the copyright notice and license text are
preserved. The software is provided "as is", without warranty of any kind.
