import * as path from 'path';
import { Construct } from 'constructs';
import { Duration, RemovalPolicy, Stack } from 'aws-cdk-lib';
import * as ec2 from 'aws-cdk-lib/aws-ec2';
import * as ecs from 'aws-cdk-lib/aws-ecs';
import * as ecsPatterns from 'aws-cdk-lib/aws-ecs-patterns';
import * as elbv2 from 'aws-cdk-lib/aws-elasticloadbalancingv2';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as ssm from 'aws-cdk-lib/aws-ssm';
import * as efs from 'aws-cdk-lib/aws-efs';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as route53 from 'aws-cdk-lib/aws-route53';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as scheduler from 'aws-cdk-lib/aws-scheduler';
import { DockerImageAsset, Platform } from 'aws-cdk-lib/aws-ecr-assets';
import { EnvConfig } from '../config';

// Inline source for the off-hours toggle Lambda (see addOffHoursScheduler).
// api + qdrant have no autoscaling, so a plain desiredCount is enough. The
// worker has CPU target-tracking, so its Application Auto Scaling MinCapacity
// must be lowered to 0 first — otherwise the policy scales it straight back up.
const OFF_HOURS_TOGGLE_SRC = `
import os
import boto3

ecs = boto3.client('ecs')
aas = boto3.client('application-autoscaling')

CLUSTER = os.environ['CLUSTER']
API = os.environ['API_SERVICE']
WORKER = os.environ['WORKER_SERVICE']
QDRANT = os.environ['QDRANT_SERVICE']
WORKER_MIN = int(os.environ['WORKER_MIN'])
WORKER_MAX = int(os.environ['WORKER_MAX'])


def handler(event, context):
    up = event.get('action') == 'up'

    for svc in (API, QDRANT):
        ecs.update_service(cluster=CLUSTER, service=svc, desiredCount=1 if up else 0)

    aas.register_scalable_target(
        ServiceNamespace='ecs',
        ResourceId='service/%s/%s' % (CLUSTER, WORKER),
        ScalableDimension='ecs:service:DesiredCount',
        MinCapacity=WORKER_MIN if up else 0,
        MaxCapacity=WORKER_MAX,
    )
    ecs.update_service(cluster=CLUSTER, service=WORKER,
                       desiredCount=WORKER_MIN if up else 0)

    return {'action': 'up' if up else 'down', 'ok': True}
`;

export interface ComputeProps {
  config: EnvConfig;
  gitHash: string;
  vpc: ec2.Vpc;
  ecsSg: ec2.SecurityGroup;
  qdrantSg: ec2.SecurityGroup;
  table: dynamodb.Table;
  bucket: s3.Bucket;
  redisUrl: string;
  fileSystem: efs.FileSystem;
  accessPoint: efs.AccessPoint;
}

/**
 * Phase 3–6 of aws-test-deploy.sh — ECS cluster, Cloud Map, the three Fargate
 * services (api behind ALB, qdrant on Spot+EFS, worker + autoscaling), and the
 * shared task role. The build/push of Phase 3 is folded into the image asset.
 *
 * Only `api` fits ApplicationLoadBalancedFargateService; qdrant and worker use
 * raw FargateService (Spot/EFS/Cloud Map and no-ALB respectively).
 */
export class ComputeConstruct extends Construct {
  readonly api: ecsPatterns.ApplicationLoadBalancedFargateService;
  /** Public URL of the app (https://domain or http://<alb-dns>). */
  readonly url: string;

  constructor(scope: Construct, id: string, props: ComputeProps) {
    super(scope, id);
    const { config, gitHash, vpc, ecsSg, qdrantSg, table, bucket, redisUrl, fileSystem, accessPoint } =
      props;
    const prefix = config.prefix;
    const region = Stack.of(this).region;

    const cluster = new ecs.Cluster(this, 'Cluster', {
      clusterName: `${prefix}ogrenote`,
      vpc,
      // config.containerInsights gates this. Container Insights emits ~88
      // billable custom metrics ($0.30/mo each ≈ $26/mo) — worth it for prod
      // observability, wasteful on the test stack. Basic AWS/ECS metrics
      // (free) still cover CPU/mem at the service level when it's off.
      containerInsightsV2: config.containerInsights
        ? ecs.ContainerInsights.ENABLED
        : ecs.ContainerInsights.DISABLED,
      // enableFargateCapacityProviders is intentionally OFF. The cluster
      // already has FARGATE + FARGATE_SPOT associated out-of-band (a
      // 2026-07-03 stack rollback dropped CFN's ownership record for the
      // AWS::ECS::ClusterCapacityProviderAssociations resource while
      // leaving the association on the cluster itself). Re-enabling
      // this flag causes `cdk deploy` to try to CREATE the CCPA resource
      // and fail with "already exists". `cdk import` refuses when the
      // pending diff also touches Outputs or adds other resources
      // (ours does — LogRetention Lambda, image swaps). The clean fix
      // is a native CFN change-set with --change-set-type IMPORT
      // scoped to just the CCPA resource; when that's been done and
      // the resource is back under stack ownership, restore this line.
      // Worker + Qdrant continue to reference FARGATE_SPOT correctly
      // because the association lives on the cluster, not the flag.
      // enableFargateCapacityProviders: true,
    });
    // Cloud Map private DNS namespace — qdrant registers here; api/worker
    // resolve it by name. QDRANT_URL is just the resulting DNS string.
    cluster.addDefaultCloudMapNamespace({ name: config.cloudMapNamespace });
    // Port 6334 is Qdrant's gRPC port — the qdrant-client (Qdrant::from_url)
    // speaks gRPC/HTTP2. Pointing at 6333 (REST/HTTP1.1) yields an "h2 protocol
    // error" and the api panics. Matches the bash QDRANT_URL.
    const qdrantUrl = `http://qdrant.${config.cloudMapNamespace}:6334`;

    // ── Shared task role (DynamoDB + S3 + Bedrock) ──
    // Mirrors the single TASK_ROLE the bash created for api + worker.
    // No explicit roleName — CDK auto-names it so a leftover role from a
    // partial bash teardown can't block a deploy on a name collision.
    const taskRole = new iam.Role(this, 'TaskRole', {
      assumedBy: new iam.ServicePrincipal('ecs-tasks.amazonaws.com'),
    });
    table.grantReadWriteData(taskRole);
    // grantReadWriteData omits TransactWriteItems; the app does transactional
    // writes (the bash task policy granted it explicitly). Add it back.
    taskRole.addToPrincipalPolicy(
      new iam.PolicyStatement({
        actions: ['dynamodb:TransactWriteItems'],
        resources: [table.tableArn, `${table.tableArn}/index/*`],
      }),
    );
    bucket.grantReadWrite(taskRole);
    // No L2 grant for Bedrock — scope to exactly the 4 embedding models.
    taskRole.addToPrincipalPolicy(
      new iam.PolicyStatement({
        actions: ['bedrock:InvokeModel', 'bedrock:InvokeModelWithResponseStream'],
        resources: [
          `arn:aws:bedrock:${region}::foundation-model/amazon.titan-embed-text-v2:0`,
          `arn:aws:bedrock:${region}::foundation-model/amazon.titan-embed-text-v1`,
          `arn:aws:bedrock:${region}::foundation-model/cohere.embed-english-v3`,
          `arn:aws:bedrock:${region}::foundation-model/cohere.embed-multilingual-v3`,
        ],
      }),
    );

    // ── Secrets from SSM SecureString (no plaintext env) ──
    const ssmSecret = (logicalId: string, name: string) =>
      ecs.Secret.fromSsmParameter(
        ssm.StringParameter.fromSecureStringParameterAttributes(this, logicalId, {
          parameterName: `/${prefix}ogrenote/${name}`,
        }),
      );

    const secrets: { [k: string]: ecs.Secret } = {
      OAUTH_CLIENT_SECRET: ssmSecret('OauthSecret', 'oauth-client-secret'),
      JWT_SECRET: ssmSecret('JwtSecret', 'jwt-secret'),
    };
    if (config.aiEnabled) {
      // The SSM param must exist before deploy or the task won't start.
      secrets.ANTHROPIC_API_KEY = ssmSecret('AnthropicKey', 'anthropic-api-key');
    }
    if (config.googleClientId) {
      // Requires SSM param /<prefix>ogrenote/google-client-secret to exist.
      secrets.GOOGLE_CLIENT_SECRET = ssmSecret('GoogleSecret', 'google-client-secret');
    }

    // URL / OAuth redirect. With a domain we know it up front; without one,
    // the ALB DNS is only known post-deploy — set frontendOrigin in config
    // after the first deploy (mirrors the bash FRONTEND_ORIGIN two-step).
    const baseUrl = config.domainName
      ? `https://${config.domainName}`
      : config.frontendOrigin ?? 'http://SET-FRONTEND_ORIGIN-AFTER-FIRST-DEPLOY';

    const commonEnv: { [k: string]: string } = {
      AWS_REGION: region,
      DYNAMODB_TABLE_PREFIX: prefix,
      S3_BUCKET: bucket.bucketName,
      REDIS_URL: redisUrl,
      QDRANT_URL: qdrantUrl,
      OAUTH_CLIENT_ID: config.oauthClientId,
      OAUTH_REDIRECT_URI: `${baseUrl}/api/v1/auth/callback`,
      FRONTEND_ORIGIN: baseUrl,
      DEV_MODE: String(config.devMode ?? false),
      SEARCH_INDEX_PATH: '/data/search-index',
      API_PORT: '3000',
      ADMIN_EMAILS: config.adminEmails ?? '',
      DEPLOY_ENV: config.deployEnv,
    };
    // Google OAuth (optional): client id is non-secret; redirect is derived by
    // the app as OAUTH_REDIRECT_URI/google. Secret comes from SSM (above).
    if (config.googleClientId) {
      commonEnv.GOOGLE_CLIENT_ID = config.googleClientId;
    }

    // CPU architecture (config.cpuArch). ARM64/Graviton is ~20% cheaper; the
    // image is built for the same arch (on an x86 host an arm64 build is
    // emulated — see config.ts). `runtimePlatform` is applied to every task
    // def + the ALB service so all tasks run on the chosen arch.
    const arm = config.cpuArch === 'ARM64';
    const dockerPlatform = arm ? Platform.LINUX_ARM64 : Platform.LINUX_AMD64;
    const runtimePlatform: ecs.RuntimePlatform = {
      cpuArchitecture: arm ? ecs.CpuArchitecture.ARM64 : ecs.CpuArchitecture.X86_64,
      operatingSystemFamily: ecs.OperatingSystemFamily.LINUX,
    };

    // Single Dockerfile builds backend + WASM frontend; GIT_HASH stamps the
    // in-app version row. Build context = repo root (one level above infra/).
    // Built once, shared by api + worker (deduped by asset hash).
    const appImage = new DockerImageAsset(this, 'AppImage', {
      directory: path.join(__dirname, '..', '..'),
      file: 'Dockerfile',
      platform: dockerPlatform,
      buildArgs: { GIT_HASH: gitHash },
    });
    const image = ecs.ContainerImage.fromDockerImageAsset(appImage);

    // Optional HTTPS: look up the (pre-existing) hosted zone and let the
    // pattern manage the ACM cert + A record + HTTP→HTTPS redirect.
    const domainZone = config.domainName
      ? route53.HostedZone.fromLookup(this, 'Zone', {
          // domainName may be a subdomain; look up its parent zone.
          domainName: config.hostedZoneName ?? config.domainName,
        })
      : undefined;

    // ── api: Fargate behind ALB ──
    this.api = new ecsPatterns.ApplicationLoadBalancedFargateService(this, 'Api', {
      cluster,
      serviceName: `${prefix}ogrenote-api`,
      cpu: config.apiCpu,
      memoryLimitMiB: config.apiMemoryMiB,
      runtimePlatform,
      desiredCount: 1,
      publicLoadBalancer: true,
      // No NAT → tasks live in public subnets with a public IP.
      assignPublicIp: true,
      taskSubnets: { subnetType: ec2.SubnetType.PUBLIC },
      securityGroups: [ecsSg],
      healthCheckGracePeriod: Duration.seconds(120),
      minHealthyPercent: 100,
      maxHealthyPercent: 200,
      // No deployment circuit breaker: on a cold start the app crash-loops
      // until Qdrant is reachable (the bash stack converged the same way). A
      // circuit breaker trips during that window and rolls the stack back.
      ...(domainZone
        ? {
            protocol: elbv2.ApplicationProtocol.HTTPS,
            domainName: config.domainName,
            domainZone,
            redirectHTTP: true,
          }
        : {}),
      taskImageOptions: {
        image,
        containerName: 'ogrenote-api',
        containerPort: 3000,
        taskRole,
        environment: commonEnv,
        secrets,
        logDriver: ecs.LogDrivers.awsLogs({
          streamPrefix: 'api',
          // Explicit, deterministic log-group name (#50). An auto-named
          // group carries a hashed suffix that changes on every task-def
          // replacement, leaving orphaned groups the scoped diagnostic
          // read-only IAM policy (and aws-test-logs.sh) can never match by
          // a stable ARN. A fixed name makes both target it reliably.
          // RETAIN (the LogGroup default) matches the table/bucket policy —
          // teardown leaves it for manual cleanup rather than losing logs.
          logGroup: new logs.LogGroup(this, 'ApiLogGroup', {
            logGroupName: `/ecs/${prefix}ogrenote-api`,
            retention: logs.RetentionDays.TWO_WEEKS,
            removalPolicy: RemovalPolicy.RETAIN,
          }),
        }),
      },
    });

    // 🔥 critical settings the convenience pattern does NOT expose:
    // 1. idle_timeout 120s — keeps idle WebSockets alive (default 60s reaps them).
    this.api.loadBalancer.setAttribute('idle_timeout.timeout_seconds', '120');
    // 2. sticky sessions (lb_cookie, 1h) — WS affinity + in-process Tantivy state.
    this.api.targetGroup.enableCookieStickiness(Duration.hours(1));
    // 3. health check /health + thresholds.
    this.api.targetGroup.configureHealthCheck({
      path: '/health',
      interval: Duration.seconds(30),
      healthyThresholdCount: 2,
      unhealthyThresholdCount: 3,
    });

    this.url = domainZone ? `https://${config.domainName}` : `http://${this.api.loadBalancer.loadBalancerDnsName}`;

    // ── qdrant: raw FargateService, EFS-backed, Cloud Map ──
    // The vector index persists on EFS across task replacement, mounted at
    // /qdrant/storage via the access point (POSIX UID/GID 1000). efsSg allows
    // 2049 from qdrantSg, and the service depends on the filesystem so tasks
    // never launch before the mount targets are available. There is NO
    // deployment circuit breaker (see api): if a fresh EFS's mount-target DNS
    // isn't resolvable yet, ECS retries until it is — rather than rolling back
    // and deleting the EFS, which is what previously prevented convergence.
    const qdrantTask = new ecs.FargateTaskDefinition(this, 'QdrantTask', {
      family: `${prefix}ogrenote-qdrant`,
      cpu: 512,
      memoryLimitMiB: 1024,
      // qdrant/qdrant is multi-arch (arm64 variant exists) and its on-disk
      // index format is portable across architectures, so the existing EFS
      // index is read fine after switching arch.
      runtimePlatform,
      volumes: [
        {
          name: 'qdrant-data',
          efsVolumeConfiguration: {
            fileSystemId: fileSystem.fileSystemId,
            transitEncryption: 'ENABLED',
            authorizationConfig: { accessPointId: accessPoint.accessPointId, iam: 'DISABLED' },
          },
        },
      ],
    });
    const qdrantContainer = qdrantTask.addContainer('qdrant', {
      image: ecs.ContainerImage.fromRegistry('qdrant/qdrant:v1.13.0'),
      essential: true,
      portMappings: [{ containerPort: 6333 }, { containerPort: 6334 }],
      environment: {
        QDRANT__SERVICE__HTTP_PORT: '6333',
        QDRANT__SERVICE__GRPC_PORT: '6334',
        QDRANT__STORAGE__STORAGE_PATH: '/qdrant/storage',
      },
      logging: ecs.LogDrivers.awsLogs({
        streamPrefix: 'qdrant',
        logRetention: logs.RetentionDays.TWO_WEEKS,
      }),
    });
    qdrantContainer.addMountPoints({
      sourceVolume: 'qdrant-data',
      containerPath: '/qdrant/storage',
      readOnly: false,
    });
    const qdrantService = new ecs.FargateService(this, 'Qdrant', {
      cluster,
      serviceName: `${prefix}ogrenote-qdrant`,
      taskDefinition: qdrantTask,
      desiredCount: 1,
      securityGroups: [qdrantSg],
      vpcSubnets: { subnetType: ec2.SubnetType.PUBLIC },
      assignPublicIp: true,
      // config.qdrantSpot gates Fargate Spot (~70% cheaper). Historically the
      // Spot capacity-provider association wedged stack rollback/delete
      // ("capacity provider in use"); the worker service now carries the same
      // association unconditionally, so this adds no new exposure. Prod keeps
      // this false (on-demand) for search availability — see config.ts.
      ...(config.qdrantSpot
        ? { capacityProviderStrategies: [{ capacityProvider: 'FARGATE_SPOT', weight: 1 }] }
        : {}),
      cloudMapOptions: { name: 'qdrant' },
      // Single writer: stop the old task before starting the new one so two
      // tasks never mount the same EFS index concurrently.
      minHealthyPercent: 0,
      maxHealthyPercent: 100,
    });
    // Don't launch tasks until the EFS mount targets exist (depending on the
    // FileSystem construct covers its mount-target children) — otherwise the
    // mount fails with ResourceInitializationError (EFS DNS won't resolve).
    qdrantService.node.addDependency(fileSystem);

    // ── worker: same image, --mode=worker, no ALB ──
    const workerTask = new ecs.FargateTaskDefinition(this, 'WorkerTask', {
      family: `${prefix}ogrenote-worker`,
      cpu: 256,
      memoryLimitMiB: 512,
      runtimePlatform,
      taskRole,
    });
    workerTask.addContainer('ogrenote-worker', {
      image,
      essential: true,
      command: ['ogrenotes-api', '--mode=worker'],
      environment: { ...commonEnv, WORKER_CONCURRENCY: '2' },
      secrets,
      logging: ecs.LogDrivers.awsLogs({
        streamPrefix: 'worker',
        // Deterministic name, same rationale as the api log group (#50).
        logGroup: new logs.LogGroup(this, 'WorkerLogGroup', {
          logGroupName: `/ecs/${prefix}ogrenote-worker`,
          retention: logs.RetentionDays.TWO_WEEKS,
          removalPolicy: RemovalPolicy.RETAIN,
        }),
      }),
    });
    const workerService = new ecs.FargateService(this, 'Worker', {
      cluster,
      serviceName: `${prefix}ogrenote-worker`,
      taskDefinition: workerTask,
      desiredCount: config.workerMin,
      securityGroups: [ecsSg],
      vpcSubnets: { subnetType: ec2.SubnetType.PUBLIC },
      assignPublicIp: true,
      minHealthyPercent: 100,
      maxHealthyPercent: 200,
      // ~70% cheaper: background jobs are interruptible, so the worker runs on
      // Spot. On a Spot reclaim ECS reschedules the task; autoscaling + the
      // 100% min-healthy keep capacity. (Unlike Qdrant — stateful, single EFS
      // writer — the worker is stateless, so the Spot capacity-provider wedge
      // that affected Qdrant doesn't apply.) No `launchType` when a capacity-
      // provider strategy is set. No circuit breaker — same cold-start
      // crash-loop convergence rationale as api.
      capacityProviderStrategies: [{ capacityProvider: 'FARGATE_SPOT', weight: 1 }],
    });
    // Worker autoscaling — CPU target-tracking @ 70% (v1 proxy for stream
    // backlog; jobs are CPU-bound DOCX/PDF conversion).
    workerService
      .autoScaleTaskCount({ minCapacity: config.workerMin, maxCapacity: config.workerMax })
      .scaleOnCpuUtilization('WorkerCpu', {
        targetUtilizationPercent: 70,
        scaleInCooldown: Duration.seconds(300),
        scaleOutCooldown: Duration.seconds(60),
      });

    // Off-hours scale-to-zero (test only; prod omits config.offHours → 24/7).
    this.addOffHoursScheduler(cluster, workerService, qdrantService, config);
  }

  /**
   * When `config.offHours` is set, create a toggle Lambda plus EventBridge
   * Scheduler up/down triggers that scale the api/worker/qdrant services to 0
   * outside the configured UP windows. Uses EventBridge Scheduler (not native
   * ECS scheduled scaling) because only it evaluates cron in a DST-aware IANA
   * timezone; the native path is UTC-only and would drift an hour twice a year.
   */
  private addOffHoursScheduler(
    cluster: ecs.ICluster,
    workerService: ecs.FargateService,
    qdrantService: ecs.FargateService,
    config: EnvConfig,
  ): void {
    const offHours = config.offHours;
    if (!offHours) return;

    const { region, account } = Stack.of(this);

    const toggle = new lambda.Function(this, 'OffHoursToggle', {
      functionName: `${config.prefix}ogrenote-offhours-toggle`,
      runtime: lambda.Runtime.PYTHON_3_12,
      handler: 'index.handler',
      timeout: Duration.seconds(60),
      code: lambda.Code.fromInline(OFF_HOURS_TOGGLE_SRC),
      // Same 14-day retention the ECS log groups get. Without
      // this the Lambda's log group inherits the account default
      // ("Never expire") and grows unbounded — a small but
      // permanent cost drip that the 2026-07-03 CloudWatch
      // audit surfaced.
      logRetention: logs.RetentionDays.TWO_WEEKS,
      environment: {
        CLUSTER: cluster.clusterName,
        API_SERVICE: this.api.service.serviceName,
        WORKER_SERVICE: workerService.serviceName,
        QDRANT_SERVICE: qdrantService.serviceName,
        WORKER_MIN: String(config.workerMin),
        WORKER_MAX: String(config.workerMax),
      },
    });

    // Least-privilege: scale services in this cluster; adjust the worker's
    // scalable target. Application Auto Scaling actions don't support
    // resource-level scoping, so they're granted on `*`.
    toggle.addToRolePolicy(
      new iam.PolicyStatement({
        actions: ['ecs:UpdateService'],
        resources: [`arn:aws:ecs:${region}:${account}:service/${cluster.clusterName}/*`],
      }),
    );
    toggle.addToRolePolicy(
      new iam.PolicyStatement({
        actions: [
          'application-autoscaling:RegisterScalableTarget',
          'application-autoscaling:DescribeScalableTargets',
        ],
        resources: ['*'],
      }),
    );

    // EventBridge Scheduler assumes this role to invoke the toggle Lambda.
    const schedulerRole = new iam.Role(this, 'OffHoursSchedulerRole', {
      assumedBy: new iam.ServicePrincipal('scheduler.amazonaws.com'),
    });
    toggle.grantInvoke(schedulerRole);

    // Two schedules (up + down) per window, evaluated in the configured tz.
    offHours.windows.forEach((w, i) => {
      const mk = (action: 'up' | 'down', expr: string) =>
        new scheduler.CfnSchedule(this, `OffHours${action === 'up' ? 'Up' : 'Down'}${i}`, {
          flexibleTimeWindow: { mode: 'OFF' },
          scheduleExpression: expr,
          scheduleExpressionTimezone: offHours.timezone,
          target: {
            arn: toggle.functionArn,
            roleArn: schedulerRole.roleArn,
            input: JSON.stringify({ action }),
          },
        });
      mk('up', w.up);
      mk('down', w.down);
    });
  }
}
