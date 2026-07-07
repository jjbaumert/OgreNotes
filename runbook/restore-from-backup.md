# Restore from S3 backup

Phase 4 M-E7 item 8 ships a daily Lambda that calls
`DynamoDB ExportTableToPointInTime` for the OgreNote table and lands
the result at `s3://<bucket>/backups/<YYYY-MM-DD>/`. PITR on the
live table is enabled at table creation, so each export is a literal
point-in-time snapshot.

This runbook walks through restoring from one of those exports into
a fresh table — the standard recovery path for an accidental data
wipe, a bad migration, or a regional incident.

## When to use this runbook

| Scenario | Tool |
|---|---|
| Incident within the last 35 days, same region | `aws dynamodb restore-table-to-point-in-time` (faster; doesn't need this runbook) |
| Incident older than 35 days, OR cross-region restore needed | this runbook |
| Want to inspect a backup without touching live | this runbook (steps 1-3 only, skip cutover) |

PITR is the first-resort tool. S3 exports are the fallback for
everything outside its 35-day single-region window.

## Prerequisites

- An AWS profile with `dynamodb:ImportTable`, `s3:GetObject` on the
  backups prefix, and the IAM rights to create a new DynamoDB table
  in the target region.
- The date of the backup you want (`YYYY-MM-DD`).
- A target table name. Importing always creates a NEW table —
  DynamoDB has no in-place restore-from-S3 path.

## 1. Identify the export

The Lambda logs its export ARN to CloudWatch Logs at
`/aws/lambda/<prefix>ogrenote-backup`. Find the run for the target
date:

    aws logs filter-log-events \
        --log-group-name "/aws/lambda/<prefix>ogrenote-backup" \
        --filter-pattern "Started DDB export" \
        --start-time $(date -d "<YYYY-MM-DD>" +%s)000

Grab the ExportArn from the log line.

Alternative — list the S3 prefix directly:

    aws s3 ls s3://<bucket>/backups/<YYYY-MM-DD>/

You'll see an `AWSDynamoDB/<export-id>/` directory containing
the manifest + data files. The export-id maps to the ExportArn.

## 2. Capture the source schema

The import call needs the full table schema (attribute definitions,
key schema, every GSI). The simplest way is to describe the live
table:

    aws dynamodb describe-table --table-name <live-table> \
        --query 'Table.{AttributeDefinitions:AttributeDefinitions,KeySchema:KeySchema,GlobalSecondaryIndexes:GlobalSecondaryIndexes}' \
        > /tmp/restore-schema.json

The GSIs in the live table at restore time may have drifted from
when the backup was taken — for example, GSI7-deleted-at landed in
Phase 4 M-E7 and isn't in older snapshots. Strip GSIs that didn't
exist in the snapshot, or accept that the imported table will have
empty indexes that backfill on subsequent writes (DynamoDB rebuilds
the index from existing rows on import).

## 3. Trigger the import

    aws dynamodb import-table \
        --s3-bucket-source S3Bucket=<bucket>,S3KeyPrefix=backups/<date>/ \
        --input-format DYNAMODB_JSON \
        --table-creation-parameters '{
            "TableName": "ogrenote-restore-<date>",
            "AttributeDefinitions": <from-step-2>,
            "KeySchema": <from-step-2>,
            "GlobalSecondaryIndexes": <from-step-2>,
            "BillingMode": "PAY_PER_REQUEST"
        }'

The CLI returns an ImportArn. Import is async — poll until done:

    aws dynamodb describe-import --import-arn <import-arn> \
        --query 'ImportTableDescription.{Status:ImportStatus,Processed:ProcessedItemCount,Imported:ImportedItemCount}'

Status progresses IN_PROGRESS → COMPLETED, typically in a few
minutes for a normal table. If status goes FAILED, the
ImportTableDescription will carry a `FailureCode` + `FailureMessage`
that points at the cause (most commonly a schema mismatch — see
step 2).

## 4. Validate the restore

Once COMPLETED, count rows:

    aws dynamodb scan --table-name ogrenote-restore-<date> --select COUNT

Compare against the export's manifest:

    aws s3 cp s3://<bucket>/backups/<date>/AWSDynamoDB/<export-id>/manifest-summary.json - \
        | jq .itemCount

The two numbers should match exactly.

Spot-check a few known rows:

    aws dynamodb get-item --table-name ogrenote-restore-<date> \
        --key '{"PK":{"S":"DOC#<known-doc-id>"},"SK":{"S":"META"}}'

## 5. Cut over (only if actually recovering)

Three modes depending on the incident:

### Full disaster — live table corrupted or deleted

Repoint the ECS task definition at the new table name. Easiest:
rename the restored table by re-running the deploy script with the
new `DYNAMODB_TABLE_PREFIX`. Restart the service:

    aws ecs update-service --cluster <cluster> --service <service> \
        --force-new-deployment

### Partial restore — some rows lost, rest of table is healthy

Don't repoint. Instead, identify the affected rows and copy them
from the restored table back to live. A one-off Python script with
boto3 is the right shape — scan the restored table with a filter,
PutItem each result into live. Open a postmortem about why the
loss happened.

### Just exploring — no cutover

Don't change anything in prod. Delete the restored table when
done:

    aws dynamodb delete-table --table-name ogrenote-restore-<date>

## 6. After cutover

- Confirm the live ECS service is healthy:
  `aws ecs describe-services --cluster <cluster> --services <service>`
- Spot-check via the production URL: can users log in? Can they see
  their docs? Are recent edits intact (or expected to be missing)?
- Open a postmortem doc capturing:
  - What was lost (delta between live-at-incident and restore-point)
  - What was recovered
  - The gap between when the incident started and when the most
    recent backup was taken (RPO measurement)
  - Whether the runbook's manual steps need automation

## Failure modes worth knowing about

- **Backup didn't run that day**: check CloudWatch alarm
  `<prefix>ogrenote-backup-failure`. The alarm uses
  `treat-missing-data=breaching`, so a Lambda that never ran
  alarms in addition to a Lambda that ran-and-errored. Re-enable
  the EventBridge rule `<prefix>ogrenote-backup-daily` if it was
  disabled.
- **Lambda role lacks permissions**: the IAM policy on
  `<prefix>ogrenote-backup-role` grants
  `dynamodb:ExportTableToPointInTime` on the table ARN and
  `s3:PutObject` on `<bucket>/backups/*`. If either is missing,
  the Lambda logs will say so explicitly.
- **PITR isn't enabled on the source table**: Phase 9a of the
  deploy script confirms PITR at deploy time. If PITR somehow gets
  disabled, the Lambda fails with `PointInTimeRecoveryUnavailable`.
  Fix: `aws dynamodb update-continuous-backups --table-name
  <name> --point-in-time-recovery-specification
  PointInTimeRecoveryEnabled=true`.
- **S3 export landed but you can't list it**: the Lambda role has
  `s3:PutObject` only — listing the prefix requires `s3:ListBucket`
  on the bucket, which your operator profile should have. The
  restore-test profile from this runbook needs `s3:GetObject` +
  `s3:ListBucket` on the bucket.

## v2 carry-forwards

- **Automated restore tooling**. This runbook is fully manual. A
  future `crates/api/src/bin/restore_from_backup.rs` could automate
  steps 1-3 with --dry-run + --target-table flags. Deferred until
  restore is exercised more than rarely.
- **Cross-region replica**. PITR is single-region. A future M-Ex
  could add a cross-region backup bucket (CRR) and a parallel
  Lambda in the DR region.
- **Restore verification suite**. We don't currently have an
  automated "monthly disaster drill" that restores a backup into
  a throwaway table and verifies row count + sampled content. A
  future ops milestone could add this — it's the only way to know
  whether the export pipeline is still working before the moment
  you need it.
