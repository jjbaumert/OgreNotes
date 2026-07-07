# Verification framework — on-demand manual-test orchestration

This directory is a **portable framework** for running on-demand
manual verification on a Rust full-stack project. It produces a
high-level test plan, hands the entries to specialist agents (or
a human) for execution, and converts findings into GitHub issues.

It is a sibling of `framework/` (the Rust code-review framework
in the same repo) — same shape (taxonomy + hints + agents +
supervisor + config + README), different domain. The code-review
framework asks "is this code well-structured?"; this one asks
"does the running system actually do what we said it does?"

This README is for someone seeding the framework into a new
project. If you're working on the project the framework already
lives in, you don't need this file.

## What's in here

| File | Purpose |
|------|---------|
| `test-taxonomy.md` | The six categories every plan entry belongs to (`R` repro / `A` acceptance / `B` boundary / `G` regression / `C` cross-context / `P` permissions). Each has a worked example. |
| `hints.md` | What makes a plan well-scoped, a finding well-evidenced, and a ticket actionable. Anti-patterns the supervisor catches. |
| `supervisor-review.md` | The two approval gates the user (or future supervising agent) applies: Gate 1 (plan), Gate 2 (tickets). |
| `agents/test-planner.md` | Prompt for the planner agent that emits the high-level plan. |
| `agents/api-tester.md` | Prompt for the new API-level specialist (curl + HTTP). |
| `agents/human-instructor.md` | Prompt for the human-driver specialist (composes steps + waits + interprets). |
| `agents/perf-investigator.md` | Prompt for the performance specialist (endpoint latency, async pitfalls, N+1, allocation, contention, bundle, algorithmic). |
| `agents/ticket-writer.md` | Prompt for the agent that drafts and (on approval) files GitHub issues. |
| `config.toml.example` | Per-project configuration schema with comments. |

The framework also depends on **`frontend-doctor`**, an already-
existing sub-agent at `.claude/agents/frontend-doctor.md` that
drives a headless Chromium for UI scenarios. The frontend
specialist is *not* in this framework's `agents/` directory
because it predates the framework and works as-is.

## Operating model

A run is on-demand: the user invokes the framework when they
want to verify something — a feature, a PR, a bug report, a
worry. The flow has exactly two approval gates:

```
user describes risk
      │
      ▼
test-planner emits high-level plan ──► Gate 1: user approves plan
      │
      ▼
parent agent invokes specialists (frontend-doctor / api-tester
/ human-instructor / perf-investigator) per plan entry. Specialists elaborate detail
at runtime; no intervening approval needed.
      │
      ▼
findings flow to ticket-writer
      │
      ▼
ticket-writer drafts tickets ──► Gate 2: user approves drafts
      │
      ▼
approved drafts file via `gh issue create`
```

Plans and findings are **ephemeral** — they live in the
conversation, not on disk. Tickets are the only durable output.

## Prerequisites

- A Rust full-stack project with a deployed system the agents
  can hit (local dev stack, test stack, or staging). The
  framework is calibrated for the configured `[api_tester]
  .base_url`.
- `gh` CLI authenticated against the issue-tracker repo.
- Claude Code installed with sub-agents at `.claude/agents/`.
- A way for an automated browser to reach the system if you
  plan to use `frontend-doctor` (this project's frontend-doctor
  drives headless Chromium against the deployed stack).

The framework does **not** require any particular feature being
present in your project (auth, sharing, CRDT, etc.) — the
taxonomy and hints generalize. The `test-taxonomy.md` examples
draw from this project for calibration; replace them with your
own as you build a track record.

## Setup steps

### 1. Copy the framework

```bash
cp -r path/to/verification /path/to/new-project/verification
```

### 2. Configure per-project knobs

```bash
cp verification/config.toml.example verification/config.toml
```

Edit the new file. Important values:

- `[issue_tracker].repo` — set to `owner/name` only if filing
  into a different repo than the local one; otherwise leave
  empty and `gh` picks the local remote.
- `[api_tester].base_url` — set to your dev or test deployment.
  *Never* set to a production URL; the
  `production_host_patterns` check is a guard rail but not a
  substitute for "don't point this at prod."
- `[ticket_writer].default_labels` — labels every filed issue
  gets. Common: `["verification-finding"]` so a triage view can
  separate framework-filed issues from human-filed ones.

### 3. Install the five sub-agents in Claude Code

```bash
mkdir -p .claude/agents
```

Create five files at `.claude/agents/test-planner.md`,
`api-tester.md`, `human-instructor.md`, `perf-investigator.md`,
`ticket-writer.md`. Each file has frontmatter (name, description,
tools, model) and a body that points at the canonical prompt in
`verification/agents/`. Use the OgreNotes installs as templates
— they're at `.claude/agents/{test-planner,api-tester,
human-instructor,perf-investigator,ticket-writer}.md` in the
calibration project.

The frontmatter's `description` field is what determines whether
the parent agent invokes the sub-agent proactively. The
descriptions in the OgreNotes installs are tuned for "Use
PROACTIVELY when..." triggers; adjust to your project's
workflow if needed.

### 4. Update `CLAUDE.md` (if you have one)

Add a section pointing the parent agent at `verification/` so it
knows where to route requests like "verify this PR" or "test the
share dialog." The OgreNotes `CLAUDE.md` has a worked example.

If you don't yet have a `CLAUDE.md`, the framework still works
— it just means the user has to explicitly invoke the planner
sub-agent rather than the parent agent routing automatically.

### 5. Make sure `frontend-doctor` (or equivalent) is installed

If your project doesn't have a frontend-doctor agent, either:
- Adapt this project's `frontend-doctor.md` (it drives Playwright
  + Chromium, captures HAR + console + WebSocket).
- Or, skip frontend verification and have the planner route all
  UI tests to `human-instructor` until you build one.

Don't put `frontend-doctor` under `verification/agents/` — it
predates the framework and is used by code outside it.

## First use — calibrating

Run the planner on a small, well-bounded risk first. Good
candidates:

- A recent PR with three or four touch points.
- A specific bug report you want to verify is fixed.
- One acceptance criterion from a recently-shipped feature.

After Gate 1, watch what the specialists actually do. The
calibration test is whether their findings match the plan's
*intent*. Common first-run friction:

- **Plan entries too vague.** Specialists ask the parent agent
  for clarification mid-run. Cure: tighten `hints.md` *§What
  makes a plan good* with your project's vocabulary.
- **Plan entries too specific.** The planner is overstepping
  into specialist territory. Cure: remind the planner (via
  the hint doc or a one-off note) that elaboration is the
  specialist's job.
- **Findings missing evidence the ticket-writer needs.** Cure:
  update `agents/api-tester.md` or `human-instructor.md` with
  the specific evidence shape your project needs (e.g., if
  CloudWatch correlation requires a specific log group, name
  it).
- **Tickets look like conversation rebroadcast.** Cure:
  re-read `hints.md` *§What makes a ticket good* and apply T1
  (self-contained) more strictly during Gate 2.

The framework's *bias toward fewer, sharper tests* (see
`hints.md` *§When in doubt, do less*) is most important on the
first run. A plan with three sharp tests that catch one real
bug is the calibration target.

## When *not* to adopt this framework

This framework is calibrated for:

- Projects with a running deployed system you can hit (i.e.
  not pure libraries).
- Workflows where on-demand manual verification has real value
  — projects with mature automated test coverage might find
  the marginal value low.
- Projects with active human review of bug tickets — the
  framework's whole point is producing actionable tickets.

It is **not** calibrated for:

- Pure CI-driven testing (you'd want a different shape: per-PR
  automatic, no human approval gates).
- Projects without an issue tracker (the framework's durable
  output is tickets).
- Projects whose verification is dominated by automated tests
  where manual coverage would just duplicate them.

If you're not sure: run the planner on one recent bug fix
mentally. Would the framework have caught a real issue
worth tracking? If yes, adopt. If no, the project's
verification needs are probably already covered.

## Relationship to the Rust review framework (`framework/`)

The two frameworks are independent and complementary:

- `framework/` reviews **source code** — layering, idioms,
  patterns, refactors.
- `verification/` (this framework) reviews **runtime
  behavior** — bugs, regressions, boundary conditions, auth
  matrices.

They share zero documents and zero agents. A bug found by
verification might motivate a refactor surfaced by the review
framework, but the two flows don't reference each other in the
runtime artifacts they produce.

If your project adopts both, install them as siblings (`framework/`
and `verification/` at the same level) and let `CLAUDE.md` route
between them.
