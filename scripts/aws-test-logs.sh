#!/usr/bin/env bash
#
# OgreNote — follow CloudWatch logs for the deployed test stack's ECS task.
#
# Prefers the read-only diagnostic profile (created by
# scripts/aws-test-create-readonly.sh) when it's configured, so tailing
# doesn't need admin credentials. Override by exporting AWS_PROFILE first.
#
# Usage:
#   source scripts/aws-test-config.env
#   ./scripts/aws-test-logs.sh                                  # follow live
#   ./scripts/aws-test-logs.sh --since 1h                       # start further back
#   ./scripts/aws-test-logs.sh --filter-pattern '"ERROR"'       # only error lines
#   ./scripts/aws-test-logs.sh --format short                   # compact output
#
# Any extra arguments are passed straight through to `aws logs tail`.

set -euo pipefail

for var in AWS_REGION STACK_PREFIX; do
    if [ -z "${!var:-}" ]; then
        echo "ERROR: $var is not set. Source scripts/aws-test-config.env first." >&2
        exit 1
    fi
done

LOG_GROUP="/ecs/${STACK_PREFIX}ogrenote"
REGION="${AWS_REGION}"
DIAG_PROFILE="${STACK_PREFIX}ogrenote-diag"

# Auto-select the read-only diag profile if it's configured and the caller
# hasn't explicitly set AWS_PROFILE. Users who want to run as admin can
# `AWS_PROFILE=default ./scripts/aws-test-logs.sh ...` or unset it first.
if [ -z "${AWS_PROFILE:-}" ] \
    && aws configure get aws_access_key_id --profile "$DIAG_PROFILE" >/dev/null 2>&1; then
    export AWS_PROFILE="$DIAG_PROFILE"
    echo "[using profile: $DIAG_PROFILE]" >&2
fi

echo "[log group: $LOG_GROUP  region: $REGION]" >&2
echo "[Ctrl-C to stop]" >&2
exec aws logs tail "$LOG_GROUP" --follow --region "$REGION" "$@"
