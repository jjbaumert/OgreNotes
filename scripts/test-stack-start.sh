#!/usr/bin/env bash
# Bring the CDK test stack up — whether it is running, or was torn down with
# scripts/test-stack-stop.sh.
#
#   scripts/test-stack-start.sh [extra cdk deploy args...]
#
# `destroy` deliberately RETAINs the DynamoDB table and S3 bucket (data-loss
# backstop, infra/lib/data.ts), and a later plain `cdk deploy` then fails
# early validation with "<name> already exists". `cdk import` can't adopt
# them either (CDK passes stack Tags/RoleArn, which an import change set
# rejects). So when the stack is gone but the data survives, this script
# adopts the table + bucket with a raw CloudFormation IMPORT change set built
# from the synthesized template, then runs the normal full deploy on top.
#
# Reads STACK_PREFIX / AWS_REGION from scripts/aws-test-config.env, like the
# deploy. Stack name follows infra/bin/app.ts: OgreNotes-<env>.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
ENV_NAME=test
STACK="OgreNotes-$ENV_NAME"

set -a
# shellcheck disable=SC1091
source "$DIR/aws-test-config.env"
set +a
: "${STACK_PREFIX:?STACK_PREFIX not set in scripts/aws-test-config.env}"
export AWS_REGION="${AWS_REGION:-us-east-1}"
export GIT_STAMP="${GIT_STAMP:-$(git -C "$ROOT" rev-parse --short HEAD)}"
DATA_NAME="${STACK_PREFIX}ogrenote"
CDK_CTX=(-c "env=$ENV_NAME" -c "prefix=$STACK_PREFIX")

log() { echo "[start] $*"; }

stack_status() {
  aws cloudformation describe-stacks --stack-name "$STACK" \
    --query 'Stacks[0].StackStatus' --output text 2>/dev/null || echo NONE
}

deploy() {
  log "full deploy of $STACK (GIT_STAMP=$GIT_STAMP)"
  (cd "$ROOT/infra" && npm run deploy -- "${CDK_CTX[@]}" --require-approval never "$@")
}

STATUS="$(stack_status)"
log "$STACK status: $STATUS"

# A failed create leaves an empty REVIEW_IN_PROGRESS shell that blocks both
# a new create and an import. It holds no resources, so it is safe to drop.
if [[ "$STATUS" == REVIEW_IN_PROGRESS ]]; then
  log "deleting empty REVIEW_IN_PROGRESS stack left by a failed create"
  aws cloudformation delete-stack --stack-name "$STACK"
  aws cloudformation wait stack-delete-complete --stack-name "$STACK"
  STATUS=NONE
fi

if [[ "$STATUS" != NONE ]]; then
  deploy "$@"
  exit 0
fi

# ── Stack is gone: adopt whatever retained data survived ──

# Stacks torn down before the api/worker log groups switched to DESTROY in
# test left them behind. They'd block the create; they only hold stale logs.
for g in "/ecs/${STACK_PREFIX}ogrenote-api" "/ecs/${STACK_PREFIX}ogrenote-worker"; do
  if aws logs describe-log-groups --log-group-name-prefix "$g" \
    --query "logGroups[?logGroupName=='$g'] | length(@)" --output text | grep -qx 1; then
    log "deleting leftover log group $g"
    aws logs delete-log-group --log-group-name "$g"
  fi
done

HAVE_TABLE=0
HAVE_BUCKET=0
aws dynamodb describe-table --table-name "$DATA_NAME" >/dev/null 2>&1 && HAVE_TABLE=1
aws s3api head-bucket --bucket "$DATA_NAME" >/dev/null 2>&1 && HAVE_BUCKET=1
log "retained table: $HAVE_TABLE, retained bucket: $HAVE_BUCKET"

if [[ $HAVE_TABLE == 0 && $HAVE_BUCKET == 0 ]]; then
  log "no retained data — fresh create"
  deploy "$@"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

log "synthesizing template"
(cd "$ROOT/infra" && npx cdk synth "$STACK" "${CDK_CTX[@]}" -q)

# Build the import template (only the retained resources) and the
# resources-to-import list, matching on the physical names so a logical-ID
# change in data.ts can't import the wrong thing.
HAVE_TABLE=$HAVE_TABLE HAVE_BUCKET=$HAVE_BUCKET DATA_NAME=$DATA_NAME \
  TEMPLATE="$ROOT/infra/cdk.out/$STACK.template.json" WORK="$WORK" node -e '
const fs = require("fs");
const t = JSON.parse(fs.readFileSync(process.env.TEMPLATE, "utf8"));
const name = process.env.DATA_NAME;
const want = [];
if (process.env.HAVE_TABLE === "1")
  want.push(["AWS::DynamoDB::Table", "TableName"]);
if (process.env.HAVE_BUCKET === "1")
  want.push(["AWS::S3::Bucket", "BucketName"]);
const resources = {};
const toImport = [];
for (const [type, prop] of want) {
  const hits = Object.entries(t.Resources).filter(
    ([, r]) => r.Type === type && r.Properties && r.Properties[prop] === name,
  );
  if (hits.length !== 1) {
    console.error(`expected exactly one ${type} named ${name}, found ${hits.length}`);
    process.exit(1);
  }
  const [id, r] = hits[0];
  if (r.DeletionPolicy !== "Retain") {
    console.error(`${id} must have DeletionPolicy Retain to be imported`);
    process.exit(1);
  }
  resources[id] = r;
  toImport.push({ ResourceType: type, LogicalResourceId: id, ResourceIdentifier: { [prop]: name } });
}
fs.writeFileSync(`${process.env.WORK}/import.json`, JSON.stringify({ Resources: resources }));
fs.writeFileSync(`${process.env.WORK}/to-import.json`, JSON.stringify(toImport));
'

log "importing retained data into a new $STACK"
aws cloudformation create-change-set --stack-name "$STACK" \
  --change-set-name import-retained-data --change-set-type IMPORT \
  --template-body "file://$WORK/import.json" \
  --resources-to-import "file://$WORK/to-import.json" >/dev/null
aws cloudformation wait change-set-create-complete \
  --stack-name "$STACK" --change-set-name import-retained-data
aws cloudformation execute-change-set --stack-name "$STACK" \
  --change-set-name import-retained-data
aws cloudformation wait stack-import-complete --stack-name "$STACK"
log "import complete"

deploy "$@"
