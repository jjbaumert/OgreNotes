#!/usr/bin/env bash
#
# OgreNote — Tear Down Read-Only Diagnostic IAM User
#
# Reverses scripts/aws-test-create-readonly.sh:
#   - deletes all access keys for the -diag IAM user
#   - deletes the inline read-only policy
#   - deletes the IAM user
#   - removes the named profile from ~/.aws/credentials and ~/.aws/config
#
# Prerequisites:
#   - AWS CLI v2 with credentials that can manage IAM
#   - source scripts/aws-test-config.env
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-destroy-readonly.sh
#
# Idempotent: safe to re-run; skips pieces that are already gone.

set -euo pipefail

for var in STACK_PREFIX; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source your config file first:" >&2
        echo "  source scripts/aws-test-config.env" >&2
        exit 1
    fi
done

PREFIX="${STACK_PREFIX}"
IAM_USER="${PREFIX}ogrenote-diag"
POLICY_NAME="ogrenote-diag-readonly"
PROFILE_NAME="${PREFIX}ogrenote-diag"

echo "=== Remove diagnostic IAM ==="
echo "User:    ${IAM_USER}"
echo "Profile: ${PROFILE_NAME}"
echo ""

if aws iam get-user --user-name "$IAM_USER" >/dev/null 2>&1; then
    KEYS=$(aws iam list-access-keys --user-name "$IAM_USER" \
        --query 'AccessKeyMetadata[].AccessKeyId' --output text)
    for k in $KEYS; do
        aws iam delete-access-key --user-name "$IAM_USER" --access-key-id "$k"
        echo "Deleted access key: $k"
    done

    if aws iam get-user-policy --user-name "$IAM_USER" --policy-name "$POLICY_NAME" >/dev/null 2>&1; then
        aws iam delete-user-policy --user-name "$IAM_USER" --policy-name "$POLICY_NAME"
        echo "Deleted inline policy: $POLICY_NAME"
    fi

    aws iam delete-user --user-name "$IAM_USER"
    echo "Deleted IAM user: $IAM_USER"
else
    echo "IAM user ${IAM_USER} does not exist — nothing to delete."
fi

# Remove the profile from ~/.aws/credentials and ~/.aws/config.
# `aws configure` has no "delete profile" verb, so use the ini files directly.
strip_profile() {
    local file=$1 header=$2
    [ -f "$file" ] || return 0
    # Print lines outside the [profile] block only.
    awk -v hdr="$header" '
        $0 == hdr { in_block = 1; next }
        /^\[.*\]$/ { in_block = 0 }
        !in_block { print }
    ' "$file" > "${file}.tmp"
    mv "${file}.tmp" "$file"
    chmod 600 "$file"
}

strip_profile "$HOME/.aws/credentials" "[${PROFILE_NAME}]"
strip_profile "$HOME/.aws/config"      "[profile ${PROFILE_NAME}]"
echo "Removed profile ${PROFILE_NAME} from ~/.aws/credentials and ~/.aws/config"

echo ""
echo "Done."
