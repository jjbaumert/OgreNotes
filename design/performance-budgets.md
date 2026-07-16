# OgreNotes Performance Budgets

Phase 5 M-P9 architectural doc. Locked-in budgets before the RUM
sampler, criterion benches, load tests, dashboard, and alarms land
— every later M-P9 piece codes against the targets defined here.

## Goals

OgreNotes has shipped Phases 1–4 with the EMF metrics pipeline in
`crates/common/src/metrics/` emitting ~46 route-latency histograms,
but **no codified SLAs**. A regression that pushed `GET /documents`
from 80 ms p95 to 600 ms p95 would emit clean telemetry and page
no one. M-P9 fixes that:

- **Codify per-route + per-page targets** so a number on a graph
  has a meaning, not just a value.
- **Add a frontend RUM sampler** so user-perceived metrics
  (LCP / FID / CLS / nav-timing) join the existing backend
  histograms in CloudWatch.
- **CI gates** for WASM bundle size and benchmark regression so
  budgets bite at PR time, not on a 3 a.m. SNS alert.
- **Dashboard + alarms** so the human-readable picture exists and
  SLA breaches actually wake someone.

Out of scope for Phase 5: SLO error-budget accounting (multi-week
rolling windows, error-budget policy), customer-facing status page,
synthetic monitoring beyond load tests, per-tenant performance
isolation. The Phase 5 deliverable is "we know when we miss; we
catch it in CI when we can; we alarm on it in prod when we can't."

## Budget categories

### API p95 latency (per route template)

Measured from `crates/api/src/middleware/metrics.rs`'s
`api.request_latency_ms` histogram, dimensioned by
`(method, route, status)`. Targets are per-route-template — the
`route` dimension uses Axum's `MatchedPath` so paths like
`/documents/{id}` collapse into one bucket.

| Route pattern | p50 | p95 | p99 | Rationale |
|---|---|---|---|---|
| `POST /auth/login`, `/auth/refresh`, `/auth/logout` | 80 ms | **200 ms** | 400 ms | Hot path; cookie writes + bcrypt verify dominate |
| `POST /auth/oauth/callback` | 150 ms | 400 ms | 800 ms | Allows for OAuth IdP round-trip latency variance |
| `GET /documents` (list) | 100 ms | **300 ms** | 600 ms | DynamoDB GSI1 query + folder JOIN |
| `GET /documents/{id}` (metadata) | 60 ms | 200 ms | 400 ms | Single DynamoDB GetItem |
| `GET /documents/{id}/content` | 200 ms | **500 ms** | 1 s | S3 snapshot + DDB pending-ops merge |
| `PUT /documents/{id}/content` | 250 ms | 700 ms | 1.5 s | yrs apply + S3 PutObject |
| `GET /search?q=…` | 200 ms | **800 ms** | 1.5 s | Tantivy BM25 + result-row hydration |
| `GET /ws` (WebSocket upgrade) | 30 ms | **100 ms** | 200 ms | Pure handshake; document load happens after |
| `POST /messages` | 100 ms | 300 ms | 600 ms | DDB PutItem + Redis pubsub |
| `POST /admin/*` mutations | 150 ms | 400 ms | 800 ms | Single-actor; audit-row write is part of the path |
| `* /scim/v2/*` | 200 ms | 500 ms | 1 s | Per-IdP automation; not user-facing |
| `POST /export/{format}` (sync) | 1 s | 3 s | 6 s | XLSX/HTML/Markdown CPU-bound; PDF/DOCX go async (Phase 6) |
| All other 2xx routes (catch-all) | 150 ms | 500 ms | 1 s | Default budget; explicit override added as new routes land |

Status-code dimension is preserved so an alarm can fire on 5xx p95
without 4xx noise dragging the signal around.

### Per-page TTI / load (web vitals)

Measured from `frontend/src/rum.rs` (M-P9 piece C). Three pages
are explicitly budgeted; everything else inherits the "other" row.
Cold WASM = first load after deploy / hard refresh; warm = WASM
cached, only data fetches on the wire.

| Page | LCP (cold) | LCP (warm) | TTI (cold) | TTI (warm) | CLS | INP |
|---|---|---|---|---|---|---|
| `/` (home) | 2.5 s | 1.0 s | **2 s** | 1 s | < 0.1 | < 200 ms |
| `/d/{id}` (editor) | 3.5 s | 1.5 s | **3 s** | 1.5 s | < 0.1 | < 200 ms |
| `/d/{id}` (spreadsheet) | 4.5 s | 2.0 s | **4 s** | 2 s | < 0.1 | < 200 ms |
| `/login`, `/share/...`, all other | 2.5 s | 1.0 s | 2 s | 1 s | < 0.1 | < 200 ms |

Web-vitals definitions track the standards: LCP (Largest
Contentful Paint), CLS (Cumulative Layout Shift), INP (Interaction
to Next Paint — supersedes FID as of 2024). The RUM sampler emits
all four; alarms fire on LCP cold and INP.

### Bundle size

Measured by the M-P9 piece B CI job: `trunk build --release`,
`gzip -9 *_bg.wasm`, byte count of resulting file.

| Asset | Aspirational budget | Current baseline (2026-06-04) | CI gate threshold |
|---|---|---|---|
| `ogrenotes-frontend_bg.wasm` (gzipped) | **800 KB** | **~1.66 MB** (last measured main, 2026-07-10; was 2.07 MB on 2026-06-03, pre size-opt) | Fail PR if > **1.76 MiB** (1,850,000 B) |
| `ogrenotes-frontend.js` glue (gzipped) | 100 KB | 12 KB | Tracked, no gate |
| Total app shell (HTML + CSS + WASM + glue, gzipped) | 1 MB | ~1.20 MB | Tracked, no gate |

**Ratchet, not anchor.** The CI gate is set at the current
measured baseline plus a small headroom (~7%), not at the
aspirational budget. The aspirational figure is what we want; the
gate is what we have. Two independent disciplines come out of this:

- **Don't regress.** A PR that pushes the bundle above the current
  threshold fails CI. As bundle-shrinking work lands, the
  threshold steps **down** (never up except for deliberate, doc-
  noted bumps).
- **Drive the budget down.** A separate optimization track
  (candidates: split out wasm-bindgen-test from release builds,
  strip serde derives from internal-only DTOs, audit duplicate
  regex compiles, evaluate `wasm-snip` for unreachable panics,
  lazy-load the spreadsheet engine out of the document bundle)
  closes the gap between baseline and aspirational budget.

> **2026-06-03 re-baseline (doc-noted bump).** The gate was raised
> from 2.0 MiB (2,097,152 B) to ~2.19 MiB (2,300,000 B). Over the ~2
> weeks since the 2026-05-21 / 1.87 MB baseline, accumulated Phase-5
> features grew the gzipped bundle to **2,168,852 B (~2.07 MiB)** —
> the link-sharing PR (#43) added the last ~10–30 KB that crossed the
> old gate; it was the trigger, not the bulk. The new ceiling is the
> measured size + ~6% headroom (matching the original ratchet), **not**
> an abandonment of the budget: the recovery is owed by the
> spreadsheet-split optimization (tracked separately). The gate must
> step back **down** as that work lands.

> **2026-06-04 ratchet DOWN (the "first cheap win" landed).** The
> deploy-only size-optimization described below shipped: the gate
> drops from 2.19 MiB (2,300,000 B) to **~1.48 MiB (1,550,000 B)**.
> The measured gzipped bundle fell from 2,168,852 B to **~1,425,747 B
> (~1.36 MiB), a ~34% cut** — `lto = "fat"` is the dominant
> contributor, with `opt-level = "z"`, `codegen-units = 1`,
> `panic = "abort"`, and `strip` stacking on top. Gate = measurement
> + ~9% headroom (absorbs local↔CI toolchain drift); tighten once CI
> confirms the baseline. The spreadsheet-split lazy-load remains the
> next lever toward the 800 KB aspiration.

The frontend `[profile.release]` (`frontend/Cargo.toml`) stays
tuned for build speed (`codegen-units = 256`, `lto = false`) so
local `trunk serve`/`--release` and the Playwright e2e build stay
fast. The size flags are applied **deploy-only**, via
`CARGO_PROFILE_RELEASE_*` env overrides on the two builds that ship
or measure the bundle — Dockerfile Stage 2 (the frontend builder)
and the `bundle-size.yml` gate step — which must stay in lockstep.
Because `lto = "fat"` makes LLVM emit bulk-memory ops while
wasm-bindgen strips the `target_features` section, those builds also
pass `data-wasm-opt-params="--all-features"` (in `index.html`) so
wasm-opt validates against the right feature set instead of MVP.

### Backend criterion benchmarks (regression gates)

Measured by the M-P9 piece D `crates/collab/benches/` suite.
Trend-relative, not absolute: each PR's bench results compared
against `main` baseline.

| Benchmark | Baseline | CI gate |
|---|---|---|
| `yrs_apply_update` (10 KB update) | TBD on first run | Fail PR if > 20% slower than main |
| `yrs_apply_update_large` (1 MB doc, 100 KB update) | TBD | Fail PR if > 20% slower |
| `doc_serialize` (1 MB doc → bytes) | TBD | Fail PR if > 20% slower |
| `doc_deserialize` (1 MB bytes → Doc) | TBD | Fail PR if > 20% slower |
| `search_snippet` (10 KB doc → snippet) | TBD | Fail PR if > 20% slower |
| `formula_eval_chain` (100-cell dependency chain) | TBD | Fail PR if > 20% slower |

Status: 4 of these 6 benches are implemented today in
`crates/collab/benches/yrs_ops.rs` (`yrs_apply_update`, its large
variant, `doc_serialize`, `doc_deserialize`). `search_snippet` and
`formula_eval_chain` are specified but not yet written, and the suite is
not yet wired into CI (see below).

The 20% threshold is loose enough to absorb GitHub-Actions runner
variance (typically ±10%) and tight enough to catch a genuine
regression. Tighter gating waits for self-hosted bench runners.

### Backend dependency p95 (not user-facing alarms, only dashboard)

These don't get alarms — they get a dashboard tile so when an
upstream alarm fires we can see where to look.

| Dependency | Target p95 | Notes |
|---|---|---|
| DynamoDB GetItem | 20 ms | Single-partition; eventual-consistency reads |
| DynamoDB Query (GSI) | 40 ms | Up to 1 MB page |
| DynamoDB PutItem / UpdateItem | 30 ms | Single-table |
| S3 GetObject (< 1 MB) | 80 ms | Snapshot loads |
| S3 PutObject (< 1 MB) | 100 ms | Snapshot writes |
| Redis publish | 5 ms | Local cluster |
| Redis get/set | 3 ms | Local cluster |
| Tantivy search query | 100 ms | In-process; CPU-bound |

## Frontend RUM

### Sampler design

`frontend/src/rum.rs` (M-P9 piece C):

```rust
// Pseudo-code
fn init() {
    if !sample_in() { return; }            // 10% sample
    on(window, "load", emit_nav_timing);
    web_vitals_observer().subscribe(|metric| emit_vital(metric));
}

fn emit_vital(m: Vital) {
    fetch_no_cors("/api/v1/metrics/rum", {
        method: "POST",
        body: { metric: m.name, value: m.value, page: page_kind(), ts: now() },
    });
}
```

- **Sampling rate** — 10%, hardcoded as `const SAMPLE_RATE = 0.10`
  in `frontend/src/rum.rs` (no env override). Sample decision is
  per-session, not per-event, so a sampled session emits all of its
  vitals or none.
- **Page kind** — `home | editor | spreadsheet | other`. Avoids
  per-doc-id cardinality on CloudWatch dimensions.
- **What's emitted (v1)** — LCP, FCP, and nav-timing
  (`domContentLoaded`, `loadEventEnd`) on a single beacon. INP, CLS,
  and a TTI estimate are deferred (they need an ongoing observer).
- **Beacon** — one `gloo_net` POST to `/api/v1/metrics/rum`, fired
  ~1.5 s after the window `load` event (not `sendBeacon`; not on unload).

### Ingestion endpoint

`POST /api/v1/metrics/rum` in `crates/api/src/routes/metrics.rs`
(new). Body shape:

```json
{
  "page": "editor",
  "vitals": {
    "lcp": 1234.5,
    "inp": 87.3,
    "cls": 0.04,
    "fcp": 890.2,
    "nav_dcl": 720.1,
    "nav_load": 1100.0
  },
  "user_agent_class": "desktop"
}
```

Server-side validation:
- `page` ∈ allowed set; reject unknown
- Each vital ≥ 0 and ≤ 60 s (sanity cap)
- 1 KB body limit
- Rate-limited per IP via the existing Redis bucket (`enforce("rum")`)
  to prevent a malicious client from poisoning aggregates

Forwarded to the metrics recorder under the same `OgreNotes`
namespace, dimensioned by `(page, user_agent_class)`. Each vital
becomes one histogram: `rum.lcp_ms`, `rum.inp_ms`, `rum.cls`
(unitless), etc. Reusing the existing namespace (instead of a
parallel `OgreNotes/RUM` namespace) keeps dashboards single-source.

The `user_agent_class` dimension is `desktop | mobile | tablet`
derived server-side from the `User-Agent` header — bounded
cardinality, useful for "mobile editor TTI" slices.

## CI gates (M-P9 piece B + D)

### WASM bundle size (piece B)

GitHub Actions workflow `.github/workflows/bundle-size.yml`. Runs
on every PR and push to main.

```yaml
env:
  WASM_GZ_LIMIT: "1850000"   # ~1.76 MiB

steps:
  - run: cd frontend && trunk build --release
  - run: |
      WASM=$(ls dist/*_bg.wasm | head -1)
      GZ=$(gzip -9 -c "$WASM" | wc -c)
      echo "wasm gzip=$GZ"
      [ "$GZ" -le "$WASM_GZ_LIMIT" ] || {
        echo "::error::WASM bundle $GZ > $WASM_GZ_LIMIT (gzipped)"; exit 1; }
```

**Threshold rationale.** The original Phase 5 target was 850 KB
gzipped — aspirational, taken from the HLD performance section
before the v1 feature surface was fully scoped. By the time
M-P9 piece A landed it was clear the realistic baseline with
Leptos runtime + yrs + fluent-rs + pulldown-cmark + ammonia +
zip + admin pages is ~1.87 MB gzipped. The CI gate started at
2 MiB (2,097,152 bytes) and was re-baselined on 2026-06-03 to
**~2.19 MiB (2,300,000 bytes)** after accumulated Phase-5 features
grew the measured bundle to ~2.07 MiB (see the "Ratchet, not
anchor" re-baseline note above) — still a ratchet, not an anchor,
so the build doesn't get bigger by accident, while leaving the
aspirational shrinking work as a separate optimization track
(lazy-loaded admin/SAML/SCIM routes, dead-code audit, splitting
the spreadsheet engine out of the document bundle).

PR comment annotation: shows current size + delta vs main.

### Bench regression (piece D)

Planned — **not yet implemented.** The bench harness exists
(`crates/collab/benches/yrs_ops.rs`, run with `cargo bench --bench
yrs_ops`) but there is no `bench-regression` job in
`.github/workflows/ci.yml` and no `scripts/check-bench-regression.sh`
driver yet. The intended shape is a job comparing criterion results
against a cached `main` baseline and failing a PR when a bench is >20%
slower:

```yaml
# planned, not yet wired:
- run: cargo bench --bench yrs_ops -- --save-baseline main
- run: git checkout ${{ github.head_ref }} && cargo bench --bench yrs_ops -- --baseline main
- run: scripts/check-bench-regression.sh 20  # fails if > 20% slower
```

Baseline would be cached per branch and rebuilt nightly from main.

## Dashboard (M-P9 piece F)

`infra/lib/dashboard.ts` — dashboard-as-code, deployed
alongside the stack. Layout:

1. **Top row** — API SLA health: 4 widgets, one per family
   (`/auth/*`, `/documents`, `/search`, `/ws`). p50/p95/p99 lines
   with horizontal SLA-target annotations.
2. **Second row** — Frontend RUM: 3 widgets per page kind (home,
   editor, spreadsheet). p75/p95 of LCP and INP with web-vitals
   threshold annotations.
3. **Third row** — Backend deps: DynamoDB, S3, Redis, Tantivy
   latency tiles.
4. **Fourth row** — Throughput + error rate: request rate per route,
   5xx rate per route.
5. **Fifth row** — Saturation: ECS CPU/memory, DynamoDB consumed
   capacity, Redis memory, WS active connections.

Annotations:
- Each SLA-target line on its widget
- Vertical lines on deploys (sourced from CodePipeline → CW Logs
  → metric filter)

## Alarms (M-P9 piece F)

One alarm per **user-facing** SLA. Backend-dependency SLAs get
dashboard tiles only — they're diagnostic, not paging signals.

| Alarm | Metric | Threshold | Window | Action |
|---|---|---|---|---|
| `api-auth-p95-breach` | `api.request_latency_ms`{route=~/auth/*} p95 | > 200 ms | 3 × 5-min | SNS → on-call email |
| `api-doc-list-p95-breach` | …{route=`GET /documents`} p95 | > 300 ms | 3 × 5-min | SNS |
| `api-doc-content-p95-breach` | …{route=`GET /documents/{id}/content`} p95 | > 500 ms | 3 × 5-min | SNS |
| `api-search-p95-breach` | …{route=`GET /search`} p95 | > 800 ms | 3 × 5-min | SNS |
| `api-ws-upgrade-p95-breach` | …{route=`GET /ws`} p95 | > 100 ms | 3 × 5-min | SNS |
| `api-5xx-rate` | `api.requests_total`{status=~5..} rate | > 1% of total for 2 × 5-min | — | SNS (paging) |
| `rum-editor-lcp-cold` | `rum.lcp_ms`{page=editor} p75 | > 3.5 s | 3 × 15-min | SNS |
| `rum-spreadsheet-lcp-cold` | `rum.lcp_ms`{page=spreadsheet} p75 | > 4.5 s | 3 × 15-min | SNS |
| `wasm-bundle-overrun` | n/a (CI gate, not runtime) | size > 2 MiB gzipped | n/a | PR fails |

The "3 consecutive 5-minute periods" pattern prevents single-spike
noise from paging. SNS topic reuses Phase 4 M-E7's
`ogrenotes-ops-alerts`. Runbook entries live in
`runbook/sla-alarms.md` (M-P9 piece F).

## Load tests (M-P9 piece E)

`tests/load/` — `goose-rs`-based. Scenarios:

| Scenario | User mix | Target |
|---|---|---|
| `read_heavy` | 80% list+open, 20% edit | 1000 concurrent, p95 in SLA |
| `edit_heavy` | 30% list, 70% sustained edit | 500 concurrent (collab is heavier) |
| `chat_heavy` | 50% read, 50% post-message | 1000 concurrent |
| `search_spike` | All searching, varied queries | 200 concurrent burst |

Runs nightly against a scratch deploy via GitHub Actions
`.github/workflows/load-tests.yml`. Pass = every route's p95
stays inside SLA at target concurrency. Weekly on full prod-shaped
test env.

Out of scope for Phase 5: chaos scenarios, multi-region failover
load, sustained 24-hour soak tests. Phase 5 ships the harness +
nightly baseline.

## Phase close criteria (M-P9)

- `design/performance-budgets.md` (this doc) shipped.
- `crates/api/src/routes/metrics.rs` exposes `POST /metrics/rum`
  with input validation + rate limiting.
- `frontend/src/rum.rs` samples 10% of sessions, emits web vitals
  + nav timing.
- WASM bundle CI gate green for last 5 main builds.
- `crates/collab/benches/` exists with at least 4 benchmarks; CI
  job runs them; threshold script flags > 20% regressions.
- `tests/load/` exists with at least `read_heavy` scenario; nightly
  workflow green for one week.
- `infra/lib/dashboard.ts` deployed; every widget renders
  data.
- 6 user-facing-SLA alarms armed; runbook entries in
  `runbook/sla-alarms.md` cover each.
- One alarm has fired & been acknowledged end-to-end (synthetic
  trigger acceptable) to prove the wiring works.

## v2 / out-of-scope carry-forwards

- SLO error-budget accounting (rolling-window burn-rate alarms,
  error-budget policy → freeze).
- Synthetic monitoring beyond load tests (uptime probes, golden-
  signal canaries).
- Per-tenant performance isolation / per-workspace SLA tiering.
- Customer-facing status page.
- Real-user-monitoring at 100% sampling (10% is the Phase 5
  target).
- Self-hosted bench runners with tighter regression gates (< 10%).
- Chaos engineering scenarios in load tests.
- Long-soak (24h+) sustained load tests.

## References

- [`high-level-design.md`](high-level-design.md) — "Performance
  budgets" Phase 5 line
- [`mvp-detailed-design.md`](mvp-detailed-design.md) — existing
  metrics inventory
- `crates/common/src/metrics/emf.rs` — EMF emitter
- `crates/api/src/middleware/metrics.rs` — per-request histograms
- AWS docs — EMF spec v1, CloudWatch dashboards-as-code
- web.dev — Core Web Vitals (LCP, INP, CLS)
