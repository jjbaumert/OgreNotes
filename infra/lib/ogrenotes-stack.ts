import { Construct } from 'constructs';
import { Stack, StackProps, CfnOutput } from 'aws-cdk-lib';
import { NetworkConstruct } from './network';
import { DataConstruct } from './data';
import { ComputeConstruct } from './compute';
import { OpsConstruct } from './ops';
import { EnvConfig } from '../config';

export interface OgreNotesStackProps extends StackProps {
  config: EnvConfig;
  gitHash: string;
}

/**
 * The whole stack, composed from four constructs that mirror the phases of
 * scripts/aws-test-deploy.sh:
 *   NetworkConstruct  → Phase 1   (VPC, SGs, endpoints)
 *   DataConstruct     → Phase 1b/2/2b (DynamoDB, S3, Redis, EFS)
 *   ComputeConstruct  → Phase 3–6 (ECS cluster, api/qdrant/worker, autoscaling)
 *   OpsConstruct      → Phase 7/9 (budget; prod-only backup export)
 *
 * Single stack (not multi-stack) — the app deploys as a unit, so constructs
 * avoid cross-stack export/import friction.
 */
export class OgreNotesStack extends Stack {
  constructor(scope: Construct, id: string, props: OgreNotesStackProps) {
    super(scope, id, props);
    const { config, gitHash } = props;

    const network = new NetworkConstruct(this, 'Network');

    const data = new DataConstruct(this, 'Data', {
      config,
      vpc: network.vpc,
      redisSg: network.redisSg,
      efsSg: network.efsSg,
    });

    const compute = new ComputeConstruct(this, 'Compute', {
      config,
      gitHash,
      vpc: network.vpc,
      ecsSg: network.ecsSg,
      qdrantSg: network.qdrantSg,
      table: data.table,
      bucket: data.bucket,
      redisUrl: data.redisUrl,
      fileSystem: data.fileSystem,
      accessPoint: data.accessPoint,
    });

    new OpsConstruct(this, 'Ops', {
      config,
      table: data.table,
      bucket: data.bucket,
    });

    new CfnOutput(this, 'Url', { value: compute.url, description: 'App URL' });
    new CfnOutput(this, 'AlbDnsName', {
      value: compute.api.loadBalancer.loadBalancerDnsName,
      description: 'ALB DNS — set as FRONTEND_ORIGIN in config for no-domain stacks, then redeploy',
    });
    new CfnOutput(this, 'TableName', { value: data.table.tableName });
    new CfnOutput(this, 'BucketName', { value: data.bucket.bucketName });
  }
}
