# OgreNotes — agent guidance

OgreNotes is a clean-room collaborative document/spreadsheet/chat
platform written in Rust. The repository is a Cargo workspace of
nine backend crates plus a separate Leptos/WASM frontend.

## Where the canonical specs live

- **`design/`** — feature-level design docs (`high-level-design.md`
  is the index; per-feature docs cover editor, spreadsheet, sharing,
  auth, search, notifications, etc.). These describe what the
  product does. Treat them as authoritative on intent; do not edit
  them as a side effect of a code change.
- **`framework/`** — the Rust **code-review** framework.
  `architecture.md` defines the layer taxonomy (L1 Foundation →
  L2 Persistence → L3 Domain → L4 Edge → L5 Client) and
  forbidden-knowledge cheat sheet. `hints.md` is the cookbook of
  preferred patterns and named anti-patterns. `hints-frontend.md`
  adds client-crate-specific guidance. `supervisor-review.md` is the
  policy for accepting agent patches. Per-project knobs live in
  `framework/config.toml`. Consult when reviewing or writing Rust
  source code.
- **`verification/`** — the **runtime-behavior verification**
  framework. `test-taxonomy.md` defines the six manual-test
  categories (R repro / A acceptance / B boundary / G regression /
  C cross-context / P permissions). `hints.md` covers plan / finding /
  ticket quality. `supervisor-review.md` is the two-approval-gate
  policy (plan, tickets). Per-project knobs in
  `verification/config.toml`. Consult when verifying that the running
  system actually behaves as intended.
- **`runbook/`** — operational reproduction recipes for production
  issues. Read when debugging deploys or live behavior.

## Sub-agent routing

The `.claude/agents/` directory holds task-specific sub-agents.
Reach for them via the `Task` tool when their description matches.
Never duplicate work an agent is already doing.

- **`rust-author-reviewer`** — review new or in-progress Rust code
  for layering, idioms, and trust boundaries. Use when a diff lands
  or a feature is being written.
- **`rust-refactor-reviewer`** — review existing code for structural
  improvements with a strict cost-benefit gate. Default verdict is
  "leave it." Outputs a `considered-and-declined` section; that
  section is a first-class artifact, not boilerplate.
- **`test-planner`** — produces an on-demand high-level manual-test
  plan from a free-form description plus optional references (PR,
  design doc). Plan is approved by you before specialists run.
- **`api-tester`** — executes one API-level test entry with curl
  and returns a structured finding. Captures `x-request-id` for
  CloudWatch correlation. Honest about uncertainty.
- **`human-instructor`** — composes precise steps for a human to
  execute one test entry, waits for the observation, interprets
  the reply into a structured finding.
- **`perf-investigator`** — investigates performance problems
  across seven kinds (endpoint latency, async pitfalls, N+1
  queries, allocation hot spots, lock contention, WASM bundle
  bloat, algorithmic complexity). Measurement-disciplined:
  reports distributions not averages. Identifies and characterizes;
  does *not* propose fixes (the dev decides the cure).
- **`ticket-writer`** — converts findings into GitHub issue drafts,
  presents them for Gate 2 approval, and files via `gh` on
  approval. Never auto-files; runs a duplicate check first.
- **`security-auditor`** — read-only static security audit; refresh
  the security-controls inventory or pre-merge security review.
- **`aws-deploy-doctor`, `aws-diagnostic`, `aws-network-doctor`,
  `aws-iam-doctor`** — production AWS triage. Use when a deploy
  fails, ECS won't reach steady state, ALB returns 5xx, or an IAM
  AccessDenied appears.
- **`ci-doctor`, `test-triage-doctor`** — failed GitHub Actions
  runs and broken tests.
- **`frontend-doctor`** — drives a headless browser to reproduce
  UI-observed bugs that can't be diagnosed from server logs alone.

## Constraints that apply across all work

- **Tests are immutable when refactoring.** Existing tests encode
  behavioral contracts. A change that requires modifying an existing
  test is a behavior change, not a refactor — surface it as a
  separate finding. Adding *new* tests for new code is fine; the
  refactoring agent has a narrow `test-coverage-gap` license for
  property-only tests gated by `framework/config.toml`.
- **Don't edit `design/`, `framework/`, or `runbook/` as a side
  effect** of a code change. Drift between code and these docs is
  reported as a finding with proposed text, never applied
  unilaterally.
- **Public-API and wire-shape changes are deliberate.** Changes to
  `pub` crate-root signatures, serialized DTOs, public error enums,
  config schemas, or DynamoDB key encodings are breaking changes
  by default. Flag separately; do not bundle with refactor work.
- **Frontend is outside the workspace.** `frontend/` is excluded
  from the root `Cargo.toml` workspace; it has its own profile and
  a WASM target. To build or test it, `cd frontend/` first.
- **Verification runs are ephemeral.** Plans produced by
  `test-planner` and findings produced by specialists live in the
  conversation only — they are *not* committed to git. The
  ticket-writer's filed GitHub issues are the only durable
  output. Don't propose to persist plans or findings to disk
  unless the user explicitly asks for it. Don't `git add -A` /
  `git add .` in this repo — the verification working area is
  meant to stay untracked alongside any other in-progress work.
- **Verification runs have two approval gates.** When orchestrating
  a `test-planner` → specialists → `ticket-writer` flow, present
  the plan for user approval before invoking specialists, and
  present ticket drafts for user approval before invoking
  `gh issue create`. The sub-agents enforce this internally; the
  parent agent's job is to respect the gate boundaries when
  orchestrating.

## Project memory at a glance

- Phase 5 (polish — mobile, embeds, integrations, themes, command
  palette, accessibility, i18n, performance budgets, all-format
  import/export) is the active milestone as of 2026-05-17. Phases
  1–4 are closed; Phase 4 (enterprise: admin console, MFA, SAML,
  SCIM, audit logging, trash worker, S3 backup exports, rate-limit
  coverage) closed at commit `f4f63d5`.
- The project uses raw `String` for all identifiers
  (`identifier_strategy = "string-grandfathered"` in
  `framework/config.toml`); do not generate newtype-on-existing-IDs
  findings.
- Schema duality between `crates/collab/src/schema.rs` (canonical)
  and `frontend/src/editor/schema.rs` (parallel) is enforced by a
  CI test on the backend side.
- Audit logging is a two-table pattern: `AdminAudit` (admin
  mutations, retained permanently) and `SecurityAudit` (login,
  MFA, SAML, SCIM, share-revoke, doc-delete; retained 90 days by
  the `audit_retention` worker). New write-paths that touch
  identity, sharing, or destructive document state should emit a
  `SecurityAudit` row via `routes::audit::record_security_event` or
  `routes::audit::record_security_event_by_actor`.
