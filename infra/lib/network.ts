import { Construct } from 'constructs';
import * as ec2 from 'aws-cdk-lib/aws-ec2';

/**
 * Phase 1 of aws-test-deploy.sh — VPC, subnets, IGW, route table, gateway
 * VPC endpoints, and the security-group mesh.
 *
 * 🔥 No NAT gateway (cost): the stack is public-subnet-only, every task gets
 *    a public IP, and isolation is enforced by security groups — exactly the
 *    bash design. `natGateways: 0` is load-bearing.
 *
 * The ALB security group is NOT created here — the ApplicationLoadBalanced-
 * FargateService pattern manages it (opens 80/443, wires ALB→task ingress on
 * the ECS SG automatically via the connections framework).
 */
export class NetworkConstruct extends Construct {
  readonly vpc: ec2.Vpc;
  /** Shared by api + worker Fargate tasks. */
  readonly ecsSg: ec2.SecurityGroup;
  readonly redisSg: ec2.SecurityGroup;
  readonly qdrantSg: ec2.SecurityGroup;
  readonly efsSg: ec2.SecurityGroup;

  constructor(scope: Construct, id: string) {
    super(scope, id);

    // VPC: 10.0.0.0/16, public subnets only, no NAT.
    this.vpc = new ec2.Vpc(this, 'Vpc', {
      ipAddresses: ec2.IpAddresses.cidr('10.0.0.0/16'),
      maxAzs: 2,
      natGateways: 0,
      subnetConfiguration: [
        { name: 'public', subnetType: ec2.SubnetType.PUBLIC, cidrMask: 24 },
      ],
    });

    // Gateway VPC endpoints — free, keep DynamoDB/S3 traffic off the IGW.
    this.vpc.addGatewayEndpoint('DynamoDbEndpoint', {
      service: ec2.GatewayVpcEndpointAwsService.DYNAMODB,
    });
    this.vpc.addGatewayEndpoint('S3Endpoint', {
      service: ec2.GatewayVpcEndpointAwsService.S3,
    });

    // ── Security groups ──
    this.ecsSg = new ec2.SecurityGroup(this, 'EcsSg', {
      vpc: this.vpc,
      description: 'OgreNotes api/worker Fargate tasks',
      allowAllOutbound: true,
    });
    this.redisSg = new ec2.SecurityGroup(this, 'RedisSg', {
      vpc: this.vpc,
      description: 'ElastiCache Redis',
      allowAllOutbound: true,
    });
    this.qdrantSg = new ec2.SecurityGroup(this, 'QdrantSg', {
      vpc: this.vpc,
      description: 'Qdrant Fargate task',
      allowAllOutbound: true,
    });
    this.efsSg = new ec2.SecurityGroup(this, 'EfsSg', {
      vpc: this.vpc,
      description: 'EFS mount targets for Qdrant data',
      allowAllOutbound: true,
    });

    // Ingress mesh (mirrors the bash authorize-security-group-ingress calls):
    //   redis  ← ecs          :6379
    //   qdrant ← ecs          :6333 (REST) + :6334 (gRPC)
    //   efs    ← qdrant        :2049 (NFS)
    // (ALB → ecs :3000 is added by the ALB pattern.)
    // NB: SG rule descriptions allow only a-zA-Z0-9. _-:/()#,@[]+=&;{}!$* —
    // no Unicode arrows (EC2 rejects them at create time, not at synth).
    this.redisSg.addIngressRule(this.ecsSg, ec2.Port.tcp(6379), 'app to redis');
    this.qdrantSg.addIngressRule(this.ecsSg, ec2.Port.tcp(6333), 'app to qdrant REST');
    this.qdrantSg.addIngressRule(this.ecsSg, ec2.Port.tcp(6334), 'app to qdrant gRPC');
    this.efsSg.addIngressRule(this.qdrantSg, ec2.Port.tcp(2049), 'qdrant to efs (NFS)');
  }
}
