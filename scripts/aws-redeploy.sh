#!/usr/bin/env bash
#
# OgreNote — Rebuild Docker image, update task definition, and redeploy to ECS.
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-redeploy.sh

set -euo pipefail

# Disable the AWS CLI v2 pager so describe/query output doesn't open `less`
# for every call (which makes the script look hung and hides progress).
export AWS_PAGER=""

# Always run from the repo root, regardless of where the script was
# invoked from. Without this, `docker build … .` would use whatever
# directory the user's shell happens to be in — silently building the
# wrong source if that dir contains a Dockerfile.
cd "$(dirname "$0")/.."

# By default the script blocks until ECS reports steady state (~2-3 min
# after task def registration), rendering a 5-phase progress bar. Pass
# `--no-wait` to fire-and-forget — useful in CI or when chaining redeploys.
WAIT_FOR_STEADY=true
for arg in "$@"; do
    case "$arg" in
        --wait)    WAIT_FOR_STEADY=true ;;   # accepted for backward compat
        --no-wait) WAIT_FOR_STEADY=false ;;
        *) echo "ERROR: unknown argument: $arg" >&2; exit 2 ;;
    esac
done

# Refuse to run under a read-only diag profile (today's CreateVpc footgun).
if [ "${AWS_PROFILE:-}" = "${STACK_PREFIX:-}ogrenote-diag" ] \
   || [ "${AWS_PROFILE:-}" = "${STACK_PREFIX:-}ogrenote-deploy-diag" ]; then
    echo "ERROR: refusing to run redeploy under read-only diag profile (${AWS_PROFILE})." >&2
    echo "       \`unset AWS_PROFILE\` first (or switch to your admin profile)." >&2
    exit 1
fi

for var in AWS_REGION STACK_PREFIX OAUTH_CLIENT_ID OAUTH_CLIENT_SECRET JWT_SECRET; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source your config file first."
        exit 1
    fi
done

REGION="${AWS_REGION}"
PREFIX="${STACK_PREFIX}"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
ECR_URI="${ACCOUNT_ID}.dkr.ecr.${REGION}.amazonaws.com/${PREFIX}ogrenote"
TASK_FAMILY="${PREFIX}ogrenote-api"
CLUSTER_NAME="${PREFIX}ogrenote"
SERVICE_NAME="${PREFIX}ogrenote-api"
LOG_GROUP="/ecs/${PREFIX}ogrenote"
# Phase 6 M-6.4 piece D: async-worker service. Redeploy must refresh
# the worker alongside the API or it silently runs the old image —
# which matters the moment a worker-side change ships (e.g. M-6.5 DOCX
# support). Created by aws-test-deploy.sh; this script only rolls it.
WORKER_TASK_FAMILY="${PREFIX}ogrenote-worker"
WORKER_SERVICE_NAME="${PREFIX}ogrenote-worker"
WORKER_LOG_GROUP="/ecs/${PREFIX}ogrenote-worker"

# Clean up temp files on exit
TASKDEF_TMP=$(mktemp)
WORKER_TASKDEF_TMP=$(mktemp)
chmod 600 "$TASKDEF_TMP" "$WORKER_TASKDEF_TMP"
trap 'rm -f "$TASKDEF_TMP" "$WORKER_TASKDEF_TMP"' EXIT

echo "=== OgreNote Redeploy ==="
echo ""

# ─── Pre-flight sanity: refuse to deploy into a mismatched runtime ─
#
# The 2026-07-03 incident: STACK_PREFIX had been `test1-` in the
# env file for weeks while CDK config declared `test-`, so the
# task role IAM policy granted access to `test-ogrenote/*` while
# the app read `test1-ogrenote/*`. Every deploy silently pushed
# to the wrong-prefixed ECR and updated a task def whose IAM
# permissions couldn't touch the tables the app was configured
# to read. Login broke, but nothing caught it until users noticed.
#
# These three assertions catch that failure mode BEFORE the
# script does anything destructive:
#
#   1. ECR repo `${PREFIX}ogrenote` exists — a push to a
#      non-existent repo returns a "repository does not exist"
#      error from ECR after the whole image is built. Catches
#      "wrong prefix" and "repo got deleted" in seconds.
#   2. The currently running task def's DYNAMODB_TABLE_PREFIX
#      env var matches STACK_PREFIX — protects against a
#      redeploy that would break existing users the moment it
#      lands, because the new task def would inherit the new
#      env-var and lose access to what's already in DDB.
#   3. The IAM task role's DynamoDB policy resource ARNs match
#      STACK_PREFIX — the specific mismatch that broke login.
#
# All three are quick reads; failing here saves 10+ min of
# rebuild + deploy + steady-state wait for a doomed change.
echo "Pre-flight sanity checks…"

if ! aws ecr describe-repositories --repository-names "${PREFIX}ogrenote" --region "$REGION" >/dev/null 2>&1; then
    echo "ERROR: ECR repository '${PREFIX}ogrenote' does not exist." >&2
    echo "       STACK_PREFIX=${STACK_PREFIX} may be wrong for this account/region," >&2
    echo "       or the repo was deleted. Create it with:" >&2
    echo "         aws ecr create-repository --repository-name ${PREFIX}ogrenote --region ${REGION}" >&2
    echo "       or run cdk deploy to recreate infra." >&2
    exit 1
fi
echo "  ✓ ECR repo ${PREFIX}ogrenote exists"

RUNNING_PREFIX=$(aws ecs describe-services --cluster "$CLUSTER_NAME" --services "$SERVICE_NAME" --region "$REGION" \
    --query 'services[0].taskDefinition' --output text 2>/dev/null || echo "NONE")
if [ "$RUNNING_PREFIX" != "NONE" ]; then
    RUNTIME_TABLE_PREFIX=$(aws ecs describe-task-definition --task-definition "$RUNNING_PREFIX" --region "$REGION" \
        --query 'taskDefinition.containerDefinitions[0].environment[?name==`DYNAMODB_TABLE_PREFIX`].value | [0]' \
        --output text 2>/dev/null || echo "NONE")
    if [ "$RUNTIME_TABLE_PREFIX" != "NONE" ] && [ "$RUNTIME_TABLE_PREFIX" != "$STACK_PREFIX" ]; then
        echo "ERROR: Running task def uses DYNAMODB_TABLE_PREFIX=${RUNTIME_TABLE_PREFIX}" >&2
        echo "       but this deploy would push STACK_PREFIX=${STACK_PREFIX}." >&2
        echo "       Deploying would break users who have data under the current prefix." >&2
        echo "       Update aws-test-config.env to match, or run cdk deploy for a controlled migration." >&2
        exit 1
    fi
    echo "  ✓ Running task def prefix matches STACK_PREFIX"
fi

TASK_ROLE_ARN=$(aws ecs describe-task-definition --task-definition "$RUNNING_PREFIX" --region "$REGION" \
    --query 'taskDefinition.taskRoleArn' --output text 2>/dev/null || echo "NONE")
if [ "$TASK_ROLE_ARN" != "NONE" ]; then
    ROLE_NAME=$(basename "$TASK_ROLE_ARN")
    # Find the first inline policy and check its DDB Resource. If we don't find any
    # DDB grant matching STACK_PREFIX, warn — this was the exact 2026-07-03 signature.
    POLICY_NAME=$(aws iam list-role-policies --role-name "$ROLE_NAME" \
        --query 'PolicyNames | [0]' --output text 2>/dev/null || echo "")
    if [ -n "$POLICY_NAME" ] && [ "$POLICY_NAME" != "None" ]; then
        DDB_TABLE_ARN=$(aws iam get-role-policy --role-name "$ROLE_NAME" --policy-name "$POLICY_NAME" \
            --query "PolicyDocument.Statement[?contains(Action[0], \`dynamodb\`)].Resource | [0][0]" \
            --output text 2>/dev/null || echo "")
        if [ -n "$DDB_TABLE_ARN" ] && [[ "$DDB_TABLE_ARN" != *":table/${STACK_PREFIX}ogrenote"* ]]; then
            echo "ERROR: Task role IAM policy grants DDB access to '${DDB_TABLE_ARN}'" >&2
            echo "       but STACK_PREFIX=${STACK_PREFIX} expects 'table/${STACK_PREFIX}ogrenote'." >&2
            echo "       This mismatch (2026-07-03 incident) causes AccessDenied on every DDB call." >&2
            echo "       Run cdk deploy to reconcile the IAM policy, or patch it inline." >&2
            exit 1
        fi
        echo "  ✓ Task role IAM policy prefix matches STACK_PREFIX"
    fi
fi

echo ""

# ─── Provenance: log exactly what's being shipped ───────────────
# Without this, "I redeployed, why isn't it live?" is unanswerable.
GIT_SHA=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
GIT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
GIT_STAMP="${GIT_SHA}"
DIRTY=""
if ! git diff-index --quiet HEAD -- 2>/dev/null; then
    DIRTY=" (dirty: uncommitted changes will be included in the image)"
    GIT_STAMP="${GIT_SHA}-dirty"
fi
echo "Source: ${GIT_BRANCH} @ ${GIT_SHA}${DIRTY}"
echo "Build context: $(pwd)"
echo ""

# ─── Build + Push ───────────────────────────────────────────────

# `GIT_HASH` is wired through to the frontend builder so the WASM bundle
# stamps the same SHA the sidebar version row displays. Without the
# build-arg, Docker has no `.git/` to read from (.dockerignore) and the
# stamp would be "unknown".
#
# DOCKER_BUILDKIT=1 is required for the `# syntax=` directive and
# `RUN --mount=type=cache,…` cache mounts in Dockerfile to take effect.
# Without it the cache mounts silently degrade to no-op bind dirs and
# every build re-fetches all crates.
echo "Building Docker image..."
DOCKER_BUILDKIT=1 docker build \
    --build-arg "GIT_HASH=${GIT_STAMP}" \
    --tag "${PREFIX}ogrenote:latest" \
    --tag "${PREFIX}ogrenote:${GIT_STAMP}" \
    -f Dockerfile .

# Push both :latest (so anything still pulling :latest gets the new build)
# and :GIT_STAMP (the immutable provenance tag the task def pins to). The
# second push is nearly free thanks to ECR layer dedup.
echo "Pushing to ECR..."
# #55: skip the explicit `docker login` — which writes a 12h ECR token in
# cleartext to ~/.docker/config.json (the "credentials stored unencrypted"
# warning) — when amazon-ecr-credential-helper is installed and mapped for
# this registry. `docker push` then authenticates via the helper on demand,
# leaving nothing on disk. Falls back to the login otherwise so the deploy
# still works without the helper. One-time opt-in: runbook/ecr-credential-helper.md.
if command -v docker-credential-ecr-login >/dev/null 2>&1 \
    && grep -qs "ecr-login" "${DOCKER_CONFIG:-$HOME/.docker}/config.json" \
    && grep -qs "${ACCOUNT_ID}.dkr.ecr" "${DOCKER_CONFIG:-$HOME/.docker}/config.json"; then
    echo "ECR auth via amazon-ecr-credential-helper (no on-disk token)."
else
    aws ecr get-login-password --region "${REGION}" | \
        docker login --username AWS --password-stdin "${ACCOUNT_ID}.dkr.ecr.${REGION}.amazonaws.com"
fi
docker tag "${PREFIX}ogrenote:${GIT_STAMP}" "${ECR_URI}:${GIT_STAMP}"
docker push "${ECR_URI}:${GIT_STAMP}"
docker tag "${PREFIX}ogrenote:latest" "${ECR_URI}:latest"
docker push "${ECR_URI}:latest"

# ─── Re-register task definition with current env vars ──────────

echo "Updating task definition..."

EXEC_ROLE_ARN=$(aws iam get-role --role-name "${PREFIX}ogrenote-exec" \
    --query 'Role.Arn' --output text)
TASK_ROLE_ARN=$(aws iam get-role --role-name "${PREFIX}ogrenote-task" \
    --query 'Role.Arn' --output text)

# Look up the ElastiCache cluster created by aws-test-deploy.sh. If it exists,
# pin REDIS_URL to its endpoint; otherwise fall back to the env var (for
# legacy stacks that never provisioned ElastiCache, or local dev).
CACHE_CLUSTER_ID="${PREFIX}redis"
REDIS_ENDPOINT_JSON=$(aws elasticache describe-cache-clusters \
    --cache-cluster-id "$CACHE_CLUSTER_ID" --show-cache-node-info \
    --query 'CacheClusters[0].CacheNodes[0].Endpoint' --output json \
    --region "${REGION}" 2>/dev/null || echo "null")
if [ "$REDIS_ENDPOINT_JSON" != "null" ] && [ -n "$REDIS_ENDPOINT_JSON" ]; then
    REDIS_HOST=$(echo "$REDIS_ENDPOINT_JSON" | sed -n 's/.*"Address": *"\([^"]*\)".*/\1/p')
    REDIS_PORT=$(echo "$REDIS_ENDPOINT_JSON" | sed -n 's/.*"Port": *\([0-9]*\).*/\1/p')
    if [ -n "$REDIS_HOST" ] && [ -n "$REDIS_PORT" ]; then
        REDIS_URL="redis://${REDIS_HOST}:${REDIS_PORT}"
        echo "Redis endpoint (ElastiCache): ${REDIS_URL}"
    fi
fi
REDIS_URL="${REDIS_URL:-redis://localhost:6379}"

# Phase 6 M-6.1 piece A: Qdrant DNS via Cloud Map. The full provisioning
# lives in aws-test-deploy.sh; the redeploy script trusts that it already
# ran and just resolves the well-known DNS name. If the namespace doesn't
# exist yet (legacy stack), QDRANT_URL is left empty and the API
# embedding pipeline starts disabled — same fallback behavior as omitting
# the env var entirely.
CLOUDMAP_NAMESPACE_NAME="${PREFIX%-}.internal"
CLOUDMAP_NS_ID=$(aws servicediscovery list-namespaces \
    --filters "Name=TYPE,Values=DNS_PRIVATE" \
    --query "Namespaces[?Name=='${CLOUDMAP_NAMESPACE_NAME}'].Id | [0]" \
    --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$CLOUDMAP_NS_ID" != "None" ] && [ -n "$CLOUDMAP_NS_ID" ] && [ "$CLOUDMAP_NS_ID" != "null" ]; then
    QDRANT_URL="http://qdrant.${CLOUDMAP_NAMESPACE_NAME}:6334"
    echo "Qdrant endpoint (Cloud Map): ${QDRANT_URL}"
else
    QDRANT_URL=""
    echo "Cloud Map namespace not present; QDRANT_URL left empty (embedding pipeline will be disabled)"
fi

# Resolve env vars (same logic as aws-test-deploy.sh)
if [ -z "${DOMAIN_NAME:-}" ]; then
    echo "ERROR: DOMAIN_NAME is not set. Export it before running this script:" >&2
    echo "  export DOMAIN_NAME=ogrenotes.example.com" >&2
    exit 1
fi
FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-https://${DOMAIN_NAME}}"
OAUTH_REDIRECT="${OAUTH_REDIRECT_URI:-https://${DOMAIN_NAME}/api/v1/auth/callback}"

DEV_MODE="${DEV_MODE:-false}"
DEPLOY_ENV="${DEPLOY_ENV:-prod}"

# Phase 6 M-6.1 piece C: ANTHROPIC_API_KEY SSM SecureString. Looks up
# the parameter created out-of-band by the operator; reference its
# ARN from the task def's `secrets` field if found, otherwise leave
# the secrets block empty (the /ask endpoint then returns 503).
ANTHROPIC_SSM_NAME="/${PREFIX}ogrenote/anthropic-api-key"
ANTHROPIC_SSM_ARN=$(aws ssm describe-parameters \
    --parameter-filters "Key=Name,Values=${ANTHROPIC_SSM_NAME}" \
    --query 'Parameters[0].ARN' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$ANTHROPIC_SSM_ARN" = "None" ] || [ -z "$ANTHROPIC_SSM_ARN" ] || [ "$ANTHROPIC_SSM_ARN" = "null" ]; then
    ANTHROPIC_SECRET_ENTRY=""
    echo "Anthropic SSM parameter absent; /ask endpoint will return 503"
else
    ANTHROPIC_SECRET_ENTRY="\"secrets\": [{\"name\": \"ANTHROPIC_API_KEY\", \"valueFrom\": \"${ANTHROPIC_SSM_ARN}\"}],"
    echo "Anthropic SSM parameter found: ${ANTHROPIC_SSM_NAME}"
fi

cat > "$TASKDEF_TMP" <<TASKDEF
{
    "family": "${TASK_FAMILY}",
    "networkMode": "awsvpc",
    "requiresCompatibilities": ["FARGATE"],
    "cpu": "256",
    "memory": "512",
    "executionRoleArn": "${EXEC_ROLE_ARN}",
    "taskRoleArn": "${TASK_ROLE_ARN}",
    "containerDefinitions": [{
        "name": "ogrenote-api",
        "image": "${ECR_URI}:${GIT_STAMP}",
        "essential": true,
        "portMappings": [{"containerPort": 3000, "protocol": "tcp"}],
        "environment": [
            {"name": "AWS_REGION", "value": "${REGION}"},
            {"name": "DYNAMODB_TABLE_PREFIX", "value": "${PREFIX}"},
            {"name": "S3_BUCKET", "value": "${PREFIX}ogrenote"},
            {"name": "REDIS_URL", "value": "${REDIS_URL}"},
            {"name": "QDRANT_URL", "value": "${QDRANT_URL}"},
            {"name": "OAUTH_CLIENT_ID", "value": "${OAUTH_CLIENT_ID}"},
            {"name": "OAUTH_CLIENT_SECRET", "value": "${OAUTH_CLIENT_SECRET}"},
            {"name": "OAUTH_REDIRECT_URI", "value": "${OAUTH_REDIRECT}"},
            {"name": "JWT_SECRET", "value": "${JWT_SECRET}"},
            {"name": "FRONTEND_ORIGIN", "value": "${FRONTEND_ORIGIN}"},
            {"name": "DEV_MODE", "value": "${DEV_MODE}"},
            {"name": "SEARCH_INDEX_PATH", "value": "/data/search-index"},
            {"name": "API_PORT", "value": "3000"},
            {"name": "ADMIN_EMAILS", "value": "${ADMIN_EMAILS:-}"},
            {"name": "DEPLOY_ENV", "value": "${DEPLOY_ENV}"}
        ],
        ${ANTHROPIC_SECRET_ENTRY}
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": "${LOG_GROUP}",
                "awslogs-region": "${REGION}",
                "awslogs-stream-prefix": "api"
            }
        }
    }]
}
TASKDEF

aws ecs register-task-definition --cli-input-json "file://${TASKDEF_TMP}" \
    --region "${REGION}" >/dev/null
echo "Registered task definition: ${TASK_FAMILY}"

# ─── Deploy ─────────────────────────────────────────────────────

echo "Deploying to ECS..."
SERVICE_STATUS=$(aws ecs describe-services --cluster "$CLUSTER_NAME" \
    --services "$SERVICE_NAME" \
    --query 'services[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$SERVICE_STATUS" != "ACTIVE" ]; then
    echo "ERROR: ECS service is in state '${SERVICE_STATUS}', not ACTIVE."
    echo "       Run ./scripts/aws-test-deploy.sh to recreate the service."
    exit 1
fi
aws ecs update-service \
    --cluster "$CLUSTER_NAME" \
    --service "$SERVICE_NAME" \
    --task-definition "$TASK_FAMILY" \
    --force-new-deployment \
    --region "${REGION}" >/dev/null

# ─── Roll the worker onto the same image (Phase 6 M-6.4 piece D) ──
# Same image + env as the API, launched as --mode=worker. No
# portMappings / ALB. If the worker service isn't present yet (legacy
# stack predating piece D), skip with a pointer to the deploy script
# rather than failing the API redeploy that already succeeded.
WORKER_SVC_STATUS=$(aws ecs describe-services --cluster "$CLUSTER_NAME" \
    --services "$WORKER_SERVICE_NAME" \
    --query 'services[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$WORKER_SVC_STATUS" = "ACTIVE" ]; then
    cat > "$WORKER_TASKDEF_TMP" <<WORKERDEF
{
    "family": "${WORKER_TASK_FAMILY}",
    "networkMode": "awsvpc",
    "requiresCompatibilities": ["FARGATE"],
    "cpu": "256",
    "memory": "512",
    "executionRoleArn": "${EXEC_ROLE_ARN}",
    "taskRoleArn": "${TASK_ROLE_ARN}",
    "containerDefinitions": [{
        "name": "ogrenote-worker",
        "image": "${ECR_URI}:${GIT_STAMP}",
        "essential": true,
        "command": ["ogrenotes-api", "--mode=worker"],
        "environment": [
            {"name": "AWS_REGION", "value": "${REGION}"},
            {"name": "DYNAMODB_TABLE_PREFIX", "value": "${PREFIX}"},
            {"name": "S3_BUCKET", "value": "${PREFIX}ogrenote"},
            {"name": "REDIS_URL", "value": "${REDIS_URL}"},
            {"name": "QDRANT_URL", "value": "${QDRANT_URL}"},
            {"name": "OAUTH_CLIENT_ID", "value": "${OAUTH_CLIENT_ID}"},
            {"name": "OAUTH_CLIENT_SECRET", "value": "${OAUTH_CLIENT_SECRET}"},
            {"name": "OAUTH_REDIRECT_URI", "value": "${OAUTH_REDIRECT}"},
            {"name": "JWT_SECRET", "value": "${JWT_SECRET}"},
            {"name": "FRONTEND_ORIGIN", "value": "${FRONTEND_ORIGIN}"},
            {"name": "DEV_MODE", "value": "${DEV_MODE}"},
            {"name": "SEARCH_INDEX_PATH", "value": "/data/search-index"},
            {"name": "ADMIN_EMAILS", "value": "${ADMIN_EMAILS:-}"},
            {"name": "DEPLOY_ENV", "value": "${DEPLOY_ENV}"},
            {"name": "WORKER_CONCURRENCY", "value": "${WORKER_CONCURRENCY:-2}"}
        ],
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": "${WORKER_LOG_GROUP}",
                "awslogs-region": "${REGION}",
                "awslogs-stream-prefix": "worker"
            }
        }
    }]
}
WORKERDEF
    aws ecs register-task-definition --cli-input-json "file://${WORKER_TASKDEF_TMP}" \
        --region "${REGION}" >/dev/null
    aws ecs update-service \
        --cluster "$CLUSTER_NAME" \
        --service "$WORKER_SERVICE_NAME" \
        --task-definition "$WORKER_TASK_FAMILY" \
        --force-new-deployment \
        --region "${REGION}" >/dev/null
    echo "Rolled worker service: ${WORKER_SERVICE_NAME}"
else
    echo "NOTE: worker service '${WORKER_SERVICE_NAME}' not ACTIVE (${WORKER_SVC_STATUS}); skipping."
    echo "      Run ./scripts/aws-test-deploy.sh to create it (Phase 6 M-6.4 piece D)."
fi

if [ "$WAIT_FOR_STEADY" = "true" ]; then
    echo ""
    echo "Waiting for ECS to roll out ${ECR_URI}:${GIT_STAMP}..."
    echo "  Phases: 1 register → 2 pull image → 3 task running →"
    echo "          4 ALB healthy / drain old → 5 stable"
    echo "  (Pass --no-wait to skip this and exit immediately.)"
    echo ""

    START=$(date +%s)
    DEADLINE=$((START + 600))   # 10 min cap, matches `aws ecs wait services-stable`
    BAR_WIDTH=30

    # \r in-place updates work fine on a TTY; in a piped/CI context they
    # just produce a long replayed line which is harmless in CloudWatch.
    print_progress() {
        local phase=$1 desc=$2 elapsed=$3
        local fill=$(( phase * BAR_WIDTH / 5 ))
        local bar="" i
        for ((i=0; i<BAR_WIDTH; i++)); do
            if [ $i -lt $fill ]; then bar="${bar}#"; else bar="${bar}-"; fi
        done
        local mins=$((elapsed / 60))
        local secs=$((elapsed % 60))
        # Pad description to 38 chars so trailing chars from a longer prior
        # message don't linger when the phase advances to a shorter one.
        printf "\r  [%s] %d/5 %-38s %d:%02d" \
            "$bar" "$phase" "$desc" "$mins" "$secs"
    }

    while :; do
        NOW=$(date +%s)
        ELAPSED=$((NOW - START))
        if [ "$NOW" -gt "$DEADLINE" ]; then
            printf "\n"
            echo "ERROR: ECS service did not stabilize within 10 minutes." >&2
            echo "       Investigate: aws ecs describe-services \\" >&2
            echo "         --cluster ${CLUSTER_NAME} --services ${SERVICE_NAME} --region ${REGION}" >&2
            echo "       Logs:        aws logs tail ${LOG_GROUP} --follow --region ${REGION}" >&2
            exit 1
        fi

        # One describe-services call yields four scalars (tab-separated):
        #   <num_deployments> <PRIMARY rolloutState> <PRIMARY runningCount> <PRIMARY desiredCount>
        # On transient API failure we treat it as "no info yet" and retry.
        STATUS=$(aws ecs describe-services \
            --cluster "$CLUSTER_NAME" \
            --services "$SERVICE_NAME" \
            --region "$REGION" \
            --query 'services[0].[length(deployments), deployments[?status==`PRIMARY`].rolloutState | [0], deployments[?status==`PRIMARY`].runningCount | [0], deployments[?status==`PRIMARY`].desiredCount | [0]]' \
            --output text 2>/dev/null || echo "0	NONE	0	0")
        NUM_DEPLOY=$(echo "$STATUS" | awk '{print $1}')
        ROLLOUT=$(echo    "$STATUS" | awk '{print $2}')
        RUN_CT=$(echo     "$STATUS" | awk '{print $3}')
        DESIRED=$(echo    "$STATUS" | awk '{print $4}')

        # rolloutState=FAILED means ECS gave up on the new deployment
        # (image-pull failure, repeated health-check failure, etc.).
        if [ "$ROLLOUT" = "FAILED" ]; then
            printf "\n"
            echo "ERROR: ECS deployment failed (rolloutState=FAILED)." >&2
            echo "       Logs: aws logs tail ${LOG_GROUP} --follow --region ${REGION}" >&2
            exit 1
        fi

        if [ "$ROLLOUT" = "COMPLETED" ] && [ "$NUM_DEPLOY" = "1" ]; then
            print_progress 5 "stable: task live and healthy" "$ELAPSED"
            printf "\n"
            break
        elif [ "$ROLLOUT" = "COMPLETED" ]; then
            print_progress 4 "draining old task" "$ELAPSED"
        elif [ "$RUN_CT" != "None" ] && [ "$RUN_CT" = "$DESIRED" ] && [ "$DESIRED" != "0" ]; then
            print_progress 3 "task running, awaiting ALB health" "$ELAPSED"
        elif [ "$NUM_DEPLOY" -ge 1 ] 2>/dev/null; then
            print_progress 2 "pulling image, starting task" "$ELAPSED"
        else
            print_progress 1 "registering deployment" "$ELAPSED"
        fi

        sleep 5
    done

    echo ""
    echo "  Service is stable."
else
    echo ""
    echo "Done. New task will start in ~2-3 minutes (--no-wait passed)."
    echo "  Logs: aws logs tail ${LOG_GROUP} --follow --region ${REGION}"
fi
echo ""
echo "  Image deployed: ${ECR_URI}:${GIT_STAMP}"
echo "  Config applied:"
echo "    OAUTH_REDIRECT_URI = ${OAUTH_REDIRECT}"
echo "    FRONTEND_ORIGIN    = ${FRONTEND_ORIGIN}"
echo "    ADMIN_EMAILS       = ${ADMIN_EMAILS:-}"
echo "    DEPLOY_ENV         = ${DEPLOY_ENV}"
