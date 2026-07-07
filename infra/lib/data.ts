import { Construct } from 'constructs';
import { Duration, RemovalPolicy } from 'aws-cdk-lib';
import * as ec2 from 'aws-cdk-lib/aws-ec2';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as efs from 'aws-cdk-lib/aws-efs';
import * as elasticache from 'aws-cdk-lib/aws-elasticache';
import { EnvConfig } from '../config';

export interface DataProps {
  config: EnvConfig;
  vpc: ec2.Vpc;
  redisSg: ec2.SecurityGroup;
  efsSg: ec2.SecurityGroup;
}

/**
 * Phase 1b + 2 + 2b of aws-test-deploy.sh — DynamoDB single-table (8 GSIs,
 * PITR), the S3 bucket, ElastiCache Redis, and EFS for Qdrant.
 */
export class DataConstruct extends Construct {
  readonly table: dynamodb.Table;
  readonly bucket: s3.Bucket;
  readonly fileSystem: efs.FileSystem;
  readonly accessPoint: efs.AccessPoint;
  /** redis://<endpoint>:6379 — token resolved at deploy. */
  readonly redisUrl: string;

  constructor(scope: Construct, id: string, props: DataProps) {
    super(scope, id);
    const { config, vpc, redisSg, efsSg } = props;
    const prefix = config.prefix;

    // Stateful resources RETAIN in prod. In test we used DESTROY for clean
    // teardown — but a mis-prefixed deploy (wrong/omitted `-c prefix`) renames
    // every resource, which CloudFormation does as a *replacement*, and DESTROY
    // would then delete the live table/bucket. So the irreplaceable data stores
    // (DynamoDB + S3) RETAIN even in test: a data-loss backstop. Teardown then
    // leaves them orphaned for manual cleanup. The regenerable Qdrant EFS index
    // keeps the test-teardown convenience (DESTROY in test).
    const isProd = config.deployEnv === 'prod';
    const removalPolicy = isProd ? RemovalPolicy.RETAIN : RemovalPolicy.DESTROY;
    const dataRemovalPolicy = RemovalPolicy.RETAIN;

    // ── DynamoDB single-table ──
    // Keys are PK/SK (uppercase). All 8 GSIs declared up front — CloudFormation
    // adds only one GSI per *update*, but a fresh create makes all 8 at once,
    // which is why the bash per-GSI idempotent dance is unnecessary here.
    this.table = new dynamodb.Table(this, 'Table', {
      tableName: `${prefix}ogrenote`,
      partitionKey: { name: 'PK', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'SK', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      // 🔥 required by the backup-export Lambda
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
      removalPolicy: dataRemovalPolicy,
    });

    const S = dynamodb.AttributeType.STRING;
    const N = dynamodb.AttributeType.NUMBER;
    const ALL = dynamodb.ProjectionType.ALL;

    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI1-owner-updated',
      partitionKey: { name: 'owner_id_gsi', type: S },
      sortKey: { name: 'updated_at', type: N },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI2-parent-title',
      partitionKey: { name: 'parent_id_gsi', type: S },
      sortKey: { name: 'title', type: S },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI3-workspace-updated',
      partitionKey: { name: 'workspace_id_gsi', type: S },
      sortKey: { name: 'updated_at', type: N },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI4-user-created',
      partitionKey: { name: 'user_id_gsi', type: S },
      sortKey: { name: 'created_at', type: N },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI5-docid-updated',
      partitionKey: { name: 'doc_id_gsi', type: S },
      sortKey: { name: 'updated_at', type: N },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI6-external-id', // SCIM external_id lookup
      partitionKey: { name: 'external_id_gsi', type: S },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI7-deleted-at', // trash-cleanup worker
      partitionKey: { name: 'is_deleted_gsi', type: S },
      sortKey: { name: 'deleted_at', type: N },
      projectionType: ALL,
    });
    this.table.addGlobalSecondaryIndex({
      indexName: 'GSI8-actor-created', // audit forensics
      partitionKey: { name: 'actor_id_gsi', type: S },
      sortKey: { name: 'created_at', type: N },
      projectionType: ALL,
    });

    // ── S3 bucket (one bucket; backups land under backups/<date>/) ──
    // Lifecycle rule mirrors the tracked infra/s3/bucket-policy.json: only
    // tmp/ objects expire (snapshots + blobs are untouched).
    this.bucket = new s3.Bucket(this, 'Bucket', {
      bucketName: `${prefix}ogrenote`,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      lifecycleRules: [
        {
          id: 'expire-tmp-objects-7-days',
          enabled: true,
          prefix: 'tmp/',
          expiration: Duration.days(7),
        },
      ],
      removalPolicy: dataRemovalPolicy,
      // RETAIN ⇒ never auto-empty (autoDeleteObjects requires DESTROY).
      autoDeleteObjects: false,
    });

    // ── ElastiCache Redis (no L2 — use L1 Cfn*) ──
    const redisSubnetGroup = new elasticache.CfnSubnetGroup(this, 'RedisSubnets', {
      description: `${prefix}ogrenote redis`,
      subnetIds: vpc.publicSubnets.map((s) => s.subnetId),
    });
    const redis = new elasticache.CfnCacheCluster(this, 'Redis', {
      engine: 'redis',
      cacheNodeType: 'cache.t4g.micro',
      numCacheNodes: 1,
      cacheSubnetGroupName: redisSubnetGroup.ref,
      vpcSecurityGroupIds: [redisSg.securityGroupId],
    });
    redis.addDependency(redisSubnetGroup);
    this.redisUrl = `redis://${redis.attrRedisEndpointAddress}:${redis.attrRedisEndpointPort}`;

    // ── EFS for Qdrant data ──
    this.fileSystem = new efs.FileSystem(this, 'QdrantData', {
      vpc,
      vpcSubnets: { subnetType: ec2.SubnetType.PUBLIC },
      securityGroup: efsSg,
      encrypted: true,
      performanceMode: efs.PerformanceMode.GENERAL_PURPOSE,
      throughputMode: efs.ThroughputMode.BURSTING,
      removalPolicy,
    });
    // Access point pins Qdrant's POSIX identity (UID/GID 1000) + root dir,
    // so it writes to /qdrant without root-on-EFS.
    this.accessPoint = this.fileSystem.addAccessPoint('QdrantAp', {
      path: '/qdrant',
      createAcl: { ownerUid: '1000', ownerGid: '1000', permissions: '0755' },
      posixUser: { uid: '1000', gid: '1000' },
    });
  }
}
