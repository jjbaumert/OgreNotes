# Async-Worker Subsystem

> **Distilled pointer doc (#88).** The async-worker is operationally documented
> in `runbook/async-worker-ops.md` and implemented in `crates/worker` +
> `crates/api/src/worker_mode.rs`; this page is the `design/`-level landing
> point criterion #9 expects, distilling
> the design and pointing at the authoritative sources.

## What it is

A background job subsystem for work too slow to run inline on a request —
primarily DOCX/PDF import/export conversion, where synchronous conversion would
blow the request budget. Jobs are enqueued via `POST /jobs`, processed by a
separate worker process, and polled via `GET /jobs/{id}`.

## Key decisions (distilled)

- **Redis Streams as the queue.** `XADD` to enqueue, `XREADGROUP` consumer
  groups to claim, `XACK` on success, a dead-letter stream for terminal
  failures, and an `XAUTOCLAIM` reaper to recover jobs orphaned by a crashed
  consumer. Wire shape + key names: `runbook/async-worker-ops.md`
  §"Wire shape / key names".
- **Single binary, two modes.** The same image runs the API or, with
  `--mode=worker`, the consumer loop — entrypoint `crates/api/src/worker_mode.rs`.
  Deployed as a second `ogrenote-worker` ECS service with CPU-target
  autoscaling (milestone M-6.4).
- **Status side-channel** is a separate Redis key the client polls
  (`pending` → `running` → `succeeded`/`failed`), decoupled from the stream so
  a completed job stops the client polling without scanning the stream.

## Canonical sources

- [`runbook/async-worker-ops.md`](../runbook/async-worker-ops.md) — wire shape,
  liveness checks, stuck-job investigation, manual `XCLAIM`, dead-letter
  recovery, retry tuning.
- **Code:** `crates/worker` (the queue + consumer), `crates/api/src/worker_mode.rs`
  (the `--mode=worker` entrypoint), and the `POST /jobs` / `GET /jobs/{id}` API.
