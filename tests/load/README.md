# OgreNotes load tests

Phase 5 M-P9 piece E — goose-rs harness for the SLAs codified in
`design/performance-budgets.md`. Single binary; scenarios selected
via goose's standard `--scenarios` flag.

## Scenarios

Goose canonicalizes scenario names by stripping every non-
alphanumeric character and lower-casing the result, and the
`--scenarios` flag refuses non-alphanumeric input. So the table
below lists each scenario's human-readable display name + the
canonical form you actually type at the CLI.

| Scenario (display) | CLI form | Status | Mix |
|---|---|---|---|
| `read-heavy` | `readheavy` | ✅ v1 | 80% list, 20% open, ~5% `/users/me` |
| `edit-heavy` | tbd | 30% list, 70% WS edit |
| `chat-heavy` | tbd | 50% read, 50% post-message |
| `search-spike` | tbd | All search, varied queries |

## Running locally

The target must have `DEV_MODE=true` so the harness can mint test
users via `/auth/dev-login`. Production builds with `DEV_MODE=false`
return 404 from that route by design — running the load test
against prod is a deliberate "no".

```bash
cargo run --release -p ogrenotes-load --manifest-path tests/load/Cargo.toml -- \
    --host http://127.0.0.1:3000 \
    --users 50 --hatch-rate 5 --run-time 60s \
    --scenarios readheavy \
    --report-file target/goose-report.html
```

The HTML report at `target/goose-report.html` shows per-request
p50/p95/p99/max + RPS. Compare the `GET /documents` p95 against
the SLA in `design/performance-budgets.md` ("API p95 latency"):
300 ms at target concurrency.

## CI

`.github/workflows/load-tests.yml` runs nightly against a fresh
local stack — DynamoDB Local + MinIO + Redis via docker-compose
and a release-built API server. The job uploads the goose report
as an artifact and fails if `GET /documents` p95 breaches its SLA.

Production-shape soak tests (>1 hour) and chaos scenarios are
out of scope for Phase 5 — see the v2 carry-forwards section of
`design/performance-budgets.md`.
