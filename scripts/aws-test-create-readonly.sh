#!/usr/bin/env bash
#
# OgreNote — Read-Only IAM User for Diagnostic Probing
#
# Creates (or verifies) a dedicated IAM user scoped to read-only access on the
# DynamoDB table, S3 bucket, CloudWatch log group, and ECS resources that belong
# to the current test stack (identified by STACK_PREFIX). Writes a named AWS
# profile to ~/.aws/credentials that the `aws-diagnostic` Claude Code subagent
# (see .claude/agents/aws-diagnostic.md) will assume via AWS_PROFILE.
#
# Prerequisites:
#   - AWS CLI v2 with credentials that can manage IAM
#   - source scripts/aws-test-config.env
#   - The test stack must already be deployed (DynamoDB table + S3 bucket exist)
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-create-readonly.sh
#
# Idempotent: safe to re-run. Self-verifies read succeeds and write is denied.
# Cleanup: ./scripts/aws-test-destroy-readonly.sh

set -euo pipefail

# ─── Validate config ───────────────────────────────────────────

for var in AWS_REGION STACK_PREFIX; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source your config file first:" >&2
        echo "  source scripts/aws-test-config.env" >&2
        exit 1
    fi
done

PREFIX="${STACK_PREFIX}"
REGION="${AWS_REGION}"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)

TABLE_NAME="${PREFIX}ogrenote"
BUCKET_NAME="${PREFIX}ogrenote"
LOG_GROUP="/ecs/${PREFIX}ogrenote"
CLUSTER_NAME="${PREFIX}ogrenote"
SERVICE_NAME="${PREFIX}ogrenote-api"

IAM_USER="${PREFIX}ogrenote-diag"
POLICY_NAME="ogrenote-diag-readonly"
PROFILE_NAME="${PREFIX}ogrenote-diag"

echo "=== OgreNote Read-Only Diagnostic IAM ==="
echo "Region:  ${REGION}"
echo "Prefix:  ${PREFIX}"
echo "Account: ${ACCOUNT_ID}"
echo "User:    ${IAM_USER}"
echo "Profile: ${PROFILE_NAME}"
echo ""

# ─── Phase 1: Pre-flight ───────────────────────────────────────

echo "--- Phase 1: Pre-flight ---"
if ! aws dynamodb describe-table --table-name "$TABLE_NAME" --region "$REGION" >/dev/null 2>&1; then
    echo "ERROR: DynamoDB table ${TABLE_NAME} not found. Deploy the stack first:" >&2
    echo "  ./scripts/aws-test-deploy.sh" >&2
    exit 1
fi
if ! aws s3api head-bucket --bucket "$BUCKET_NAME" --region "$REGION" >/dev/null 2>&1; then
    echo "ERROR: S3 bucket ${BUCKET_NAME} not found. Deploy the stack first." >&2
    exit 1
fi
echo "Table and bucket confirmed."
echo ""

# ─── Phase 2: IAM user ─────────────────────────────────────────

echo "--- Phase 2: IAM user ---"
if aws iam get-user --user-name "$IAM_USER" >/dev/null 2>&1; then
    echo "IAM user exists: $IAM_USER"
else
    aws iam create-user --user-name "$IAM_USER" \
        --tags "Key=Purpose,Value=ogrenote-diagnostic-readonly" "Key=Stack,Value=${PREFIX}" \
        >/dev/null
    echo "Created IAM user: $IAM_USER"
fi
echo ""

# ─── Phase 3: Inline policy ────────────────────────────────────

echo "--- Phase 3: Inline policy ---"
POLICY_DOC=$(cat <<POLICY
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "DynamoReadOnly",
            "Effect": "Allow",
            "Action": [
                "dynamodb:DescribeTable",
                "dynamodb:GetItem",
                "dynamodb:BatchGetItem",
                "dynamodb:Query",
                "dynamodb:Scan"
            ],
            "Resource": [
                "arn:aws:dynamodb:${REGION}:${ACCOUNT_ID}:table/${TABLE_NAME}",
                "arn:aws:dynamodb:${REGION}:${ACCOUNT_ID}:table/${TABLE_NAME}/index/*"
            ]
        },
        {
            "Sid": "S3ReadOnly",
            "Effect": "Allow",
            "Action": ["s3:GetObject"],
            "Resource": "arn:aws:s3:::${BUCKET_NAME}/*"
        },
        {
            "Sid": "S3ListBucket",
            "Effect": "Allow",
            "Action": ["s3:ListBucket", "s3:GetBucketLocation"],
            "Resource": "arn:aws:s3:::${BUCKET_NAME}"
        },
        {
            "Sid": "LogsReadOnly",
            "Effect": "Allow",
            "Action": [
                "logs:DescribeLogGroups",
                "logs:DescribeLogStreams",
                "logs:GetLogEvents",
                "logs:FilterLogEvents",
                "logs:StartLiveTail"
            ],
            "Resource": [
                "arn:aws:logs:${REGION}:${ACCOUNT_ID}:log-group:${LOG_GROUP}",
                "arn:aws:logs:${REGION}:${ACCOUNT_ID}:log-group:${LOG_GROUP}:*"
            ]
        },
        {
            "Sid": "EcsDescribeOnly",
            "Effect": "Allow",
            "Action": [
                "ecs:DescribeServices",
                "ecs:DescribeTasks",
                "ecs:DescribeTaskDefinition",
                "ecs:ListTasks"
            ],
            "Resource": "*"
        },
        {
            "Sid": "CloudWatchMetricsReadOnly",
            "Effect": "Allow",
            "Action": [
                "cloudwatch:ListMetrics",
                "cloudwatch:GetMetricStatistics",
                "cloudwatch:GetMetricData"
            ],
            "Resource": "*"
        },
        {
            "Sid": "StsWhoAmI",
            "Effect": "Allow",
            "Action": ["sts:GetCallerIdentity"],
            "Resource": "*"
        }
    ]
}
POLICY
)

aws iam put-user-policy \
    --user-name "$IAM_USER" \
    --policy-name "$POLICY_NAME" \
    --policy-document "$POLICY_DOC"
echo "Attached inline policy: $POLICY_NAME"
echo ""

# ─── Phase 4: Access key ───────────────────────────────────────

echo "--- Phase 4: Access key ---"
EXISTING_KEYS=$(aws iam list-access-keys --user-name "$IAM_USER" \
    --query 'AccessKeyMetadata[].AccessKeyId' --output text)

AKID=""
SECRET=""

if [ -n "$EXISTING_KEYS" ]; then
    # Check if the credentials file already has a working profile for this user.
    # If yes, reuse; if no, we can't recover the secret — rotate.
    if aws configure get aws_access_key_id --profile "$PROFILE_NAME" >/dev/null 2>&1; then
        STORED_AKID=$(aws configure get aws_access_key_id --profile "$PROFILE_NAME")
        if echo "$EXISTING_KEYS" | tr '\t' '\n' | grep -qx "$STORED_AKID"; then
            echo "Reusing existing access key from profile: $STORED_AKID"
            AKID="$STORED_AKID"
        fi
    fi

    if [ -z "$AKID" ]; then
        echo "Existing access keys present but profile secret unknown — rotating."
        for old in $EXISTING_KEYS; do
            aws iam delete-access-key --user-name "$IAM_USER" --access-key-id "$old"
            echo "  Deleted old key: $old"
        done
    fi
fi

if [ -z "$AKID" ]; then
    # Use --query to extract both fields in a single call; --output text with
    # a list query emits them tab-separated. More robust than sed-matching
    # the raw JSON whitespace.
    KEY_PAIR=$(aws iam create-access-key --user-name "$IAM_USER" \
        --query '[AccessKey.AccessKeyId, AccessKey.SecretAccessKey]' \
        --output text)
    AKID=$(echo "$KEY_PAIR" | cut -f1)
    SECRET=$(echo "$KEY_PAIR" | cut -f2)
    if [ -z "$AKID" ] || [ -z "$SECRET" ]; then
        echo "ERROR: failed to parse access key response." >&2
        exit 1
    fi
    echo "Created access key: $AKID"
fi

# ─── Phase 5: Write ~/.aws/credentials profile ─────────────────

echo ""
echo "--- Phase 5: Profile ---"
# IAM needs a few seconds for a fresh access key to propagate before the
# positive verification step will succeed.
if [ -n "$SECRET" ]; then
    aws configure set aws_access_key_id     "$AKID"   --profile "$PROFILE_NAME"
    aws configure set aws_secret_access_key "$SECRET" --profile "$PROFILE_NAME"
    aws configure set region                "$REGION" --profile "$PROFILE_NAME"
    echo "Wrote profile '${PROFILE_NAME}' to ~/.aws/credentials"
    echo "Waiting 10s for IAM propagation..."
    sleep 10
else
    # Key reused; just make sure region is right.
    aws configure set region "$REGION" --profile "$PROFILE_NAME"
    echo "Profile '${PROFILE_NAME}' already present — refreshed region only"
fi

# ─── Phase 6: Positive verification ────────────────────────────

echo ""
echo "--- Phase 6: Verify read access ---"
if AWS_PROFILE="$PROFILE_NAME" aws dynamodb describe-table \
        --table-name "$TABLE_NAME" --region "$REGION" \
        --query 'Table.TableStatus' --output text >/dev/null; then
    echo "OK: describe-table succeeded"
else
    echo "ERROR: describe-table failed under profile $PROFILE_NAME" >&2
    echo "       (IAM propagation can take up to 30s — try re-running.)" >&2
    exit 1
fi

if AWS_PROFILE="$PROFILE_NAME" aws s3api list-objects-v2 \
        --bucket "$BUCKET_NAME" --max-items 1 --region "$REGION" >/dev/null; then
    echo "OK: s3 list-objects-v2 succeeded"
else
    echo "ERROR: s3 list-objects-v2 failed under profile $PROFILE_NAME" >&2
    exit 1
fi

# ─── Phase 7: Negative verification ────────────────────────────

echo ""
echo "--- Phase 7: Verify writes are denied ---"
set +e
DENY_OUT=$(AWS_PROFILE="$PROFILE_NAME" aws dynamodb put-item \
    --table-name "$TABLE_NAME" \
    --item '{"PK":{"S":"__DIAG_PROBE__"},"SK":{"S":"__DIAG_PROBE__"}}' \
    --region "$REGION" 2>&1)
DENY_RC=$?
set -e
if [ $DENY_RC -eq 0 ]; then
    echo "ERROR: put-item SUCCEEDED under read-only profile — policy is too permissive." >&2
    echo "       Rolling back the probe row..." >&2
    # Use the ADMIN credentials (default profile) to remove the row we just wrote,
    # so we don't leave garbage behind.
    aws dynamodb delete-item --table-name "$TABLE_NAME" \
        --key '{"PK":{"S":"__DIAG_PROBE__"},"SK":{"S":"__DIAG_PROBE__"}}' \
        --region "$REGION" >/dev/null || true
    exit 1
fi
if echo "$DENY_OUT" | grep -q "AccessDeniedException"; then
    echo "OK: put-item correctly denied (AccessDeniedException)"
else
    echo "ERROR: put-item failed but not with AccessDeniedException:" >&2
    echo "$DENY_OUT" >&2
    exit 1
fi

# ─── Done ──────────────────────────────────────────────────────

echo ""
echo "==========================================="
echo "  Read-only profile ready!"
echo "==========================================="
echo ""
echo "  Profile:     ${PROFILE_NAME}"
echo "  IAM user:    ${IAM_USER}"
echo "  Scope:       table=${TABLE_NAME}, bucket=${BUCKET_NAME},"
echo "               log-group=${LOG_GROUP},"
echo "               ecs=${CLUSTER_NAME}/${SERVICE_NAME}"
echo ""
echo "  The aws-diagnostic subagent will export AWS_PROFILE=${PROFILE_NAME}"
echo "  automatically after sourcing scripts/aws-test-config.env."
echo ""
echo "  To remove: ./scripts/aws-test-destroy-readonly.sh"
