# OgreNotes Infrastructure (AWS CDK / TypeScript)

Infrastructure-as-code for OgreNotes. Replaces the bespoke
`scripts/aws-test-deploy.sh` / `aws-redeploy.sh` bash harness. CDK
generates CloudFormation from the TypeScript app and deploys it.

## Layout

```
infra/
  bin/app.ts              entrypoint — selects env, builds the stack
  config.ts               typed per-env config (the config.env equivalent)
  lib/
    ogrenotes-stack.ts    wires the four constructs
    network.ts            Phase 1   — VPC (no NAT), SGs, gateway endpoints
    data.ts               Phase 1b/2/2b — DynamoDB (8 GSIs, PITR), S3, Redis, EFS
    compute.ts            Phase 3–6 — ECS cluster, api/qdrant/worker, autoscaling
    ops.ts                Phase 7/9 — budget; SLA dashboard; prod-only backup
    dashboard.ts          Phase 5 M-P9 — SLA dashboard widget definitions
  lambda/backup/index.py  DDB→S3 export Lambda (prod only)
```

## 1. Configure (one-time)

```bash
cd infra
npm install
npx cdk bootstrap        # once per account/region
```

`config.ts` holds the **structural** per-env settings (sizing, feature
flags). The **deployment-specific** values — `OAUTH_CLIENT_ID`,
`GOOGLE_CLIENT_ID`, `DOMAIN_NAME`, `NOTIFICATION_EMAIL` — are read from the
environment at synth time (step 2), not edited into `config.ts`. Copy the
template and fill it in:

```bash
cp ../scripts/aws-test-config.env.example ../scripts/aws-test-config.env
# then edit ../scripts/aws-test-config.env
```

### Prerequisite: secrets (SSM SecureString)

Secrets are NOT in config or env — they're read from SSM at deploy time.
Create them before the first deploy (per stack `prefix`):

```bash
PREFIX=test1-         # MUST match the `-c prefix=` you deploy with (step 2)
REGION=us-east-1

aws ssm put-parameter --name "/${PREFIX}ogrenote/oauth-client-secret" \
    --type SecureString --value "<github-oauth-secret>" --region "$REGION"
aws ssm put-parameter --name "/${PREFIX}ogrenote/jwt-secret" \
    --type SecureString --value "<32+ byte random>" --region "$REGION"
# only if config.aiEnabled = true:
aws ssm put-parameter --name "/${PREFIX}ogrenote/anthropic-api-key" \
    --type SecureString --value "sk-ant-..." --region "$REGION"
```

Bedrock model access (for embeddings) is still a one-time console toggle —
see `scripts/aws-test-config.env.example`.

## Deploying — shared preamble

Every deploy/diff/destroy command starts by exporting the deployment values
and **must** pass the stack prefix:

> **Export the values with `set -a`.** `config.ts` reads them from the
> environment at synth time. The file is plain `VAR=...`, so a plain `source`
> leaves them unexported and the synth silently degrades (placeholder OAuth,
> HTTPS torn down). Always:
> `set -a && source ../scripts/aws-test-config.env && set +a`
> - `DOMAIN_NAME` (e.g. `ogrenotes.example.com`) → HTTPS + ACM + Route53; unset
>   means a bare-ALB http stack (auto-enables `DEV_MODE`). The Route53 zone is
>   derived from the domain; set `HOSTED_ZONE_NAME` only for a multi-level TLD.
> - `OAUTH_CLIENT_ID` (GitHub) and `GOOGLE_CLIENT_ID` (optional). Skip them and
>   GitHub OAuth deploys a placeholder id (login breaks) / Google is off.
> - `NOTIFICATION_EMAIL` for the budget + alarm SNS.
>
> Secrets (`*_SECRET`) come from SSM, never these vars.

> ⚠️ **The prefix is load-bearing.** `bin/app.ts` reads it ONLY from CDK
> context (`-c prefix=`); the env file's `STACK_PREFIX` is ignored and the
> default is `test-`. Deploying with the wrong/missing prefix renames every
> resource → a CloudFormation **replacement** of the live stack. Always pass
> the existing prefix — **the live test stack is `test1-`** — and `npm run
> diff` first.

## 2. First deploy

```bash
set -a && source ../scripts/aws-test-config.env && set +a
GIT_STAMP=$(git rev-parse --short HEAD) npm run deploy -- -c env=test -c prefix=test1-
```

If you deployed **without a `DOMAIN_NAME`** (bare-ALB http stack), copy the
`AlbDnsName` output into `config.frontendOrigin` and redeploy so the OAuth
redirect / CORS resolve (the FRONTEND_ORIGIN two-step). With `DOMAIN_NAME`
set this is unnecessary. `-c env=prod` selects the prod slice (HTTPS via ACM,
larger sizing, backup export).

## 3. Redeploy / iterate

```bash
set -a && source ../scripts/aws-test-config.env && set +a

# preview — for a code change, confirm the only diff is the ECS task-def
# image rolls before you apply.
npm run diff -- -c env=test -c prefix=test1-

# full deploy (infra + code)
GIT_STAMP=$(git rev-parse --short HEAD) npm run deploy -- -c env=test -c prefix=test1-
```

> **hotswap is currently a no-op on this stack.** `deploy:hotswap` can't
> resolve the `RedisEndpoint.Port` attribute and silently falls back to "no
> changes" without rolling the service — use the full `deploy` for code
> changes until that's fixed. (`--hotswap` is dev/test only regardless; it
> drifts CloudFormation — never use it for prod.)

## 4. Tear down

```bash
set -a && source ../scripts/aws-test-config.env && set +a
npm run destroy -- -c env=test -c prefix=test1-
```

`destroy` removes the stack, but the **DynamoDB table and S3 bucket are
RETAINed even in test** (a data-loss backstop — see `data.ts`); they survive
and must be deleted by hand if you really want them gone:

```bash
aws dynamodb delete-table --table-name test1-ogrenote --region us-east-1
aws s3 rb s3://test1-ogrenote --force --region us-east-1
```

The SSM secrets from step 1 aren't stack-managed either — delete them when
retiring a prefix:

```bash
for n in oauth-client-secret jwt-secret anthropic-api-key; do
  aws ssm delete-parameter --name "/test1-ogrenote/$n" --region us-east-1
done
```

## Environment cost knobs

Each environment in `config.ts` carries a few deliberate cost/observability
switches. Pick per env — the defaults below are what ships.

| Knob | Default | test | prod | Choose based on |
|---|---|---|---|---|
| `cpuArch` | `X86_64` | `X86_64` | `X86_64` | ARM64 (Graviton) saves ~20% compute **but** needs a native arm64 build runner — on an x86 host the image builds under emulation (~70 min/commit, vs ~10 min native). Only set a stack to `ARM64` once it has that runner. |
| `offHours` | *(omitted → 24/7)* | Tue-night + weekend | *(omitted)* | Scale api/worker/qdrant to 0 outside dev hours. Non-prod only — never on prod (availability; no auto-wake). |
| `qdrantSpot` | `false` | `true` | `false` | Fargate Spot saves ~70% but a reclaim drops search/`/ask` ~1–2 min. Fine for test; prod on-demand. |
| `containerInsights` | *(prod on)* | `false` | `true` | ~88 custom metrics ≈ $26/mo. Worth it for prod observability; wasteful on test (basic free `AWS/ECS` metrics suffice). |

### Off-hours scale-to-zero (`offHours`)

When `config.offHours` is set, `compute.ts` provisions:

- an **off-hours toggle Lambda** (`<prefix>ogrenote-offhours-toggle`) that sets
  api/qdrant `desiredCount` and, for the worker, lowers its Application Auto
  Scaling **MinCapacity to 0** first (else target-tracking scales it straight
  back up), then sets its desired count;
- **EventBridge Scheduler** up/down triggers — two per window — evaluated in
  the config's IANA `timezone` (DST-aware). EventBridge Scheduler is used
  rather than native ECS scheduled scaling specifically because only it honours
  a timezone; the native path is UTC-only and would drift an hour twice a year.

**Reshape the schedule** by editing the `windows` cron pairs in `config.ts`
(each `{ up, down }` is one UP window, in the stack's local `timezone`). Add or
remove a pair to add/remove a window; delete `offHours` to return to 24/7.

**Behaviour when down:** the ALB target group is empty, so any request gets a
**503** until the next scheduled `up`. There is no auto-wake. For an
off-schedule session, wake it manually (healthy in ~2 min):

```bash
scripts/offhours.sh up      # or: down
```

A manual `up`/`down` only lasts until the next scheduled trigger.

**Cost note:** scale-to-zero only removes the *variable* cost (Fargate + the
per-task public IPv4s). The ALB and ElastiCache Redis bill 24/7 regardless.

## Notes

- `GIT_STAMP` flows into the Docker `GIT_HASH` build-arg so the in-app
  version row matches the deployed image (CDK otherwise tags assets by
  content hash, not git SHA).
- Critical non-default settings preserved in `compute.ts`: ALB idle_timeout
  120s, sticky sessions (1h), `/health` check, Qdrant on FARGATE_SPOT. See
  `../private_local/iac-migration-mapping.md` for the full mapping.
- The SLA dashboard is defined in `dashboard.ts` and created via
  `CfnDashboard` in `ops.ts` (name `<prefix>ogrenote-sla`). The old
  `aws cloudwatch put-dashboard` + `cloudwatch-dashboard.json` flow is gone —
  CDK is authoritative.
- S3 lifecycle (`tmp/` objects expire after 7 days) and the DynamoDB schema
  (8 GSIs) live in `data.ts`. The former standalone fragments
  (`dynamodb/tables.json`, `s3/bucket-policy.json`) were deleted.
