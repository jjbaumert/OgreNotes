#!/usr/bin/env bash
#
# Reset development data: wipes DynamoDB table contents and S3 bucket objects.
# Preserves user accounts by default (--keep-users, on by default).
# Use --all to delete everything including users.
#
# Requires: AWS CLI, DYNAMODB_TABLE_PREFIX, S3_BUCKET, AWS_REGION env vars
# (same as the API server).
#
# Usage:
#   ./scripts/reset-dev-data.sh              # keeps users, deletes docs/folders/etc
#   ./scripts/reset-dev-data.sh --all        # deletes everything including users

set -euo pipefail

# ── Parse args ──

KEEP_USERS=true
for arg in "$@"; do
    case "$arg" in
        --all) KEEP_USERS=false ;;
        --keep-users) KEEP_USERS=true ;;
        *) echo "Unknown argument: $arg"; exit 1 ;;
    esac
done

# ── Load config ──

TABLE_PREFIX="${DYNAMODB_TABLE_PREFIX:?DYNAMODB_TABLE_PREFIX is required}"
S3_BUCKET="${S3_BUCKET:?S3_BUCKET is required}"
AWS_REGION="${AWS_REGION:-us-east-1}"
TABLE_NAME="${TABLE_PREFIX}ogrenotes"

# ── Safety checks ──

# Refuse to run against production-looking names
if [[ "$TABLE_PREFIX" == "prod"* ]] || [[ "$TABLE_PREFIX" == "production"* ]]; then
    echo "ERROR: TABLE_PREFIX starts with 'prod' — refusing to run against production."
    exit 1
fi

if [[ "$S3_BUCKET" == *"prod"* ]] || [[ "$S3_BUCKET" == *"production"* ]]; then
    echo "ERROR: S3_BUCKET contains 'prod' — refusing to run against production."
    exit 1
fi

# ── Show what will be deleted ──

echo "============================================"
if $KEEP_USERS; then
echo "  WARNING: This will DELETE ALL DOCUMENTS"
echo "  (User accounts will be preserved)"
else
echo "  WARNING: This will DELETE ALL DATA"
echo "  (Including user accounts)"
fi
echo "============================================"
echo ""
echo "  DynamoDB table:  $TABLE_NAME"
echo "  S3 bucket:       $S3_BUCKET"
echo "  AWS region:      $AWS_REGION"
echo "  Keep users:      $KEEP_USERS"
echo ""

# Count items in DynamoDB (sample)
ITEM_COUNT=$(aws dynamodb scan \
    --table-name "$TABLE_NAME" \
    --select COUNT \
    --region "$AWS_REGION" \
    --query 'Count' \
    --output text 2>/dev/null || echo "?")
echo "  DynamoDB items:  ~$ITEM_COUNT"

# Count objects in S3
OBJECT_COUNT=$(aws s3api list-objects-v2 \
    --bucket "$S3_BUCKET" \
    --region "$AWS_REGION" \
    --query 'KeyCount' \
    --output text 2>/dev/null || echo "?")
echo "  S3 objects:      ~$OBJECT_COUNT"

echo ""
echo "============================================"
echo ""

# ── Are you sure? ──

read -p "Type 'DELETE' to confirm: " CONFIRM
if [[ "$CONFIRM" != "DELETE" ]]; then
    echo "Aborted."
    exit 1
fi

echo ""

# ── Delete DynamoDB items ──

echo "Deleting all items from DynamoDB table: $TABLE_NAME ..."

# Scan all items and batch-delete them (DynamoDB has no "truncate" command)
ITEMS_DELETED=0
LAST_KEY=""
while true; do
    if [[ -z "$LAST_KEY" ]]; then
        SCAN_RESULT=$(aws dynamodb scan \
            --table-name "$TABLE_NAME" \
            --region "$AWS_REGION" \
            --projection-expression "PK, SK" \
            --output json)
    else
        SCAN_RESULT=$(aws dynamodb scan \
            --table-name "$TABLE_NAME" \
            --region "$AWS_REGION" \
            --projection-expression "PK, SK" \
            --exclusive-start-key "$LAST_KEY" \
            --output json)
    fi

    # Extract items, optionally filtering out USER# rows
    ITEMS=$(echo "$SCAN_RESULT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
items = data.get('Items', [])
keep_users = '$KEEP_USERS' == 'true'
# Build batch delete requests (max 25 per batch)
batch = []
for item in items:
    pk = item['PK'].get('S', '')
    if keep_users and pk.startswith('USER#'):
        continue
    batch.append({'DeleteRequest': {'Key': {'PK': item['PK'], 'SK': item['SK']}}})
    if len(batch) == 25:
        print(json.dumps({'$TABLE_NAME': batch}))
        batch = []
if batch:
    print(json.dumps({'$TABLE_NAME': batch}))
" 2>/dev/null)

    if [[ -n "$ITEMS" ]]; then
        while IFS= read -r BATCH; do
            aws dynamodb batch-write-item \
                --request-items "$BATCH" \
                --region "$AWS_REGION" \
                --output text > /dev/null
            COUNT=$(echo "$BATCH" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['$TABLE_NAME']))")
            ITEMS_DELETED=$((ITEMS_DELETED + COUNT))
            echo "  Deleted $ITEMS_DELETED items..."
        done <<< "$ITEMS"
    fi

    # Check for pagination
    LAST_KEY=$(echo "$SCAN_RESULT" | python3 -c "
import json, sys
data = json.load(sys.stdin)
key = data.get('LastEvaluatedKey')
print(json.dumps(key) if key else '')
" 2>/dev/null)

    if [[ -z "$LAST_KEY" ]]; then
        break
    fi
done

echo "  Done. Deleted $ITEMS_DELETED items from DynamoDB."
echo ""

# ── Delete S3 objects ──

echo "Deleting all objects from S3 bucket: $S3_BUCKET ..."

aws s3 rm "s3://$S3_BUCKET" \
    --recursive \
    --region "$AWS_REGION" \
    2>&1 | tail -5

echo "  Done."
echo ""
echo "All development data has been deleted."
echo ""

# ── Re-create dev users (Alice and Bob) ──

API_URL="${API_URL:-http://localhost:3000}"

echo "Re-creating dev users via $API_URL ..."

for USER_JSON in \
    '{"email":"alice@ogrenotes.local","name":"Alice"}' \
    '{"email":"bob@ogrenotes.local","name":"Bob"}'
do
    NAME=$(echo "$USER_JSON" | python3 -c "import json,sys; print(json.load(sys.stdin)['name'])")
    RESULT=$(curl -s -o /dev/null -w "%{http_code}" \
        -X POST "$API_URL/api/v1/auth/dev-login" \
        -H "Content-Type: application/json" \
        -d "$USER_JSON" 2>/dev/null || echo "000")

    if [[ "$RESULT" == "200" ]]; then
        echo "  Created $NAME"
    elif [[ "$RESULT" == "000" ]]; then
        echo "  Could not reach API server at $API_URL — start the server and log in manually."
        break
    else
        echo "  Failed to create $NAME (HTTP $RESULT) — is DEV_MODE=true?"
    fi
done

echo ""
echo "Done. The database is clean with dev users ready."
