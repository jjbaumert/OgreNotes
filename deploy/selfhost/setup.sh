#!/usr/bin/env bash
# One-time AWS setup for a self-hosted OgreNotes. Safe to re-run: every
# step checks before it creates.
#
# Runs with YOUR AWS CLI credentials (an admin profile), not the app's —
# it creates the table and bucket the app's own policy is not allowed to.
#
#   AWS_PROFILE=admin ./setup.sh
#
# Needs: aws (CLI v2), docker (compose v2), openssl. Reads .env and
# ogrenotes.env from this directory (copy the *.example files first).
set -euo pipefail
cd "$(dirname "$0")"

for f in .env ogrenotes.env caddy.env; do
  [[ -f $f ]] || { echo "missing $f — copy $f.example and fill it in" >&2; exit 1; }
done
set -a
# shellcheck disable=SC1091
source ./.env
# The app's own AWS keys in ogrenotes.env must not be used here.
eval "$(grep -E '^(AWS_REGION|DYNAMODB_TABLE_PREFIX|S3_BUCKET|SECRETS_SSM_PATH)=' ogrenotes.env)"
set +a

: "${OGRENOTES_DOMAIN:?set in .env}" "${AWS_REGION:?}" "${DYNAMODB_TABLE_PREFIX:?}" "${S3_BUCKET:?}" "${SECRETS_SSM_PATH:?}"
REGION=$AWS_REGION
TABLE="${DYNAMODB_TABLE_PREFIX}ogrenote"
BUCKET=$S3_BUCKET
SECRETS_PATH="/${SECRETS_SSM_PATH#/}"; SECRETS_PATH="${SECRETS_PATH%/}/"
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
ORIGIN="https://${OGRENOTES_DOMAIN}${OGRENOTES_PORT:+:$OGRENOTES_PORT}"

say() { printf '\n== %s\n' "$*"; }

say "Account $ACCOUNT_ID, region $REGION, table $TABLE, bucket $BUCKET"

# ── S3 bucket ────────────────────────────────────────────────────────
say "S3 bucket"
if aws s3api head-bucket --bucket "$BUCKET" 2>/dev/null; then
  echo "exists"
elif [[ $REGION == us-east-1 ]]; then
  aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" >/dev/null
else
  aws s3api create-bucket --bucket "$BUCKET" --region "$REGION" \
    --create-bucket-configuration "LocationConstraint=$REGION" >/dev/null
fi
aws s3api put-public-access-block --bucket "$BUCKET" --public-access-block-configuration \
  BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
aws s3api put-bucket-encryption --bucket "$BUCKET" --server-side-encryption-configuration \
  '{"Rules":[{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"AES256"}}]}'
# Versioning is the file backup: an overwritten or deleted object stays
# recoverable. Old versions expire after 30 days to bound the cost.
aws s3api put-bucket-versioning --bucket "$BUCKET" --versioning-configuration Status=Enabled
aws s3api put-bucket-lifecycle-configuration --bucket "$BUCKET" --lifecycle-configuration \
  '{"Rules":[{"ID":"expire-old-versions","Status":"Enabled","Filter":{},
     "NoncurrentVersionExpiration":{"NoncurrentDays":30},
     "AbortIncompleteMultipartUpload":{"DaysAfterInitiation":7}}]}'
# Browsers upload and view images directly against S3 through presigned
# URLs, so the bucket must accept requests from the site's origin.
aws s3api put-bucket-cors --bucket "$BUCKET" --cors-configuration "{\"CORSRules\":[{
  \"AllowedOrigins\":[\"$ORIGIN\"],\"AllowedMethods\":[\"GET\",\"PUT\",\"HEAD\"],
  \"AllowedHeaders\":[\"*\"],\"ExposeHeaders\":[\"ETag\"],\"MaxAgeSeconds\":3600}]}"
echo "public access blocked, encrypted, versioned, CORS for $ORIGIN"

# ── DynamoDB table ───────────────────────────────────────────────────
say "DynamoDB table"
if aws dynamodb describe-table --table-name "$TABLE" --region "$REGION" >/dev/null 2>&1; then
  echo "exists"
else
  # setup_dev (in the app image) owns the table + index definitions; run it
  # with these admin credentials rather than the app's.
  eval "$(aws configure export-credentials --format env)"
  GIT_HASH=$(git rev-parse --short HEAD 2>/dev/null || echo unknown) docker compose build api
  docker compose run --rm --no-deps \
    -e AWS_ACCESS_KEY_ID -e AWS_SECRET_ACCESS_KEY -e AWS_SESSION_TOKEN \
    -e AWS_REGION="$REGION" api setup_dev
  aws dynamodb wait table-exists --table-name "$TABLE" --region "$REGION"
fi
# A new table rejects these for a minute or two while AWS finishes setting
# up its backups, so retry while it's still settling.
settle() {
  local err i
  for i in $(seq 1 30); do
    err=$("$@" 2>&1 >/dev/null) && return 0
    case $err in
      *ContinuousBackupsUnavailableException*|*ResourceInUseException*)
        echo "table still settling, retrying in 10s ($i/30)"; sleep 10 ;;
      *) echo "$err" >&2; return 1 ;;
    esac
  done
  echo "$err" >&2; return 1
}
# Point-in-time recovery is the database backup (35 days of restore points).
settle aws dynamodb update-continuous-backups --table-name "$TABLE" --region "$REGION" \
  --point-in-time-recovery-specification PointInTimeRecoveryEnabled=true
if [[ $(aws dynamodb describe-table --table-name "$TABLE" --region "$REGION" \
          --query Table.DeletionProtectionEnabled --output text) != True ]]; then
  settle aws dynamodb update-table --table-name "$TABLE" --region "$REGION" \
    --deletion-protection-enabled
fi
echo "point-in-time recovery on, deletion protection on"

# ── Secrets in SSM ───────────────────────────────────────────────────
say "Secrets under $SECRETS_PATH"
have() { aws ssm get-parameter --name "${SECRETS_PATH}$1" --region "$REGION" >/dev/null 2>&1; }
put() { aws ssm put-parameter --name "${SECRETS_PATH}$1" --type SecureString \
          --value "$2" --region "$REGION" --overwrite >/dev/null; echo "stored $1"; }
have JWT_SECRET || put JWT_SECRET "$(openssl rand -base64 48)"
have MFA_ENCRYPTION_KEY || put MFA_ENCRYPTION_KEY "$(openssl rand 32 | base64 | tr '+/' '-_' | tr -d '=\n')"
if ! have OAUTH_CLIENT_SECRET; then
  read -rsp "GitHub OAuth client secret (Enter to skip for now): " secret; echo
  [[ -n $secret ]] && put OAUTH_CLIENT_SECRET "$secret"
fi
echo "Optional, add the same way when you use them:"
echo "  aws ssm put-parameter --type SecureString --name ${SECRETS_PATH}<NAME> --value ..."
echo "  (SMTP_PASSWORD, GOOGLE_CLIENT_SECRET, ANTHROPIC_API_KEY)"

# ── IAM policies ─────────────────────────────────────────────────────
say "IAM policies"
mkdir -p iam/rendered
sed -e "s|__REGION__|$REGION|g" -e "s|__ACCOUNT_ID__|$ACCOUNT_ID|g" \
    -e "s|__TABLE__|$TABLE|g" -e "s|__TABLE_PREFIX__|$DYNAMODB_TABLE_PREFIX|g" \
    -e "s|__BUCKET__|$BUCKET|g" -e "s|__SECRETS_PATH__|$SECRETS_PATH|g" \
    iam/app-policy.json > iam/rendered/app-policy.json
ZONE_ID=$(aws route53 list-hosted-zones-by-name --dns-name "${OGRENOTES_DOMAIN#*.}." \
  --query 'HostedZones[0].Id' --output text 2>/dev/null | sed 's|/hostedzone/||') || true
sed -e "s|__HOSTED_ZONE_ID__|${ZONE_ID:-REPLACE_WITH_ZONE_ID}|g" \
    iam/dns-policy.json > iam/rendered/dns-policy.json
cat <<EOF
Wrote iam/rendered/app-policy.json and iam/rendered/dns-policy.json (zone ${ZONE_ID:-not found}).
If you haven't created the two IAM users yet:

  aws iam create-user --user-name ogrenotes-app
  aws iam put-user-policy --user-name ogrenotes-app --policy-name ogrenotes \\
      --policy-document file://iam/rendered/app-policy.json
  aws iam create-access-key --user-name ogrenotes-app     # -> ogrenotes.env

  aws iam create-user --user-name ogrenotes-dns
  aws iam put-user-policy --user-name ogrenotes-dns --policy-name route53 \\
      --policy-document file://iam/rendered/dns-policy.json
  aws iam create-access-key --user-name ogrenotes-dns     # -> caddy.env

Then: GIT_HASH=\$(git rev-parse --short HEAD) docker compose up -d --build
EOF
