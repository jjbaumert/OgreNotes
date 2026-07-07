import * as path from 'path';
import { Construct } from 'constructs';
import { Duration } from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as events from 'aws-cdk-lib/aws-events';
import * as targets from 'aws-cdk-lib/aws-events-targets';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as sns from 'aws-cdk-lib/aws-sns';
import * as subs from 'aws-cdk-lib/aws-sns-subscriptions';
import * as cloudwatch from 'aws-cdk-lib/aws-cloudwatch';
import * as cwactions from 'aws-cdk-lib/aws-cloudwatch-actions';
import * as budgets from 'aws-cdk-lib/aws-budgets';
import { EnvConfig } from '../config';
import { slaDashboardBody } from './dashboard';

export interface OpsProps {
  config: EnvConfig;
  table: dynamodb.Table;
  bucket: s3.Bucket;
}

/**
 * Phase 7 (Budget — always) + Phase 9 (DDB→S3 backup export — prod only) of
 * aws-test-deploy.sh.
 */
export class OpsConstruct extends Construct {
  constructor(scope: Construct, id: string, props: OpsProps) {
    super(scope, id);
    const { config, table, bucket } = props;
    const prefix = config.prefix;

    // ── Phase 7: monthly cost budget, alert at 80% of $50 ──
    new budgets.CfnBudget(this, 'Budget', {
      budget: {
        budgetName: `${prefix}ogrenote-budget`,
        budgetType: 'COST',
        timeUnit: 'MONTHLY',
        budgetLimit: { amount: 50, unit: 'USD' },
      },
      notificationsWithSubscribers: [
        {
          notification: {
            notificationType: 'ACTUAL',
            comparisonOperator: 'GREATER_THAN',
            threshold: 80,
            thresholdType: 'PERCENTAGE',
          },
          subscribers: [{ subscriptionType: 'EMAIL', address: config.notificationEmail }],
        },
      ],
    });

    // ── Phase 5 M-P9: SLA dashboard ──
    // Definition lives in ./dashboard.ts (CDK-authoritative). SEARCH
    // expressions match services by regex, so they're prefix-agnostic; the
    // per-widget region is injected from config.
    new cloudwatch.CfnDashboard(this, 'SlaDashboard', {
      dashboardName: `${prefix}ogrenote-sla`,
      dashboardBody: slaDashboardBody(config.region),
    });

    // ── Phase 9: backup export — prod only ──
    if (config.deployEnv !== 'prod') {
      return;
    }

    const backupFn = new lambda.Function(this, 'BackupFn', {
      functionName: `${prefix}ogrenote-backup`,
      runtime: lambda.Runtime.PYTHON_3_12,
      handler: 'index.lambda_handler',
      code: lambda.Code.fromAsset(path.join(__dirname, '..', 'lambda', 'backup')),
      timeout: Duration.seconds(60),
      memorySize: 128,
      environment: {
        OGRENOTES_TABLE_NAME: table.tableName,
        OGRENOTES_BUCKET: bucket.bucketName,
      },
    });
    // ExportTableToPointInTime + the S3 write are performed by the caller.
    // Action set matches the bash backup-policy (incl. DescribeContinuousBackups,
    // which the export path can consult).
    backupFn.addToRolePolicy(
      new iam.PolicyStatement({
        actions: [
          'dynamodb:ExportTableToPointInTime',
          'dynamodb:DescribeTable',
          'dynamodb:DescribeContinuousBackups',
        ],
        resources: [table.tableArn, `${table.tableArn}/*`],
      }),
    );
    bucket.grantWrite(backupFn);
    // Bucket-level reads the export needs (grantWrite omits ListBucket).
    backupFn.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ['s3:GetBucketLocation', 's3:ListBucket'],
        resources: [bucket.bucketArn],
      }),
    );

    // Daily 04:00 UTC.
    new events.Rule(this, 'BackupSchedule', {
      ruleName: `${prefix}ogrenote-backup-schedule`,
      description: 'Daily DynamoDB backup export to S3',
      schedule: events.Schedule.cron({ minute: '0', hour: '4' }),
      targets: [new targets.LambdaFunction(backupFn)],
    });

    // SNS + email for alarm notifications.
    const topic = new sns.Topic(this, 'BackupTopic', {
      topicName: `${prefix}ogrenote-backup-alarm`,
    });
    topic.addSubscription(new subs.EmailSubscription(config.notificationEmail));

    // Alarm: Lambda errored OR didn't run in 24h.
    // treatMissingData=BREACHING catches an accidentally-disabled schedule.
    const alarm = backupFn
      .metricErrors({ period: Duration.days(1), statistic: 'Sum' })
      .createAlarm(this, 'BackupAlarm', {
        alarmName: `${prefix}ogrenote-backup-alarm`,
        alarmDescription: 'DDB backup Lambda errored or did not run today',
        threshold: 1,
        evaluationPeriods: 1,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        treatMissingData: cloudwatch.TreatMissingData.BREACHING,
      });
    alarm.addAlarmAction(new cwactions.SnsAction(topic));
  }
}
