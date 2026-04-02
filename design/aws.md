# AWS Deployment Architecture — Ogre Notes Collaborative Editor

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
                          │   API Gateway   │  (HTTP routes: auth, doc CRUD)
                          └────────┬────────┘
                                   │
                    ┌──────────────▼──────────────┐
                    │         ALB                  │
                    │  (WebSocket + stickiness)    │
                    └──┬──────────────────────┬───┘
                       │                      │
              ┌────────▼───┐          ┌───────▼────┐
              │  ECS Task  │          │  ECS Task  │   (your Axum servers)
              │  (Axum)    │          │  (Axum)    │
              └────────┬───┘          └───────┬────┘
                       │                      │
              ┌────────▼──────────────────────▼────┐
              │         ElastiCache (Redis)         │
              │    pub/sub + awareness + SV cache   │
              └─────────────────────────────────────┘
                       │                      │
              ┌────────▼───┐          ┌───────▼────┐
              │  DynamoDB  │          │     S3     │
              │  (updates) │          │ (snapshots)│
              └────────────┘          └────────────┘
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
- Task-level IAM roles (clean credentials for DynamoDB/S3/ElastiCache)

Start with 2–3 tasks minimum for availability. Scale on **ALB active connection count** rather than CPU — your workload is I/O bound, not compute bound.

### API Gateway for HTTP (not WebSocket)

Use API Gateway for your REST/HTTP surface — auth, document creation, user management, etc. Let ALB handle only WebSocket traffic. This keeps your Axum server focused and lets you put WAF rules on API Gateway cheaply.

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

## Networking

Put everything except the ALB inside a **VPC**:

```
VPC 10.0.0.0/16
  Public subnets  (10.0.1.0/24, 10.0.2.0/24)   — ALB only
  Private subnets (10.0.10.0/24, 10.0.11.0/24)  — ECS tasks, ElastiCache
  No public IPs on ECS tasks
```

ECS tasks reach DynamoDB and S3 via **VPC endpoints** (no traffic leaves AWS backbone, lower latency, no NAT Gateway costs for that traffic).

ElastiCache sits in private subnets, reachable only from the ECS security group.

---

## IAM (Task Role)

Your ECS task role needs exactly:

```json
{
  "Effect": "Allow",
  "Action": [
    "dynamodb:GetItem",
    "dynamodb:PutItem",
    "dynamodb:Query",
    "dynamodb:BatchWriteItem",
    "dynamodb:DeleteItem"
  ],
  "Resource": "arn:aws:dynamodb:*:*:table/ogre-docs*"
},
{
  "Effect": "Allow",
  "Action": ["s3:GetObject", "s3:PutObject"],
  "Resource": "arn:aws:s3:::your-snapshots-bucket/*"
}
```

No access keys anywhere. Task role credentials are rotated automatically.

---

## Secrets and Config

Use **SSM Parameter Store** (not Secrets Manager — cheaper, sufficient for config) for anything environment-specific:

```
/quip/prod/redis_url
/quip/prod/database_url   (if you add Postgres later)
/quip/prod/jwt_secret
```

Your Axum server reads these at startup via the AWS SDK. Fargate tasks can pull SSM params at launch via the ECS secrets integration — no SDK calls needed in your app code.

---

## Observability

Set these up before you have users, not after.

**CloudWatch Container Insights** — CPU, memory, network per task. Free with ECS.

**Structured logging** — emit JSON logs from Axum via `tracing` + `tracing-subscriber`. CloudWatch Logs Insights can then query fields. Log every WebSocket connect/disconnect, every Update applied, every snapshot written.

**Custom metrics you'll actually want:**

- `doc.active_connections` (per doc ID)
- `doc.update_apply_latency_ms`
- `doc.cold_load_duration_ms`
- `redis.publish_failures`
- `dynamo.write_failures`

Emit these as CloudWatch custom metrics from your Axum code. Set alarms on `dynamo.write_failures > 0` because a failed DynamoDB write is a data loss risk.

**X-Ray** — optional, but add the `aws-xray-sdk` early. Tracing WebSocket sessions across ECS → DynamoDB → S3 is much harder to add retroactively.

---

## Deployment Pipeline

```
GitHub → CodeBuild (cargo build --release, docker build)
       → ECR (image push)
       → CodeDeploy (ECS rolling deploy)
```

ECS rolling deploys work well here — new tasks start, ALB health checks pass, old tasks drain connections gracefully. Your Axum server should handle `SIGTERM` by stopping new connections and waiting for existing WebSockets to close (or a 30-second timeout).

---

## What to Defer

These are real concerns but not day-one concerns:

- **CloudFront in front of ALB** — WebSocket + CloudFront works but requires HTTP/2 and careful cache config. Add it when you care about global latency.
- **Multi-region** — DynamoDB Global Tables + ElastiCache Global Datastore exist but add significant complexity. One region is fine until you have users on multiple continents.
- **Aurora Postgres** — you may eventually want a relational store for user accounts, billing, and doc metadata. Add it when you need it, not speculatively.
- **SQS for snapshot triggers** — instead of triggering snapshots in-process, you could publish to SQS and have a separate worker handle it. Cleaner, but adds a moving part.

---

## The Minimal Starting Point

If you want to ship fast, the smallest viable version is:

| Component | Spec |
|---|---|
| ECS Service | 2 Fargate tasks, 512MB / 0.25 vCPU each |
| Load Balancer | ALB with sticky sessions enabled |
| Redis | ElastiCache cache.t3.micro, single shard |
| DynamoDB | On-demand, one table, PITR enabled |
| Storage | One S3 bucket, Intelligent-Tiering |
| Networking | VPC with public/private subnets, VPC endpoints |

Deployable in an afternoon with Terraform or CDK. Costs roughly **$50–80/month** at zero traffic and scales horizontally by increasing ECS task count.