# OgreNotes — Administrator's Guide

Working reference for operating a deployed OgreNotes stack. Focuses on the
tasks you actually do (grant admin, enable AI, redeploy, tail logs, save
cost) rather than the full CDK/architecture surface — the design docs in
`design/` cover that.

Every command in this guide assumes you've sourced the deployment env once:

```sh
set -a
source scripts/aws-test-config.env
set +a
```

Fill it from `scripts/aws-test-config.env.example`. The CDK stack + AWS
CLI both read from these variables.

---

## 1. First-admin bootstrap

The first time a stack comes up there are no admins. Two ways to get one:

**Env-var promotion (recommended).** Set `ADMIN_EMAILS` in the deploy env
BEFORE the user logs in for the first time:

```sh
ADMIN_EMAILS=you@example.com,ops@example.com
```

Every login checks the caller's email against this comma-separated list
(case-insensitive) and promotes the User row to `role=admin` if it matches.
Existing users get promoted on their next login. See
`crates/api/src/auth_policy.rs::apply_admin_email_promotion`.

Redeploy after editing the value (env changes ride the task definition;
existing tasks don't pick them up until they restart):

```sh
sg docker -c 'set -a; source scripts/aws-test-config.env; set +a; \
  GIT_STAMP=$(git rev-parse --short HEAD) \
  npm --prefix infra run deploy -- -c env=test -c prefix=test1- --require-approval never'
```

**Direct DDB write.** For a one-off, no-redeploy path:

```sh
USER_ID=<user_id>                        # see §3 for lookup
aws dynamodb update-item \
  --table-name "${DYNAMODB_TABLE_PREFIX}ogrenote" \
  --key '{"PK":{"S":"USER#'"$USER_ID"'"},"SK":{"S":"PROFILE"}}' \
  --update-expression "SET #r = :admin, updated_at = :ts" \
  --expression-attribute-names '{"#r":"role"}' \
  --expression-attribute-values '{":admin":{"S":"admin"},":ts":{"N":"'"$(date +%s%6N)"'"}}' \
  --region "$AWS_REGION"
```

`is_admin` is read live from the User row on every request
(`crates/api/src/middleware/auth.rs:17`), so the change takes effect on the
next API call — no logout required.

---

## 2. Admin API endpoints

Bearer-token auth. Every endpoint requires `role >= Admin`; non-admins get
403 before any state mutation.

| Method + Path | Purpose |
|---|---|
| `GET /api/v1/admin/users` | list users |
| `GET /api/v1/admin/users/:id` | fetch one |
| `POST /api/v1/admin/users/:id/disable` | soft-disable (blocks login, keeps data) |
| `POST /api/v1/admin/users/:id/enable` | unblock login |
| `POST /api/v1/admin/users/:id/promote` | grant admin |
| `POST /api/v1/admin/users/:id/demote` | revoke admin |
| `GET  /api/v1/admin/users/:id/ask-policy` | read AI policy |
| `PUT  /api/v1/admin/users/:id/ask-policy` | set AI policy — see §4 |
| `GET /api/v1/admin/audit` | admin + security audit rows |
| `GET /api/v1/admin/metrics` | in-process counters + gauges |
| `POST /api/v1/admin/documents/:id/compact` | force snapshot compaction |
| `POST /api/v1/admin/documents/:id/repair-liveapp-attrs` | repair Kanban/Calendar attrs |
| `PATCH /api/v1/admin/documents/:id/link-settings` | override link-sharing on a doc |

Rate-limited via `admin_mut_rate_limit` per acting admin. Every mutation
records an `AdminAudit` row (`AdminAuditAction::{Disable,Enable,Promote,
Demote,SetAskPolicy}`) with `{from, to}` detail — retained permanently.

Example (get an admin token via dev-login, then use it):

```sh
BASE=https://ogrenotes.example.com
TOKEN=$(curl -s -X POST "$BASE/api/v1/auth/dev-login" \
  -H 'Content-Type: application/json' \
  -d '{"email":"you@example.com","name":"You"}' | jq -r .token)

# Promote another user
curl -X POST "$BASE/api/v1/admin/users/$TARGET_ID/promote" \
  -H "Authorization: Bearer $TOKEN"
```

---

## 3. Finding a user

By email (DDB scan — fine for small stacks):

```sh
aws dynamodb scan \
  --table-name "${DYNAMODB_TABLE_PREFIX}ogrenote" \
  --filter-expression "email = :e" \
  --expression-attribute-values '{":e":{"S":"you@example.com"}}' \
  --max-items 3 \
  --region "$AWS_REGION" \
  --query 'Items[*].{user_id:user_id.S,role:role.S,ask_policy:ask_policy.S,ask_enabled:ask_enabled.BOOL}'
```

By user_id (targeted get):

```sh
aws dynamodb get-item \
  --table-name "${DYNAMODB_TABLE_PREFIX}ogrenote" \
  --key '{"PK":{"S":"USER#'"$USER_ID"'"},"SK":{"S":"PROFILE"}}' \
  --region "$AWS_REGION"
```

User rows: `PK=USER#<user_id>`, `SK=PROFILE`. See
`crates/storage/src/models/user.rs::User` for the field list.

---

## 4. AI-assistant policy

Three-state per-user policy on `/api/v1/ask` (#148). Set via the admin
endpoint or by writing `ask_policy` directly to the User row.

| Policy | Availability | BYOK header (`x-anthropic-key`) |
|---|---|---|
| `disabled` | 403 | ignored (never reached) |
| `system_only` | 200 via operator's key | 400 with "Remove your custom key in Settings" |
| `system_or_byok` | 200 either way | operator's key OR user's key |

**Defaults:**
- New OAuth users default to `disabled` — admin must explicitly opt them in.
- Dev-login (`POST /api/v1/auth/dev-login`) auto-opens to `system_or_byok`
  for test convenience.

**Admin bypass:** `role >= Admin` bypasses **both** the `disabled` and the
`system_only`-with-BYOK denial paths. Admins can always ask and can always
BYOK. Operator cost caps still apply on the system-key path (admin doesn't
skip quotas).

Set policy via API:

```sh
curl -X PUT "$BASE/api/v1/admin/users/$USER_ID/ask-policy" \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"policy":"system_or_byok"}'   # or "system_only" or "disabled"
```

Or directly on the DDB row (also removes the legacy `ask_enabled` field in
the same update, which is what the repo write path does):

```sh
POLICY=system_or_byok      # or system_only, disabled
aws dynamodb update-item \
  --table-name "${DYNAMODB_TABLE_PREFIX}ogrenote" \
  --key '{"PK":{"S":"USER#'"$USER_ID"'"},"SK":{"S":"PROFILE"}}' \
  --update-expression "SET ask_policy = :val, updated_at = :ts REMOVE ask_enabled" \
  --expression-attribute-values '{":val":{"S":"'"$POLICY"'"},":ts":{"N":"'"$(date +%s%6N)"'"}}' \
  --region "$AWS_REGION"
```

**Legacy migration.** Users written before this policy shipped only have
`ask_enabled: bool`. The `User::ask_policy()` getter derives from that
(`true` → `SystemOrByok`, `false` → `Disabled`), so no backfill is
required. Any policy write via `set_ask_policy` REMOVEs the legacy field
in the same update.

### BYOK — bring your own key

Users can supply their own Anthropic API key via browser localStorage. The
frontend forwards it in the `x-anthropic-key` header; the server NEVER
persists it. BYOK requests skip the operator's per-user quotas + cost
circuit-breaker (the user is paying). `disabled` and `system_only` policies
block BYOK; `system_or_byok` allows it.

### Global AI cost caps

Set via `AppConfig` (`crates/common/src/config.rs`). Not admin-flippable at
runtime; requires a redeploy to change.

---

## 5. Deployment

### First-time / clean deploy

```sh
./scripts/aws-test-deploy.sh
```

Provisions VPC, IAM, ALB, ECS, ECR, DynamoDB, S3, Redis, Qdrant. The
`aws-test-config.env` file governs table prefix, region, custom domain,
OAuth secrets. See `crates/scripts/aws-test-deploy.sh` for the phase-by-
phase breakdown; deploy failures should route through the
`aws-deploy-doctor` diagnostic agent.

### Redeploy code changes

```sh
sg docker -c 'set -a; source scripts/aws-test-config.env; set +a; \
  GIT_STAMP=$(git rev-parse --short HEAD) \
  npm --prefix infra run deploy -- -c env=test -c prefix=test1- --require-approval never'
```

Builds the WASM frontend + backend, pushes ECR images, updates ECS task
definitions. Rolling deploy — no downtime. Total time ~9 minutes for the
`test1-` stack.

Prod uses `scripts/aws-redeploy.sh` which reads the same env but points at
a different prefix / stack.

### Off-hours cost saver

The test stack has an EventBridge schedule that scales ECS + Qdrant to
zero outside working hours (see `scripts/offhours.sh` for the toggle
Lambda). Manual override:

```sh
./scripts/offhours.sh up      # scale everything back up (~2 min)
./scripts/offhours.sh down    # scale to zero
```

Manual `up` lasts only until the next scheduled `down`.

### Tear down

```sh
./scripts/aws-test-destroy.sh
```

Nukes the stack. Non-recoverable.

---

## 6. Diagnostics and logs

### CloudWatch logs

```sh
./scripts/aws-test-logs.sh                        # follow live
./scripts/aws-test-logs.sh --since 1h             # backfill
./scripts/aws-test-logs.sh --filter-pattern '"ERROR"'
```

Prefers the read-only diagnostic profile if configured (see
§`scripts/aws-test-create-readonly.sh`), so tailing doesn't need admin
creds. `x-request-id` is emitted by TraceLayer on every request — grep for
it to correlate frontend / backend logs.

### Metrics snapshot

```sh
curl -s "$BASE/api/v1/admin/metrics" -H "Authorization: Bearer $TOKEN" | jq
```

Returns three `BTreeMap`s (counters, gauges, histograms). Relevant AI
counters:

- `ask.rejected_disabled_total`
- `ask.rejected_byok_not_allowed_total`
- `ask.byok_calls_total`
- `ask.calls_total`

### Audit log

```sh
curl -s "$BASE/api/v1/admin/audit?limit=50" -H "Authorization: Bearer $TOKEN" | jq
```

Combines `AdminAudit` (mutations, retained permanently) and
`SecurityAudit` (login, MFA, SAML, SCIM, share-revoke, doc-delete;
retained 90 days by the `audit_retention` worker).

### AWS-side diagnostic agents

Route runtime issues through:

- `aws-deploy-doctor` — deploy or ECS steady-state failures
- `aws-diagnostic` — general read-only inspection (DDB, S3, CW, ECS state)
- `aws-network-doctor` — 5xx / ALB / target group / DNS / ACM
- `aws-iam-doctor` — AccessDenied / policy questions
- `ci-doctor` / `test-triage-doctor` — GitHub Actions failures

These are Claude Code sub-agents (`.claude/agents/`); not shell scripts.

---

## 7. Environment / feature knobs

Set in the deployment env (`scripts/aws-test-config.env`) or the task
definition. Changing any of these requires a redeploy.

| Variable | Purpose |
|---|---|
| `ADMIN_EMAILS` | comma-sep list; matching users auto-promoted on login (see §1) |
| `ANTHROPIC_API_KEY` | operator's Claude key; unset ⇒ system-key path returns 503 |
| `AWS_REGION` | e.g. `us-east-1` |
| `AWS_PROFILE` | AWS CLI + CDK credentials |
| `DYNAMODB_TABLE_PREFIX` | e.g. `test1-` |
| `DEPLOY_ENV` | metric dimension label (`test` / `staging` / `prod`) |
| `DOMAIN_NAME` | custom domain (requires Route 53 hosted zone) |
| `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET` | OAuth |
| `LIVEAPP_STRICT_VALIDATION` | `true`/`false` — enforce Kanban/Calendar attr shape |
| `LIVEAPP_GATE_WALK_SCOPE` | `full` / `changed` — Phase 3 rollout dial |
| `LIVEAPP_GATE_EXEMPT_DOC_IDS` | comma-sep doc ids that bypass validation |
| `MFA_ENCRYPTION_KEY` | base64 32-byte key; wraps stored MFA secrets |
| `QDRANT_URL` | auto-populated by deploy; do not set manually |
| `REDIS_URL` | in-VPC ElastiCache endpoint |
| `SMTP_USERNAME`, `SMTP_PASSWORD` | outbound email (notifications, invites) |
| `OGRE_REQUIRE_SAML_HAPPY` | SAML SCIM test toggle |

---

## 8. Data hygiene

### Dev-data reset

Wipe docs / folders / snapshots but keep user accounts:

```sh
./scripts/reset-dev-data.sh
```

Add `--all` to wipe users too. Never run against prod.

### Trash cleanup

The `trash_worker` background task hard-deletes trashed docs after their
retention window (default 30 days). Runs continuously in the worker ECS
service.

### Audit retention

The `audit_retention` worker prunes `SecurityAudit` rows older than 90
days. `AdminAudit` rows are permanent.

### Duplicate-user merge

If two accounts share an email (rare — historical bug):

```sh
./scripts/migrate-merge-duplicate-users.sh
```

Merges the older row into the newer one; check the script for the
exact matching heuristic before running.

### S3 backup exports

Phase 4 M-E4 daily exports write to the S3 bucket named in the
`OgreNotes.BucketName` CDK output. Retention on the bucket itself — set
via lifecycle rule (see the CDK stack).

---

## 9. Ops runbooks

Long-form reproduction recipes for production issues live under
`runbook/`. Refer there when triaging a live incident; that dir is the
authoritative source for step-by-step procedures.

---

## 10. When something's broken

- **Login fails** — check ADMIN_EMAILS wasn't stale; check
  `SecurityAudit` for the failed row (§6).
- **`/api/v1/ask` returns 403** — the user's `ask_policy` is `disabled`,
  see §4.
- **`/api/v1/ask` returns 400 with "Remove your custom key"** — user's
  policy is `system_only` but they sent an `x-anthropic-key`. Either flip
  policy to `system_or_byok` or have them remove the key from Settings.
- **ALB returns 503** — off-hours schedule kicked in; run
  `scripts/offhours.sh up`.
- **ECS won't reach steady state** — route to `aws-deploy-doctor`.
- **CI is red** — route to `ci-doctor` or `test-triage-doctor`.
