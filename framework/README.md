# Rust Review Framework — adopting it on a new project

This directory is a **portable framework** for steering Rust
development on full-stack projects with a similar shape (Cargo
workspace, multiple library crates, an HTTP/WebSocket edge,
optionally a Rust→WASM frontend). It defines a layer taxonomy,
shared review patterns, two review sub-agents, and a supervisor
checklist.

This README is for someone seeding the framework into a new
project. If you're working on the project the framework already
lives in, you don't need this file.

## What's in here

| File | Purpose |
|------|---------|
| `architecture.md` | The 5-layer taxonomy (Foundation / Persistence / Domain / Edge / Client), forbidden-knowledge rules, trust boundaries. Has a blank template at the bottom. |
| `hints.md` | Cookbook of preferred Rust patterns, gated patterns, named anti-patterns, the reason-to-change test, multi-audience checklist, and three calibration examples (author-time fix / refactor-worthwhile / wouldn't bother). |
| `hints-frontend.md` | *Conditional* supplement for projects with a Rust→WASM client (Leptos, Yew, Dioxus, Sycamore). Skip if your client is JS/TS or there's no client. |
| `agents/authoring.md` | The prompt for the new-code review agent. |
| `agents/refactoring.md` | The prompt for the existing-code structural review agent. |
| `supervisor-review.md` | The policy a reviewer (human or supervising agent) applies when accepting patches and proposed spec changes. |
| `config.toml.example` | Per-project configuration schema with comments. Copy to `config.toml` and edit. |
| `CALIBRATION_NOTES.md` | The framework's reasoning trace — survey of how OgreNotes (the calibration model) layers, generalizability classification, mechanical-vs-judgment checks, and resolved decisions. Read once for context; not a runtime doc. |

## Prerequisites

- A Rust project laid out as a Cargo workspace. The framework
  generalizes best to `crates/<name>/` plus optional `frontend/`,
  but it adapts to single-crate or multi-binary layouts (record
  the layout in your project's `architecture.md`).
- Claude Code installed and configured for the project's repository.
  The sub-agents live at `.claude/agents/`.
- Rust 1.75+ recommended (the patterns assume edition 2021 or
  2024).

The framework does **not** require a particular HTTP framework,
storage backend, or auth strategy. Those go in the project-
conventions slot of your `architecture.md`.

## Step-by-step setup

### 1. Copy the framework into the new project

```bash
cp -r path/to/framework /path/to/new-project/framework
```

Then in the new project:

```bash
cd /path/to/new-project
rm framework/CALIBRATION_NOTES.md  # OgreNotes-specific reasoning trace; replace later if useful
```

The `CALIBRATION_NOTES.md` is the worked-example reasoning for the
project the framework was calibrated against. You can either delete
it (cleaner) or keep it as a reference for the kind of trace your
project should produce after its first month of use.

### 2. Fill in `architecture.md` for your project

Open `framework/architecture.md` and scroll to the **Worked
example** section. Replace the OgreNotes worked example with your
project's L1–L5 fillings, using the blank template at the bottom of
the file as a guide. Concretely:

- Name the crates that fill each layer.
- State each layer's one-sentence purpose.
- List the forbidden-knowledge entries specific to your stack.
- Fill in the project-conventions slot: identifier strategy,
  observability ownership, cross-target schema agreement, plus
  whatever is specific to your domain (key encoding, CRDT choice,
  deployment shape).

If you can't fit a fact in a slot, that's the friction the
framework wants you to resolve before the agents are useful. Don't
paper over it.

### 3. Fill in the framework-conventions slot in `hints.md`

Open `framework/hints.md`, scroll to **Framework-conventions slot**.
Replace the OgreNotes-specific lines with your project's:

- Where the boundary error type lives, and what it's called.
- Where storage models and access patterns live (or your project's
  equivalent decomposition).
- Where route handlers live; the `pub fn router()` convention or
  whatever your edge framework expects.
- Any project-specific conventions a reviewer benefits from
  knowing (key encoding rules, CRDT bytes-as-opaque, frontend
  mirror conventions, etc.).

The three before/after calibration examples at the bottom of the
file should *eventually* be replaced with examples drawn from your
own codebase — but you can leave the OgreNotes examples in place
for the first month while you collect your own. Concrete examples
calibrate the agents better than abstract rules; replace them as
soon as you have stronger candidates from your own review history.

### 4. Set per-project knobs in `config.toml`

```bash
cp framework/config.toml.example framework/config.toml
```

Edit the new file. Read the comments in `config.toml.example` for
trade-offs on each knob. Common defaults:

- `supervisor.kind = "human"` — keep human-supervised until you've
  watched the agents handle simpler cases without scope creep.
- `agents.may_propose_breaking_changes = false` — keep off until
  the project trusts the agents' breaking-change discipline.
- `agents.may_add_test_coverage_gap_tests = false` — same.
- `identifier_strategy = "newtype"` — for greenfield projects.
  Use `"string-grandfathered"` only when adopting the framework
  on a project that already used raw `String` IDs.

### 5. Install the two review sub-agents in Claude Code

```bash
mkdir -p .claude/agents
```

Create `.claude/agents/rust-author-reviewer.md` with frontmatter
and a body that points at `framework/agents/authoring.md`. Same
for `rust-refactor-reviewer.md` pointing at
`framework/agents/refactoring.md`.

Use the wired versions from the OgreNotes project as a copy
template — they're at `.claude/agents/rust-author-reviewer.md`
and `.claude/agents/rust-refactor-reviewer.md` in the calibration
project. The frontmatter (`name`, `description`, `tools`, `model`)
is the only project-portable part; the body just routes to the
canonical prompt at `framework/agents/`.

### 6. Optional: create a `CLAUDE.md` at the repo root

Without a `CLAUDE.md`, the parent Claude Code session won't have
ambient awareness of the framework — it will only invoke the
sub-agents when you ask, or when their `description` triggers
match. With one, the parent agent automatically reads the framework
when writing or reviewing Rust.

A minimal `CLAUDE.md` is 50–80 lines: project orientation, where
the canonical specs live, sub-agent routing, immutable-test and
don't-edit-specs constraints, plus any project memory the parent
should know on every session.

The trade-off is token cost on every session in this directory.
For framework-heavy projects this is worth it; for projects where
most sessions don't need the framework, you can skip it.

### 7. Configure mechanical checks (recommended)

The framework reserves agent attention for judgment calls. The
following are mechanical and belong in lint or build infra, not
in agent output:

- **`cargo-deny`** with a `[deps.bans]` policy enforcing the
  forbidden-knowledge layer rules (e.g. "the `notify` crate must
  not import `axum`"). Layer violations become compile errors.
- **Clippy at workspace level**: deny `clippy::unwrap_used` and
  `clippy::expect_used` outside `#[cfg(test)]` and `main`.
- **`cargo public-api`** in CI to surface `pub` signature changes
  as deliberate breaking-change commits.
- **`rustfmt`** non-negotiable.
- A pre-commit hook that fails if `crates/<name>/` exists but
  isn't a workspace member (or vice versa).

The agents *assume* these checks have run. A finding the
mechanical layer would have caught is wasted agent attention.

## First use — calibrating the agents on your project

After steps 1–7, run the authoring agent on the next PR landing
in your project. Read its findings as a *calibration test*:

- Are the findings specific (file:line, concrete code)?
- Do they cite specific sections of `architecture.md` or `hints.md`?
- Do they invoke pattern names with code-specific rationale, not
  ceremony?
- Are any findings actually clippy-able? (If yes, refine your
  lint config rather than letting the agent generate them.)

If the answers are no, the framework docs need refinement — not
the agent. The most common cause of bad findings is a hint that's
written abstractly without rationale or without a concrete example.
Find the offending section in `hints.md` and add the specificity
the agent is missing.

Run the refactoring agent on a chunk of legacy code in your
project. Watch especially for the `considered-and-declined`
section: an empty one means the agent is biased toward suggesting
(a calibration bug); a substantial one is a sign the cost-benefit
gate is working as intended.

## Iterating over time

The framework will only get better if you fold recurrent friction
back into the docs:

- Findings the supervisor keeps rejecting for the same reason →
  add a line to `hints.md` or `architecture.md` so the agent stops
  generating them.
- Spec changes proposed by the agents that turn out to be right →
  apply them via the supervisor doc's Section B workflow, in a
  separate commit.
- Recurring escalations from the supervisor checklist's escape
  hatch → write down the policy decision and add it to
  `supervisor-review.md` so it's testable next time.

After your project's first month with the framework, replace the
OgreNotes calibration examples in `hints.md` with examples drawn
from your own review history. Concrete examples drawn from real
review situations are the most valuable part of the framework;
the rules are scaffolding for them.

## When *not* to adopt this framework

This framework is calibrated for:

- Rust projects with multiple library crates and a clear edge.
- Projects where layering and structural review is high-value
  (i.e., not single-binary scripts or 500-line CLIs).
- Projects with at least one full-time Rust contributor — the
  framework's suggestions assume the reviewer can evaluate them
  on Rust-idiomatic terms.

It is **not** calibrated for:

- Single-crate libraries.
- Non-Rust projects (the patterns assume Cargo, `thiserror`, the
  Rust ownership model, etc.).
- Projects where the cost of reading the framework docs exceeds
  the value of structured review (most prototypes).

If you're not sure, run the framework against an old PR from your
project mentally: would the agent's likely findings have been
worth the time? If yes, adopt. If no, the project may not be at
the size where the framework pays for itself yet — revisit when
it grows.
