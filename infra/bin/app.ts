#!/usr/bin/env node
import 'source-map-support/register';
import * as cdk from 'aws-cdk-lib';
import { OgreNotesStack } from '../lib/ogrenotes-stack';
import { resolveConfig } from '../config';

const app = new cdk.App();

// Select the environment slice: `cdk deploy -c env=prod` (defaults to test
// via cdk.json context). This is the idiomatic replacement for sourcing
// aws-test-config.env before running the bash deploy.
const envName = app.node.tryGetContext('env') ?? 'test';
// Spread so a prefix override doesn't mutate the shared config object.
const config = { ...resolveConfig(envName) };

// `-c prefix=…` is retained as a one-off override for exceptional
// cases (e.g. deploying to a differently-prefixed stack from the
// same checkout), but WARNS when it disagrees with the env-sourced
// prefix — silent divergence is exactly what caused the
// 2026-07-03 drift-and-recover incident.
const prefixOverride = app.node.tryGetContext('prefix');
if (prefixOverride) {
  if (prefixOverride !== config.prefix) {
    console.warn(
      `[cdk] WARNING: -c prefix=${prefixOverride} differs from STACK_PREFIX=${config.prefix}. ` +
        'Using the -c override, but the runtime env still points at STACK_PREFIX. ' +
        'Make sure that is intentional.',
    );
  }
  config.prefix = prefixOverride;
}

// Provenance: the git stamp flows into the Docker build-arg so the in-app
// version row matches what's deployed — same role GIT_STAMP played in
// aws-redeploy.sh. Pass it explicitly: `GIT_STAMP=$(git rev-parse --short HEAD)`.
const gitHash = process.env.GIT_STAMP ?? 'unknown';

new OgreNotesStack(app, `OgreNotes-${envName}`, {
  config,
  gitHash,
  // Account/region must be concrete (not env-agnostic) because the stack
  // uses fromLookup (hosted zone) and AZ enumeration.
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: config.region,
  },
  description: `OgreNotes ${envName} stack (CDK; supersedes scripts/aws-test-deploy.sh)`,
  tags: {
    Project: 'OgreNotes',
    Environment: config.deployEnv,
    ManagedBy: 'cdk',
  },
});

app.synth();
