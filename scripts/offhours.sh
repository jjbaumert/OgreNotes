#!/usr/bin/env bash
# Manually toggle the off-hours scale-to-zero for the test stack.
#
#   scripts/offhours.sh up     # scale api/worker/qdrant back to running
#   scripts/offhours.sh down   # scale them to zero
#
# Outside the scheduled UP windows the ALB returns 503 (there is no auto-wake).
# Run `up` for an off-schedule session — the stack is healthy in ~2 min. The
# EventBridge schedule keeps firing, so a manual `up` only lasts until the next
# scheduled `down` (and vice-versa).
#
# Function name follows `<prefix>ogrenote-offhours-toggle`; the test stack
# deploys with prefix `test1-`. Override via the 2nd arg for other stacks.
set -euo pipefail

ACTION="${1:-}"
FN="${2:-test1-ogrenote-offhours-toggle}"
case "$ACTION" in
  up | down) ;;
  *)
    echo "usage: $0 up|down [function-name]" >&2
    exit 2
    ;;
esac

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Pick up AWS_REGION / credentials the same way the deploy does.
# shellcheck disable=SC1091
set -a
source "$DIR/aws-test-config.env" 2>/dev/null || true
set +a

echo "invoking $FN with action=$ACTION ..." >&2
aws lambda invoke \
  --function-name "$FN" \
  --payload "{\"action\":\"$ACTION\"}" \
  --cli-binary-format raw-in-base64-out \
  /dev/stdout
echo
