#!/usr/bin/env bash
# Tear the CDK test stack down. The DynamoDB table and S3 bucket are RETAINed
# (infra/lib/data.ts) and survive; scripts/test-stack-start.sh adopts them
# back into the stack on the next start.
#
#   scripts/test-stack-stop.sh [extra cdk destroy args, e.g. --force]
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
ENV_NAME=test

set -a
# shellcheck disable=SC1091
source "$DIR/aws-test-config.env"
set +a
: "${STACK_PREFIX:?STACK_PREFIX not set in scripts/aws-test-config.env}"

(cd "$ROOT/infra" && npm run destroy -- -c "env=$ENV_NAME" -c "prefix=$STACK_PREFIX" "$@")

echo "[stop] OgreNotes-$ENV_NAME destroyed; ${STACK_PREFIX}ogrenote table + bucket retained."
echo "[stop] Bring it back with scripts/test-stack-start.sh"
