# Supervisor review checklist

The two agents (authoring, refactoring) produce patches and findings
under explicit oversight. This document is the policy a supervisor
applies when deciding whether to accept a patch or a proposed
specification change.

The current oversight configuration on this project is **human
oversight**: a person reads agent output, applies patches, and
reviews proposed specification updates. The configuration is
per-project and may change later — for example, to a supervising
agent applying a documented policy. This checklist is written so
that either reviewer (a human now, an agent later) can apply it
without reinterpreting it.

A supervising agent inheriting this checklist must apply it
mechanically to the agent's structured output and produce a
disposition (`accept`, `accept-with-edits`, `reject`,
`escalate-to-human`) for each finding. Escalation is always an
allowed move; for any finding the agent supervisor is unsure about,
escalation is the correct disposition.

---

## A. Patch disposition

Apply each test below to every patch. A patch must pass all of
them to ship as proposed. A failure is not necessarily a rejection
— most failures route to "request changes" rather than "reject" —
but the failed test names what's wrong.

### A1. Behavior-preservation claim is present and credible

Every patch ships with an explicit "this preserves the behavior
existing tests verify" claim. Read it. If it's missing, request
the claim. If it's present but you don't believe it, the patch is
a `behavior-change` patch in disguise — return it for relabeling.

A patch claiming behavior-preservation but touching control flow
in non-trivial ways earns extra scrutiny: read the diff and
identify at least one concrete input where the old and new code
demonstrably do the same thing. If you can't, escalate to running
the relevant tests, or reject the claim.

### A2. Existing tests are not modified

Any patch that touches a file containing `#[test]`, `#[cfg(test)]`,
or anything under `tests/` or `benches/` is suspect. The agent's
output should call out the test edit explicitly under
`mechanical-test-edit` (rename propagation, import updates) or
`behavior-change` (anything else). If a test edit is present but
not flagged, return the patch.

The narrow exception is a *new* test added in the same patch as
new production code — that's an authoring-agent output and is
allowed.

### A3. No edits to specification categories

Reject any patch that modifies, even tangentially:

- Files under `design/`, `runbook/`, or `framework/`.
- `pub` items at the crate root that are imported by other crates.
- Boundary serialized types (request/response DTOs, `pub` error
  enums returned from a crate, config schemas).
- Database migration files.

These categories are the agent's evidence base, not their editing
surface. A finding that proposes a change to one of them is fine;
a *patch* that applies the change is not. Convert it to a
proposed-spec-update finding and route through Section B.

### A4. Layer-violation patches actually fix the layer violation

When a patch is labeled as fixing a layer violation, verify the
violation in the architecture doc's forbidden-knowledge cheat
sheet. The patch must remove the forbidden import or reference,
not paper over it with a new abstraction that re-imports
internally.

### A5. Refactor patches show their gate trace

Refactoring-agent patches include a "cost-benefit gate trace" —
one sentence per gate item. Read it. If the trace is missing or
the reasoning is generic ("this is cleaner"), reject the finding
and request a specific trace. If the trace cites a hint-doc
section, verify the citation exists and applies.

### A6. Patch fits the finding

A patch that does more than the finding describes ("while I was
in there, I also…") is rejected as scope creep, even when each
sub-change is individually correct. Each sub-change goes in its
own finding with its own gate.

### A7. Pattern names are backed by code-specific rationale

If the finding invokes a pattern name from the hints doc (e.g.
"primitive obsession," "boolean blindness"), verify the rationale
explains the concrete improvement to *this code*, not the
abstract concept. If the rationale is "this is X; the cure is Y"
without saying which line is harder to read or which kind of
change just got easier, reject as ceremony.

### A8. Author-time / refactor-worthwhile / refactor-only-if-touching label is honest

Author-time: the code is not yet shipped, the edit is local, the
cost of fixing later would be substantially higher. If any of
those is false, it should have been a refactor-worthwhile finding.

Refactor-worthwhile: the cost-benefit gate passes on its own. The
finding stands without needing other work to be happening in this
area.

Refactor-only-if-touching: the change is correct in isolation but
isn't worth its own PR. Normally rolled into another change. If
the agent shipped this label as a standalone PR, ask why.

### A9. test-coverage-gap findings assert properties, not outputs

`test-coverage-gap` is the refactoring agent's narrow license to
add tests for currently-untested behavior. Every such patch must
assert a property — round-trip, idempotence, monotonicity, ordering
invariant, "doesn't crash on input from this generator." Reject
any `test-coverage-gap` patch whose test pins a *specific output
value* against a specific input ("expected: 42"). Specific-output
tests freeze incidental behavior the project may intend to evolve;
that's not a coverage gap, it's a specification, and a
specification belongs in a deliberate change, not a coverage-gap
patch.

The patch must also include the agent's high-confidence claim
about the property being intentional. If the claim is hedged
("the code does this today"), demote to a `consider`-severity
finding without a patch.

### A10. breaking-api-deprecation-shim patches keep the old signature available

A `breaking-api-deprecation-shim` patch adds a new public
signature *alongside* the old one and marks the old one
`#[deprecated]` with a documented removal date. Reject any such
patch that *removes* the old signature in the same diff —
removal happens in a separate change, after the documented date,
under explicit human approval. The point of the deprecation shim
is to give callers a window; removing it the same day defeats
the point.

The supervisor also verifies the removal date is reasonable
(typically one minor-release cycle, or 30+ days on a continuous-
deploy project) and that the deprecation note tells callers what
to use instead.

---

## B. Specification-change disposition

Specification changes are anything that modifies architecture
docs, hint docs, design docs, public API surfaces, wire shapes,
or test contracts. They cannot ship as part of a patch — they
arrive as separate findings with proposed text.

### B1. The proposed change has a concrete trigger

A spec change is justified by a code reality that the spec doesn't
match (drift), a deliberate design decision the team made and
hasn't yet recorded, or an observed gap the spec should close. It
is not justified by "the doc could be clearer."

### B2. The change is the minimum sufficient edit

A proposed spec update should change as little as possible. If
the agent's proposal rewrites a whole section to fix one factual
error, request a narrower edit. The smaller the diff, the easier
to review, and the easier to roll back if the underlying decision
changes.

### B3. The change is consistent with sibling specs

A change to one framework doc must not contradict another. If the
authoring-agent prompt now says "X" and the architecture doc still
says "not X," one of them is now wrong. The supervisor catches
this; the agent does not have visibility into all sibling docs.

### B4. The change does not weaken safety guarantees

Spec changes that loosen a constraint (lower a coverage target,
remove a forbidden-knowledge entry, broaden what the agent may
edit) get extra scrutiny. The default disposition is "no, justify
again" — these are the changes most likely to be drift toward
agent convenience rather than project benefit.

### B5. Public API and wire-shape changes have a deprecation path

When the spec change is a `breaking-change` finding, the proposal
must address how existing callers migrate. "Just change the
signature" is rejected; the proposal should include a deprecated
shim, a migration window, or an explicit "we accept the break and
here's the rollout."

---

## C. Triage

After A and B, classify each disposition:

- **accept** — the patch or proposal ships as-is.
- **accept-with-edits** — minor corrections required (typo,
  comment improvement, narrower commit message). Apply and ship.
- **request-changes** — the patch or proposal has a real defect
  the agent can fix. Return with the failing test name from
  Sections A or B.
- **reject** — the patch or proposal should not exist in this
  form. Return with a one-line reason. Examples: scope creep that
  can't be unbundled, ceremony without a rationale, behavior
  change disguised as refactor.
- **escalate** (agent supervisor only) — the supervisor isn't
  certain. Hand to a human with the supervisor's reasoning so far.

A patch can ship with several findings in `accept` and several in
`request-changes`; the patch is a single deployable unit, but the
findings are reviewed individually.

---

## D. Per-project configuration

Per-project configuration lives in `framework/config.toml` at the
project root. Both agents and the supervisor must read it before
acting. The full schema is in `framework/config.toml.example`,
which doubles as a copy-paste seed for new projects. The knobs the
supervisor cares about:

- `supervisor.kind` — `"human"` or `"agent"`. Selects which review
  pipeline applies. Default `"human"`.
- `supervisor.identity` — name(s) of human reviewers, or the
  identifier of the supervising-agent role.
- `branch_protection.allow_direct_to_main` — whether the agent
  may push directly to `main`. Default `false`.
- `agents.may_propose_breaking_changes` — enables
  `breaking-api-deprecation-shim` patches. Default `false`. When
  `false`, public-API findings flag and stop (no patch).
- `agents.may_add_test_coverage_gap_tests` — enables
  `test-coverage-gap` patches. Default `false`. When `false`, the
  refactoring agent surfaces coverage gaps as `consider`-severity
  findings without a patch.
- `identifier_strategy` — `"newtype"` or `"string-grandfathered"`
  or a project-specific value. Suppresses or enables
  newtype-on-existing-IDs findings.

Agents and supervisors that find a missing knob fall back to the
most-restrictive default and emit an `open-question` finding
asking the project owner to set it explicitly. Silent fallback to
permissive defaults is a configuration bug.

Spec changes to anything in `framework/` follow Section B; the
config file is *not* in `framework/` for that purpose because it
holds per-project values, not framework spec. Changes to
`config.toml` are ordinary commits; changes to
`config.toml.example` are spec changes.

---

## E. The escape hatch

Both agents and supervisors will encounter situations this
checklist doesn't cover. The escape hatch is:

- The agent flags it as `open-question` and stops.
- The supervisor escalates to a human, or, if the human is the
  supervisor, makes a recorded judgment call.

Recording the judgment call is the important part — over time,
recurrent escapes become input to the next revision of this
checklist. The framework gets better only if the gaps it doesn't
cover are written down.
