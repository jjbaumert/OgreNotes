# SLA alarm runbook

Phase 5 M-P9 piece F. One entry per CloudWatch alarm defined in
`design/performance-budgets.md`. The intent is **first 5 minutes
of an alarm page** — what does it mean, what do I look at first,
what's the most likely cause. Deep-investigation playbooks live
elsewhere; this file gets the on-call to "I know what to do."

All alarms publish to the SNS topic `ogrenotes-ops-alerts` (reuses
M-E7's topic — email subscription stays the same). Acknowledge in
your usual on-call tool; **don't silence the alarm in AWS** unless
you've already diagnosed a known bad state, because a silenced
threshold means a real subsequent breach won't page.

## Common first steps for any p95-breach alarm

Before diving into a specific alarm:

1. Open the CloudWatch dashboard `OgreNotes-SLA`. Confirm the
   alarm hasn't recovered on its own (3 × 5-min window — if it
   went green during the page write, the wave may have passed).
2. Check the **5xx error-rate** widget. A coincident error-rate
   bump points at a backend-dependency outage; latency without
   errors usually points at a query / cache / saturation issue.
3. Check the **Request rate by status** widget for a sudden
   traffic surge — capacity-limited latency is a different
   response than a degraded backend.
4. Look at the most-recent deploy. The release pipeline writes a
   metric filter that draws a vertical annotation on every
   dashboard widget; a latency cliff aligned to a deploy is the
   strongest single signal.

## `api-auth-p95-breach`

| | |
|---|---|
| **Metric** | `api.request_latency_ms` p95 where route ∈ `/auth/login`, `/auth/refresh`, `/auth/logout` |
| **Threshold** | > 200 ms for 3 × 5-min windows |
| **User impact** | Sign-in or refresh-token flow is sluggish; users may see spinner-then-bounce-back to login |
| **First look** | (1) DynamoDB write-failures widget — refresh-token rotation writes to DDB; (2) Redis publish-failures — rate-limit module enforces via Redis |
| **Common causes** | bcrypt CPU spike from a refresh-token storm; DDB capacity throttle on the SESSIONS partition |
| **Escalation** | If 5xx coincident → on-call lead. If 5xx flat → likely capacity; consider scale-out of the API ECS service |

## `api-doc-list-p95-breach`

| | |
|---|---|
| **Metric** | `api.request_latency_ms` p95 where route = `GET /api/v1/documents` |
| **Threshold** | > 300 ms for 3 × 5-min windows |
| **User impact** | Home / folder views stall; user sees "Loading…" for 0.5–1 s |
| **First look** | DynamoDB GSI1 (`owner_id` / `updated_at`) capacity. Hot-folder rendering also lists per-folder children via GSI2 |
| **Common causes** | A user with thousands of docs paginating through their home; GSI consumed-capacity throttle |
| **Escalation** | Capacity-bound → raise GSI1 / GSI2 RCU; otherwise check for a regressed N+1 query path in `routes::documents::list` |

## `api-doc-content-p95-breach`

| | |
|---|---|
| **Metric** | `api.request_latency_ms` p95 where route = `GET /api/v1/documents/:id/content` |
| **Threshold** | > 500 ms for 3 × 5-min windows |
| **User impact** | Opening any document is slow; cold-load WASM users compound the delay |
| **First look** | S3 GetObject latency (snapshot fetch) + DDB Query latency (pending updates list) |
| **Common causes** | S3 throttling on a hot bucket; a doc with 1000s of un-compacted pending updates because idle compaction hasn't run |
| **Escalation** | If S3 is hot → consider distributing snapshots across more prefixes; if compaction is behind → kick the compaction worker manually (see [reproduce-editor-failure.md](reproduce-editor-failure.md)) |

## `api-search-p95-breach`

| | |
|---|---|
| **Metric** | `api.request_latency_ms` p95 where route = `GET /api/v1/search` |
| **Threshold** | > 800 ms for 3 × 5-min windows |
| **User impact** | Search typeahead lags noticeably; users may double-submit |
| **First look** | Tantivy index path on disk + index size. The hybrid-mode search also queries the embedding pipeline (Bedrock) — its latency surfaces here |
| **Common causes** | Disk I/O contention with a deploy that just wrote to `/tmp/ogrenotes-search`; semantic mode hitting Bedrock during a regional slowdown |
| **Escalation** | Force hybrid → keyword-only via `SEARCH_FORCE_KEYWORD=true` to take Bedrock out of the path while you investigate |

## `api-ws-upgrade-p95-breach`

| | |
|---|---|
| **Metric** | `api.request_latency_ms` p95 where route = WebSocket upgrade |
| **Threshold** | > 100 ms for 3 × 5-min windows |
| **User impact** | Real-time edits / presence appear "stuck"; users get bounced to the REST save fallback |
| **First look** | (1) Active WS-connections widget for saturation; (2) `MAX_WS_CONNECTIONS_PER_DOC` runtime config — a hot doc bumping that cap rejects upgrades, which counts in the p95 as a fast 4xx but slow ones can hide here |
| **Common causes** | A document with many concurrent viewers exceeding per-doc cap; ALB target-group health-check thrash causing TCP fingerprinting drops |
| **Escalation** | Capacity-bound → scale-out API ECS; suspect ALB → check target-group health in the AWS console |

## `api-5xx-rate`

| | |
|---|---|
| **Metric** | `api.requests_total` rate where status starts with `5` |
| **Threshold** | > 1% of total for 2 × 5-min windows |
| **User impact** | A measurable fraction of requests are failing outright. Anything user-initiated may surface as an error toast |
| **First look** | The 5xx-by-route breakdown widget on the dashboard — the failure is rarely uniform |
| **Common causes** | Backend dependency outage (DDB, S3, Redis); a panic-in-handler regression after a deploy |
| **Escalation** | **This is a paging-priority alarm.** Engage on-call immediately. If a recent deploy aligns, prepare to roll back via the deploy pipeline |

## `rum-editor-lcp-cold`

| | |
|---|---|
| **Metric** | `rum.lcp_ms` p75 where page = `editor` |
| **Threshold** | > 3500 ms for 3 × 15-min windows |
| **User impact** | First impression of the editor is slow; perception is "the app is sluggish today" |
| **First look** | (1) WASM bundle CI gate green? — a bundle bloat lands without bumping the gate could push LCP; (2) ALB target health — CDN-cached shell still loads, but if the deploy pipeline is half-rolled the WASM file might be missing |
| **Common causes** | A new dependency bloated the WASM; a CDN miss after deploy (cache invalidation lag) |
| **Escalation** | Low-priority. Compare LCP p75 across pages — if only editor is hit, look at editor-only assets; if all pages are hit, look at network-level |

## `rum-spreadsheet-lcp-cold`

| | |
|---|---|
| **Metric** | `rum.lcp_ms` p75 where page = `spreadsheet` |
| **Threshold** | > 4500 ms for 3 × 15-min windows |
| **User impact** | Spreadsheet first paint is slow; same perception as editor but on a different surface |
| **First look** | Same as `rum-editor-lcp-cold`. Spreadsheet has its own dynamic-import chunk on top of the editor shell; a regression specific to that chunk would show here but not in editor |
| **Common causes** | Same family as editor LCP. Plus formula-engine cold-init cost if a recent change moved work earlier in the load |
| **Escalation** | Low-priority unless ALSO seeing api-doc-content-p95-breach — combined slowness on doc-open is the worse user experience |

## Applying the dashboard

```
aws cloudwatch put-dashboard \
    --dashboard-name OgreNotes-SLA \
    --dashboard-body file://infra/cloudwatch-dashboard.json
```

Re-apply after any edit to `infra/cloudwatch-dashboard.json`. The
dashboard JSON is the source of truth; the AWS console copy is
disposable.

## Provisioning the alarms

Each alarm is a separate `aws cloudwatch put-metric-alarm` call.
The provisioning script lives in the deploy pipeline (terraform
or equivalent — see the ops repo). Below is the shape; **do not
hand-create alarms in the console**, they drift.

```
aws cloudwatch put-metric-alarm \
    --alarm-name api-auth-p95-breach \
    --alarm-description "p95 latency on /auth/* over 200ms" \
    --metric-name api.request_latency_ms \
    --namespace OgreNotes \
    --dimensions Name=route,Value=/api/v1/auth/login Name=method,Value=POST \
    --statistic ExtendedStatistic --extended-statistic p95 \
    --period 300 --evaluation-periods 3 \
    --threshold 200 --comparison-operator GreaterThanThreshold \
    --treat-missing-data notBreaching \
    --alarm-actions arn:aws:sns:us-east-1:<acct>:ogrenotes-ops-alerts
```
