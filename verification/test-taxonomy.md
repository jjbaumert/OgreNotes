# Test taxonomy — kinds of manual verification

This document defines the categories of manual test the framework
recognizes. Every entry in a test plan belongs to exactly one
category. The planner picks the category; specialists adapt their
output shape to match.

The taxonomy exists for two reasons. First, it gives the planner a
finite menu to choose from, which keeps plans coherent across runs.
Second, it tells each specialist what "done" looks like for a given
test — a *repro verification* succeeds differently than an
*acceptance verification*, and the specialist's structured output
should reflect that.

If a test doesn't fit any of these categories, that's a signal
either the test is too vague to execute or the taxonomy needs a new
category. Default to "too vague" first; tighten the test before
adding a category.

---

## R — Repro verification

**Question:** Does this suspected or reported bug actually happen?

**Pass condition:** A clear yes or no, with the steps that produced
the observed behavior. A test passes by *answering the question*,
not by the system behaving correctly. A confirmed repro is a pass.

**Specialist output shape:**
- `result: repro_confirmed | repro_not_confirmed | inconclusive`
- `steps_executed:` numbered list of what was actually done
- `evidence:` curl output / screenshot path / HAR slice / log excerpt
- `if_inconclusive_why:` short prose

**Example.**
*Plan entry:* "User reports the share dialog silently fails when
inviting an external email and external-sharing is disabled at the
workspace level. Verify the failure mode."
*Possible findings:*
- `repro_confirmed`: dialog accepts the email, shows no error, but
  no member is added. (This is a bug — feedback is missing.)
- `repro_not_confirmed`: dialog shows a 403 error toast with the
  external-sharing-disabled message. (No bug.)
- `inconclusive`: dialog throws a 500; could be the bug or could
  be a separate dev-stack issue.

---

## A — Acceptance verification

**Question:** Does this feature meet the spec it was built against?

**Pass condition:** All acceptance criteria observable. The
specialist must enumerate each criterion and record observed/not-
observed individually — a feature that passes 9 of 10 criteria is
not a pass, it's nine passes and one failure.

**Specialist output shape:**
- `criteria:` list of `{criterion, status (met | not_met | n/a), evidence}`
- `overall:` `met | partially_met | not_met`
- `spec_reference:` design doc path or feature ticket ID

**Example.**
*Plan entry:* "Feature: notification email digest. Verify against
`design/high-level-design.md §Notifications`."
*Criteria the specialist enumerates:*
1. Inactive users (no in-app activity in last 24h) receive the
   email at the configured UTC hour.
2. Active users do not.
3. The email lists unread mentions, replies, and shares since the
   last digest.
4. The email's unsubscribe link disables `email_digest_enabled` for
   that user.

Each criterion gets its own observed/not-observed entry. The plan
entry is "verify the digest"; the criteria come from the spec.

---

## B — Boundary exploration

**Question:** What happens at the edges of the input space?

**Pass condition:** Behavior at each named boundary is recorded and
matches an expected category (graceful rejection, graceful
degradation, deliberate error). Crashes or silent acceptance of
invalid input are failures.

**Specialist output shape:**
- `boundaries:` list of `{boundary, input_used, observed_behavior,
  classification (graceful | degraded | crash | silent_accept)}`
- `surprises:` boundaries where observed behavior differs from
  what the spec or prior knowledge predicted

**Example.**
*Plan entry:* "Boundary exploration on `POST /api/v1/documents/{id}/
content`."
*Boundaries the specialist exercises:*
- Body size: 0 bytes, 1 byte, exactly `MAX_CONTENT_SIZE`, one byte
  over.
- Body content: empty, non-UTF8 bytes, valid yrs state, malformed
  yrs state.
- Auth: missing token, expired token, valid token without write
  permission, valid token with write permission.

The specialist records what happened at each. The output isn't
pass/fail of the feature — it's a *map* of the boundary behaviors.

---

## G — Regression check

**Question:** A change touched this area; did anything adjacent
still work?

**Pass condition:** Named adjacent flows still produce the
behavior that pre-existed the change. The specialist confirms each
flow individually; "looked fine" is not a pass.

**Specialist output shape:**
- `flows:` list of `{flow, status (ok | regressed | unknown),
  evidence}`
- `change_under_review:` what was touched (commit SHA, PR
  number, or area description)

**Example.**
*Plan entry:* "PR #N rewrites the auth middleware. Regression-
check sharing, document open, and WebSocket auth."
*Flows the specialist exercises:*
- Owner opens own doc → 200, content loads.
- Member opens shared doc → 200, content loads.
- Non-member opens private doc → 404 (existence-hiding).
- WebSocket upgrade with valid token → 101 + handshake.
- WebSocket upgrade with expired token → 401.

Each flow gets its own ok/regressed entry. Unlike A-category
acceptance, the criteria are "what the system did *before* the
change," not what a spec says it should do.

---

## C — Cross-context verification

**Question:** This works in isolation; does it work alongside
something else happening concurrently?

**Pass condition:** Each named context produces the expected
behavior. Common contexts: two browsers / two tabs, slow network,
expired session mid-flow, concurrent edits, mobile + desktop,
WebSocket disconnect/reconnect during operation.

**Specialist output shape:**
- `contexts:` list of `{context, observed_behavior,
  classification (expected | unexpected | broken)}`
- `concurrency_model_assumed:` short prose stating what the
  specialist took as "correct" concurrent behavior (so the
  reviewer can challenge the assumption)

**Example.**
*Plan entry:* "Cross-context: two browser windows editing the
same document, network briefly dropped on one."
*Contexts the specialist exercises:*
- Both online, both editing simultaneously → CRDT convergence
  visible in both within 2s.
- Window A goes offline, keeps editing, comes back online → A's
  edits land, B sees them.
- Window A reloads mid-edit → edit history shows a clean cut, no
  duplicate paragraphs.

---

## P — Permissions / auth verification

**Question:** Does the right principal see the right thing, and
does the wrong principal not see it?

**Pass condition:** A `(principal × resource × action)` matrix
where every cell has observed = expected. Both *positive*
(allowed cases work) and *negative* (denied cases are properly
denied, ideally 404 not 403 to prevent existence-probing) must be
verified — verifying only the positive cases is a common bug class.

**Specialist output shape:**
- `matrix:` list of `{principal, resource, action, expected
  (allow | deny_404 | deny_403), observed, evidence}`
- `existence_hiding:` boolean for whether deny cases use the
  expected non-leaking response codes

**Example.**
*Plan entry:* "Auth matrix for `GET /api/v1/documents/{id}`,
including link-sharing modes."
*Cells the specialist exercises:*
- Owner, own doc → 200.
- Member with View access, shared doc → 200.
- Non-member, doc with link-sharing=View → 200 (link works).
- Non-member, doc with link-sharing=None → 404 (not 403).
- Authenticated user, doc that was soft-deleted by its owner →
  404.

The matrix is the test. Each row is a separate observation.

---

## L — Latency, load, and performance

**Question:** Is this code path fast enough, scalable enough, and
free of perf footguns?

**Pass condition:** Depends on the sub-kind. For latency: an
observed distribution that meets a stated threshold (or, when no
threshold was stated, a *characterized* distribution the user
can decide on). For code-level perf (N+1 queries, async pitfalls,
allocation hot spots, lock contention, bundle bloat, algorithmic
complexity): a specific location named with a pattern description
and a cost estimate.

Unlike most verification, perf findings rarely produce a clean
pass/fail — they produce a *characterization*. The framework
prefers characterization findings the dev can act on over binary
verdicts, because perf decisions usually need context the
specialist doesn't have.

**Specialist output shape:**
- `kind: latency | async_pitfall | n_plus_one | allocation |
  contention | bundle_size | algorithmic` — exactly one
- `result: regression_confirmed | regression_not_confirmed |
  characterized | hot_path_identified | inconclusive`
- For latency: `distribution: {samples, p50, p95, p99, max,
  environment, threshold_compared_to}`
- For code-level: `locations: [{file, line, pattern,
  cost_estimate}]`

**Example.**
*Plan entry:* "Investigate `GET /api/v1/folders` for N+1 query
patterns. The home page feels slow for users with many folders."
*Possible findings:*
- `hot_path_identified`: handler at
  `crates/api/src/routes/folders.rs:87` calls
  `folder_repo.list_children(folder_id)` in a loop over the
  user's folders → N+1 DDB queries scaling with folder count.
  Cost estimate: ~10ms per query, 50 folders = 500ms added
  latency.
- `regression_not_confirmed`: handler already uses a batch read;
  no N+1 pattern observed.

The specialist that runs L-category entries is
**`perf-investigator`**.

---

## Picking the right category

The planner picks the category at plan time and labels each test
entry with one of: `R | A | B | G | C | P | L`. Specialists adapt
their elaboration and structured output to match the category.

If a single test entry seems to want two categories, *it's two
entries.* Splitting up front gives the specialists clearer
output shapes and produces sharper tickets. The planner's
elaboration burden is roughly the same; the supervisor's review
gets easier.

When in doubt:
- "Is this fixed?" → **R**
- "Is this what we said we'd build?" → **A**
- "What does this do at the edges?" → **B**
- "Did we break anything?" → **G**
- "Does this still work when X is also happening?" → **C**
- "Can the wrong person do this?" → **P**
- "Is this fast enough / are there perf footguns?" → **L**

---

## What this taxonomy does NOT cover

- **Sustained load and chaos testing.** The framework's L
  category covers single-endpoint latency and code-level perf
  patterns, but not high-concurrency load tests (k6, locust,
  vegeta) or fault-injection / chaos. Those need a different
  toolchain and a different output shape; this framework's
  perf-investigator will return `inconclusive` for them and
  surface the gap. Add a project-specific specialist if your
  project needs them.
- **Exploratory testing without a hypothesis.** "Just poke
  around" isn't a verification task in this framework's sense.
  Exploratory work that surfaces *new* hypotheses gets reported
  via the `scope-expansion-suggested` finding kind for the next
  planner run, not as an inline test.
- **Static review of code.** That's the Rust review framework's
  job (`framework/`). The verification framework looks at
  running behavior, not source — except for the perf-investigator,
  which crosses into static code review when the perf concern
  is a code-pattern issue (N+1, async pitfall, allocation
  hot spot). That crossover is intentional; the boundary is
  whether the question is "does this code follow good Rust
  patterns" (review framework) or "is this code costing the
  user observable time" (verification framework).
