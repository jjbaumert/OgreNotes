# AWS Deployment Architecture — OgreNotes Collaborative Editor

## The Core Constraint: WebSockets

WebSockets are long-lived, stateful connections. This creates tension with typical AWS scaling patterns, which assume stateless HTTP. Every architectural decision flows from this.

Your servers need to be **sticky per document session** (not per user) — meaning all clients editing the same doc ideally connect to the same server instance, which holds that doc's in-memory `Doc`. Redis pub/sub is your escape hatch when they don't.

---

## Recommended Architecture

```
                          ┌─────────────────┐
                          │   Route 53 DNS  │
                          └────────┬────────┘
                                   │
                          ┌────────▼────────┐
                          │   CloudFront    │  (static assets only)
                          └────────┬────────┘
                                   │
                          ┌────────▼────────┐
                          │   API Gateway   │  (HTTP routes: auth, doc CRUD, search, ask)
                          └────────┬────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │         ALB                  │
                    │  (WebSocket + stickiness)    │
                    └──┬──────────────────────┬───┘
                       │                      │
              ┌────────▼───┐          ┌───────▼────┐
              │  ECS Task  │          │  ECS Task  │   (Axum servers)
              │  (Axum)    │          │  (Axum)    │
              │  + Tantivy │          │  + Tantivy │   (in-process BM25 index)
              └────┬───┬───┘          └───┬───┬────┘
                   │   │                  │   │
              ┌────▼───▼──────────────────▼───▼────┐
              │         ElastiCache (Redis)         │
              │    pub/sub + awareness + SV cache   │
              └─────────────────────────────────────┘
                   │         │            │
              ┌────▼───┐  ┌──▼─────┐  ┌──▼────────┐
              │DynamoDB │  │   S3   │  │  Qdrant   │  (optional — vector search)
              │(updates)│  │(snaps) │  │  (ECS)    │
              └────────┘  └────────┘  └──┬────────┘
                                         │
                              ┌──────────▼──────────┐
                              │   Bedrock Titan     │  (optional — embeddings)
                              └─────────────────────┘

              External API: api.anthropic.com (optional — AI assistant)
```

---

## Component Choices and Why

### ALB (Application Load Balancer) not NLB

ALB understands WebSocket upgrades natively. Critically, it supports **sticky sessions via cookies** — when Alice's browser first connects, ALB sets a cookie, and all her subsequent connections route to the same ECS task. This keeps her in-memory `Doc` warm.

Configure stickiness with a reasonable duration (1–4 hours). Stickiness is best-effort — if a task dies, ALB will route her to another instance, which cold-loads from DynamoDB. That's fine.

### ECS on Fargate (not EC2, not Lambda)

Lambda cannot hold a WebSocket connection — it times out at 15 minutes maximum and has no persistent memory. ECS Fargate gives you:

- Long-lived processes (your in-memory Doc cache survives across requests)
- No server management
- Simple horizontal scaling
- Task-level IAM roles (clean credentials for DynamoDB/S3/ElastiCache/Bedrock)

Start with 2–3 tasks minimum for availability. Scale on **ALB active connection count** rather than CPU — your workload is I/O bound, not compute bound.

### API Gateway for HTTP (not WebSocket)

Use API Gateway for your REST/HTTP surface — auth, document creation, user management, search, AI assistant, etc. Let ALB handle only WebSocket traffic. This keeps your Axum server focused and lets you put WAF rules on API Gateway cheaply.

### ElastiCache (Redis) — Cluster Mode Off

Use a **single-shard Redis** with read replicas, not cluster mode. Cluster mode shards keys across nodes, which breaks pub/sub (can only pub/sub within a shard). Since your channel names are `doc:{id}:updates`, you need all subscribers reachable from any publisher.

For your scale, one primary + one replica is fine. Enable Multi-AZ with automatic failover.

### DynamoDB On-Demand

Don't provision capacity yet. On-demand pricing is more expensive per operation but requires zero capacity planning, and your write pattern (bursty during active editing, quiet otherwise) is exactly what on-demand handles well. Switch to provisioned + autoscaling once you have real traffic data.

Enable **Point-in-Time Recovery (PITR)** from day one. It costs almost nothing and has saved many teams.

### S3 for Snapshots

Standard bucket, private, server-side encryption enabled. Your Axum tasks write snapshots directly — no Lambda needed here.

Enable **Intelligent-Tiering** on the snapshots prefix so old snapshots automatically migrate to cheaper storage classes.

---

## Search Infrastructure

### Tantivy (In-Process BM25 Search)

Tantivy runs inside each ECS task as an embedded library — no external service needed. The search index lives on local disk.

- **Storage:** ECS ephemeral storage (20 GB default) for the index. For durability across task restarts, mount an EFS volume at the `SEARCH_INDEX_PATH` location.
- **Multi-task consistency:** Each task maintains its own index, updated via fire-and-forget hooks on document mutations (create, update, delete, import, restore). Indexes may briefly diverge between tasks; ALB stickiness ensures a user's requests consistently hit the same task. On cold start, the index rebuilds automatically from DynamoDB + S3 as documents are accessed.
- **Schema versioning:** On startup, the server validates the on-disk index schema against the expected schema. A mismatch (e.g., after a code deploy that adds a field) causes a startup error — delete the index directory to force a rebuild.

### Qdrant (Optional — Vector Search)

Qdrant provides the vector store for semantic search. It runs as a separate ECS Fargate service.

- **Deployment:** Single Qdrant container (256MB RAM / 0.25 vCPU for small corpus). Use `qdrant/qdrant:latest` image.
- **Persistence:** EFS-backed volume mounted at `/qdrant/storage` so vectors survive task restarts.
- **Ports:** gRPC on 6334 (used by the Axum server via `qdrant-client`), REST on 6333 (Qdrant dashboard for debugging).
- **Networking:** Private subnet only. Security group allows inbound 6334 from the Axum ECS task security group.
- **Disabled by default:** When the `QDRANT_URL` env var is not set, the embedding pipeline is skipped entirely and search uses keyword-only mode. No Qdrant infrastructure needed.

### Amazon Bedrock (Optional — Embeddings)

Bedrock Titan Embed v2 generates vector embeddings for documents. It's a managed AWS service — no infrastructure to deploy.

- **Model:** `amazon.titan-embed-text-v2:0` (1024-dimensional vectors, configurable via `EMBEDDING_DIMENSIONS`).
- **Setup:** Enable the Titan Embed model in the AWS console (Bedrock > Model access). The ECS task role needs `bedrock:InvokeModel` permission.
- **Usage pattern:** Fire-and-forget. Embedding calls happen asynchronously after document mutations. Failures are logged but don't block API responses.
- **Cost:** ~$0.0001 per 1K tokens. At 10K embedding operations/month, ~$5/month.

---

## Networking

Put everything except the ALB inside a **VPC**:

```
VPC 10.0.0.0/16
  Public subnets  (10.0.1.0/24, 10.0.2.0/24)   — ALB only
  Private subnets (10.0.10.0/24, 10.0.11.0/24)  — ECS tasks, ElastiCache, Qdrant
  No public IPs on ECS tasks
```

ECS tasks reach DynamoDB and S3 via **VPC endpoints** (no traffic leaves AWS backbone, lower latency, no NAT Gateway costs for that traffic).

ElastiCache and Qdrant sit in private subnets, reachable only from the ECS Axum task security group.

**Outbound internet access:** The Anthropic Claude API (`api.anthropic.com`) requires outbound HTTPS. If the AI assistant is enabled, ECS tasks need a NAT Gateway (~$32/month) or a NAT instance for outbound traffic. If the AI assistant is disabled (no `ANTHROPIC_API_KEY`), no NAT is needed — DynamoDB, S3, and Bedrock all use VPC endpoints.

---

## IAM (Task Role)

Your ECS task role needs:

```json
{
  "Effect": "Allow",
  "Action": [
    "dynamodb:GetItem",
    "dynamodb:PutItem",
    "dynamodb:Query",
    "dynamodb:BatchWriteItem",
    "dynamodb:DeleteItem",
    "dynamodb:UpdateItem",
    "dynamodb:TransactWriteItems"
  ],
  "Resource": "arn:aws:dynamodb:*:*:table/ogrenotes*"
},
{
  "Effect": "Allow",
  "Action": ["s3:GetObject", "s3:PutObject"],
  "Resource": "arn:aws:s3:::your-snapshots-bucket/*"
},
{
  "Effect": "Allow",
  "Action": ["bedrock:InvokeModel"],
  "Resource": "arn:aws:bedrock:*::foundation-model/amazon.titan-embed-text-v2:0"
}
```

No access keys anywhere. Task role credentials are rotated automatically.

The Bedrock permission is only needed if vector search is enabled. Scope it down to the specific model ARN.

---

## Secrets and Config

Use **SSM Parameter Store** for non-secret config and **Secrets Manager** for API keys and secrets:

**SSM Parameter Store:**
```
/ogrenotes/prod/redis_url
/ogrenotes/prod/oauth_client_id
/ogrenotes/prod/oauth_redirect_uri
/ogrenotes/prod/frontend_origin
/ogrenotes/prod/search_index_path
/ogrenotes/prod/qdrant_url                 (optional)
/ogrenotes/prod/embedding_model_id         (optional)
/ogrenotes/prod/anthropic_model            (optional)
```

**Secrets Manager** (for values that need rotation):
```
/ogrenotes/prod/jwt_secret
/ogrenotes/prod/oauth_client_secret
/ogrenotes/prod/anthropic_api_key          (optional)
```

Fargate tasks can pull SSM params and Secrets Manager values at launch via the ECS secrets integration — no SDK calls needed in your app code. All secrets are redacted in the application's Debug output.

---

## Observability

Set these up before you have users, not after.

**CloudWatch Container Insights** — CPU, memory, network per task. Free with ECS.

**Structured logging** — emit JSON logs from Axum via `tracing` + `tracing-subscriber`. CloudWatch Logs Insights can then query fields. Log every WebSocket connect/disconnect, every Update applied, every snapshot written.

**Custom metrics you'll actually want:**

```
# Core document operations
doc.active_connections           (per doc ID)
doc.update_apply_latency_ms
doc.cold_load_duration_ms
redis.publish_failures
dynamo.write_failures

# Search
search.keyword_latency_ms
search.semantic_latency_ms
search.hybrid_latency_ms

# Embeddings
embedding.index_latency_ms
embedding.index_failures
qdrant.connection_failures

# AI assistant
ask.agent_rounds                 (histogram: how many tool rounds per query)
ask.total_latency_ms
ask.claude_api_errors
```

Emit these as CloudWatch custom metrics from your Axum code. Set alarms on `dynamo.write_failures > 0` because a failed DynamoDB write is a data loss risk. Also alarm on `qdrant.connection_failures > 5/min` to detect Qdrant task crashes.

**X-Ray** — optional, but add the `aws-xray-sdk` early. Tracing WebSocket sessions across ECS → DynamoDB → S3 is much harder to add retroactively.

---

## Deployment Pipeline

```
GitHub → CodeBuild (cargo build --release, docker build)
       → ECR (image push)
       → CodeDeploy (ECS rolling deploy)
```

ECS rolling deploys work well here — new tasks start, ALB health checks pass, old tasks drain connections gracefully. Your Axum server should handle `SIGTERM` by stopping new connections and waiting for existing WebSockets to close (or a 30-second timeout).

**Note on Tantivy index:** After a rolling deploy, new tasks start with an empty search index. The index rebuilds as documents are accessed. For faster warm-up, consider a startup script that pre-loads the index from a snapshot stored in S3.

---

## What to Defer

These are real concerns but not day-one concerns:

- **CloudFront in front of ALB** — WebSocket + CloudFront works but requires HTTP/2 and careful cache config. Add it when you care about global latency.
- **Multi-region** — DynamoDB Global Tables + ElastiCache Global Datastore exist but add significant complexity. One region is fine until you have users on multiple continents.
- **Aurora Postgres** — you may eventually want a relational store for user accounts, billing, and doc metadata. Add it when you need it, not speculatively.
- **SQS for snapshot triggers** — instead of triggering snapshots in-process, you could publish to SQS and have a separate worker handle it. Cleaner, but adds a moving part.
- **Qdrant cluster mode** — a single Qdrant instance handles up to ~1M documents easily. Qdrant supports distributed mode for horizontal scaling when needed.
- **Embedding backfill** — documents created before Qdrant was enabled have no vectors. A one-time batch job to re-embed all documents is needed eventually but not urgent — new/updated documents are embedded automatically.
- **Shared Tantivy index** — each ECS task maintains its own index. For strict cross-task consistency, mount a shared EFS volume. Not needed at small scale where ALB stickiness keeps users on the same task.

---

## The Minimal Starting Point

If you want to ship fast, the smallest viable version is:

| Component | Spec | Monthly Cost |
|---|---|---|
| ECS Service | 2 Fargate tasks, 512MB / 0.25 vCPU each | ~$20 |
| Load Balancer | ALB with sticky sessions enabled | ~$20 |
| Redis | ElastiCache cache.t3.micro, single shard | ~$15 |
| DynamoDB | On-demand, one table, PITR enabled | ~$5 |
| Storage | One S3 bucket, Intelligent-Tiering | ~$1 |
| Search | Tantivy in-process (no extra service) | $0 |
| Networking | VPC with public/private subnets, VPC endpoints | ~$10 |
| **Base total** | | **~$70/month** |

**Optional add-ons for search and AI features:**

| Component | Spec | Monthly Cost |
|---|---|---|
| Vector Search | Qdrant on Fargate, 256MB / 0.25 vCPU + EFS | ~$15 |
| Embeddings | Bedrock Titan Embed, pay-per-call | ~$5 |
| AI Assistant | Anthropic Claude API, pay-per-call | ~$20 |
| NAT Gateway | Required for outbound Anthropic API calls | ~$32 |
| **AI/Search total** | | **~$70/month** |

Deployable in an afternoon with Terraform or CDK. The base system scales horizontally by increasing ECS task count. Qdrant and the AI assistant can be enabled later by setting environment variables — no code changes required.

---

## Budget Testing Deployment (~$13–15/month)

For early testing with a small number of users, everything can run on a single EC2 instance. This eliminates ALB, ElastiCache, NAT Gateway, and ECS — the biggest cost drivers.

### Architecture

```
┌──────────────────────────────────┐
│  EC2 t4g.small (ARM, 2GB RAM)   │
│  ┌───────────────────────────┐   │
│  │  Axum server + Tantivy   │   │   (cargo build --release)
│  └───────────────────────────┘   │
│  ┌────────────┐  ┌───────────┐   │
│  │  Redis     │  │  Qdrant   │   │   (optional, Docker containers)
│  └────────────┘  └───────────┘   │
└──────────────┬───────────────────┘
               │  public IP + Let's Encrypt
               │
      ┌────────▼───┐     ┌────────┐
      │  DynamoDB  │     │   S3   │   (managed, on-demand)
      └────────────┘     └────────┘
```

### What's Different from Production

| Concern | Production | Testing |
|---|---|---|
| Load balancing | ALB with sticky sessions | None — single instance, direct access |
| Redis | ElastiCache (managed, Multi-AZ) | Local `redis-server` on the instance |
| Qdrant | Separate ECS Fargate task | Docker container on the same instance |
| TLS | ALB terminates SSL | Let's Encrypt + nginx reverse proxy (or skip TLS for localhost testing) |
| Availability | 2+ tasks across AZs | Single instance, no failover |
| Networking | VPC with private subnets, VPC endpoints, NAT | Public subnet, direct internet access |

### Cost Breakdown

| Component | Spec | Monthly Cost |
|---|---|---|
| EC2 | t4g.small, 2 vCPU / 2GB RAM | ~$12 |
| DynamoDB | On-demand (essentially free at test traffic) | ~$0–1 |
| S3 | One bucket | ~$0 |
| Route 53 | Hosted zone (optional, skip if using IP directly) | $0–0.50 |
| **Total** | | **~$13–15/month** |

Even cheaper: use **EC2 Spot** pricing for ~$4–5/month (risk of interruption, acceptable for testing). Use a persistent EBS volume so data survives Spot interruptions.

### Setup

```bash
# Launch a t4g.small (Amazon Linux 2023, ARM)
# SSH in, then:

# Install dependencies
sudo dnf install -y docker redis6
sudo systemctl start docker redis6
sudo systemctl enable docker redis6

# Build the server (cross-compile for ARM or build on-instance)
cargo build --release
# Binary at: target/release/ogrenotes-api

# Start Qdrant (optional — vector search)
sudo docker run -d --name qdrant -p 6334:6334 -p 6333:6333 \
  -v /data/qdrant:/qdrant/storage qdrant/qdrant

# Set environment variables
export DYNAMODB_TABLE_PREFIX=test-
export S3_BUCKET=test-ogrenotes
export REDIS_URL=redis://localhost:6379
export OAUTH_CLIENT_ID=your-github-client-id
export OAUTH_CLIENT_SECRET=your-github-client-secret
export OAUTH_REDIRECT_URI=http://your-ip:3000/auth/complete
export JWT_SECRET=your-secret-at-least-32-bytes-long
export DEV_MODE=true
export SEARCH_INDEX_PATH=/data/search-index

# Optional: enable vector search
export QDRANT_URL=http://localhost:6334

# Initialize DynamoDB table + S3 bucket (first time only)
cargo run --bin setup_dev

# Start the server
./target/release/ogrenotes-api
```

The instance needs an IAM instance profile with DynamoDB + S3 permissions (same policy as the ECS task role, minus Bedrock if not using embeddings).

### What to Skip for Testing

- **ALB** — not needed with a single instance
- **NAT Gateway** — instance is in a public subnet with direct internet
- **VPC endpoints** — adds complexity for no benefit at this scale
- **ElastiCache** — local Redis is fine
- **ECS/Fargate** — run the binary directly
- **CloudFront, API Gateway, X-Ray** — add when moving to production
- **AI assistant / Bedrock** — add later by setting `ANTHROPIC_API_KEY` and `QDRANT_URL`

### Migrating to Production

When ready to move to the production architecture:

1. Push the Docker image to ECR
2. Create the ECS service, ALB, ElastiCache, and VPC resources
3. Move environment variables to SSM Parameter Store / Secrets Manager
4. Point Route 53 to the ALB
5. The application code is identical — only infrastructure changes
