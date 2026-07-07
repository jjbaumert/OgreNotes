# Async-worker operations runbook

Phase 6 M-6.4 piece E. Operator-facing recipes for the Redis-streams
async-job subsystem: investigating stuck jobs, recovering work from a
crashed worker, draining the dead-letter stream, and tuning retries.

The moving parts (all in `crates/worker/src/lib.rs` and
`crates/api/src/worker_mode.rs`):

- **Producer** — the API server (`POST /api/v1/jobs` and, from M-6.5/6,
  the import routes). `XADD`s a job envelope onto the work stream and
  writes a `pending` status hash.
- **Consumers** — the `ogrenote-worker` ECS service, launched as
  `--mode=worker`. Each task runs `WORKER_CONCURRENCY` consumer loops
  (`XREADGROUP`) plus one reaper loop (`XAUTOCLAIM`).
- **Status side-channel** — a `job:{id}` HASH that backs
  `GET /api/v1/jobs/{id}`, TTL'd at 24h.

## Wire shape / key names

Defaults shown; the stream name is `JOB_STREAM_NAME` (default
`ogrenotes-jobs`) and is shared by producer and consumer.

| Key | Type | Purpose |
|---|---|---|
| `ogrenotes-jobs` | stream | main work queue. `XADD` enqueue, `XREADGROUP` consume, `XACK`+`XDEL` on success. |
| `ogrenotes-jobs:dlq` | stream | dead-letter. One `XADD` per job whose retry budget is exhausted; carries a `lastError` field. |
| `job:{id}` | hash | status side-channel. Field `json` holds the serialized `JobStatus`; 24h TTL. |
| consumer group | `workers` | one group; every worker task joins as a distinct consumer. |

Consumer names are `{HOSTNAME}-{nanoid}-{i}` for the work loops and
`{HOSTNAME}-{nanoid}-reaper` for the reaper. On Fargate `HOSTNAME` is
the container id, so a consumer name points back at a specific task in
the CloudWatch logs.

Relevant constants (in `crates/api/src/worker_mode.rs`):

- `MAX_RETRIES = 3` — attempts before dead-letter (attempt 0 + 3 retries).
- `CONSUME_BLOCK_MS = 5000` — `XREADGROUP` block window; also the
  shutdown-drain granularity.
- `REAPER_INTERVAL_SECS = 30` — how often the reaper sweeps.
- `REAPER_MIN_IDLE_MS = 60000` — a pending entry must be idle this long
  before the reaper claims it (i.e. ~60s after a worker dies, its
  in-flight job is recovered).

## Getting a redis-cli prompt

In the deployed stack Redis is ElastiCache, reachable **only from
inside the VPC** — the Redis security group allows ingress from the ECS
SG alone. You cannot `redis-cli` from a laptop. Two practical paths:

```bash
# (a) Local dev / CI — Redis is the docker-compose container:
docker compose exec redis redis-cli

# (b) Deployed stack — run a one-off Fargate task on the same VPC/SG
#     using an image that ships redis-cli (redis:7-alpine), then
#     `redis-cli -h <elasticache-endpoint>`. The endpoint is:
aws elasticache describe-cache-clusters \
    --cache-cluster-id "${STACK_PREFIX}redis" --show-cache-node-info \
    --query 'CacheClusters[0].CacheNodes[0].Endpoint.Address' \
    --output text --region "$AWS_REGION"
```

All `redis-cli` snippets below assume you have a prompt against the
right instance and substitute the real stream name for `ogrenotes-jobs`
if `JOB_STREAM_NAME` was overridden.

## Is the worker alive?

```bash
# ECS service: runningCount should match desiredCount.
aws ecs describe-services --cluster "${STACK_PREFIX}ogrenote" \
    --services "${STACK_PREFIX}ogrenote-worker" \
    --query 'services[0].{desired:desiredCount,running:runningCount,status:status}' \
    --region "$AWS_REGION"

# Recent worker logs — "consumer started" / "job succeeded" lines.
aws logs tail "/ecs/${STACK_PREFIX}ogrenote-worker" \
    --since 15m --follow --region "$AWS_REGION"
```

A healthy worker logs `worker mode: consumer started` once per
concurrency slot at boot and a `job succeeded` / `job failed` line per
job. No log churn with a non-empty stream means consumers aren't
reading — check the service is RUNNING and Redis is reachable from the
ECS SG.

## A job is stuck — investigate

Start from the job id the client is polling.

```bash
# 1. What does the status side-channel say?
redis-cli HGET job:<id> json
#   {"state":"pending"}                  → enqueued, not yet claimed
#   {"state":"running","worker":"…"}     → a consumer is on it
#   {"state":"succeeded",…}              → done (client should stop polling)
#   {"state":"failed","error":"…"}       → dead-lettered; see below
#   (nil)                                → unknown id or >24h since terminal
```

```bash
# 2. Is the stream backed up? (entries waiting + group lag)
redis-cli XLEN ogrenotes-jobs
redis-cli XINFO GROUPS ogrenotes-jobs
#   `lag` = entries the group hasn't delivered yet. `pending` =
#   delivered-but-unacked (a consumer is working, or died mid-job).
```

```bash
# 3. Who holds unacked entries, and for how long?
redis-cli XPENDING ogrenotes-jobs workers
#   Shows the count + the consumer names holding pending entries.
#   An entry idle > 60s with no live consumer is reaper-eligible and
#   should self-heal within REAPER_INTERVAL_SECS. If it doesn't, the
#   reaper loop isn't running — check worker logs for "reaper started".
```

Decision guide:

- **`pending` status, `XLEN` > 0, no log churn** → consumers aren't
  reading. Worker service down, or Redis ingress blocked. Fix the
  service / SG; the backlog drains on its own once a consumer reads.
- **`running` status, stuck > a few minutes** → the consumer is wedged
  inside `execute` (or its task died without the reaper having caught
  up yet). Wait one reaper interval; if still stuck, see "recover work
  from a crashed worker."
- **`failed` status** → retry budget exhausted; the entry is on the
  dead-letter stream. See "drain / recover the dead-letter stream."
- **`nil` status but client still polling** → either a bad id, or the
  job finished > 24h ago and the status hash TTL'd out. Not a worker
  problem.

## Recover work from a crashed worker

Normally automatic: the reaper `XAUTOCLAIM`s any entry idle longer than
`REAPER_MIN_IDLE_MS` (60s) and re-executes it, so a task that dies
mid-job is recovered within ~60–90s without intervention.

Force it manually only if the reaper itself is down and you can't
restart the worker service quickly:

```bash
# Claim entries idle > 60s for a named live consumer, then let the
# worker's normal loop pick them up. `0-0` starts the scan at the
# beginning of the pending list.
redis-cli XAUTOCLAIM ogrenotes-jobs workers manual-recovery 60000 0-0 COUNT 16
```

Prefer just bouncing the service — `force-new-deployment` brings up
fresh tasks whose reapers will sweep the pending list:

```bash
aws ecs update-service --cluster "${STACK_PREFIX}ogrenote" \
    --service "${STACK_PREFIX}ogrenote-worker" \
    --force-new-deployment --region "$AWS_REGION"
```

## Drain / recover the dead-letter stream

A job lands on `ogrenotes-jobs:dlq` after `MAX_RETRIES` failed attempts.
Each DLQ entry carries the original `envelope` plus a `lastError`
field naming the final failure.

```bash
# How many dead-lettered jobs, and what failed?
redis-cli XLEN ogrenotes-jobs:dlq
redis-cli XRANGE ogrenotes-jobs:dlq - + COUNT 20
#   Read `lastError` to triage: a transient cause (S3 throttle, Redis
#   blip) is replayable; a deterministic one (corrupt DOCX, bad
#   owner_id) is not — replaying just dead-letters it again.
```

Recovery, once the underlying cause is fixed, is a manual replay: copy
the `envelope` JSON back onto the main stream and delete the DLQ entry.
There is intentionally no automatic DLQ drain — replaying poison
messages in a loop is worse than leaving them parked for a human.

```bash
# Replay one entry (substitute the envelope JSON and the DLQ entry id
# from XRANGE above):
redis-cli XADD ogrenotes-jobs '*' envelope '<envelope-json>'
redis-cli XDEL ogrenotes-jobs:dlq <dlq-entry-id>
```

Note the replayed envelope keeps its original `attempt` count, so a
job dead-lettered at attempt 3 gets no fresh retries on replay — fix
the root cause first, or it dead-letters again on the next failure.

## Tuning retries

`MAX_RETRIES` is a single flat constant in
`crates/api/src/worker_mode.rs` (`execute_and_finalize` passes it to
`retry_or_dead_letter`). Changing it is a code change + redeploy, not a
config knob — deliberately, so the retry budget is reviewable in the
diff.

When DOCX (M-6.5) and PDF (M-6.6) land, per-kind budgets belong here:
cheap idempotent work tolerates many retries; expensive
non-idempotent rendering should retry zero times and dead-letter
immediately. The `retry_or_dead_letter` API already takes `max_retries`
per call — the flat constant is just the v1 default.

`WORKER_CONCURRENCY` (consumers per task) *is* an env knob, set on the
worker task definition (default `2`). Raise it for I/O-bound job mixes;
keep it low for CPU-bound conversion work so consumers on a 0.25-vCPU
task don't thrash.

## Autoscaling

The `ogrenote-worker` service scales on **CPU utilization**
(`ECSServiceAverageCPUUtilization`, target 70%), min/max set by
`WORKER_MIN_COUNT` / `WORKER_MAX_COUNT` in `aws-test-deploy.sh`
(defaults 1 / 3). CPU is a sound v1 signal because the jobs the worker
exists to run — DOCX/PDF conversion — are CPU-bound: a real backlog
shows up as sustained CPU.

**Backlog-based scaling is a v2 refinement.** Scaling directly on
stream depth would be more precise, but ElastiCache emits no native
stream-length metric. Implementing it means a publisher: the worker
`PUT`s `XLEN ogrenotes-jobs` to a custom CloudWatch metric (e.g.
`OgreNotes/Worker` `StreamBacklog`) on a timer, then a second
target-tracking policy tracks that metric. That's deferred until
production job volume justifies the extra metric + IAM
(`cloudwatch:PutMetricData`) surface. Until then, watch backlog
manually with `XLEN` (above) and bump `WORKER_MAX_COUNT` if CPU-based
scaling can't keep the queue drained.

## Related

- `crates/worker/src/lib.rs` — queue primitives + wire shape.
- `crates/api/src/worker_mode.rs` — the consumer loop, reaper, dispatch.
- `crates/api/src/routes/jobs.rs` — the `POST`/`GET /jobs` API.
- `scripts/aws-test-deploy.sh` (Phase 6 M-6.4) — worker task + service +
  autoscaling.
