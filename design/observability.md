# OgreNotes Observability

Symmetric client + server instrumentation so the system can answer
its own debugging questions in seconds, not in hours of HAR-greppping
and CloudWatch archaeology.

## Why

OgreNotes already has a respectable backend observability story:
the EMF pipeline in `crates/common/src/metrics/` emits route-latency
histograms and per-operation counters; CloudWatch logs are
structured-tracing JSON; the `aws-diagnostic` agent can pull rows
from DynamoDB and the snapshot from S3 to triage live behavior.

The recent edits-not-persisting bug exposed a hard limit. The
symptom — *"edits broadcast in real time to other browsers but
don't survive a refresh"* — required us to determine which stage
of a 7-step client→server→storage pipeline silently dropped the
keystroke. We could not answer that question from the running
system. The release-build client log calls are a no-op
(`frontend/src/editor/debug.rs:125`), Firefox's HAR doesn't
capture WebSocket payloads, and the deployed `tracing` level is
`info` (the per-update debug lines never reach CloudWatch). We
spent hours pulling individual signals — HAR exports, partial DDB
queries, server-only counters — and still ended up with two
contradictory observations (server broadcast works; server persist
doesn't fire) and no instrumentation that could discriminate them.

The fix is not "add more logging this once." It is to make the
system **self-diagnosing by default**, so the next class-of-bug
costs minutes not hours.

## Goals

- **Symmetric metrics.** Every counter or histogram the server
  emits has a client counterpart where the operation has a client
  side. The ratio between them is the discriminator for
  "did the client send this" vs. "did the server receive this."
- **Tiered logging that always compiles in.** Release builds of
  the WASM frontend retain log call sites; what changes between
  release and dev is just the default emit threshold. A URL flag
  or admin endpoint flips it without a rebuild.
- **End-to-end correlation.** Every user-initiated action gets a
  `correlation_id` that flows from the keystroke through the WS
  frame envelope, into the server's `tracing` span, into the
  persist row's metadata. Then "find every trace for this
  keystroke" is a single grep, not a join across three systems.
- **An agent that knows the shape.** The triage agent reads the
  symmetric metric set and surfaces *discrepancies*, not raw
  counts. Operator asks "are client edits reaching the server?"
  and gets a single sentence with the gap and the suspected
  stage.

## Out of scope

- Per-tenant analytics, billing-grade usage attribution, customer
  status pages, synthetic monitoring beyond the existing load
  tests. Those are Phase 6+ business concerns.
- A new metrics backend. We continue emitting EMF; CloudWatch
  Metric Insights and existing dashboards keep working.
- PII / GDPR-class logging. The client-side ring buffer must
  never contain document content; only counts, sizes, and shape.

## Design

### 1. Client metric surface

A new module `frontend/src/observability/metrics.rs` mirrors the
shape of `crates/common/src/metrics/`. Counters and histograms
record into an in-memory buffer at the WASM boundary; a periodic
flush ships deltas to a new POST endpoint.

| Concern | Server metric (existing) | Client metric (proposed) |
|---|---|---|
| WS frame send/recv | `ws.messages_total{type}` | `client.ws.frames_sent_total{type}` |
| Editor → CRDT diff | n/a | `client.editor.transactions_total{kind}` |
| `observe_update_v1` | n/a | `client.collab.observe_fired_total{remote}` |
| Pre-flush buffer | n/a | `client.collab.pending_updates_drained_total` |
| Persist row write | `dynamo.write_failures_total{op}` | n/a (server-only) |
| WS recv apply | `ws.update_apply_latency_ms` | `client.collab.remote_apply_latency_ms` |
| HTTP round-trip | `api.request_latency_ms` | `client.http.request_latency_ms` |
| Page TTI | (M-P9 RUM, partial) | extended to all editor mount paths |

`MetricKey` and emit semantics match the server side exactly, so a
CloudWatch dashboard panel can plot `client.ws.frames_sent_total`
and `ws.messages_total` on the same axis. The discrepancy is the
answer to most live-collab bugs.

A new authenticated POST endpoint `/api/v1/client-telemetry`
accepts a JSON batch of metric deltas and projects them into the
EMF pipeline under a `client.*` namespace. Rate-limited per JWT
via the existing `middleware::rate_limit` (default cap ~1 req/sec;
metric flushes batch every 10 s so the cap is generous). Body size
capped at 16 KiB. Drops requests that exceed the cap rather than
queuing them — telemetry must never block user interaction.

> **Status (not yet built).** This batched `/client-telemetry`
> endpoint and its metric-name allowlist are still design, not shipped.
> The client-telemetry analog in the codebase today is the RUM endpoint
> `POST /api/v1/metrics/rum` (`crates/api/src/routes/metrics.rs`),
> rate-limited at `rate_limit_rum_per_min` (default **60/min**, ≈1 req/s)
> and with **no metric-name allowlist** — the metric names are
> server-side constants (`rum.lcp_ms`, `rum.fcp_ms`, …); the validated
> closed set is the `page` dimension (`validate_page`). Treat the
> per-JWT / allowlist / `client.*`-namespace details below as the target
> design for the batched endpoint, not current behavior.

### 2. Tiered client logging

`frontend/src/editor/debug.rs` is rewritten to **always compile
in** the log emit paths in both dev and release. The cost is one
unconditional ring-buffer push per call; benchmarked at ~20 ns,
acceptable on the keystroke hot path.

| Level | What surfaces | Default in dev | Default in release |
|---|---|---|---|
| `off` | nothing | — | — |
| `warn` | always — categorized as "shouldn't be silently swallowed" | ✓ | ✓ |
| `info` | important events (paste, collab connect/disconnect, errors) | ✓ | off |
| `debug` | per-WS-frame, per-CRDT-apply, per-keystroke | off | off |
| `verbose` | per-input-event, per-selection-change | off | off |

Runtime toggling via three mechanisms, in priority order:

1. **URL flag**: `?debug=collab,ws&level=debug` enables `debug`
   level for the `collab` and `ws` categories only, for this tab.
   Survives navigation within the SPA; cleared on tab close.
2. **JS console**: `window.__ogre_debug = "verbose"` (current
   mechanism, preserved for muscle memory).
3. **Server-pushed config**: a `client_log_config` field on the
   `/users/me` response (or a dedicated `/api/v1/client-config`
   endpoint when we get there) lets an operator turn on `debug`
   for a specific user without their cooperation. Useful for
   reproducing user-reported bugs.

The ring buffer holds the last 1 000 entries regardless of emit
level — so even with `off`, the last few seconds of debug-level
events are available. A new button in the (yet-to-build) dev
console — and a POST `/api/v1/client-telemetry/logs` endpoint —
ships the ring buffer to the server for inclusion in a support
bundle.

### 3. End-to-end correlation

The WS protocol gets one new envelope field: `correlation_id`
(16 random bytes, base64url; encoded as a 22-byte prefix on every
outbound frame). Generated on the client per user-initiated
action (a single keystroke producing N debounced frames shares
one correlation_id).

| Layer | Where the correlation_id lands |
|---|---|
| Client log entries | per-call field |
| Outbound WS frame | prefix on `MessageType::Update` frames |
| Server WS recv | `tracing::info_span!(correlation_id = ...)` wraps the per-frame handler |
| Server persist | new optional `correlation_id` field on `DocUpdate` (DDB attribute) |
| Server response / broadcast | echoed back so the originating client can confirm receipt |

The existing `x-request-id` middleware (see TraceLayer in the
README §6) covers REST requests. WS frames are not REST requests
and currently have no correlation. Adding correlation_id to the
WS envelope is a one-time protocol bump — current clients ignore
unknown leading bytes if we negotiate via the existing
`client_version` field on the WS-token request.

A client-side toast in dev / debug mode shows the correlation_id
on persist failure, so the user can paste it into a bug report
and we can pull every related signal in one query.

### 4. The diagnostic agent

`aws-diagnostic` gets a new knowledge module: a `discrepancy_map`
that knows which client metric pairs with which server metric.
Given a doc id + time window, it pulls both sides, computes the
gaps, and reports:

> Over 14:00–19:00 UTC, user `JT785...` produced **42** editor
> transactions on doc `YJS-UYsg...` (client.editor.transactions_total),
> sent **40** MSG_UPDATE frames (client.ws.frames_sent_total),
> the server received **39** (ws.messages_total) and persisted
> **38** (dynamo.write_success). **One transaction was buffered
> but never sent** (client/server gap of 2) and **one was sent
> but not persisted** (server-side log shows no error). Likely
> root cause: a client-side debounce timer fired but the
> WebSocket was in CLOSING state at flush time.

This is the agent that would have answered our current bug in
under a minute.

## Version-skew tolerance

OgreNotes ships frontend and backend in a single Docker image, so
operator-level deploys are atomic. But a user's browser caches the
previous WASM bundle until the next hard refresh, so the running
API is routinely talking to a *one-version-old* client for the
lifetime of every long-lived editor tab. Rollbacks produce the
reverse skew: an older server fielding frames from a newer client.
Both directions are normal operational states and must be handled
without (a) silently dropping user edits or (b) limping along in a
state the operator cannot see.

The principle: **be tolerant where possible; never accept silent
failures.** Tolerance means additive-only changes pass without
negotiation. Non-silence means every act of tolerance emits a
counter, and the cases where tolerance is unsafe disconnect with
a user-visible reason that the FE knows how to surface.

### Compatibility matrix

The matrix below is the contract for the five kinds of change
OgreNotes ships. "Required emission" is the counter that MUST fire
so the discrepancy_map agent (Phase 3) can recognize the skew.

| Change kind | Old client → new server | New client → old server | Required emission |
|---|---|---|---|
| **New optional DTO field** (template: `client_version` on `WsTokenRequest` at `crates/api/src/routes/ws.rs:46`) | Server reads `#[serde(default)]` → `None`; behavior identical to historical path | Client sends field, server ignores (no `deny_unknown_fields` anywhere — this is the existing policy) | `server.dto.field_absent_total{route,field}` on routes flagged version-sensitive when an `Option<T>` is `None` |
| **New DDB attribute** (template: `update_s3_key` reader fallback at `crates/storage/src/repo/doc_repo.rs:687`) | Reader falls back to inline / default per the existing pattern | New attribute written; old reader ignores it. Data loss only if the attribute carries the *only* copy of canonical data | `storage.ddb.attr_absent_total{table,attr}` when an optional-but-expected attribute is missing on read |
| **New WS `MessageType` byte** | Server may emit a new type to an old client; old client's `MessageType::from_byte` (`crates/collab/src/protocol.rs:56-71`) returns `None`, frame dropped silently today | New client sends byte the old server doesn't know; `decode_message` returns `None`, frame dropped silently today | `ws.unknown_msg_type_total{direction,byte}` on whichever side returned `None` |
| **New WS envelope field** (Phase 2's `correlation_id` prefix) | Frame shape changes; old client cannot parse a frame with the prefix. Requires negotiation — see "envelope versioning" below | Old server reads the prefix as a `MessageType` byte → unknown byte → silent drop today | `ws.envelope_version_mismatch_total{client_sent,server_supports}` at WS-token validation |
| **New client→server telemetry metric name** (the `/api/v1/client-telemetry` allowlist) | Old client never emits new name — invisible | New client emits a name the server's allowlist doesn't yet contain → rejected as `InvalidArgument` per the trust-boundary policy in §Design.1 | `client_telemetry.unknown_metric_total{name}` so client-newer-than-server is observable, not silent |

The matrix is not exhaustive. New HTTP routes are out of scope:
Axum returns 404 without breaking other routes. It covers the five
change kinds the rest of this doc and the Phase 2 plan
contemplate.

### WS protocol envelope versioning

The current envelope is `[MessageType:u8][payload...]`
(`crates/collab/src/protocol.rs:74-90`). Phase 2 wants a 16-byte
`correlation_id` prefix on `MessageType::Update` and
`MessageType::ForeignDocUpdate` frames so the server's tracing
span and DDB `DocUpdate` row carry the same correlation key as
the client log entry that produced the frame.

Three negotiation strategies were considered:

**(a) Version byte at start of every frame.** Self-describing and
stateless, but doubles the prefix size of small frames (Ping,
Awareness) for a discriminator that changes only on protocol bumps,
and the *negotiation* mechanism is itself a protocol change with
no out-of-band negotiation. Rejected.

**(b) Piggyback on `MessageType::Auth`.** The Auth frame exists
(`protocol.rs:13`) but is unused — the token is in the URL query
string (`ws.rs:128`). Repurposing Auth as a first-frame
envelope-version handshake would break every current client.
Rejected.

**(c) Negotiate via the existing `client_version` on the WS-token
POST.** The field is already plumbed end-to-end: client sends it
on `POST /documents/:id/ws-token` (`ws.rs:46`), Redis stores it
with the token (`ws.rs:71`), `handle_ws` reads it back from Redis.
The server can derive the envelope shape *before* the first frame
and stamp it on the connection. Old clients (no `client_version`
or a pre-V2 version) get the legacy envelope; new clients get the
extended envelope. **Recommended.**

Implementation shape: define `EnvelopeVersion { V1, V2 }` in
`protocol.rs`, with `From<Option<&str>>` mapping `client_version`
to the right variant. Store the enum on the connection at
`handle_ws` entry; thread `(envelope, data)` through
`decode_message` and `encode_message`. V1 = legacy 1-byte type
prefix; V2 = adds 16-byte `correlation_id` prefix on `Update` and
`ForeignDocUpdate` frames only — every other type stays unchanged
under V2 since they don't carry user-meaningful state worth
correlating.

**Server-newer-than-client.** Server receives V1 frames; downgrades
its own emission to V1 for the connection; generates the
`correlation_id` server-side at frame-receive time so the
server-side tracing span and DDB row still get one. Emits
`ws.envelope_downgrade_total{from,to}` once per connection on first
downgrade — a per-connection counter, not per frame.

**Client-newer-than-server.** Client sends V2; old server reads
the first byte of `correlation_id` as if it were a `MessageType`
byte → unknown → drops the frame today. Handled by the hard-stop
rule below: the server stamps its supported `max_envelope_version`
on the WS-token *response*; the client compares against its
required version on connect; if the server is too old the client
surfaces a user-visible message rather than send frames that
vanish.

### The hard-stop rule

The principle: a discrepancy that produces **silent data loss**
must disconnect; one that produces an *observable* degradation
can limp with a counter.

| Situation | Rule | Counter |
|---|---|---|
| Client sends `MessageType::Update` whose envelope decodes but yrs apply fails | Counter + `warn`. Don't disconnect — a single bad update should not eject the session. Already covered by `ws.update_apply_failures_total` (existing) | `ws.update_apply_failures_total` |
| Client sends a byte that doesn't map to any `MessageType` | Increment + continue. May be a malformed frame OR a Phase-N+1 message we don't yet handle. Either way the user can keep editing | `ws.unknown_msg_type_total` |
| Client `client_version` is below server's configured `min_client_version` | **Disconnect.** Send `MessageType::Error("upgrade-required:{min}")` and close the socket. Client treats `upgrade-required` as a hard signal: render a non-dismissable toast "OgreNotes was updated. Please refresh." and call `window.location.reload(true)` after a 5-second grace so the user can read it | `ws.skew_upgrade_required_total{client_version}` |
| Client `envelope_version` is above server's `max_envelope_version` | Same disconnect path with `Error("server-too-old:{server_max}")`. Client renders a transient toast and auto-retries the WS connection at 10s — the case is transient (rolling deploy completion) | `ws.skew_server_too_old_total{client_envelope}` |
| Server cannot decode a frame's envelope at all (length too short for the declared version, corrupt prefix) | Disconnect with `Error("envelope-corrupt")`. This is the only "can't even tell what the client meant" case; limping risks applying a partial payload as a CRDT update | `ws.envelope_decode_failures_total` |

**`min_client_version` semantics.** Server-side env var
`OGRE_MIN_CLIENT_VERSION`, semver string, default unset = no floor.
Rolled forward only when a *server-side* change removes legacy
support — e.g. when the V1 envelope branch is finally deleted.
Operational rule: only raise ≥ 24 hours after the corresponding
server image rolls out, so the new client bundle has been served
from CloudFront long enough for caches to warm. The runbook entry
for raising `min_client_version` references the discrepancy_map
output for the `client_version` distribution before and after.

**`upgrade-required` is the only mandatory FE behavior.** Document
in the frontend FE-hints: on receipt of a
`MessageType::Error("upgrade-required:…")` frame, show the toast,
do not retry, hard-refresh after 5s. The server can compel this
behavior; everything else is a counter the operator watches.

### `client_version` shape

The field carries two parts so we can both order versions
(for `min_client_version` enforcement) and identify exact builds
(for skew reports).

| Part | Source | Used for |
|---|---|---|
| `client_version` (semver) | `CARGO_PKG_VERSION` from `frontend/Cargo.toml` | Ordering basis for `min_client_version` comparisons |
| `client_build` (git SHA) | `build.rs`-derived `BUILD_GIT_SHA` env var baked into the WASM at compile time | Dimension on skew counters; lets the `skew-report` agent distinguish two builds at the same semver (e.g. two same-day test-stack deploys) |

Both are sent on the WS-token POST and stored together in Redis
with the single-use token. The semver is the gate; the SHA is the
discriminator.

### Drift counters (Phase 1)

The following six counters land in Phase 1 so the discrepancy_map
agent (Phase 3) has the signal it needs to recognize skew. Each is
sized to keep CloudWatch cost bounded — dimensions are
low-cardinality enums, not user/doc ids.

| Counter | Dimensions | Emit rule | Site |
|---|---|---|---|
| `ws.unknown_msg_type_total` | `direction` ∈ {send, recv}, `byte` (raw u8 as 2-digit hex) | Server: emit on `from_byte` returning `None` in the recv loop. Client: emit on the symmetric client-side decode. Replaces the silent `_ => None` at `protocol.rs:69` | `crates/collab/src/protocol.rs::decode_message` and frontend mirror |
| `server.dto.field_absent_total` | `route` (Axum MatchedPath), `field` (compile-time string) | Emit on routes flagged version-sensitive when an `Option<T>` field is `None` AND the route's version policy treats absence as "old client." Initial scope: `/ws-token`'s `client_version` field | `crates/api/src/routes/ws.rs::create_ws_token` |
| `client_telemetry.unknown_metric_total` | `name` (the rejected metric name, capped at 64 chars) | Emit at the `/api/v1/client-telemetry` allowlist check. Names the counter referenced in §Trust boundary | `crates/api/src/routes/client_telemetry.rs` (new in Phase 1) |
| `ws.skew_client_version_mismatch_total` | `result` ∈ {ok, below_min, above_max, unparseable, absent}, `client_build` (SHA) | Emit once per WS-token POST. Lets us see the distribution of versions in flight before raising `min_client_version` | `crates/api/src/routes/ws.rs::create_ws_token` |
| `ws.update_decode_failures_total` | none | Emit when a frame carries `MessageType::Update` but yrs cannot decode the payload as an update (distinct from apply-failure, which means decode succeeded but the update was incompatible — different remediation) | `crates/collab/src/document.rs::apply_update` |
| `ws.send_errors_total` | `side` ∈ {primary, foreign, error_frame, sync} | Emit on the `Err` branch of `handle.sender.send(...)` at `crates/collab/src/room.rs:143/152/162` — today these are `let _ = ...` and the channel-closed signal is silently swallowed, meaning we cannot distinguish "client gone" from "broadcast worked" | `crates/collab/src/room.rs:143/152/162` |

Phase 2 adds four more counters alongside the envelope change
itself: `ws.envelope_downgrade_total`,
`ws.envelope_decode_failures_total`,
`ws.skew_upgrade_required_total`,
`ws.skew_server_too_old_total`. Phase 3 wires all 10 into the
discrepancy_map's skew report (§Operator experience below).

### Silent-failure audit (Phase 1 triage rule)

A scan for `let _ = ...`, `_ => None`, and `if let Err(_) { return }`
across the WS / collab / persist path turns up ~8 candidate sites.
We do not instrument all of them in Phase 1. The rule is:

> **Phase 1 emits a counter at any silent-swallow site where
> (a) the swallowed signal would indicate skew, lost data, or a
> peer that has gone away, AND (b) the site is on the
> persist-or-broadcast path of a `MessageType::Update`.**

Applied to the candidate sites:

| Site | Phase 1 decision | Reason |
|---|---|---|
| `crates/collab/src/room.rs:143` (broadcast sender) | **Emit** — `ws.send_errors_total{side=primary}` | On the broadcast path. Channel-closed means a peer disappeared mid-broadcast; the current bug class includes "did the broadcast actually reach the client" |
| `crates/collab/src/room.rs:152` (`send_to_client`) | **Emit** — `ws.send_errors_total{side=error_frame}` | Used for error frames. Silently swallowing the failure to deliver an error frame is the worst possible silent failure |
| `crates/collab/src/room.rs:162` (`sync_client`) | **Emit** — `ws.send_errors_total{side=sync}` | The new client never receives initial sync; downstream the recv loop will silently produce an inconsistent CRDT state |
| `crates/api/src/routes/ws.rs::handle_ws` `SyncStep2` apply | **Defer** | Not on broadcast/persist path; SyncStep2 failure surfaces later as missing content, doesn't cause skew on its own |
| Cold-load `apply_update` sites in `ws.rs` | **Defer** | Cold-load failures already show up as the document not opening; a counter would duplicate signal |
| `activity_repo.create` swallow in `ws.rs` | **Defer** | Activity log is supplementary; loss doesn't affect a user's edits |
| `crates/collab/src/protocol.rs:69` (`_ => None` on unknown byte) | **Emit** — covered by `ws.unknown_msg_type_total` above | The primary skew detector |
| `crates/collab/src/diff.rs`, `schema.rs`, `import.rs` parse fallbacks | **Defer** | Internal parse fallbacks; not on the live edit path |

The rule is a heuristic, not a hard gate. Anything not on the
live-edit hot path stays a candidate for ad-hoc instrumentation
when a specific bug demands it.

### Operator experience

When the discrepancy_map agent says "there is skew," the operator
workflow is:

1. **Single CloudWatch dashboard tile.** A Phase 3 tile in
   `infra/lib/dashboard.ts` titled "Version skew" plots
   four lines on one axis:
   `ws.skew_client_version_mismatch_total{result=below_min}`,
   `…{result=above_max}`, `ws.envelope_downgrade_total`, and
   `client_telemetry.unknown_metric_total` (summed across names).
   Flat zero = no skew today; any non-zero line = active mismatch
   on at least one connection.

2. **Agent prompt.** The `aws-diagnostic` agent extended in
   Phase 3 exposes a `skew-report` knowledge module. The operator
   asks: *"what versions are connected to doc X right now?"*
   The agent reads `ws.skew_client_version_mismatch_total` by
   `client_version` dimension for the last 15 minutes and
   summarizes: *"12 sessions on client 2.4.1 (current), 3 sessions
   on 2.3.7 (1 version behind, within tolerance), 1 session on
   2.1.0 (below floor — will disconnect on next reconnect)."*

3. **`make-skew-report` script (optional, Phase 3).** A
   `scripts/make-skew-report.sh` helper that emits a one-page
   Markdown report: distribution of `client_version` across
   active sessions, count of unknown msg types in the last hour,
   current `min_client_version`, recommended next
   `min_client_version` based on the 99th percentile of currently-
   connected versions. Used as input to the "raise
   min_client_version" runbook.

The operator does not need to query individual log lines for
routine skew checks. The dashboard tile is the routine signal;
the agent is the deep-dive.

### Phase mapping

| Item | Phase 1 | Phase 2 | Phase 3 |
|---|---|---|---|
| Six baseline drift counters | ✓ | | |
| Three silent-failure fixes (room.rs:143/152/162) | ✓ | | |
| `EnvelopeVersion` enum + V2 with `correlation_id` | | ✓ | |
| `min_client_version` config + `Error` frame disconnect | | ✓ | |
| Four envelope-version counters | | ✓ | |
| `BUILD_GIT_SHA` baked into WASM + `client_build` field on WS-token | | ✓ | |
| Version-skew dashboard tile | | | ✓ |
| `skew-report` agent module + `make-skew-report.sh` | | | ✓ |

This mapping keeps Phase 1 to the unblock-current-bug minimum:
six counters and three trivial `let _ =` substitutions. The
protocol envelope change waits for Phase 2 because it must land
with the `correlation_id` work it shares an envelope with.

### Open questions (this section)

- **`min_client_version` per-room vs. global.** A per-doc floor
  would let us isolate a single shared workspace that's been
  "frozen" for a customer-specific reason without bricking
  everyone. Probably not worth the complexity for a self-hosted,
  single-tenant deploy in v1.
- **`MessageType::CommentEvent` version-sensitive?** Comments live
  outside the CRDT, so a missed CommentEvent is "user has to
  refresh to see a new thread" — annoying but not destructive.
  Probably leave un-versioned in Phase 2.
- **`ws.send_errors_total` per-recipient vs. per-broadcast.**
  Per-recipient gives finer signal but inflates the counter when
  one zombie client closes slowly. Recommend per-recipient with a
  bucketed `client_count` dimension, but worth confirming.
- **"Client newer than server" disconnect at WS-token (HTTP
  426-style) vs. mid-stream Error frame.** The former is cleaner
  for the user; the latter is more uniform with the
  `min_client_version` path. Recommend HTTP at token time when
  the server can detect it from `client_version`, Error frame as
  the fallback for envelope-corrupt cases that only surface
  mid-stream.

## Phasing

### Phase 1 — Unblock current debugging (~1 sprint)

The minimum that makes the current edits-not-persisting bug
diagnosable. Implementation:

- `frontend/src/observability/metrics.rs` skeleton + `MetricKey`
  re-export from `ogrenotes-common` so the same type identifies
  metrics on both sides.
- Six client counters: `client.editor.transactions_total`,
  `client.collab.observe_fired_total`,
  `client.collab.pending_updates_drained_total`,
  `client.ws.frames_sent_total`, `client.ws.send_errors_total`,
  `client.ws.remote_frames_received_total`.
- Six server drift counters and three silent-failure fixes per
  the §Version-skew tolerance phase-mapping table — emits at
  `crates/collab/src/protocol.rs::decode_message`,
  `crates/collab/src/room.rs:143/152/162`,
  `crates/collab/src/document.rs::apply_update`, and the WS-token
  POST handler. These are 1–2 line emits at existing call sites
  plus three `let _ = ...` substitutions — small surface area,
  high signal for the current bug class.
- POST `/api/v1/client-telemetry` endpoint, rate-limited,
  projecting into the existing EMF pipeline.
- The `?debug=...` URL flag and unconditional ring buffer.
- Documented runbook entry: how to pull a doc's client+server
  metrics for a time window via the existing CloudWatch CLI.

Phase 1 should be enough to compile a frontend that, deployed,
tells us where in the keystroke→persist pipeline the current bug
lives. No new schema, no protocol bump, no server changes beyond
the new endpoint.

### Phase 2 — Correlation + runtime config (~2 sprints)

- Add `correlation_id` to the WS frame envelope. Bump
  `client_version` so the server can detect old clients and
  generate one server-side as a fallback (no breakage).
- Add `correlation_id` field to `DocUpdate` (new optional
  attribute, additive only).
- `tracing::info_span!` wrap on the server's WS frame handler so
  every log line in the span carries the correlation_id.
- Server-side `/admin/log-level` endpoint to flip tracing level
  at runtime, gated by the same admin policy as the audit log.
- `client_log_config` field on `/users/me` so operators can flip
  a specific user's client log level remotely.

### Phase 3 — Dashboard + agent + RUM expansion (~2 sprints)

- A CloudWatch dashboard panel pairs each client/server counter
  on the same axis with the discrepancy plotted explicitly.
- `aws-diagnostic` `discrepancy_map` extension. Operator asks
  "are edits flowing on doc X?" and gets a one-sentence verdict.
- RUM sampler (M-P9 covers the page-load slice; we extend with
  TTI of the editor mount, time-to-first-WS-Synced, etc.).
- Per-page client-side performance budgets that emit
  `client.page.tti_ms{page=document|spreadsheet|chat}` so
  Phase 5's perf budgets cover frontend paths too.

## Trust boundary

The client telemetry endpoint sits at L4 (edge) and accepts
authenticated input from untrusted clients. The trust posture:

- **Authentication required.** No anonymous metric writes.
- **Rate-limited per JWT.** Default ~1 req/sec (batched every ~10 s, so
  the cap is generous). Drops exceeding requests — never queues.
- **Body capped at 16 KiB.** Counters and the ring-buffer log
  shipment fit comfortably.
- **Metric names validated against an allowlist.** Clients
  cannot create arbitrary CloudWatch metric names; the server
  knows the set of `client.*` metric keys, rejects unknown ones
  (`InvalidArgument` with a counter so we notice client / server
  schema drift). This also bounds the CloudWatch cost surface.
- **Dimension values capped.** No unbounded high-cardinality
  dimensions (`doc_id` is NOT a dimension; the client side keeps
  this in correlation_id only).
- **Logs MUST NOT contain document content.** The ring buffer
  records `payload_len` and `update_bytes_count`, never the
  bytes. Asserted in the L4 boundary check.

## Migration / compatibility

- Phase 1 is pure addition — no schema changes, no protocol
  bumps, no existing call sites moved. Existing
  `MetricKey::new(...)` callers keep working.
- Phase 2's WS protocol bump uses the existing `client_version`
  field, with the server detecting old clients and generating
  correlation_ids itself for backward compatibility.
- Phase 3 is dashboard + agent work — no code change beyond the
  new endpoint surface added in Phase 1.

## Open questions

- **Should the client ring buffer persist across page navigations
  within the SPA?** Pro: a user reporting "the doc broke when I
  navigated from /docs to /d/X" needs both sides. Con: memory
  pressure and PII exposure window grows.
- **What's the right unit of correlation_id rotation?** Per
  keystroke is too noisy; per "burst of editing" (debounce
  window) is more useful but harder to define cleanly.
- **Do we want server-side rate-limiting on the endpoint by
  doc_id as well as by user?** Unclear until we see the volume
  pattern from Phase 1.
- **Should Phase 1 land with a CI gate that fails the build if a
  call site uses `tracing::debug!` but doesn't have a matching
  client metric?** Probably overkill until Phase 2 lands.

## What this does NOT replace

- `framework/` — the code-review framework. Observability is a
  runtime concern; the architectural taxonomy is unchanged.
- `verification/` — the runtime-behavior verification framework.
  The verification flow uses agents to confirm behaviors; this
  doc gives those agents better signal to work with, not a new
  flow.
- Existing EMF pipeline. We're extending the metric namespace,
  not replacing the emission mechanism.
