/**
 * Typed, committed, NON-secret configuration — the `aws-test-config.env`
 * equivalent. One entry per environment; selected at deploy time via
 * `cdk deploy -c env=<name>`.
 *
 * SECRETS DO NOT LIVE HERE. OAUTH_CLIENT_SECRET, JWT_SECRET, and
 * ANTHROPIC_API_KEY are read from SSM SecureString parameters at deploy
 * time (see infra/README.md → "Prerequisite: secrets"). This closes the
 * plaintext-env gap in the old bash deploy.
 */

export interface EnvConfig {
  /** Resource-name prefix, e.g. "test-". Lowercase, ends with hyphen. */
  prefix: string;
  /** Deploy region. */
  region: string;
  /** Environment label → CloudWatch `Environment` dimension + prod gating. */
  deployEnv: 'test' | 'staging' | 'prod';

  /** Optional custom domain. When set, HTTPS + ACM cert are provisioned. */
  domainName?: string;
  /** Route53 hosted zone to create the record in. Defaults to `domainName`;
   *  set explicitly when `domainName` is a subdomain (e.g. domainName
   *  `app.example.com` lives in the `example.com` zone). */
  hostedZoneName?: string;
  /** Frontend origin when there's no domain (set after first deploy). */
  frontendOrigin?: string;

  /** GitHub OAuth client id (non-secret; the *secret* is in SSM). */
  oauthClientId: string;
  /** Google OAuth client id (optional, non-secret). When set, Google login is
   *  wired and GOOGLE_CLIENT_SECRET is read from SSM. Redirect is derived as
   *  <OAUTH_REDIRECT_URI>/google. */
  googleClientId?: string;
  /** Comma-separated admin emails (auto-promoted on login). */
  adminEmails?: string;
  /** Enable the dev-login endpoint (bypasses OAuth). Never true in prod. */
  devMode?: boolean;

  /** Budget alert / SNS notification email. */
  notificationEmail: string;

  /** api Fargate sizing. */
  apiCpu: number;
  apiMemoryMiB: number;
  /** worker autoscaling bounds (CPU target-tracking @ 70%). */
  workerMin: number;
  workerMax: number;
  /**
   * Run the Qdrant task on Fargate Spot (~70% cheaper). Qdrant is a stateful
   * EFS-backed singleton, so a Spot reclaim drops vector search / `/ask` for
   * the ~1-2 min it takes a replacement task to start and remount the index
   * (the index survives — it lives on EFS). Acceptable for test; keep prod on
   * on-demand FARGATE for search availability. NOTE: a Spot capacity-provider
   * association has historically wedged stack rollback/delete ("capacity
   * provider in use"); the worker already carries that same association, so
   * enabling it here adds no new exposure.
   */
  qdrantSpot: boolean;
  /**
   * Enable ECS Container Insights (per-task/service CPU, memory, network,
   * storage metrics). ~88 billable custom metrics (~$26/mo) — worthwhile for
   * prod observability, wasteful on test where basic (free) AWS/ECS metrics
   * suffice. Keep true for prod, false for test.
   */
  containerInsights: boolean;
  /**
   * Fargate CPU architecture. Default 'X86_64' (Intel) — it builds natively
   * on an x86 host in minutes.
   *
   * 'ARM64' (Graviton) is ~20% cheaper per vCPU/GB and a good fit for the Rust
   * workload, BUT on an x86 build host the image must be built under emulation
   * (~70 min per commit, since the deploy bakes the commit SHA in as a build
   * arg; also needs `docker run --privileged tonistiigi/binfmt --install arm64`
   * once). And once Fargate Spot + off-hours scheduling are applied, compute is
   * a small slice of the bill, so the Graviton saving shrinks to ~$1/mo — not
   * worth the emulated-build tax. Only switch a stack to 'ARM64' when it has a
   * native arm64/Graviton build runner.
   */
  cpuArch: 'ARM64' | 'X86_64';

  /**
   * Off-hours scale-to-zero. When set, a scheduled Lambda scales the
   * api/worker/qdrant services to 0 outside the UP windows and back up inside
   * them — cutting Fargate + per-task IPv4 cost on a stack not needed 24/7.
   * Omit (prod) to run 24/7. Outside every window the ALB returns 503 (there
   * is no auto-wake); run `scripts/offhours.sh up` to bring it up early.
   *
   * Each window is a pair of EventBridge cron() expressions evaluated in
   * `timezone` (IANA, DST-aware). The stack stays UP from `up` until `down`.
   */
  offHours?: {
    timezone: string;
    windows: Array<{ up: string; down: string }>;
  };

  /**
   * Wire the Anthropic /ask endpoint. When true, the api+worker mount
   * ANTHROPIC_API_KEY from SSM — the parameter MUST exist first or tasks
   * fail to start. When false, /ask returns 503 (rest of stack unaffected).
   */
  aiEnabled: boolean;

  /** Cloud Map private DNS namespace for service discovery (Qdrant). */
  cloudMapNamespace: string;
}

// OAuth client ids are NOT secret, but they're specific to this deployment's
// OAuth apps, so they're kept OUT of the committed config. Supply them via env
// for a real deploy (e.g. `source scripts/aws-test-config.env` before
// `cdk deploy`). A clean checkout falls back to a placeholder (GitHub) /
// disabled (Google). Secrets always come from SSM, never here.
const GITHUB_CLIENT_ID = process.env.OAUTH_CLIENT_ID ?? 'REPLACE_WITH_GITHUB_OAUTH_CLIENT_ID';
const GOOGLE_CLIENT_ID = process.env.GOOGLE_CLIENT_ID; // unset → Google login disabled

// Domain is deployment-specific too, so it's env-driven. Unset → no-domain
// (bare ALB, http) stack. The Route53 zone defaults to the registered domain
// (last two labels — fine for x.com; set HOSTED_ZONE_NAME for multi-level TLDs
// or a non-obvious zone). Notification email likewise.
const DOMAIN_NAME = process.env.DOMAIN_NAME;
const HOSTED_ZONE_NAME =
  process.env.HOSTED_ZONE_NAME ?? DOMAIN_NAME?.split('.').slice(-2).join('.');
const NOTIFICATION_EMAIL = process.env.NOTIFICATION_EMAIL ?? 'you@example.com';

// Prefix is env-driven so CDK and the runtime read the SAME
// source of truth (scripts/aws-test-config.env sets
// STACK_PREFIX; both CDK and the app read from there). Hardcoded
// prefixes drifted from the env-sourced ones on 2026-07-03 and
// took down login when the task-role IAM policy pointed at
// `test-ogrenote/*` while the app read `test1-ogrenote/*`. The
// same drift class is now structurally impossible: change the
// env file → both CDK and app pick up the new value.
//
// Fail loudly if STACK_PREFIX isn't set — a silent fallback
// would just recreate the drift risk.
const STACK_PREFIX = process.env.STACK_PREFIX;
if (!STACK_PREFIX) {
  throw new Error(
    'STACK_PREFIX env var not set. Source scripts/aws-test-config.env before running cdk.',
  );
}

export const environments: Record<string, EnvConfig> = {
  test: {
    prefix: STACK_PREFIX,
    region: 'us-east-1',
    deployEnv: 'test',
    // With a domain (HTTPS via ACM) FRONTEND_ORIGIN is https, so DEV_MODE stays
    // off (real OAuth, stable redirect). With no domain the stack is http-only,
    // which requires DEV_MODE to avoid the non-https-origin panic — so devMode
    // tracks domain presence.
    domainName: DOMAIN_NAME,
    hostedZoneName: HOSTED_ZONE_NAME,
    oauthClientId: GITHUB_CLIENT_ID,
    googleClientId: GOOGLE_CLIENT_ID,
    adminEmails: '',
    devMode: !DOMAIN_NAME,
    notificationEmail: NOTIFICATION_EMAIL,
    apiCpu: 256,
    apiMemoryMiB: 512,
    workerMin: 1,
    workerMax: 2,
    qdrantSpot: true,
    containerInsights: false,
    cpuArch: 'X86_64',
    // offHours (scheduled scale-to-zero) is DISABLED: when down, the ALB
    // target group is empty and requests get a hard 503 until the next
    // scheduled `up` — there is no wake-on-use. That fails the "only delay
    // a page, never deny it" bar, so this stack runs 24/7. Re-enable only
    // once a request-triggered wake path exists. Windows kept for reference:
    //   { up: 'cron(30 16 ? * TUE *)', down: 'cron(30 0 ? * WED *)' },
    //   { up: 'cron(30 16 ? * FRI *)', down: 'cron(30 0 ? * MON *)' },
    aiEnabled: false,
    cloudMapNamespace: 'ogrenote.local',
  },
  prod: {
    prefix: 'prod-',
    region: 'us-east-1',
    deployEnv: 'prod',
    // Prod requires a domain (HTTPS via ACM) — set DOMAIN_NAME. devMode stays
    // hard-off so there is never a dev-login backdoor on prod.
    domainName: DOMAIN_NAME,
    hostedZoneName: HOSTED_ZONE_NAME,
    // Prod GitHub OAuth needs its OWN app (a GitHub app is tied to one callback
    // host, so the test app can't be reused). Set OAUTH_CLIENT_ID for prod once
    // that app exists, or replace this placeholder.
    oauthClientId: 'REPLACE_WITH_GITHUB_OAUTH_CLIENT_ID',
    googleClientId: GOOGLE_CLIENT_ID,
    adminEmails: '',
    devMode: false,
    notificationEmail: NOTIFICATION_EMAIL,
    apiCpu: 512,
    apiMemoryMiB: 1024,
    workerMin: 2,
    workerMax: 6,
    qdrantSpot: false,
    containerInsights: true,
    cpuArch: 'X86_64',
    aiEnabled: true,
    cloudMapNamespace: 'ogrenote.local',
  },
};

export function resolveConfig(envName: string): EnvConfig {
  const cfg = environments[envName];
  if (!cfg) {
    throw new Error(
      `Unknown environment "${envName}". Known: ${Object.keys(environments).join(', ')}. ` +
        `Select with: cdk deploy -c env=<name>`,
    );
  }
  return cfg;
}
