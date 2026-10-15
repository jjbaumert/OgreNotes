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
| `read-heavy` | `readheavy` | Smoke baseline | 80% home-folder listing, 20% `GET /users/me` |
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
    --report-file target/goose-report.html \
    --no-reset-metrics
```

The HTML report shows per-request latency and throughput. The driver also
prints a JSON gate summary and exits nonzero if login fails, fewer than
100 authenticated requests succeed, either measured route has no successes,
the total request error rate exceeds 1%, `GET /home-folder` p95 exceeds
300 ms, or `GET /users/me` p95 exceeds the 500 ms catch-all API budget.
Set `LOAD_SUMMARY_FILE` to retain the JSON summary,
`LOAD_MIN_SUCCESSFUL_LOGINS` to require a specific number of usable
sessions (nightly requires every configured user), and
`LOAD_MIN_AUTHENTICATED_REQUESTS` to override the 100-request floor for
a deliberately small local run. Metrics reset must remain disabled
(`--no-reset-metrics`) so login requests remain visible to the gates.

## CI

`.github/workflows/load-tests.yml` runs nightly against a fresh
local stack — DynamoDB Local + MinIO + Redis via docker-compose
and a release-built API server. The job uploads the goose report
and gate summary as artifacts, including on failure. It fails when any
of the gates above are breached.

This baseline creates fresh users, resolves each home folder from
`GET /users/me`, and reads that folder's empty child list through
`GET /folders/{id}`. The API's `/documents` collection is POST-only.
It does not seed accessible documents, open document content, or edit.
It therefore does not yet exercise the `read_heavy` list+open/edit mix
or the 1000-user target in `design/performance-budgets.md`; its nightly
50-user result is a smoke signal, not proof of that full target.

Production-shape soak tests (>1 hour) and chaos scenarios are
out of scope for Phase 5 — see the v2 carry-forwards section of
`design/performance-budgets.md`.
