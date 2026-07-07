# Qdrant operations runbook

Phase 6 M-6.1 piece D. Operator-facing recipes for the vector
store that backs the Phase 6.2 semantic-search path and the
Phase 6.3 agent's `semantic_search` tool. Covers the most common
"what just broke" investigations plus the rarer rebuild /
migration flows.

The Qdrant deployment is in `scripts/aws-test-deploy.sh`
Phase 2b (EFS), Phase 6 (Cloud Map + ECS service). Reference
config lives with the M-6.1 milestone.

## What's deployed

| Resource | Name | Notes |
|---|---|---|
| ECS service | `${PREFIX}ogrenote-qdrant` | Fargate Spot, 1 task, `qdrant/qdrant:v1.13.0` |
| ECS cluster | `${PREFIX}ogrenote` | Same cluster as the API |
| Task family | `${PREFIX}ogrenote-qdrant` | 512 CPU / 1024 MiB |
| EFS filesystem | `${PREFIX}qdrant-data` | encrypted, generalPurpose, bursting |
| EFS access point | `${PREFIX}qdrant-data-ap` | POSIX UID/GID 1000, root `/qdrant`, 0755 |
| Mount targets | one per public subnet | restricted to EFS SG |
| Cloud Map namespace | `${PREFIX%-}.internal` | private DNS, tied to the VPC |
| Cloud Map service | `qdrant.${PREFIX%-}.internal` | A records, TTL 10 |
| Qdrant SG | `${PREFIX}qdrant-sg` | ingress 6333/6334 from ECS SG only |
| EFS SG | `${PREFIX}efs-sg` | ingress 2049 from Qdrant SG only |
| Log group | `/ecs/${PREFIX}ogrenote-qdrant` | 14-day retention |

API talks to Qdrant via `QDRANT_URL=http://qdrant.${PREFIX%-}.internal:6334`.
**Port 6334 is gRPC, not REST.** The `qdrant_client` crate the API
uses (`Qdrant::from_url`) speaks gRPC over HTTP/2; pointing at the
REST port 6333 panics the API at startup with an h2 protocol
error. The REST port is exposed for the operator-side `curl`
recipes elsewhere in this runbook but not used by the application
code. The URL is computed by the deploy script and baked into the
API task definition; operators should not override it.

Cost (steady state, test stack): ~$15-20/mo — Fargate Spot
($8-10), EFS storage <$1, Cloud Map registration $0.50/HMS,
data transfer $0 (same-VPC).

## Health check

The fastest "is Qdrant up" check from outside the VPC isn't
possible — the service is intentionally not reachable from the
public ALB. To verify from operator laptop:

```bash
# 1. Confirm the ECS service has a healthy task running.
aws ecs describe-services \
    --cluster "${PREFIX}ogrenote" \
    --services "${PREFIX}ogrenote-qdrant" \
    --query 'services[0].{Status:status,Running:runningCount,Desired:desiredCount,Events:events[0:3]}' \
    --output table --region "${AWS_REGION}"
```

Expect `Status=ACTIVE`, `Running=Desired=1`. If `Running<Desired`,
the task is restarting — read the events for the reason.

```bash
# 2. Confirm Cloud Map resolves to a live task IP.
aws servicediscovery list-instances \
    --service-id "$(aws servicediscovery list-services \
        --filters \"Name=NAMESPACE_ID,Values=$(aws servicediscovery \
            list-namespaces --filters 'Name=TYPE,Values=DNS_PRIVATE' \
            --query \"Namespaces[?Name=='${PREFIX%-}.internal'].Id|[0]\" \
            --output text)\" \
        --query 'Services[?Name==`qdrant`].Id|[0]' --output text)" \
    --query 'Instances[*].{Id:Id,IP:Attributes.AWS_INSTANCE_IPV4}' \
    --output table --region "${AWS_REGION}"
```

Expect ≥ 1 instance row with a private IP in your VPC CIDR.

```bash
# 3. Hit Qdrant from inside the cluster via an exec into the API task.
TASK=$(aws ecs list-tasks --cluster "${PREFIX}ogrenote" \
    --service-name "${PREFIX}ogrenote-api" \
    --query 'taskArns[0]' --output text --region "${AWS_REGION}")
aws ecs execute-command --cluster "${PREFIX}ogrenote" \
    --task "$TASK" --container ogrenote-api --interactive \
    --command "curl -s http://qdrant.${PREFIX%-}.internal:6333/" \
    --region "${AWS_REGION}"
```

Expect JSON like `{"title":"qdrant - vector search engine","version":"1.13.0"}`.
If the curl hangs, the SG rule between API and Qdrant is wrong;
if it errors fast, the DNS isn't resolving.

## Common failures

### Qdrant task stuck in PROVISIONING

Most common: EFS mount target missing in the AZ the task landed
in. Fargate picks a subnet at random from the configured pair;
if only one subnet has a mount target, half of all task starts
hang at "Resource initialization failed."

Check:
```bash
aws efs describe-mount-targets --file-system-id <fs-id> \
    --query 'MountTargets[*].{Subnet:SubnetId,State:LifeCycleState}' \
    --output table --region "${AWS_REGION}"
```

Expect two `available` mount targets, one per subnet. If one is
missing, re-run `aws-test-deploy.sh` — Phase 2b is idempotent
and will create the missing mount target.

### Qdrant task crashes immediately with `permission denied`

The container starts as root but the EFS access point coerces
operations to UID/GID 1000. If you've manually written into the
EFS root directory as a different UID (e.g. via a debug bastion
mount), the Qdrant data path may have stale ownership.

Recover by deleting the access point and re-running the deploy
script (which recreates it with the right POSIX user). The
filesystem itself stays — only the access point is recreated.
**Note**: deleting the access point + recreating does NOT lose
the existing data, but Qdrant may fail to bind if previously-
written files are owned by a different UID. In that case, see
"Full reindex from scratch" below.

### `/api/v1/ask` returns 503 even with the SSM key set

Three things to verify in order:

1. **SSM parameter visible to the lookup**:
   ```bash
   aws ssm describe-parameters \
       --parameter-filters "Key=Name,Values=/${PREFIX}ogrenote/anthropic-api-key" \
       --query 'Parameters[0].{Name:Name,Type:Type,LastModified:LastModifiedDate}' \
       --output table --region "${AWS_REGION}"
   ```
2. **Execution role has `ssm:GetParameters` on the parameter**:
   ```bash
   aws iam get-role-policy --role-name "${PREFIX}ogrenote-exec" \
       --policy-name "ogrenote-exec-ssm" --output table
   ```
   If `NoSuchEntity`, re-run `aws-test-deploy.sh` — the policy is
   applied unconditionally in Phase 4 IAM since M-6.1 piece C.
3. **Task def actually has the `secrets` block**: open the
   latest revision in the AWS console; you should see
   `secrets:[{name: ANTHROPIC_API_KEY, valueFrom: arn:aws:ssm:...}]`
   in the container definition. If absent, the deploy ran before
   the SSM parameter existed — re-run after creating it.

This is the footgun mentioned in the M-6.1 piece C commit:
running `aws-redeploy.sh` against a stack whose `aws-test-deploy.sh`
hasn't been re-run since the piece-C commit means the execution
role lacks `ssm:GetParameters`. The fix is one
`aws-test-deploy.sh` run.

### Embedding pipeline starts in degraded mode

API log line at startup:
```
WARN qdrant_url not set; embedding pipeline disabled
```

Means the API's `QDRANT_URL` env var is empty. Either:

- `aws-test-deploy.sh` was run before Phase 6 M-6.1 piece A
  landed (no Cloud Map namespace yet). Re-run the deploy.
- The Cloud Map namespace exists but the lookup returned `None`
  (typo / region mismatch). Verify with the health-check
  command at the top of this runbook.

### `bedrock:AccessDenied` in embedding-pipeline logs

API log line:
```
ERROR bedrock: AccessDeniedException — model access not granted
```

The IAM grant is in place (task role since M-6.1 piece B) but
the AWS account hasn't opted into the model. Open the Bedrock
console, Model access → Modify model access, tick
`amazon.titan-embed-text-v2:0` (or whatever
`EMBEDDING_MODEL_ID` resolves to), submit. First-party Amazon
models approve instantly; Cohere models may take a minute.
Force a new ECS deployment to retry — or wait, the pipeline
retries on the next document mutation.

## EFS backup + restore

EFS doesn't have native snapshots; use **AWS Backup** for
scheduled snapshots, or take a one-shot snapshot before a risky
operation.

### One-shot snapshot before a rebuild

```bash
# Find the recovery point ARN for the existing backup vault.
VAULT=$(aws backup list-backup-vaults \
    --query 'BackupVaultList[0].BackupVaultName' --output text)

# Start a backup of the EFS filesystem.
EFS_ARN=$(aws efs describe-file-systems \
    --query "FileSystems[?Tags[?Key=='Name' && Value=='${PREFIX}qdrant-data']].FileSystemArn|[0]" \
    --output text)
aws backup start-backup-job \
    --backup-vault-name "$VAULT" \
    --resource-arn "$EFS_ARN" \
    --iam-role-arn "arn:aws:iam::${ACCOUNT_ID}:role/service-role/AWSBackupDefaultServiceRole" \
    --region "${AWS_REGION}"
```

The backup runs async; check via `aws backup describe-backup-job`.

### Restore from a snapshot

```bash
# List recovery points for the resource.
aws backup list-recovery-points-by-resource \
    --resource-arn "$EFS_ARN" \
    --query 'RecoveryPoints[*].{Date:CreationDate,Arn:RecoveryPointArn}' \
    --output table

# Restore to a new EFS filesystem (you can't restore in-place).
aws backup start-restore-job \
    --recovery-point-arn "<arn-from-above>" \
    --metadata '{"newFileSystem":"true"}' \
    --iam-role-arn "arn:aws:iam::${ACCOUNT_ID}:role/service-role/AWSBackupDefaultServiceRole" \
    --resource-type EFS \
    --region "${AWS_REGION}"
```

Then update the Qdrant task definition's
`efsVolumeConfiguration.fileSystemId` to the new filesystem id
and force a new deployment.

## Full reindex from scratch

When to do this:

- After upgrading the embedding model (e.g. Titan v1 → v2 — the
  vector dimensions change, existing vectors become useless).
- After EFS corruption / unrecoverable state.
- During M-6.3 validation when tuning chunking parameters
  invalidates the existing index.

The flow:

1. **Stop the Qdrant service** (don't delete it):
   ```bash
   aws ecs update-service --cluster "${PREFIX}ogrenote" \
       --service "${PREFIX}ogrenote-qdrant" \
       --desired-count 0 --region "${AWS_REGION}"
   ```
2. **Clear the EFS data** — exec into a one-shot task with the
   EFS mounted:
   ```bash
   # Build a small task def with debian:latest + the EFS volume,
   # or run the qdrant task with a different command. Either way,
   # `rm -rf /qdrant/storage/*` from inside the container.
   ```
   Alternative: delete the access point + recreate (see the
   permission-denied recipe above), which leaves the underlying
   files orphaned. Both paths work; the rm is cleaner.
3. **Scale Qdrant back up**:
   ```bash
   aws ecs update-service --cluster "${PREFIX}ogrenote" \
       --service "${PREFIX}ogrenote-qdrant" \
       --desired-count 1 --region "${AWS_REGION}"
   ```
4. **Trigger reindex from the API**. The current scheme is
   fire-and-forget on document mutation. The cleanest way is an
   admin endpoint that walks every doc and calls
   `EmbeddingPipeline::index_document`. As of M-6.1 there's no
   such endpoint; the M-6.3 validation milestone is the first
   piece to add one. Until then, the workaround is to touch each
   doc by re-saving its snapshot via the existing
   `PUT /documents/{id}/content` route — best for small corpora.

## Schema migration (vector-dimension change)

When changing `EMBEDDING_MODEL_ID` to a model with different
vector dimensions (e.g. 1024 → 1536), the existing Qdrant
collection rejects the new vectors with a dimension-mismatch
error.

Recovery:

1. Delete the Qdrant collection that the embeddings crate
   creates (named `documents` by default). Exec into the API
   task and `curl -X DELETE http://qdrant....:6333/collections/documents`.
2. Update `EMBEDDING_MODEL_ID` and `EMBEDDING_DIMENSIONS` env
   vars on the ECS task definition; redeploy.
3. The embeddings crate creates the new collection on first
   write with the right dimensions.
4. Run a full reindex (above).

## Quick cost check

```bash
aws ce get-cost-and-usage \
    --time-period Start=$(date -u -d '30 days ago' +%Y-%m-%d),End=$(date -u +%Y-%m-%d) \
    --granularity DAILY \
    --metrics UnblendedCost \
    --filter '{
        "And": [
            {"Dimensions":{"Key":"SERVICE","Values":["Amazon Elastic Container Service","Amazon Elastic File System","Amazon Bedrock"]}},
            {"Tags":{"Key":"Stack","Values":["'${PREFIX}'"]}}
        ]
    }' \
    --query 'ResultsByTime[*].{Date:TimePeriod.Start,Cost:Total.UnblendedCost.Amount}' \
    --output table
```

Steady-state expectation: $0.50-0.70/day per stack. Bedrock
embedding charges show up only on indexing days (re-embed of a
freshly imported corpus or model swap).

## v2 / Phase 6.3 carry-forwards

- **Multi-task Qdrant cluster**. Single-task is the v1 design;
  the moment we need HA the Cloud Map service routing flips to
  MULTIVALUE A records with > 1 instance and the Qdrant config
  picks up the cluster-mode env vars. Today's MULTIVALUE config
  is set up for the multi-task future even though only one task
  registers.
- **Snapshots to S3** instead of EFS. Qdrant supports built-in
  S3 snapshot upload; cheaper than EFS for the static index but
  doesn't get the live-rebuild story EFS provides.
- **Bedrock Knowledge Bases** as an alternative to Qdrant. Zero-
  ops managed service. Migration path documented in
  `design/rag-implementation-plan.md`.
