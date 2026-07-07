#!/usr/bin/env bash
#
# OgreNote — AWS Budget Test Deployment
#
# Provisions a minimal AWS stack for testing:
#   VPC + subnets, DynamoDB, S3, ECR, ALB, ECS Fargate (1 task), budget alarm.
#
# Prerequisites:
#   - AWS CLI v2 configured with credentials
#   - Docker (for building the image)
#   - source scripts/aws-test-config.env (see aws-test-config.env.example)
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-deploy.sh
#
# Idempotent: safe to re-run. Existing resources are detected and skipped.

set -euo pipefail

# Disable the AWS CLI v2 pager so describe/query output doesn't open `less`
# for every call (which makes the script look hung and hides progress).
export AWS_PAGER=""

# Always run from the repo root, regardless of where the script was
# invoked from. Without this, `docker build … .` would use whatever
# directory the user's shell happens to be in — silently building the
# wrong source if that dir contains a Dockerfile.
cd "$(dirname "$0")/.."

# ─── Guard: refuse to run under a read-only diag profile ──────

if [ "${AWS_PROFILE:-}" = "${STACK_PREFIX:-}ogrenote-diag" ] \
   || [ "${AWS_PROFILE:-}" = "${STACK_PREFIX:-}ogrenote-deploy-diag" ]; then
    echo "ERROR: refusing to run deploy under read-only diag profile (${AWS_PROFILE})." >&2
    echo "       \`unset AWS_PROFILE\` first (or switch to your admin profile)." >&2
    exit 1
fi

# ─── Validate config ───────────────────────────���───────────────

# Clean up temp files on exit
TASKDEF_TMP=$(mktemp)
TASKDEF_TMP2=$(mktemp)
chmod 600 "$TASKDEF_TMP" "$TASKDEF_TMP2"
trap 'rm -f "$TASKDEF_TMP" "$TASKDEF_TMP2"' EXIT

for var in AWS_REGION STACK_PREFIX OAUTH_CLIENT_ID OAUTH_CLIENT_SECRET JWT_SECRET NOTIFICATION_EMAIL; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source your config file first:"
        echo "  source scripts/aws-test-config.env"
        exit 1
    fi
done

PREFIX="${STACK_PREFIX}"
REGION="${AWS_REGION}"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)

TABLE_NAME="${PREFIX}ogrenote"
BUCKET_NAME="${PREFIX}ogrenote"
ECR_REPO="${PREFIX}ogrenote"
CLUSTER_NAME="${PREFIX}ogrenote"
SERVICE_NAME="${PREFIX}ogrenote-api"
REDIS_SG_NAME="${PREFIX}redis-sg"
CACHE_CLUSTER_ID="${PREFIX}redis"
CACHE_SUBNET_GROUP="${PREFIX}redis-subnets"
TASK_FAMILY="${PREFIX}ogrenote-api"
LOG_GROUP="/ecs/${PREFIX}ogrenote"
ALB_NAME="${PREFIX}ogrenote-alb"
TG_NAME="${PREFIX}ogrenote-tg"
VPC_NAME="${PREFIX}ogrenote-vpc"
EXEC_ROLE_NAME="${PREFIX}ogrenote-exec"
TASK_ROLE_NAME="${PREFIX}ogrenote-task"
BUDGET_NAME="${PREFIX}ogrenote-budget"
# Phase 6 M-6.1: Qdrant vector store names. The ECS task carries the
# Qdrant data on an EFS volume so the service can be redeployed
# without re-embedding the corpus. Cloud Map gives the API a stable
# DNS name to dial (Fargate task IPs are ephemeral).
QDRANT_SG_NAME="${PREFIX}qdrant-sg"
EFS_SG_NAME="${PREFIX}efs-sg"
EFS_NAME="${PREFIX}qdrant-data"
QDRANT_TASK_FAMILY="${PREFIX}ogrenote-qdrant"
QDRANT_SERVICE_NAME="${PREFIX}ogrenote-qdrant"
QDRANT_LOG_GROUP="/ecs/${PREFIX}ogrenote-qdrant"
CLOUDMAP_NAMESPACE_NAME="${PREFIX%-}.internal"
CLOUDMAP_SERVICE_NAME="qdrant"
# Phase 6 M-6.1 piece C: SSM SecureString parameter that the ECS
# task definition references via the `secrets` field. Operator
# manages the value via `aws ssm put-parameter`; the deploy script
# never reads or writes the secret itself.
ANTHROPIC_SSM_NAME="/${PREFIX}ogrenote/anthropic-api-key"
# Phase 6 M-6.4 piece D: async-worker ECS service. Same image as the
# API, launched with `--mode=worker`. No ALB target and no
# portMappings — the worker only talks to Redis (XREADGROUP) plus
# DynamoDB/S3 (the import jobs in M-6.5/6.6). Scales on CPU because
# the DOCX/PDF conversions it exists to run are CPU-bound; see the
# autoscaling note at Phase 6 M-6.4 for why backlog-based scaling is
# a v2 refinement.
WORKER_TASK_FAMILY="${PREFIX}ogrenote-worker"
WORKER_SERVICE_NAME="${PREFIX}ogrenote-worker"
WORKER_LOG_GROUP="/ecs/${PREFIX}ogrenote-worker"
WORKER_MIN_COUNT="${WORKER_MIN_COUNT:-1}"
WORKER_MAX_COUNT="${WORKER_MAX_COUNT:-3}"

echo "=== OgreNote Test Deployment ==="
echo "Region:  ${REGION}"
echo "Prefix:  ${PREFIX}"
echo "Account: ${ACCOUNT_ID}"
echo ""

# ─── Phase 1: VPC ──────────────────────────────────────────────

echo "--- Phase 1: VPC ---"

VPC_ID=$(aws ec2 describe-vpcs \
    --filters "Name=tag:Name,Values=${VPC_NAME}" \
    --query 'Vpcs[0].VpcId' --output text --region "${REGION}" 2>/dev/null || echo "None")

if [ "$VPC_ID" = "None" ] || [ -z "$VPC_ID" ]; then
    echo "Creating VPC..."
    VPC_ID=$(aws ec2 create-vpc \
        --cidr-block 10.0.0.0/16 \
        --query 'Vpc.VpcId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$VPC_ID" \
        --tags "Key=Name,Value=${VPC_NAME}" --region "${REGION}"
    aws ec2 modify-vpc-attribute --vpc-id "$VPC_ID" \
        --enable-dns-support --region "${REGION}"
    aws ec2 modify-vpc-attribute --vpc-id "$VPC_ID" \
        --enable-dns-hostnames --region "${REGION}"
    echo "Created VPC: $VPC_ID"
else
    echo "VPC exists: $VPC_ID"
fi

# Get AZs
AZ1=$(aws ec2 describe-availability-zones \
    --query 'AvailabilityZones[0].ZoneName' --output text --region "${REGION}")
AZ2=$(aws ec2 describe-availability-zones \
    --query 'AvailabilityZones[1].ZoneName' --output text --region "${REGION}")

# Public subnets (for ALB — ALB requires 2 AZs)
create_subnet() {
    local name=$1 cidr=$2 az=$3
    local sub_id
    sub_id=$(aws ec2 describe-subnets \
        --filters "Name=tag:Name,Values=${name}" "Name=vpc-id,Values=${VPC_ID}" \
        --query 'Subnets[0].SubnetId' --output text --region "${REGION}" 2>/dev/null || echo "None")
    if [ "$sub_id" = "None" ] || [ -z "$sub_id" ]; then
        sub_id=$(aws ec2 create-subnet \
            --vpc-id "$VPC_ID" --cidr-block "$cidr" --availability-zone "$az" \
            --query 'Subnet.SubnetId' --output text --region "${REGION}")
        aws ec2 create-tags --resources "$sub_id" \
            --tags "Key=Name,Value=${name}" --region "${REGION}"
        echo "  Created subnet $name: $sub_id"
    else
        echo "  Subnet $name exists: $sub_id"
    fi
    echo "$sub_id"
}

PUB_SUB1=$(create_subnet "${PREFIX}public-1" "10.0.1.0/24" "$AZ1" | tail -1)
PUB_SUB2=$(create_subnet "${PREFIX}public-2" "10.0.2.0/24" "$AZ2" | tail -1)

# Enable auto-assign public IP on public subnets
aws ec2 modify-subnet-attribute --subnet-id "$PUB_SUB1" \
    --map-public-ip-on-launch --region "${REGION}" 2>/dev/null || true
aws ec2 modify-subnet-attribute --subnet-id "$PUB_SUB2" \
    --map-public-ip-on-launch --region "${REGION}" 2>/dev/null || true

# Internet Gateway
IGW_ID=$(aws ec2 describe-internet-gateways \
    --filters "Name=tag:Name,Values=${PREFIX}igw" \
    --query 'InternetGateways[0].InternetGatewayId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$IGW_ID" = "None" ] || [ -z "$IGW_ID" ]; then
    IGW_ID=$(aws ec2 create-internet-gateway \
        --query 'InternetGateway.InternetGatewayId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$IGW_ID" \
        --tags "Key=Name,Value=${PREFIX}igw" --region "${REGION}"
    aws ec2 attach-internet-gateway --internet-gateway-id "$IGW_ID" \
        --vpc-id "$VPC_ID" --region "${REGION}" 2>/dev/null || true
    echo "Created IGW: $IGW_ID"
else
    echo "IGW exists: $IGW_ID"
fi

# Route table for public subnets
RTB_ID=$(aws ec2 describe-route-tables \
    --filters "Name=tag:Name,Values=${PREFIX}public-rt" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'RouteTables[0].RouteTableId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$RTB_ID" = "None" ] || [ -z "$RTB_ID" ]; then
    RTB_ID=$(aws ec2 create-route-table \
        --vpc-id "$VPC_ID" \
        --query 'RouteTable.RouteTableId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$RTB_ID" \
        --tags "Key=Name,Value=${PREFIX}public-rt" --region "${REGION}"
    aws ec2 create-route --route-table-id "$RTB_ID" \
        --destination-cidr-block 0.0.0.0/0 --gateway-id "$IGW_ID" --region "${REGION}"
    echo "Created route table: $RTB_ID"
else
    echo "Route table exists: $RTB_ID"
fi
aws ec2 associate-route-table --route-table-id "$RTB_ID" \
    --subnet-id "$PUB_SUB1" --region "${REGION}" 2>/dev/null || true
aws ec2 associate-route-table --route-table-id "$RTB_ID" \
    --subnet-id "$PUB_SUB2" --region "${REGION}" 2>/dev/null || true

# VPC endpoints for DynamoDB and S3 (free, avoids NAT)
for svc in dynamodb s3; do
    VPCE_ID=$(aws ec2 describe-vpc-endpoints \
        --filters "Name=vpc-id,Values=${VPC_ID}" "Name=service-name,Values=com.amazonaws.${REGION}.${svc}" \
        --query 'VpcEndpoints[0].VpcEndpointId' --output text --region "${REGION}" 2>/dev/null || echo "None")
    if [ "$VPCE_ID" = "None" ] || [ -z "$VPCE_ID" ]; then
        aws ec2 create-vpc-endpoint \
            --vpc-id "$VPC_ID" \
            --service-name "com.amazonaws.${REGION}.${svc}" \
            --route-table-ids "$RTB_ID" \
            --region "${REGION}" >/dev/null
        echo "  Created VPC endpoint for ${svc}"
    else
        echo "  VPC endpoint for ${svc} exists"
    fi
done

# Security groups
ALB_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${PREFIX}alb-sg" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$ALB_SG" = "None" ] || [ -z "$ALB_SG" ]; then
    ALB_SG=$(aws ec2 create-security-group \
        --group-name "${PREFIX}alb-sg" --description "ALB security group" \
        --vpc-id "$VPC_ID" --query 'GroupId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$ALB_SG" \
        --tags "Key=Name,Value=${PREFIX}alb-sg" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$ALB_SG" \
        --protocol tcp --port 80 --cidr 0.0.0.0/0 --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$ALB_SG" \
        --protocol tcp --port 443 --cidr 0.0.0.0/0 --region "${REGION}"
    echo "Created ALB SG: $ALB_SG"
else
    echo "ALB SG exists: $ALB_SG"
fi

ECS_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${PREFIX}ecs-sg" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$ECS_SG" = "None" ] || [ -z "$ECS_SG" ]; then
    ECS_SG=$(aws ec2 create-security-group \
        --group-name "${PREFIX}ecs-sg" --description "ECS task security group" \
        --vpc-id "$VPC_ID" --query 'GroupId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$ECS_SG" \
        --tags "Key=Name,Value=${PREFIX}ecs-sg" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$ECS_SG" \
        --protocol tcp --port 3000 --source-group "$ALB_SG" --region "${REGION}"
    echo "Created ECS SG: $ECS_SG"
else
    echo "ECS SG exists: $ECS_SG"
fi

# Redis SG — ingress 6379/tcp from ECS SG only.
REDIS_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${REDIS_SG_NAME}" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$REDIS_SG" = "None" ] || [ -z "$REDIS_SG" ]; then
    REDIS_SG=$(aws ec2 create-security-group \
        --group-name "${REDIS_SG_NAME}" --description "ElastiCache Redis security group" \
        --vpc-id "$VPC_ID" --query 'GroupId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$REDIS_SG" \
        --tags "Key=Name,Value=${REDIS_SG_NAME}" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$REDIS_SG" \
        --protocol tcp --port 6379 --source-group "$ECS_SG" --region "${REGION}"
    echo "Created Redis SG: $REDIS_SG"
else
    echo "Redis SG exists: $REDIS_SG"
fi

# Qdrant SG — ingress 6333/tcp (REST) + 6334/tcp (gRPC) from ECS SG only.
# Phase 6 M-6.1 piece A. The vector store is reachable only from the
# API task; never from the public ALB.
QDRANT_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${QDRANT_SG_NAME}" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$QDRANT_SG" = "None" ] || [ -z "$QDRANT_SG" ]; then
    QDRANT_SG=$(aws ec2 create-security-group \
        --group-name "${QDRANT_SG_NAME}" --description "Qdrant vector-store security group" \
        --vpc-id "$VPC_ID" --query 'GroupId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$QDRANT_SG" \
        --tags "Key=Name,Value=${QDRANT_SG_NAME}" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$QDRANT_SG" \
        --protocol tcp --port 6333 --source-group "$ECS_SG" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$QDRANT_SG" \
        --protocol tcp --port 6334 --source-group "$ECS_SG" --region "${REGION}"
    echo "Created Qdrant SG: $QDRANT_SG"
else
    echo "Qdrant SG exists: $QDRANT_SG"
fi

# EFS SG — ingress 2049/tcp (NFS) from the Qdrant task SG only. The
# Qdrant container is the only consumer of this volume.
EFS_SG=$(aws ec2 describe-security-groups \
    --filters "Name=tag:Name,Values=${EFS_SG_NAME}" "Name=vpc-id,Values=${VPC_ID}" \
    --query 'SecurityGroups[0].GroupId' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$EFS_SG" = "None" ] || [ -z "$EFS_SG" ]; then
    EFS_SG=$(aws ec2 create-security-group \
        --group-name "${EFS_SG_NAME}" --description "EFS Qdrant data security group" \
        --vpc-id "$VPC_ID" --query 'GroupId' --output text --region "${REGION}")
    aws ec2 create-tags --resources "$EFS_SG" \
        --tags "Key=Name,Value=${EFS_SG_NAME}" --region "${REGION}"
    aws ec2 authorize-security-group-ingress --group-id "$EFS_SG" \
        --protocol tcp --port 2049 --source-group "$QDRANT_SG" --region "${REGION}"
    echo "Created EFS SG: $EFS_SG"
else
    echo "EFS SG exists: $EFS_SG"
fi

echo ""

# ─── Phase 1b: ElastiCache ─────────────────────────────────────
# Single cache.t4g.micro Redis node. Required by the ws-token flow — without
# Redis, WebSocket handshakes hang until the ALB 60s idle timeout fires.
# Creation takes ~3-5 min; start it now so it overlaps with the Docker build
# below, then `wait cache-cluster-available` just before task-def registration.

echo "--- Phase 1b: ElastiCache ---"

# Subnet group across the two public subnets (same subnets ECS tasks run in —
# the security group limits ingress to the ECS SG, so public subnets are fine
# for a test deployment).
if aws elasticache describe-cache-subnet-groups \
        --cache-subnet-group-name "$CACHE_SUBNET_GROUP" \
        --region "${REGION}" >/dev/null 2>&1; then
    echo "Cache subnet group exists: $CACHE_SUBNET_GROUP"
else
    aws elasticache create-cache-subnet-group \
        --cache-subnet-group-name "$CACHE_SUBNET_GROUP" \
        --cache-subnet-group-description "ogrenote cache subnets" \
        --subnet-ids "$PUB_SUB1" "$PUB_SUB2" \
        --region "${REGION}" >/dev/null
    echo "Created cache subnet group: $CACHE_SUBNET_GROUP"
fi

# Cache cluster. Do not wait here — `aws elasticache wait cache-cluster-available`
# will run later once the rest of the stack is ready.
CACHE_STATUS=$(aws elasticache describe-cache-clusters \
    --cache-cluster-id "$CACHE_CLUSTER_ID" \
    --query 'CacheClusters[0].CacheClusterStatus' --output text \
    --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$CACHE_STATUS" = "NONE" ]; then
    aws elasticache create-cache-cluster \
        --cache-cluster-id "$CACHE_CLUSTER_ID" \
        --engine redis \
        --cache-node-type cache.t4g.micro \
        --num-cache-nodes 1 \
        --cache-subnet-group-name "$CACHE_SUBNET_GROUP" \
        --security-group-ids "$REDIS_SG" \
        --tags "Key=Stack,Value=${PREFIX}" \
        --region "${REGION}" >/dev/null
    echo "Creating cache cluster: $CACHE_CLUSTER_ID (3-5 min, runs in parallel)"
else
    echo "Cache cluster exists: $CACHE_CLUSTER_ID ($CACHE_STATUS)"
fi

echo ""

# ─── Phase 2: Storage ──────────────────────────────────────────

echo "--- Phase 2: Storage ---"

# DynamoDB table
TABLE_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
    --query 'Table.TableStatus' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$TABLE_STATUS" = "NONE" ]; then
    echo "Creating DynamoDB table: $TABLE_NAME"
    aws dynamodb create-table \
        --table-name "$TABLE_NAME" \
        --billing-mode PAY_PER_REQUEST \
        --attribute-definitions \
            AttributeName=PK,AttributeType=S \
            AttributeName=SK,AttributeType=S \
            AttributeName=owner_id_gsi,AttributeType=S \
            AttributeName=updated_at,AttributeType=N \
            AttributeName=parent_id_gsi,AttributeType=S \
            AttributeName=title,AttributeType=S \
            AttributeName=doc_id_gsi,AttributeType=S \
            AttributeName=workspace_id_gsi,AttributeType=S \
            AttributeName=user_id_gsi,AttributeType=S \
            AttributeName=created_at,AttributeType=N \
            AttributeName=external_id_gsi,AttributeType=S \
            AttributeName=is_deleted_gsi,AttributeType=S \
            AttributeName=deleted_at,AttributeType=N \
            AttributeName=actor_id_gsi,AttributeType=S \
        --key-schema \
            AttributeName=PK,KeyType=HASH \
            AttributeName=SK,KeyType=RANGE \
        --global-secondary-indexes \
            "IndexName=GSI1-owner-updated,KeySchema=[{AttributeName=owner_id_gsi,KeyType=HASH},{AttributeName=updated_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI2-parent-title,KeySchema=[{AttributeName=parent_id_gsi,KeyType=HASH},{AttributeName=title,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI3-workspace-updated,KeySchema=[{AttributeName=workspace_id_gsi,KeyType=HASH},{AttributeName=updated_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI4-user-created,KeySchema=[{AttributeName=user_id_gsi,KeyType=HASH},{AttributeName=created_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI5-docid-updated,KeySchema=[{AttributeName=doc_id_gsi,KeyType=HASH},{AttributeName=updated_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI6-external-id,KeySchema=[{AttributeName=external_id_gsi,KeyType=HASH}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI7-deleted-at,KeySchema=[{AttributeName=is_deleted_gsi,KeyType=HASH},{AttributeName=deleted_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
            "IndexName=GSI8-actor-created,KeySchema=[{AttributeName=actor_id_gsi,KeyType=HASH},{AttributeName=created_at,KeyType=RANGE}],Projection={ProjectionType=ALL}" \
        --region "${REGION}" >/dev/null
    aws dynamodb wait table-exists --table-name "$TABLE_NAME" --region "${REGION}"
    aws dynamodb update-continuous-backups --table-name "$TABLE_NAME" \
        --point-in-time-recovery-specification PointInTimeRecoveryEnabled=true \
        --region "${REGION}" >/dev/null
    echo "Created table with PITR enabled"
else
    echo "DynamoDB table exists: $TABLE_NAME ($TABLE_STATUS)"
fi

# Idempotent GSI add: a live stack created before Phase 4 has GSI1–GSI5
# only. Detect the new index by name and add it if missing. Skipped
# when the index is already present, when the index is mid-create
# (UPDATING), or when the table is in any non-ACTIVE state.
EXTERNAL_ID_GSI_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
    --region "${REGION}" \
    --query "Table.GlobalSecondaryIndexes[?IndexName=='GSI6-external-id'].IndexStatus | [0]" \
    --output text 2>/dev/null || echo "NONE")
if [ "$EXTERNAL_ID_GSI_STATUS" = "None" ] || [ "$EXTERNAL_ID_GSI_STATUS" = "NONE" ]; then
    TABLE_LIVE_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
        --query 'Table.TableStatus' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
    if [ "$TABLE_LIVE_STATUS" = "ACTIVE" ]; then
        echo "Adding GSI6-external-id (Phase 4 SCIM external_id lookup)"
        aws dynamodb update-table --table-name "$TABLE_NAME" \
            --attribute-definitions AttributeName=external_id_gsi,AttributeType=S \
            --global-secondary-index-updates \
                "Create={IndexName=GSI6-external-id,KeySchema=[{AttributeName=external_id_gsi,KeyType=HASH}],Projection={ProjectionType=ALL}}" \
            --region "${REGION}" >/dev/null
        echo "GSI6 add initiated (table stays ACTIVE; index builds in background)"
    else
        echo "Skipping GSI6 add: table status is $TABLE_LIVE_STATUS"
    fi
elif [ "$EXTERNAL_ID_GSI_STATUS" = "ACTIVE" ]; then
    echo "GSI6-external-id present and ACTIVE"
else
    echo "GSI6-external-id present (status: $EXTERNAL_ID_GSI_STATUS)"
fi

# Idempotent GSI7 add: live stacks created before Phase 4 M-E7
# don't have the deleted-at GSI yet. Same shape as GSI6 logic
# above — detect-by-name, only add if missing and table is ACTIVE.
DELETED_AT_GSI_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
    --region "${REGION}" \
    --query "Table.GlobalSecondaryIndexes[?IndexName=='GSI7-deleted-at'].IndexStatus | [0]" \
    --output text 2>/dev/null || echo "NONE")
if [ "$DELETED_AT_GSI_STATUS" = "None" ] || [ "$DELETED_AT_GSI_STATUS" = "NONE" ]; then
    TABLE_LIVE_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
        --query 'Table.TableStatus' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
    if [ "$TABLE_LIVE_STATUS" = "ACTIVE" ]; then
        echo "Adding GSI7-deleted-at (Phase 4 M-E7 trash-cleanup worker)"
        aws dynamodb update-table --table-name "$TABLE_NAME" \
            --attribute-definitions \
                AttributeName=is_deleted_gsi,AttributeType=S \
                AttributeName=deleted_at,AttributeType=N \
            --global-secondary-index-updates \
                "Create={IndexName=GSI7-deleted-at,KeySchema=[{AttributeName=is_deleted_gsi,KeyType=HASH},{AttributeName=deleted_at,KeyType=RANGE}],Projection={ProjectionType=ALL}}" \
            --region "${REGION}" >/dev/null
        echo "GSI7 add initiated (table stays ACTIVE; index builds in background)"
    else
        echo "Skipping GSI7 add: table status is $TABLE_LIVE_STATUS"
    fi
elif [ "$DELETED_AT_GSI_STATUS" = "ACTIVE" ]; then
    echo "GSI7-deleted-at present and ACTIVE"
else
    echo "GSI7-deleted-at present (status: $DELETED_AT_GSI_STATUS)"
fi

# Idempotent GSI8 add (#49): live stacks predating the actor-centric
# audit index don't have it yet. Same detect-by-name shape as GSI6/GSI7.
# Backfill is automatic — the index is sparse and the AdminAudit table
# is small, so existing rows surface as DynamoDB rebuilds the index.
ACTOR_GSI_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
    --region "${REGION}" \
    --query "Table.GlobalSecondaryIndexes[?IndexName=='GSI8-actor-created'].IndexStatus | [0]" \
    --output text 2>/dev/null || echo "NONE")
if [ "$ACTOR_GSI_STATUS" = "None" ] || [ "$ACTOR_GSI_STATUS" = "NONE" ]; then
    TABLE_LIVE_STATUS=$(aws dynamodb describe-table --table-name "$TABLE_NAME" \
        --query 'Table.TableStatus' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
    if [ "$TABLE_LIVE_STATUS" = "ACTIVE" ]; then
        echo "Adding GSI8-actor-created (#49 actor-centric audit forensics)"
        aws dynamodb update-table --table-name "$TABLE_NAME" \
            --attribute-definitions \
                AttributeName=actor_id_gsi,AttributeType=S \
                AttributeName=created_at,AttributeType=N \
            --global-secondary-index-updates \
                "Create={IndexName=GSI8-actor-created,KeySchema=[{AttributeName=actor_id_gsi,KeyType=HASH},{AttributeName=created_at,KeyType=RANGE}],Projection={ProjectionType=ALL}}" \
            --region "${REGION}" >/dev/null
        echo "GSI8 add initiated (table stays ACTIVE; index builds in background)"
    else
        echo "Skipping GSI8 add: table status is $TABLE_LIVE_STATUS"
    fi
elif [ "$ACTOR_GSI_STATUS" = "ACTIVE" ]; then
    echo "GSI8-actor-created present and ACTIVE"
else
    echo "GSI8-actor-created present (status: $ACTOR_GSI_STATUS)"
fi

# S3 bucket
if aws s3api head-bucket --bucket "$BUCKET_NAME" --region "${REGION}" 2>/dev/null; then
    echo "S3 bucket exists: $BUCKET_NAME"
else
    echo "Creating S3 bucket: $BUCKET_NAME"
    if [ "$REGION" = "us-east-1" ]; then
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "${REGION}"
    else
        aws s3api create-bucket --bucket "$BUCKET_NAME" --region "${REGION}" \
            --create-bucket-configuration "LocationConstraint=${REGION}"
    fi
    aws s3api put-public-access-block --bucket "$BUCKET_NAME" \
        --public-access-block-configuration \
        "BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true" \
        --region "${REGION}"
    echo "Created bucket with public access blocked"
fi

# ─── Phase 2b: EFS for Qdrant data (M-6.1 piece A) ─────────────
# Qdrant's storage is dense-binary index files; we want them to
# survive Fargate task replacement. EFS is the cheapest persistent
# block-shared option ($0.30/GB-month, no provisioned IOPS needed
# at the corpus sizes we run). Mount targets in both AZs so the
# Qdrant task can land on either subnet.

echo "--- Phase 2b: EFS for Qdrant data ---"

EFS_ID=$(aws efs describe-file-systems \
    --query "FileSystems[?Tags[?Key=='Name' && Value=='${EFS_NAME}']].FileSystemId | [0]" \
    --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$EFS_ID" = "None" ] || [ -z "$EFS_ID" ] || [ "$EFS_ID" = "null" ]; then
    EFS_ID=$(aws efs create-file-system \
        --performance-mode generalPurpose \
        --throughput-mode bursting \
        --encrypted \
        --tags "Key=Name,Value=${EFS_NAME}" "Key=Stack,Value=${PREFIX}" \
        --query 'FileSystemId' --output text --region "${REGION}")
    echo "Creating EFS: ${EFS_ID}"
fi

# EFS does not ship a CLI-level waiter (unlike DynamoDB / ElastiCache
# / ACM, which do). Poll describe-file-systems until LifeCycleState
# reads `available`. Creation typically completes in <30 s.
# Idempotent — exits immediately if the filesystem is already available.
echo "Waiting for EFS to become available..."
for _ in $(seq 1 60); do
    EFS_STATE=$(aws efs describe-file-systems \
        --file-system-id "${EFS_ID}" \
        --query 'FileSystems[0].LifeCycleState' --output text \
        --region "${REGION}" 2>/dev/null || echo "unknown")
    if [ "${EFS_STATE}" = "available" ]; then
        echo "  EFS available: ${EFS_ID}"
        break
    fi
    if [ "${EFS_STATE}" = "error" ] || [ "${EFS_STATE}" = "deleted" ] || [ "${EFS_STATE}" = "deleting" ]; then
        echo "ERROR: EFS in unexpected state ${EFS_STATE}" >&2
        exit 1
    fi
    sleep 5
done

# Mount targets — one per subnet the Qdrant task may land on.
create_efs_mount_target() {
    local subnet="$1"
    local existing
    existing=$(aws efs describe-mount-targets \
        --file-system-id "${EFS_ID}" \
        --query "MountTargets[?SubnetId=='${subnet}'].MountTargetId | [0]" \
        --output text --region "${REGION}" 2>/dev/null || echo "None")
    if [ "$existing" = "None" ] || [ -z "$existing" ] || [ "$existing" = "null" ]; then
        aws efs create-mount-target \
            --file-system-id "${EFS_ID}" \
            --subnet-id "${subnet}" \
            --security-groups "${EFS_SG}" \
            --region "${REGION}" >/dev/null
        echo "Created EFS mount target in subnet ${subnet}"
    else
        echo "EFS mount target exists in subnet ${subnet}: ${existing}"
    fi
}
create_efs_mount_target "${PUB_SUB1}"
create_efs_mount_target "${PUB_SUB2}"

# Access point — Qdrant runs as UID/GID 1000 inside the official
# image. The access point pins the POSIX identity + root directory
# so Qdrant can write to /data without root-on-EFS shenanigans.
EFS_AP_ID=$(aws efs describe-access-points \
    --file-system-id "${EFS_ID}" \
    --query "AccessPoints[?Tags[?Key=='Name' && Value=='${EFS_NAME}-ap']].AccessPointId | [0]" \
    --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$EFS_AP_ID" = "None" ] || [ -z "$EFS_AP_ID" ] || [ "$EFS_AP_ID" = "null" ]; then
    EFS_AP_ID=$(aws efs create-access-point \
        --file-system-id "${EFS_ID}" \
        --posix-user "Uid=1000,Gid=1000" \
        --root-directory "Path=/qdrant,CreationInfo={OwnerUid=1000,OwnerGid=1000,Permissions=0755}" \
        --tags "Key=Name,Value=${EFS_NAME}-ap" "Key=Stack,Value=${PREFIX}" \
        --query 'AccessPointId' --output text --region "${REGION}")
    echo "Created EFS access point: ${EFS_AP_ID}"
else
    echo "EFS access point exists: ${EFS_AP_ID}"
fi

echo ""

# ─── Phase 3: Container Registry + Build ───────────────────────

echo "--- Phase 3: ECR + Docker Build ---"

ECR_URI=$(aws ecr describe-repositories --repository-names "$ECR_REPO" \
    --query 'repositories[0].repositoryUri' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$ECR_URI" = "NONE" ]; then
    ECR_URI=$(aws ecr create-repository --repository-name "$ECR_REPO" \
        --query 'repository.repositoryUri' --output text --region "${REGION}")
    echo "Created ECR repo: $ECR_URI"
else
    echo "ECR repo exists: $ECR_URI"
fi

# Provenance: log exactly what's being shipped. Without this, "I deployed,
# why isn't it live?" is unanswerable.
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
    --tag "${ECR_REPO}:latest" \
    --tag "${ECR_REPO}:${GIT_STAMP}" \
    -f Dockerfile .

# Push :GIT_STAMP (the immutable provenance tag the task def pins to)
# alongside :latest. The second push is nearly free thanks to ECR layer
# dedup.
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
docker tag "${ECR_REPO}:${GIT_STAMP}" "${ECR_URI}:${GIT_STAMP}"
docker push "${ECR_URI}:${GIT_STAMP}"
docker tag "${ECR_REPO}:latest" "${ECR_URI}:latest"
docker push "${ECR_URI}:latest"
echo "Image pushed: ${ECR_URI}:${GIT_STAMP} (also tagged :latest)"

echo ""

# ─── Phase 4: IAM ──────────────────────────────────────────────

echo "--- Phase 4: IAM ---"

# ECS task execution role (pulls images, writes logs)
EXEC_ROLE_ARN=$(aws iam get-role --role-name "$EXEC_ROLE_NAME" \
    --query 'Role.Arn' --output text 2>/dev/null || echo "NONE")
if [ "$EXEC_ROLE_ARN" = "NONE" ]; then
    EXEC_ROLE_ARN=$(aws iam create-role \
        --role-name "$EXEC_ROLE_NAME" \
        --assume-role-policy-document '{
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "ecs-tasks.amazonaws.com"},
                "Action": "sts:AssumeRole"
            }]
        }' \
        --query 'Role.Arn' --output text)
    aws iam attach-role-policy --role-name "$EXEC_ROLE_NAME" \
        --policy-arn "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
    echo "Created execution role: $EXEC_ROLE_ARN"
else
    echo "Execution role exists: $EXEC_ROLE_ARN"
fi

# Phase 6 M-6.1 piece C: SSM SecureString access for the execution
# role. ECS resolves task-definition `secrets` entries by calling
# ssm:GetParameters as the execution role at task-start time, so the
# grant lives there rather than on the task role. Scope to just the
# stack's own parameter path so a leak doesn't escalate.
# Applied unconditionally (put-role-policy is idempotent) so adding
# new SSM-backed secrets later doesn't require role re-creation.
aws iam put-role-policy --role-name "$EXEC_ROLE_NAME" \
    --policy-name "ogrenote-exec-ssm" \
    --policy-document "{
        \"Version\": \"2012-10-17\",
        \"Statement\": [{
            \"Effect\": \"Allow\",
            \"Action\": [\"ssm:GetParameters\"],
            \"Resource\": \"arn:aws:ssm:${REGION}:${ACCOUNT_ID}:parameter/${PREFIX}ogrenote/*\"
        }]
    }"
echo "Execution role granted ssm:GetParameters on /${PREFIX}ogrenote/*"

# ECS task role (DynamoDB + S3 access)
TASK_ROLE_ARN=$(aws iam get-role --role-name "$TASK_ROLE_NAME" \
    --query 'Role.Arn' --output text 2>/dev/null || echo "NONE")
if [ "$TASK_ROLE_ARN" = "NONE" ]; then
    TASK_ROLE_ARN=$(aws iam create-role \
        --role-name "$TASK_ROLE_NAME" \
        --assume-role-policy-document '{
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "ecs-tasks.amazonaws.com"},
                "Action": "sts:AssumeRole"
            }]
        }' \
        --query 'Role.Arn' --output text)

    echo "Created task role: $TASK_ROLE_ARN"
else
    echo "Task role exists: $TASK_ROLE_ARN"
fi

# Inline policy is applied idempotently outside the create branch so
# additions (like the Phase 6 M-6.1 piece B bedrock:InvokeModel grant)
# also flow through to existing stacks on the next aws-test-deploy run.
# put-role-policy replaces the named inline policy in-place.
aws iam put-role-policy --role-name "$TASK_ROLE_NAME" \
    --policy-name "ogrenote-task-policy" \
    --policy-document "{
        \"Version\": \"2012-10-17\",
        \"Statement\": [
            {
                \"Effect\": \"Allow\",
                \"Action\": [
                    \"dynamodb:GetItem\",
                    \"dynamodb:PutItem\",
                    \"dynamodb:Query\",
                    \"dynamodb:Scan\",
                    \"dynamodb:BatchWriteItem\",
                    \"dynamodb:DeleteItem\",
                    \"dynamodb:UpdateItem\",
                    \"dynamodb:TransactWriteItems\"
                ],
                \"Resource\": \"arn:aws:dynamodb:${REGION}:${ACCOUNT_ID}:table/${TABLE_NAME}*\"
            },
            {
                \"Effect\": \"Allow\",
                \"Action\": [\"s3:GetObject\", \"s3:PutObject\", \"s3:DeleteObject\"],
                \"Resource\": \"arn:aws:s3:::${BUCKET_NAME}/*\"
            },
            {
                \"Effect\": \"Allow\",
                \"Action\": [\"s3:ListBucket\"],
                \"Resource\": \"arn:aws:s3:::${BUCKET_NAME}\"
            },
            {
                \"Effect\": \"Allow\",
                \"Action\": [
                    \"bedrock:InvokeModel\",
                    \"bedrock:InvokeModelWithResponseStream\"
                ],
                \"Resource\": [
                    \"arn:aws:bedrock:${REGION}::foundation-model/amazon.titan-embed-text-v2:0\",
                    \"arn:aws:bedrock:${REGION}::foundation-model/amazon.titan-embed-text-v1\",
                    \"arn:aws:bedrock:${REGION}::foundation-model/cohere.embed-english-v3\",
                    \"arn:aws:bedrock:${REGION}::foundation-model/cohere.embed-multilingual-v3\"
                ]
            }
        ]
    }"
echo "Task-role policy synced (includes bedrock embedding-model grants)"

echo ""

# ─── Phase 5: ALB ──────────────────────────────────────────────

echo "--- Phase 5: ALB ---"

ALB_ARN=$(aws elbv2 describe-load-balancers --names "$ALB_NAME" \
    --query 'LoadBalancers[0].LoadBalancerArn' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$ALB_ARN" = "NONE" ]; then
    ALB_ARN=$(aws elbv2 create-load-balancer \
        --name "$ALB_NAME" \
        --subnets "$PUB_SUB1" "$PUB_SUB2" \
        --security-groups "$ALB_SG" \
        --scheme internet-facing \
        --type application \
        --query 'LoadBalancers[0].LoadBalancerArn' --output text --region "${REGION}")
    echo "Created ALB: $ALB_ARN"
else
    echo "ALB exists: $ALB_ARN"
fi

ALB_DNS=$(aws elbv2 describe-load-balancers --load-balancer-arns "$ALB_ARN" \
    --query 'LoadBalancers[0].DNSName' --output text --region "${REGION}")

# Bump idle timeout to 120s (default is 60s) so the WebSocket heartbeat
# (25s cadence on the client) has plenty of margin against jitter and
# browser throttling. Without this, an idle WS gets reaped after 60s and
# remote edits silently stop syncing on quiet documents.
aws elbv2 modify-load-balancer-attributes \
    --load-balancer-arn "$ALB_ARN" \
    --attributes Key=idle_timeout.timeout_seconds,Value=120 \
    --region "${REGION}" >/dev/null
echo "ALB idle_timeout: 120s"

# Target group
TG_ARN=$(aws elbv2 describe-target-groups --names "$TG_NAME" \
    --query 'TargetGroups[0].TargetGroupArn' --output text --region "${REGION}" 2>/dev/null || echo "NONE")
if [ "$TG_ARN" = "NONE" ]; then
    TG_ARN=$(aws elbv2 create-target-group \
        --name "$TG_NAME" \
        --protocol HTTP --port 3000 \
        --vpc-id "$VPC_ID" \
        --target-type ip \
        --health-check-path "/health" \
        --health-check-interval-seconds 30 \
        --healthy-threshold-count 2 \
        --unhealthy-threshold-count 3 \
        --query 'TargetGroups[0].TargetGroupArn' --output text --region "${REGION}")
    # Enable sticky sessions (1 hour)
    aws elbv2 modify-target-group-attributes \
        --target-group-arn "$TG_ARN" \
        --attributes "Key=stickiness.enabled,Value=true" \
                     "Key=stickiness.type,Value=lb_cookie" \
                     "Key=stickiness.lb_cookie.duration_seconds,Value=3600" \
        --region "${REGION}" >/dev/null
    echo "Created target group with sticky sessions: $TG_ARN"
else
    echo "Target group exists: $TG_ARN"
fi

# HTTP listener — ensure it exists and points at the correct target group
LISTENER_ARN=$(aws elbv2 describe-listeners --load-balancer-arn "$ALB_ARN" \
    --query "Listeners[?Port==\`80\`].ListenerArn" --output text --region "${REGION}" 2>/dev/null)
if [ -z "$LISTENER_ARN" ] || [ "$LISTENER_ARN" = "None" ]; then
    aws elbv2 create-listener \
        --load-balancer-arn "$ALB_ARN" \
        --protocol HTTP --port 80 \
        --default-actions "Type=forward,TargetGroupArn=${TG_ARN}" \
        --region "${REGION}" >/dev/null
    echo "Created HTTP listener"
else
    # Update listener to point at the current target group (fixes orphaned state)
    aws elbv2 modify-listener --listener-arn "$LISTENER_ARN" \
        --default-actions "Type=forward,TargetGroupArn=${TG_ARN}" \
        --region "${REGION}" >/dev/null 2>/dev/null || true
    echo "HTTP listener exists (verified target group)"
fi

echo ""

# ─── Phase 6: ECS ──────────────────────────────────────────────

echo "--- Phase 6: ECS ---"

# Cluster — only ACTIVE clusters are usable; inactive ones need re-creation
CLUSTER_STATUS=$(aws ecs describe-clusters --clusters "$CLUSTER_NAME" \
    --query 'clusters[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$CLUSTER_STATUS" = "ACTIVE" ]; then
    CLUSTER_ARN=$(aws ecs describe-clusters --clusters "$CLUSTER_NAME" \
        --query 'clusters[0].clusterArn' --output text --region "${REGION}")
    echo "ECS cluster exists: $CLUSTER_ARN"
else
    # Delete inactive cluster if it lingers from a previous teardown
    aws ecs delete-cluster --cluster "$CLUSTER_NAME" --region "${REGION}" 2>/dev/null || true
    CLUSTER_ARN=$(aws ecs create-cluster --cluster-name "$CLUSTER_NAME" \
        --query 'cluster.clusterArn' --output text --region "${REGION}")
    echo "Created ECS cluster: $CLUSTER_ARN"
fi

# Log group
aws logs create-log-group --log-group-name "$LOG_GROUP" --region "${REGION}" 2>/dev/null || true
aws logs put-retention-policy --log-group-name "$LOG_GROUP" --retention-in-days 14 --region "${REGION}"
echo "Log group: $LOG_GROUP (14-day retention)"

# Determine frontend origin (ALB DNS)
FRONTEND_ORIGIN="${FRONTEND_ORIGIN:-http://${ALB_DNS}}"
OAUTH_REDIRECT="${OAUTH_REDIRECT_URI}"
if echo "$OAUTH_REDIRECT" | grep -q "FILL_IN_ALB_DNS"; then
    OAUTH_REDIRECT="http://${ALB_DNS}/api/v1/auth/callback"
    echo "NOTE: OAUTH_REDIRECT_URI was not set — using ${OAUTH_REDIRECT}"
    echo "      Update your GitHub OAuth app callback URL to match."
fi

# Block until the ElastiCache cluster started in Phase 1b is reachable, then
# pin REDIS_URL to its endpoint. `wait cache-cluster-available` polls every
# 15s with a 40-attempt cap (~10 min) — cluster usually takes 3-5 min so this
# returns quickly when the Docker build ate up the wait time already.
echo "Waiting for ElastiCache cluster to be available..."
aws elasticache wait cache-cluster-available \
    --cache-cluster-id "$CACHE_CLUSTER_ID" --region "${REGION}"
REDIS_ENDPOINT_JSON=$(aws elasticache describe-cache-clusters \
    --cache-cluster-id "$CACHE_CLUSTER_ID" --show-cache-node-info \
    --query 'CacheClusters[0].CacheNodes[0].Endpoint' --output json \
    --region "${REGION}")
REDIS_HOST=$(echo "$REDIS_ENDPOINT_JSON" | sed -n 's/.*"Address": *"\([^"]*\)".*/\1/p')
REDIS_PORT=$(echo "$REDIS_ENDPOINT_JSON" | sed -n 's/.*"Port": *\([0-9]*\).*/\1/p')
if [ -z "$REDIS_HOST" ] || [ -z "$REDIS_PORT" ]; then
    echo "ERROR: failed to resolve ElastiCache endpoint for ${CACHE_CLUSTER_ID}" >&2
    exit 1
fi
REDIS_URL="redis://${REDIS_HOST}:${REDIS_PORT}"
echo "Redis endpoint: ${REDIS_URL}"

# ─── Phase 6 M-6.1 piece A: Cloud Map service discovery ────────
# Fargate task IPs are ephemeral, so the API needs a stable DNS
# name to dial Qdrant. AWS Cloud Map is the standard pattern —
# create a private DNS namespace inside the VPC, register the
# Qdrant ECS service against it, and the API can `dig
# qdrant.<prefix>.internal` to find the live task IP.

# Private DNS namespace.
NAMESPACE_ID=$(aws servicediscovery list-namespaces \
    --filters "Name=TYPE,Values=DNS_PRIVATE" \
    --query "Namespaces[?Name=='${CLOUDMAP_NAMESPACE_NAME}'].Id | [0]" \
    --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$NAMESPACE_ID" = "None" ] || [ -z "$NAMESPACE_ID" ] || [ "$NAMESPACE_ID" = "null" ]; then
    NAMESPACE_OP_ID=$(aws servicediscovery create-private-dns-namespace \
        --name "${CLOUDMAP_NAMESPACE_NAME}" \
        --vpc "${VPC_ID}" \
        --tags "Key=Stack,Value=${PREFIX}" \
        --query 'OperationId' --output text --region "${REGION}")
    echo "Creating Cloud Map namespace ${CLOUDMAP_NAMESPACE_NAME} (op=${NAMESPACE_OP_ID})"
    # Poll until the op completes — create-private-dns-namespace is
    # async (it provisions a Route 53 private hosted zone).
    for _ in $(seq 1 30); do
        OP_STATUS=$(aws servicediscovery get-operation \
            --operation-id "${NAMESPACE_OP_ID}" \
            --query 'Operation.Status' --output text --region "${REGION}" 2>/dev/null || echo "PENDING")
        if [ "${OP_STATUS}" = "SUCCESS" ]; then
            break
        fi
        if [ "${OP_STATUS}" = "FAIL" ]; then
            echo "ERROR: Cloud Map namespace create failed" >&2
            exit 1
        fi
        sleep 5
    done
    NAMESPACE_ID=$(aws servicediscovery list-namespaces \
        --filters "Name=TYPE,Values=DNS_PRIVATE" \
        --query "Namespaces[?Name=='${CLOUDMAP_NAMESPACE_NAME}'].Id | [0]" \
        --output text --region "${REGION}")
    echo "Cloud Map namespace created: ${NAMESPACE_ID}"
else
    echo "Cloud Map namespace exists: ${NAMESPACE_ID}"
fi

# Qdrant service inside the namespace. The DNS record type is A (one
# Qdrant task; if we ever run > 1 we'd switch to SRV).
QDRANT_SD_SERVICE_ARN=$(aws servicediscovery list-services \
    --filters "Name=NAMESPACE_ID,Values=${NAMESPACE_ID}" \
    --query "Services[?Name=='${CLOUDMAP_SERVICE_NAME}'].Arn | [0]" \
    --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$QDRANT_SD_SERVICE_ARN" = "None" ] || [ -z "$QDRANT_SD_SERVICE_ARN" ] || [ "$QDRANT_SD_SERVICE_ARN" = "null" ]; then
    QDRANT_SD_SERVICE_ARN=$(aws servicediscovery create-service \
        --name "${CLOUDMAP_SERVICE_NAME}" \
        --namespace-id "${NAMESPACE_ID}" \
        --dns-config "NamespaceId=${NAMESPACE_ID},RoutingPolicy=MULTIVALUE,DnsRecords=[{Type=A,TTL=10}]" \
        --health-check-custom-config "FailureThreshold=1" \
        --query 'Service.Arn' --output text --region "${REGION}")
    echo "Created Cloud Map service ${CLOUDMAP_SERVICE_NAME}: ${QDRANT_SD_SERVICE_ARN}"
else
    echo "Cloud Map service exists: ${QDRANT_SD_SERVICE_ARN}"
fi
QDRANT_DNS="${CLOUDMAP_SERVICE_NAME}.${CLOUDMAP_NAMESPACE_NAME}"
QDRANT_URL="http://${QDRANT_DNS}:6334"
echo "Qdrant DNS will resolve at: ${QDRANT_URL}"

# ─── Phase 6 M-6.1 piece A: Qdrant task + service ──────────────
# Single qdrant/qdrant:v1.13.0 task pinned to the EFS access point.
# Compose-style: data path /qdrant/storage, REST 6333, gRPC 6334.

aws logs create-log-group --log-group-name "$QDRANT_LOG_GROUP" --region "${REGION}" 2>/dev/null || true
aws logs put-retention-policy --log-group-name "$QDRANT_LOG_GROUP" --retention-in-days 14 --region "${REGION}"

QDRANT_TASKDEF_TMP=$(mktemp)
trap 'rm -f "$TASKDEF_TMP" "$QDRANT_TASKDEF_TMP"' EXIT
cat > "$QDRANT_TASKDEF_TMP" <<QDRANTDEF
{
    "family": "${QDRANT_TASK_FAMILY}",
    "networkMode": "awsvpc",
    "requiresCompatibilities": ["FARGATE"],
    "cpu": "512",
    "memory": "1024",
    "executionRoleArn": "${EXEC_ROLE_ARN}",
    "taskRoleArn": "${TASK_ROLE_ARN}",
    "volumes": [{
        "name": "qdrant-data",
        "efsVolumeConfiguration": {
            "fileSystemId": "${EFS_ID}",
            "transitEncryption": "ENABLED",
            "authorizationConfig": {
                "accessPointId": "${EFS_AP_ID}",
                "iam": "DISABLED"
            }
        }
    }],
    "containerDefinitions": [{
        "name": "qdrant",
        "image": "qdrant/qdrant:v1.13.0",
        "essential": true,
        "portMappings": [
            {"containerPort": 6333, "protocol": "tcp"},
            {"containerPort": 6334, "protocol": "tcp"}
        ],
        "mountPoints": [{
            "sourceVolume": "qdrant-data",
            "containerPath": "/qdrant/storage",
            "readOnly": false
        }],
        "environment": [
            {"name": "QDRANT__SERVICE__HTTP_PORT", "value": "6333"},
            {"name": "QDRANT__SERVICE__GRPC_PORT", "value": "6334"},
            {"name": "QDRANT__STORAGE__STORAGE_PATH", "value": "/qdrant/storage"}
        ],
        "logConfiguration": {
            "logDriver": "awslogs",
            "options": {
                "awslogs-group": "${QDRANT_LOG_GROUP}",
                "awslogs-region": "${REGION}",
                "awslogs-stream-prefix": "qdrant"
            }
        }
    }]
}
QDRANTDEF

aws ecs register-task-definition --cli-input-json file://${QDRANT_TASKDEF_TMP} \
    --region "${REGION}" >/dev/null
echo "Registered task definition: ${QDRANT_TASK_FAMILY}"

# Qdrant ECS service — Fargate, 1 task, registers with Cloud Map.
QDRANT_SVC_STATUS=$(aws ecs describe-services --cluster "$CLUSTER_NAME" --services "$QDRANT_SERVICE_NAME" \
    --query 'services[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$QDRANT_SVC_STATUS" = "ACTIVE" ]; then
    aws ecs update-service \
        --cluster "$CLUSTER_NAME" \
        --service "$QDRANT_SERVICE_NAME" \
        --task-definition "$QDRANT_TASK_FAMILY" \
        --force-new-deployment \
        --region "${REGION}" >/dev/null
    echo "Updated Qdrant ECS service: $QDRANT_SERVICE_NAME"
else
    echo "Qdrant service status: $QDRANT_SVC_STATUS — creating new service"
    # Fargate Spot to keep cost predictable; allowed since Qdrant
    # is restart-tolerant (EFS persists the index, brief outage on
    # task replacement is acceptable for a test stack).
    aws ecs create-service \
        --cluster "$CLUSTER_NAME" \
        --service-name "$QDRANT_SERVICE_NAME" \
        --task-definition "$QDRANT_TASK_FAMILY" \
        --desired-count 1 \
        --capacity-provider-strategy "capacityProvider=FARGATE_SPOT,weight=1" \
        --network-configuration "awsvpcConfiguration={subnets=[${PUB_SUB1},${PUB_SUB2}],securityGroups=[${QDRANT_SG}],assignPublicIp=ENABLED}" \
        --service-registries "registryArn=${QDRANT_SD_SERVICE_ARN}" \
        --region "${REGION}" >/dev/null
    echo "Created Qdrant ECS service: $QDRANT_SERVICE_NAME (1 task, FARGATE_SPOT)"
fi

# Phase 6 M-6.1 piece C: SSM SecureString for ANTHROPIC_API_KEY.
# The parameter is operator-managed; the deploy script never reads
# or writes the secret itself, it only references the ARN from the
# ECS task definition's `secrets` field. ECS resolves the ARN at
# task-start time via the execution role (granted ssm:GetParameters
# above) and injects the decrypted value as an env var to the
# container — same shape as a plaintext env var from the API's
# perspective, but the value never lands in the task-def JSON or
# any deploy log.
#
# Operator setup (one-time per stack):
#   aws ssm put-parameter \
#       --name "${ANTHROPIC_SSM_NAME}" \
#       --type SecureString \
#       --value "sk-ant-..." \
#       --region "${REGION}"
#
# Update flow (key rotation):
#   aws ssm put-parameter --overwrite --name "${ANTHROPIC_SSM_NAME}" \
#       --type SecureString --value "sk-ant-..." --region "${REGION}"
#   aws ecs update-service --cluster "${CLUSTER_NAME}" \
#       --service "${SERVICE_NAME}" --force-new-deployment --region "${REGION}"
ANTHROPIC_SSM_ARN=$(aws ssm describe-parameters \
    --parameter-filters "Key=Name,Values=${ANTHROPIC_SSM_NAME}" \
    --query 'Parameters[0].ARN' --output text --region "${REGION}" 2>/dev/null || echo "None")
if [ "$ANTHROPIC_SSM_ARN" = "None" ] || [ -z "$ANTHROPIC_SSM_ARN" ] || [ "$ANTHROPIC_SSM_ARN" = "null" ]; then
    HAS_ANTHROPIC_SSM=0
    ANTHROPIC_SECRET_ENTRY=""
    echo ""
    echo "NOTE: ANTHROPIC_API_KEY SSM parameter not found."
    echo "      The /api/v1/ask endpoint will return 503 until it's set."
    echo "      To enable, run:"
    echo ""
    echo "        aws ssm put-parameter \\"
    echo "            --name '${ANTHROPIC_SSM_NAME}' \\"
    echo "            --type SecureString \\"
    echo "            --value 'sk-ant-...' \\"
    echo "            --region '${REGION}'"
    echo ""
    echo "      Then re-run aws-test-deploy.sh to wire it into the task def."
    echo ""
else
    HAS_ANTHROPIC_SSM=1
    ANTHROPIC_SECRET_ENTRY="\"secrets\": [{\"name\": \"ANTHROPIC_API_KEY\", \"valueFrom\": \"${ANTHROPIC_SSM_ARN}\"}],"
    echo "Anthropic SSM parameter found: ${ANTHROPIC_SSM_NAME}"
fi

# Task definition
# NOTE: Plaintext secrets (OAUTH_CLIENT_SECRET, JWT_SECRET) flow
# through the `environment` block for legacy reasons. The
# Phase 6 RAG key uses the `secrets` block via SSM (above). The
# remaining secrets should follow the same pattern in a future
# tightening pass.
DEV_MODE="${DEV_MODE:-false}"
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
            {"name": "S3_BUCKET", "value": "${BUCKET_NAME}"},
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
            {"name": "DEPLOY_ENV", "value": "${DEPLOY_ENV:-test}"}
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

aws ecs register-task-definition --cli-input-json file://${TASKDEF_TMP} \
    --region "${REGION}" >/dev/null
echo "Registered task definition: ${TASK_FAMILY}"

# ECS service — only "ACTIVE" services can be updated; anything else needs a fresh create
SERVICE_STATUS=$(aws ecs describe-services --cluster "$CLUSTER_NAME" --services "$SERVICE_NAME" \
    --query 'services[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$SERVICE_STATUS" = "ACTIVE" ]; then
    # Update existing service to use latest task definition
    aws ecs update-service \
        --cluster "$CLUSTER_NAME" \
        --service "$SERVICE_NAME" \
        --task-definition "$TASK_FAMILY" \
        --force-new-deployment \
        --region "${REGION}" >/dev/null
    echo "Updated ECS service: $SERVICE_NAME"
else
    echo "Service status: $SERVICE_STATUS — creating new service"
    aws ecs create-service \
        --cluster "$CLUSTER_NAME" \
        --service-name "$SERVICE_NAME" \
        --task-definition "$TASK_FAMILY" \
        --desired-count 1 \
        --launch-type FARGATE \
        --network-configuration "awsvpcConfiguration={subnets=[${PUB_SUB1},${PUB_SUB2}],securityGroups=[${ECS_SG}],assignPublicIp=ENABLED}" \
        --load-balancers "targetGroupArn=${TG_ARN},containerName=ogrenote-api,containerPort=3000" \
        --region "${REGION}" >/dev/null
    echo "Created ECS service: $SERVICE_NAME (1 task)"
fi

echo ""

# ─── Phase 6 M-6.4 piece D: async-worker task + service ────────
# Second ECS service off the same image, launched as `--mode=worker`.
# No ALB wiring (it serves no HTTP), no portMappings, no health-check
# grace — liveness is "the consumer loop is reading the stream", which
# ECS can't probe, so we rely on the task staying RUNNING and the
# CloudWatch logs. Shares the API's task + execution roles (same
# DynamoDB/S3 grants the import jobs need) and the same VPC/SG.

echo "--- Phase 6 M-6.4: Worker service ---"

aws logs create-log-group --log-group-name "$WORKER_LOG_GROUP" --region "${REGION}" 2>/dev/null || true
aws logs put-retention-policy --log-group-name "$WORKER_LOG_GROUP" --retention-in-days 14 --region "${REGION}"
echo "Log group: $WORKER_LOG_GROUP (14-day retention)"

WORKER_TASKDEF_TMP=$(mktemp)
WORKER_SCALING_TMP=$(mktemp)
trap 'rm -f "$TASKDEF_TMP" "$TASKDEF_TMP2" "$QDRANT_TASKDEF_TMP" "$WORKER_TASKDEF_TMP" "$WORKER_SCALING_TMP"' EXIT

# CMD in the Dockerfile is ["ogrenotes-api"] with no ENTRYPOINT, so an
# ECS `command` override replaces it wholesale — the worker command
# must carry the binary name itself, then the mode flag main.rs greps
# argv for. The environment block mirrors the API's so AppConfig
# ::from_env() parses identically (it env_required's JWT_SECRET,
# OAUTH_*, etc.); JOB_STREAM_NAME is left unset so both default to
# "ogrenotes-jobs" and share the one stream. WORKER_CONCURRENCY is
# deliberately low for the 0.25-vCPU task — DOCX/PDF conversion is
# CPU-bound, so over-subscribing consumers per task just thrashes.
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
            {"name": "S3_BUCKET", "value": "${BUCKET_NAME}"},
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
            {"name": "DEPLOY_ENV", "value": "${DEPLOY_ENV:-test}"},
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

aws ecs register-task-definition --cli-input-json file://${WORKER_TASKDEF_TMP} \
    --region "${REGION}" >/dev/null
echo "Registered task definition: ${WORKER_TASK_FAMILY}"

# Worker service — no load-balancer attachment. Desired count is the
# autoscaling floor on first create; thereafter Application Auto
# Scaling owns DesiredCount, so updates use --force-new-deployment
# without passing --desired-count (which would fight the scaler).
WORKER_SVC_STATUS=$(aws ecs describe-services --cluster "$CLUSTER_NAME" --services "$WORKER_SERVICE_NAME" \
    --query 'services[0].status' --output text --region "${REGION}" 2>/dev/null || echo "MISSING")
if [ "$WORKER_SVC_STATUS" = "ACTIVE" ]; then
    aws ecs update-service \
        --cluster "$CLUSTER_NAME" \
        --service "$WORKER_SERVICE_NAME" \
        --task-definition "$WORKER_TASK_FAMILY" \
        --force-new-deployment \
        --region "${REGION}" >/dev/null
    echo "Updated worker ECS service: $WORKER_SERVICE_NAME"
else
    echo "Worker service status: $WORKER_SVC_STATUS — creating new service"
    aws ecs create-service \
        --cluster "$CLUSTER_NAME" \
        --service-name "$WORKER_SERVICE_NAME" \
        --task-definition "$WORKER_TASK_FAMILY" \
        --desired-count "${WORKER_MIN_COUNT}" \
        --launch-type FARGATE \
        --network-configuration "awsvpcConfiguration={subnets=[${PUB_SUB1},${PUB_SUB2}],securityGroups=[${ECS_SG}],assignPublicIp=ENABLED}" \
        --region "${REGION}" >/dev/null
    echo "Created worker ECS service: $WORKER_SERVICE_NAME (${WORKER_MIN_COUNT} task)"
fi

# Application Auto Scaling. register-scalable-target + put-scaling-policy
# are both idempotent upserts, so no existence guard needed. The
# service-linked role (AWSServiceRoleForApplicationAutoScaling_ECSService)
# is auto-created by AWS on the first register call — no manual IAM.
#
# Metric choice: ECSServiceAverageCPUUtilization, not Redis stream
# backlog. ElastiCache emits no native stream-length metric, so true
# backlog-driven scaling needs a publisher (the worker PUTs XLEN to a
# custom CloudWatch metric on a timer) — that's a code change tracked
# as a v2 refinement in runbook/async-worker-ops.md. CPU is a sound
# v1 proxy precisely because the jobs this service runs (DOCX/PDF
# conversion) are CPU-bound: a growing backlog of real work shows up
# as sustained CPU, and Noop jobs that don't move CPU also don't need
# more workers.
aws application-autoscaling register-scalable-target \
    --service-namespace ecs \
    --resource-id "service/${CLUSTER_NAME}/${WORKER_SERVICE_NAME}" \
    --scalable-dimension ecs:service:DesiredCount \
    --min-capacity "${WORKER_MIN_COUNT}" \
    --max-capacity "${WORKER_MAX_COUNT}" \
    --region "${REGION}"

cat > "$WORKER_SCALING_TMP" <<SCALING
{
    "TargetValue": 70.0,
    "PredefinedMetricSpecification": {
        "PredefinedMetricType": "ECSServiceAverageCPUUtilization"
    },
    "ScaleInCooldown": 300,
    "ScaleOutCooldown": 60
}
SCALING

aws application-autoscaling put-scaling-policy \
    --service-namespace ecs \
    --resource-id "service/${CLUSTER_NAME}/${WORKER_SERVICE_NAME}" \
    --scalable-dimension ecs:service:DesiredCount \
    --policy-name "${PREFIX}ogrenote-worker-cpu" \
    --policy-type TargetTrackingScaling \
    --target-tracking-scaling-policy-configuration "file://${WORKER_SCALING_TMP}" \
    --region "${REGION}" >/dev/null
echo "Worker autoscaling: CPU target 70% (min ${WORKER_MIN_COUNT}, max ${WORKER_MAX_COUNT})"

echo ""

# ─── Phase 7: Budget Alarm ─────────────────────────────────────

echo "--- Phase 7: Budget Alarm ---"

aws budgets create-budget \
    --account-id "$ACCOUNT_ID" \
    --budget "{
        \"BudgetName\": \"${BUDGET_NAME}\",
        \"BudgetLimit\": {\"Amount\": \"50\", \"Unit\": \"USD\"},
        \"BudgetType\": \"COST\",
        \"TimeUnit\": \"MONTHLY\"
    }" \
    --notifications-with-subscribers "[{
        \"Notification\": {
            \"NotificationType\": \"ACTUAL\",
            \"ComparisonOperator\": \"GREATER_THAN\",
            \"Threshold\": 80,
            \"ThresholdType\": \"PERCENTAGE\"
        },
        \"Subscribers\": [{
            \"SubscriptionType\": \"EMAIL\",
            \"Address\": \"${NOTIFICATION_EMAIL}\"
        }]
    }]" 2>/dev/null && echo "Created budget alarm: \$50/month, alert at 80%" \
    || echo "Budget alarm already exists or failed (non-critical)"

echo ""

# ─── Phase 8: DNS + HTTPS (optional) ───────────────────────────

DOMAIN_NAME="${DOMAIN_NAME:-}"
SITE_URL="http://${ALB_DNS}"

if [ -n "$DOMAIN_NAME" ]; then
    echo "--- Phase 8: DNS + HTTPS ---"

    # Look up the Route 53 hosted zone for this domain.
    # For subdomains (e.g., ogrenotes.example.com), find the parent zone (example.com).
    ZONE_ID=""
    LOOKUP_DOMAIN="${DOMAIN_NAME}"
    while [ -n "$LOOKUP_DOMAIN" ]; do
        ZONE_ID=$(aws route53 list-hosted-zones-by-name --dns-name "${LOOKUP_DOMAIN}." \
            --query "HostedZones[?Name=='${LOOKUP_DOMAIN}.'].Id" --output text 2>/dev/null | head -1 | sed 's|/hostedzone/||')
        if [ -n "$ZONE_ID" ] && [ "$ZONE_ID" != "None" ]; then
            echo "Found hosted zone for ${LOOKUP_DOMAIN}: $ZONE_ID"
            break
        fi
        # Strip the leftmost label and try the parent domain
        LOOKUP_DOMAIN="${LOOKUP_DOMAIN#*.}"
        # Stop if we've run out of labels
        if ! echo "$LOOKUP_DOMAIN" | grep -q '\.'; then
            ZONE_ID=""
            break
        fi
    done

    if [ -z "$ZONE_ID" ] || [ "$ZONE_ID" = "None" ]; then
        echo "WARNING: No Route 53 hosted zone found for ${DOMAIN_NAME} or any parent domain"
        echo "         Create one first: aws route53 create-hosted-zone --name ${DOMAIN_NAME} --caller-reference \$(date +%s)"
        echo "         Then update your domain registrar's nameservers to point to Route 53."
        echo "         Skipping DNS/HTTPS setup."
    else
        echo "Found hosted zone: $ZONE_ID for ${DOMAIN_NAME}"

        # Request ACM certificate (or find existing one)
        CERT_ARN=$(aws acm list-certificates \
            --query "CertificateSummaryList[?DomainName=='${DOMAIN_NAME}'].CertificateArn" \
            --output text --region "${REGION}" 2>/dev/null | head -1)

        if [ -z "$CERT_ARN" ] || [ "$CERT_ARN" = "None" ]; then
            echo "Requesting ACM certificate for ${DOMAIN_NAME}..."
            CERT_ARN=$(aws acm request-certificate \
                --domain-name "${DOMAIN_NAME}" \
                --validation-method DNS \
                --query 'CertificateArn' --output text --region "${REGION}")
            echo "Certificate requested: $CERT_ARN"

            # Wait for ACM to generate the validation record
            echo "Waiting for DNS validation details..."
            sleep 5

            # Get the DNS validation record
            VALIDATION_NAME=$(aws acm describe-certificate --certificate-arn "$CERT_ARN" \
                --query 'Certificate.DomainValidationOptions[0].ResourceRecord.Name' \
                --output text --region "${REGION}" 2>/dev/null)
            VALIDATION_VALUE=$(aws acm describe-certificate --certificate-arn "$CERT_ARN" \
                --query 'Certificate.DomainValidationOptions[0].ResourceRecord.Value' \
                --output text --region "${REGION}" 2>/dev/null)

            if [ -n "$VALIDATION_NAME" ] && [ -n "$VALIDATION_VALUE" ]; then
                echo "Adding DNS validation record..."
                aws route53 change-resource-record-sets --hosted-zone-id "$ZONE_ID" \
                    --change-batch "{
                        \"Changes\": [{
                            \"Action\": \"UPSERT\",
                            \"ResourceRecordSet\": {
                                \"Name\": \"${VALIDATION_NAME}\",
                                \"Type\": \"CNAME\",
                                \"TTL\": 300,
                                \"ResourceRecords\": [{\"Value\": \"${VALIDATION_VALUE}\"}]
                            }
                        }]
                    }" >/dev/null
                echo "Validation CNAME created. Waiting for certificate validation..."
                echo "(This can take 2-5 minutes)"
                aws acm wait certificate-validated --certificate-arn "$CERT_ARN" --region "${REGION}"
                echo "Certificate validated!"
            else
                echo "WARNING: Could not retrieve validation record. Validate manually in ACM console."
            fi
        else
            echo "ACM certificate exists: $CERT_ARN"
            # Check if it's issued
            CERT_STATUS=$(aws acm describe-certificate --certificate-arn "$CERT_ARN" \
                --query 'Certificate.Status' --output text --region "${REGION}")
            if [ "$CERT_STATUS" != "ISSUED" ]; then
                echo "WARNING: Certificate status is ${CERT_STATUS}, not ISSUED."
                echo "         Complete DNS validation before HTTPS will work."
            fi
        fi

        # Add HTTPS listener to ALB (if cert is issued)
        CERT_STATUS=$(aws acm describe-certificate --certificate-arn "$CERT_ARN" \
            --query 'Certificate.Status' --output text --region "${REGION}")
        if [ "$CERT_STATUS" = "ISSUED" ]; then
            HTTPS_LISTENER=$(aws elbv2 describe-listeners --load-balancer-arn "$ALB_ARN" \
                --query "Listeners[?Port==\`443\`].ListenerArn" --output text --region "${REGION}" 2>/dev/null)
            if [ -z "$HTTPS_LISTENER" ] || [ "$HTTPS_LISTENER" = "None" ]; then
                aws elbv2 create-listener \
                    --load-balancer-arn "$ALB_ARN" \
                    --protocol HTTPS --port 443 \
                    --certificates "CertificateArn=${CERT_ARN}" \
                    --default-actions "Type=forward,TargetGroupArn=${TG_ARN}" \
                    --region "${REGION}" >/dev/null
                echo "Created HTTPS listener"
            else
                echo "HTTPS listener exists"
            fi

            # Redirect HTTP → HTTPS
            # Update the existing HTTP listener to redirect
            HTTP_LISTENER=$(aws elbv2 describe-listeners --load-balancer-arn "$ALB_ARN" \
                --query "Listeners[?Port==\`80\`].ListenerArn" --output text --region "${REGION}" 2>/dev/null)
            if [ -n "$HTTP_LISTENER" ] && [ "$HTTP_LISTENER" != "None" ]; then
                aws elbv2 modify-listener --listener-arn "$HTTP_LISTENER" \
                    --default-actions '[{"Type":"redirect","RedirectConfig":{"Protocol":"HTTPS","Port":"443","StatusCode":"HTTP_301"}}]' \
                    --region "${REGION}" >/dev/null 2>/dev/null && \
                    echo "HTTP listener now redirects to HTTPS" || true
            fi

            SITE_URL="https://${DOMAIN_NAME}"
        else
            echo "Skipping HTTPS listener (certificate not yet issued)"
        fi

        # Create Route 53 A record aliased to ALB
        ALB_HOSTED_ZONE=$(aws elbv2 describe-load-balancers --load-balancer-arns "$ALB_ARN" \
            --query 'LoadBalancers[0].CanonicalHostedZoneId' --output text --region "${REGION}")

        echo "Creating DNS A record: ${DOMAIN_NAME} → ALB"
        aws route53 change-resource-record-sets --hosted-zone-id "$ZONE_ID" \
            --change-batch "{
                \"Changes\": [{
                    \"Action\": \"UPSERT\",
                    \"ResourceRecordSet\": {
                        \"Name\": \"${DOMAIN_NAME}\",
                        \"Type\": \"A\",
                        \"AliasTarget\": {
                            \"HostedZoneId\": \"${ALB_HOSTED_ZONE}\",
                            \"DNSName\": \"${ALB_DNS}\",
                            \"EvaluateTargetHealth\": false
                        }
                    }
                }]
            }" >/dev/null
        echo "DNS record created: ${DOMAIN_NAME} → ${ALB_DNS}"

        # Update ECS task with HTTPS URLs if domain is configured
        echo "Updating ECS task with HTTPS URLs..."
        # Use the user's OAUTH_REDIRECT_URI if set, otherwise default to the API callback
        OAUTH_REDIRECT="${OAUTH_REDIRECT_URI:-https://${DOMAIN_NAME}/api/v1/auth/callback}"
        FRONTEND_ORIGIN="https://${DOMAIN_NAME}"

        # Re-register task definition with HTTPS URLs
        cat > "$TASKDEF_TMP2" <<TASKDEF
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
            {"name": "S3_BUCKET", "value": "${BUCKET_NAME}"},
            {"name": "REDIS_URL", "value": "${REDIS_URL}"},
            {"name": "QDRANT_URL", "value": "${QDRANT_URL}"},
            {"name": "OAUTH_CLIENT_ID", "value": "${OAUTH_CLIENT_ID}"},
            {"name": "OAUTH_CLIENT_SECRET", "value": "${OAUTH_CLIENT_SECRET}"},
            {"name": "OAUTH_REDIRECT_URI", "value": "${OAUTH_REDIRECT}"},
            {"name": "JWT_SECRET", "value": "${JWT_SECRET}"},
            {"name": "FRONTEND_ORIGIN", "value": "https://${DOMAIN_NAME}"},
            {"name": "DEV_MODE", "value": "${DEV_MODE:-false}"},
            {"name": "SEARCH_INDEX_PATH", "value": "/data/search-index"},
            {"name": "API_PORT", "value": "3000"},
            {"name": "ADMIN_EMAILS", "value": "${ADMIN_EMAILS:-}"},
            {"name": "DEPLOY_ENV", "value": "${DEPLOY_ENV:-prod}"}
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
        aws ecs register-task-definition --cli-input-json "file://${TASKDEF_TMP2}" \
            --region "${REGION}" >/dev/null
        aws ecs update-service --cluster "$CLUSTER_NAME" --service "$SERVICE_NAME" \
            --task-definition "$TASK_FAMILY" --force-new-deployment \
            --region "${REGION}" >/dev/null
        echo "ECS task updated with HTTPS URLs"
    fi

    echo ""
else
    echo "--- Phase 8: DNS + HTTPS (skipped — DOMAIN_NAME not set) ---"
    echo ""
fi

# ─── Phase 9: DDB → S3 backup exports (prod only) ──────────────
#
# Daily Lambda calls dynamodb:ExportTableToPointInTime; output
# lands under s3://<bucket>/backups/<YYYY-MM-DD>/. S3 lifecycle
# transitions to Glacier at 30d and expires at 365d.
#
# Gated on DEPLOY_ENV=prod because the test stack already has
# PITR and doesn't need durable long-window S3 exports — would
# just inflate the budget alarm.
#
# Resources created:
#   - S3 lifecycle policy on the backups/ prefix
#   - IAM role for the Lambda (DDB Export + S3 Put on backups/)
#   - Lambda function (inline Python; ~30 lines)
#   - EventBridge schedule rule (daily 04:00 UTC)
#   - SNS topic + email subscription for alarm notifications
#   - CloudWatch alarm: Errors metric on the Lambda

if [ "${DEPLOY_ENV:-test}" = "prod" ]; then
    echo "--- Phase 9: Backup exports (prod) ---"

    BACKUP_LAMBDA_NAME="${PREFIX}ogrenote-backup"
    BACKUP_ROLE_NAME="${PREFIX}ogrenote-backup-role"
    BACKUP_SCHEDULE_NAME="${PREFIX}ogrenote-backup-daily"
    BACKUP_TOPIC_NAME="${PREFIX}ogrenote-backup-alarms"
    BACKUP_ALARM_NAME="${PREFIX}ogrenote-backup-failure"

    # 9a. Ensure PITR is enabled on the source table. The initial
    # create-table block above turns this on; an older table from
    # before that block would need it enabled here. Idempotent:
    # update-continuous-backups on an already-enabled table is a
    # no-op that returns success.
    aws dynamodb update-continuous-backups --table-name "$TABLE_NAME" \
        --point-in-time-recovery-specification PointInTimeRecoveryEnabled=true \
        --region "${REGION}" >/dev/null
    echo "Confirmed PITR enabled on $TABLE_NAME"

    # 9b. S3 lifecycle on backups/ prefix. Overwrites any existing
    # config — the deploy is the source of truth, not the bucket.
    aws s3api put-bucket-lifecycle-configuration \
        --bucket "$BUCKET_NAME" \
        --lifecycle-configuration '{
            "Rules": [{
                "ID": "ogrenote-backup-lifecycle",
                "Status": "Enabled",
                "Filter": {"Prefix": "backups/"},
                "Transitions": [{"Days": 30, "StorageClass": "GLACIER"}],
                "Expiration": {"Days": 365}
            }]
        }' --region "${REGION}" >/dev/null
    echo "Configured S3 lifecycle for backups/ (Glacier @30d, expire @365d)"

    # 9c. IAM role for the Lambda. Permissions: DDB export + describe,
    # S3 Put under backups/, GetBucketLocation, CloudWatch Logs (via
    # the managed AWSLambdaBasicExecutionRole policy).
    BACKUP_ROLE_ARN=$(aws iam get-role --role-name "$BACKUP_ROLE_NAME" \
        --query 'Role.Arn' --output text 2>/dev/null || echo "NONE")
    if [ "$BACKUP_ROLE_ARN" = "NONE" ]; then
        BACKUP_ROLE_ARN=$(aws iam create-role \
            --role-name "$BACKUP_ROLE_NAME" \
            --assume-role-policy-document '{
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "lambda.amazonaws.com"},
                    "Action": "sts:AssumeRole"
                }]
            }' \
            --query 'Role.Arn' --output text)
        aws iam attach-role-policy --role-name "$BACKUP_ROLE_NAME" \
            --policy-arn "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
        aws iam put-role-policy --role-name "$BACKUP_ROLE_NAME" \
            --policy-name "ogrenote-backup-policy" \
            --policy-document "{
                \"Version\": \"2012-10-17\",
                \"Statement\": [
                    {
                        \"Effect\": \"Allow\",
                        \"Action\": [
                            \"dynamodb:ExportTableToPointInTime\",
                            \"dynamodb:DescribeTable\",
                            \"dynamodb:DescribeContinuousBackups\"
                        ],
                        \"Resource\": \"arn:aws:dynamodb:${REGION}:${ACCOUNT_ID}:table/${TABLE_NAME}\"
                    },
                    {
                        \"Effect\": \"Allow\",
                        \"Action\": [\"s3:PutObject\", \"s3:AbortMultipartUpload\"],
                        \"Resource\": \"arn:aws:s3:::${BUCKET_NAME}/backups/*\"
                    },
                    {
                        \"Effect\": \"Allow\",
                        \"Action\": [\"s3:GetBucketLocation\", \"s3:ListBucket\"],
                        \"Resource\": \"arn:aws:s3:::${BUCKET_NAME}\"
                    }
                ]
            }"
        echo "Created backup Lambda role: $BACKUP_ROLE_ARN"
        # IAM eventual consistency — give the new role a moment to
        # propagate before the Lambda create-function call assumes it.
        sleep 10
    else
        echo "Backup Lambda role exists: $BACKUP_ROLE_ARN"
    fi

    # 9d. Lambda function. Build the zip in a temp dir from inline
    # Python; `zip` is part of any reasonable deploy environment.
    BACKUP_LAMBDA_DIR=$(mktemp -d)
    cat > "${BACKUP_LAMBDA_DIR}/lambda_function.py" << 'PY_EOF'
"""DynamoDB to S3 backup export, fired daily by EventBridge.

Reads OGRENOTES_TABLE_NAME + OGRENOTES_BUCKET from Lambda env and
calls dynamodb:ExportTableToPointInTime with a date-prefixed S3
key. PITR must already be enabled on the source table — the deploy
script's Phase 9a confirms this at deploy time.

The export is async: the Lambda returns immediately with the
export ARN. Actual data lands under s3://<bucket>/backups/<date>/
within ~5-30 minutes depending on table size. Restore steps are
in runbook/restore-from-backup.md.
"""
import os
import boto3
from datetime import datetime, timezone


def lambda_handler(event, context):
    table = os.environ['OGRENOTES_TABLE_NAME']
    bucket = os.environ['OGRENOTES_BUCKET']
    date_prefix = datetime.now(timezone.utc).strftime('%Y-%m-%d')

    ddb = boto3.client('dynamodb')
    desc = ddb.describe_table(TableName=table)
    table_arn = desc['Table']['TableArn']

    resp = ddb.export_table_to_point_in_time(
        TableArn=table_arn,
        S3Bucket=bucket,
        S3Prefix=f'backups/{date_prefix}/',
        ExportFormat='DYNAMODB_JSON',
    )
    export_arn = resp['ExportDescription']['ExportArn']
    print(f'Started DDB export: {export_arn} -> s3://{bucket}/backups/{date_prefix}/')
    return {'export_arn': export_arn, 'date': date_prefix}
PY_EOF
    BACKUP_LAMBDA_ZIP="${BACKUP_LAMBDA_DIR}/lambda.zip"
    (cd "$BACKUP_LAMBDA_DIR" && zip -q lambda.zip lambda_function.py)

    if aws lambda get-function --function-name "$BACKUP_LAMBDA_NAME" \
        --region "${REGION}" >/dev/null 2>&1; then
        aws lambda update-function-code \
            --function-name "$BACKUP_LAMBDA_NAME" \
            --zip-file "fileb://${BACKUP_LAMBDA_ZIP}" \
            --region "${REGION}" >/dev/null
        aws lambda update-function-configuration \
            --function-name "$BACKUP_LAMBDA_NAME" \
            --environment "Variables={OGRENOTES_TABLE_NAME=${TABLE_NAME},OGRENOTES_BUCKET=${BUCKET_NAME}}" \
            --region "${REGION}" >/dev/null
        echo "Updated backup Lambda: $BACKUP_LAMBDA_NAME"
    else
        aws lambda create-function \
            --function-name "$BACKUP_LAMBDA_NAME" \
            --runtime python3.12 \
            --role "$BACKUP_ROLE_ARN" \
            --handler lambda_function.lambda_handler \
            --zip-file "fileb://${BACKUP_LAMBDA_ZIP}" \
            --timeout 60 \
            --memory-size 128 \
            --environment "Variables={OGRENOTES_TABLE_NAME=${TABLE_NAME},OGRENOTES_BUCKET=${BUCKET_NAME}}" \
            --region "${REGION}" >/dev/null
        echo "Created backup Lambda: $BACKUP_LAMBDA_NAME"
    fi
    rm -rf "$BACKUP_LAMBDA_DIR"

    BACKUP_LAMBDA_ARN=$(aws lambda get-function --function-name "$BACKUP_LAMBDA_NAME" \
        --query 'Configuration.FunctionArn' --output text --region "${REGION}")

    # 9e. EventBridge schedule. cron(0 4 * * ? *) = 04:00 UTC daily.
    # put-rule + put-targets are both idempotent on the same Name.
    aws events put-rule \
        --name "$BACKUP_SCHEDULE_NAME" \
        --schedule-expression "cron(0 4 * * ? *)" \
        --description "Daily DynamoDB backup export to S3 (Phase 4 M-E7 item 8)" \
        --state ENABLED \
        --region "${REGION}" >/dev/null
    # add-permission is NOT idempotent on the StatementId; ignore the
    # ResourceConflict on re-runs (idempotency by retry-and-ignore).
    aws lambda add-permission \
        --function-name "$BACKUP_LAMBDA_NAME" \
        --statement-id "${BACKUP_SCHEDULE_NAME}-invoke" \
        --action lambda:InvokeFunction \
        --principal events.amazonaws.com \
        --source-arn "arn:aws:events:${REGION}:${ACCOUNT_ID}:rule/${BACKUP_SCHEDULE_NAME}" \
        --region "${REGION}" >/dev/null 2>&1 || true
    aws events put-targets \
        --rule "$BACKUP_SCHEDULE_NAME" \
        --targets "Id=1,Arn=${BACKUP_LAMBDA_ARN}" \
        --region "${REGION}" >/dev/null
    echo "Scheduled daily backup at 04:00 UTC: $BACKUP_SCHEDULE_NAME"

    # 9f. SNS topic + email subscription for alarm notifications.
    # create-topic is idempotent (returns the existing ARN on
    # subsequent calls); subscribe is idempotent on (topic, protocol,
    # endpoint).
    BACKUP_TOPIC_ARN=$(aws sns create-topic --name "$BACKUP_TOPIC_NAME" \
        --query 'TopicArn' --output text --region "${REGION}")
    aws sns subscribe \
        --topic-arn "$BACKUP_TOPIC_ARN" \
        --protocol email \
        --notification-endpoint "$NOTIFICATION_EMAIL" \
        --region "${REGION}" >/dev/null 2>&1 || true

    # 9g. CloudWatch alarm. Errors metric over a 24h period. With
    # treat-missing-data=breaching, the alarm also fires when the
    # Lambda DIDN'T RUN — catches the case where the EventBridge
    # rule gets accidentally disabled. False alarms on first deploy
    # (no data yet) clear themselves after the first run.
    aws cloudwatch put-metric-alarm \
        --alarm-name "$BACKUP_ALARM_NAME" \
        --alarm-description "DDB backup Lambda errored or didn't run today" \
        --namespace AWS/Lambda \
        --metric-name Errors \
        --statistic Sum \
        --dimensions "Name=FunctionName,Value=${BACKUP_LAMBDA_NAME}" \
        --period 86400 \
        --evaluation-periods 1 \
        --threshold 1 \
        --comparison-operator GreaterThanOrEqualToThreshold \
        --alarm-actions "$BACKUP_TOPIC_ARN" \
        --treat-missing-data breaching \
        --region "${REGION}" >/dev/null
    echo "Configured CloudWatch alarm: $BACKUP_ALARM_NAME"

    echo ""
else
    echo "--- Phase 9: Backup exports (skipped — DEPLOY_ENV != prod) ---"
    echo ""
fi

# ─── Done ───────────────────────────────────────────────────────

echo "==========================================="
echo "  Deployment complete!"
echo "==========================================="
echo ""
echo "  URL:     ${SITE_URL}"
echo "  Health:  ${SITE_URL}/health"
if [ -n "$DOMAIN_NAME" ]; then
    echo "  Domain:  ${DOMAIN_NAME}"
fi
echo ""
echo "  The ECS task may take 2-3 minutes to start."
echo "  Check status: aws ecs describe-services --cluster ${CLUSTER_NAME} --services ${SERVICE_NAME} --region ${REGION}"
echo "  View logs:    aws logs tail ${LOG_GROUP} --follow --region ${REGION}"
echo ""
echo "  IMPORTANT: Update your GitHub OAuth app callback URL to:"
if [ -n "$DOMAIN_NAME" ]; then
    echo "    https://${DOMAIN_NAME}/auth/complete"
else
    echo "    http://${ALB_DNS}/auth/complete"
fi
echo ""
echo "  To redeploy after code changes:"
echo "    docker build -t ${ECR_REPO}:latest -f Dockerfile ."
echo "    docker tag ${ECR_REPO}:latest ${ECR_URI}:latest"
echo "    aws ecr get-login-password --region ${REGION} | docker login --username AWS --password-stdin ${ACCOUNT_ID}.dkr.ecr.${REGION}.amazonaws.com"
echo "    docker push ${ECR_URI}:latest"
echo "    aws ecs update-service --cluster ${CLUSTER_NAME} --service ${SERVICE_NAME} --force-new-deployment --region ${REGION}"
echo ""
echo "  To tear down: ./scripts/aws-test-destroy.sh"
