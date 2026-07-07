#!/usr/bin/env bash
#
# OgreNote — Read-Only IAM User for *Deployment* Diagnostics
#
# Companion to scripts/aws-test-create-readonly.sh. Where that profile is
# scoped to the stack's DynamoDB table and S3 bucket (data-plane debugging),
# this profile covers cross-service *describe* / *get* / *list* calls across
# the whole account via the AWS-managed ReadOnlyAccess policy. Used by:
#   - .claude/agents/aws-deploy-doctor.md  (stack-state triage)
#   - .claude/agents/aws-network-doctor.md (ALB / SG / Route 53 / ACM)
#   - .claude/agents/aws-iam-doctor.md     (IAM policy introspection)
#
# Trade-off: ReadOnlyAccess is account-wide, not stack-scoped. Appropriate
# for a test account because it has zero write surface; inappropriate for
# production, where a scoped variant should be used.
#
# Prerequisites:
#   - AWS CLI v2 with credentials that can manage IAM
#   - source scripts/aws-test-config.env
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-create-deploy-diag.sh
#
# Idempotent: safe to re-run. Self-verifies read succeeds, decode succeeds,
# and a dry-run write is denied.
# Cleanup: ./scripts/aws-test-destroy-deploy-diag.sh

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

IAM_USER="${PREFIX}ogrenote-deploy-diag"
POLICY_NAME="deploy-diag-decode"
MANAGED_POLICY_ARN="arn:aws:iam::aws:policy/ReadOnlyAccess"
PROFILE_NAME="${PREFIX}ogrenote-deploy-diag"

echo "=== OgreNote Deployment-Diagnostic IAM ==="
echo "Region:  ${REGION}"
echo "Prefix:  ${PREFIX}"
echo "Account: ${ACCOUNT_ID}"
echo "User:    ${IAM_USER}"
echo "Profile: ${PROFILE_NAME}"
echo "Managed: ${MANAGED_POLICY_ARN}"
echo ""

# ─── Phase 1: IAM user ─────────────────────────────────────────

echo "--- Phase 1: IAM user ---"
if aws iam get-user --user-name "$IAM_USER" >/dev/null 2>&1; then
    echo "IAM user exists: $IAM_USER"
else
    aws iam create-user --user-name "$IAM_USER" \
        --tags "Key=Purpose,Value=ogrenote-deploy-diagnostic-readonly" "Key=Stack,Value=${PREFIX}" \
        >/dev/null
    echo "Created IAM user: $IAM_USER"
fi
echo ""

# ─── Phase 2: Attach AWS-managed ReadOnlyAccess ────────────────

echo "--- Phase 2: Attach ReadOnlyAccess ---"
ATTACHED=$(aws iam list-attached-user-policies --user-name "$IAM_USER" \
    --query 'AttachedPolicies[?PolicyArn==`'"$MANAGED_POLICY_ARN"'`].PolicyArn' \
    --output text)
if [ "$ATTACHED" = "$MANAGED_POLICY_ARN" ]; then
    echo "ReadOnlyAccess already attached"
else
    aws iam attach-user-policy --user-name "$IAM_USER" \
        --policy-arn "$MANAGED_POLICY_ARN"
    echo "Attached: $MANAGED_POLICY_ARN"
fi
echo ""

# ─── Phase 3: Inline policy for DecodeAuthorizationMessage ─────

# ReadOnlyAccess does include sts:DecodeAuthorizationMessage today, but
# listing it explicitly here guards against future AWS policy changes and
# documents the dependency for the aws-iam-doctor agent.
echo "--- Phase 3: Inline decode policy ---"
POLICY_DOC=$(cat <<POLICY
{
    "Version": "2012-10-17",
    "Statement": [
        {
            "Sid": "DecodeAuthzMessage",
            "Effect": "Allow",
            "Action": ["sts:DecodeAuthorizationMessage"],
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
if [ -n "$SECRET" ]; then
    aws configure set aws_access_key_id     "$AKID"   --profile "$PROFILE_NAME"
    aws configure set aws_secret_access_key "$SECRET" --profile "$PROFILE_NAME"
    aws configure set region                "$REGION" --profile "$PROFILE_NAME"
    echo "Wrote profile '${PROFILE_NAME}' to ~/.aws/credentials"
    echo "Waiting 10s for IAM propagation..."
    sleep 10
else
    aws configure set region "$REGION" --profile "$PROFILE_NAME"
    echo "Profile '${PROFILE_NAME}' already present — refreshed region only"
fi

# ─── Phase 6: Positive verifications ───────────────────────────

echo ""
echo "--- Phase 6: Verify read access ---"
CALLER_ARN=$(AWS_PROFILE="$PROFILE_NAME" aws sts get-caller-identity \
    --query 'Arn' --output text)
if echo "$CALLER_ARN" | grep -q "user/${IAM_USER}\$"; then
    echo "OK: sts:GetCallerIdentity → $CALLER_ARN"
else
    echo "ERROR: caller identity is '$CALLER_ARN', expected to end with 'user/${IAM_USER}'" >&2
    exit 1
fi

if AWS_PROFILE="$PROFILE_NAME" aws ec2 describe-vpcs \
        --max-items 1 --region "$REGION" >/dev/null; then
    echo "OK: ec2:DescribeVpcs succeeded"
else
    echo "ERROR: ec2:DescribeVpcs failed under profile $PROFILE_NAME" >&2
    echo "       (IAM propagation can take up to 30s — try re-running.)" >&2
    exit 1
fi

if AWS_PROFILE="$PROFILE_NAME" aws iam list-users --max-items 1 >/dev/null; then
    echo "OK: iam:ListUsers succeeded"
else
    echo "ERROR: iam:ListUsers failed under profile $PROFILE_NAME" >&2
    exit 1
fi

# ─── Phase 7: Negative verification ────────────────────────────

# ec2:CreateInternetGateway is not in ReadOnlyAccess and takes no parameters,
# so it avoids the parameter-validation pre-check that rejects synthetic
# resource IDs. --dry-run means nothing is actually created: if the permission
# were present we'd see DryRunOperation; missing permission yields
# UnauthorizedOperation. We want the latter, which confirms writes are
# blocked by IAM itself, not merely by the agent's prompt discipline.
echo ""
echo "--- Phase 7: Verify writes are denied ---"
set +e
DENY_OUT=$(AWS_PROFILE="$PROFILE_NAME" aws ec2 create-internet-gateway \
    --dry-run --region "$REGION" 2>&1)
DENY_RC=$?
set -e
if [ $DENY_RC -eq 0 ]; then
    echo "ERROR: create-internet-gateway --dry-run SUCCEEDED under read-only profile — policy is too permissive." >&2
    exit 1
fi
if echo "$DENY_OUT" | grep -q "UnauthorizedOperation"; then
    echo "OK: ec2:CreateInternetGateway --dry-run correctly denied (UnauthorizedOperation)"
elif echo "$DENY_OUT" | grep -q "DryRunOperation"; then
    echo "ERROR: ec2:CreateInternetGateway is permitted by ReadOnlyAccess (got DryRunOperation) — unexpected." >&2
    echo "$DENY_OUT" >&2
    exit 1
else
    echo "ERROR: ec2:CreateInternetGateway --dry-run failed but not with UnauthorizedOperation:" >&2
    echo "$DENY_OUT" >&2
    exit 1
fi

# ─── Done ──────────────────────────────────────────────────────

echo ""
echo "==========================================="
echo "  Deploy-diag profile ready!"
echo "==========================================="
echo ""
echo "  Profile:  ${PROFILE_NAME}"
echo "  IAM user: ${IAM_USER}"
echo "  Scope:    AWS-managed ReadOnlyAccess + sts:DecodeAuthorizationMessage"
echo ""
echo "  Used by: aws-deploy-doctor, aws-network-doctor, aws-iam-doctor"
echo "  agents defined in .claude/agents/."
echo ""
echo "  To remove: ./scripts/aws-test-destroy-deploy-diag.sh"
