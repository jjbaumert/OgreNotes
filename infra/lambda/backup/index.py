"""DynamoDB to S3 backup export, fired daily by EventBridge.

Reads OGRENOTES_TABLE_NAME + OGRENOTES_BUCKET from Lambda env and calls
dynamodb:ExportTableToPointInTime with a date-prefixed S3 key. PITR must
already be enabled on the source table (it is — the Data construct sets
pointInTimeRecovery: true).

The export is async: the Lambda returns immediately with the export ARN.
Data lands under s3://<bucket>/backups/<date>/ within ~5-30 minutes.
Restore steps: runbook/restore-from-backup.md.
"""
import os
from datetime import datetime, timezone

import boto3


def lambda_handler(event, context):
    table = os.environ["OGRENOTES_TABLE_NAME"]
    bucket = os.environ["OGRENOTES_BUCKET"]
    date_prefix = datetime.now(timezone.utc).strftime("%Y-%m-%d")

    ddb = boto3.client("dynamodb")
    desc = ddb.describe_table(TableName=table)
    table_arn = desc["Table"]["TableArn"]

    resp = ddb.export_table_to_point_in_time(
        TableArn=table_arn,
        S3Bucket=bucket,
        S3Prefix=f"backups/{date_prefix}/",
        ExportFormat="DYNAMODB_JSON",
    )
    export_arn = resp["ExportDescription"]["ExportArn"]
    print(f"Started DDB export: {export_arn} -> s3://{bucket}/backups/{date_prefix}/")
    return {"export_arn": export_arn, "date": date_prefix}
