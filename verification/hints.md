# Hints — what makes a plan, a finding, and a ticket good

The verification framework produces three artifacts. Each has its
own quality bar and its own failure modes. The planner, the
specialists, and the ticket-writer all consult this document; the
supervisor uses it as the rubric for the two approval gates.

The three artifacts:

1. **Plan** — the planner's output, a small structured list of
   tests to run, approved by the user before specialists touch
   anything.
2. **Findings** — the specialists' structured output, one per
   test entry. The supervisor doesn't approve findings
   individually (that's the point of "high-level plan
   approval"), but the ticket-writer reads them as the source of
   truth.
3. **Tickets** — the ticket-writer's output, GitHub issues
   drafted from the findings and approved by the user before
   filing.

Findings are the bridge between machine work (specialists running
tests) and human work (the user reading tickets). They must carry
enough evidence that a developer reading only the ticket — not
the original conversation — can act on it.

---

## What makes a plan good

A plan is good when the supervisor can approve it in under a
minute, and the specialists can execute it without coming back
to ask the planner what was meant.

### Plan entries name a precondition + action + observable

A plan entry that says "test the share dialog" is too vague — the
specialist can't tell what precondition matters, what action to
take, or what to observe. A good entry reads:

> **R / share dialog external-disabled silent-fail**
> Precondition: workspace has `link_sharing_allow_external = false`.
> Action: open share dialog on a document, enter an external email,
> click invite.
> Expected observable: error toast naming the external-sharing-
> disabled policy.
> Bug we're checking for: silent acceptance with no member added.

Every entry has those four parts (category, precondition, action,
observable). The specialist still elaborates the exact selectors
or curl headers at execution time, but the *meaning* of the test
is fixed at plan time.

### Plan entries are the size of one finding

If an entry would produce more than one structured finding when
executed, split it. The taxonomy's six categories all assume one
entry = one finding. A category-P entry that lists ten
`(principal, resource, action)` cells is one finding with a ten-
row matrix; a category-R entry that suspects two unrelated bugs
is two entries.

### The plan covers the named risk

The user's free-form input names a risk (a feature, a PR, a bug
report, a worry). Every plan entry traces back to that risk. If
the planner finds itself adding entries that don't trace back —
"while we're at it, let's also test X" — that's
*scope-expansion-suggested* (a finding kind that surfaces back
to the user), not silent expansion of the plan.

### Effort estimates are honest

Each entry has a rough effort estimate (minutes, sometimes hours).
The estimate exists so the user can scope the plan down before
approving. Estimates lie when the planner is optimistic about
specialists' ability to elaborate; calibrate by checking actual
run times against estimates and updating this hint.

---

## What makes a finding good

A finding is what a specialist returns after executing one plan
entry. The ticket-writer reads it; the supervisor doesn't approve
it individually but does see it pass through.

### A finding has structured fields, not prose

Prose findings — "I tried the share dialog and it didn't error" —
are not findings, they're conversation. A finding is a typed
record with fields the ticket-writer can map to ticket sections.
The required fields depend on category (see `test-taxonomy.md`),
but at minimum every finding has:

- `plan_entry_ref:` the plan entry's identifier
- `category:` one of `R | A | B | G | C | P`
- `result:` category-appropriate status
- `evidence:` paths to screenshots, curl outputs, HAR slices, log
  excerpts, x-request-id values — *paths, not pasted-in-line
  blobs.* The ticket-writer collects and embeds them in the
  ticket body.
- `surprises:` anything observed that wasn't expected, even if
  it isn't the bug being checked

### Evidence is correlatable, not just present

A screenshot alone is weak. A screenshot + an `x-request-id` that
the developer can grep in CloudWatch is strong. A 500 response
body alone is weak; the response body + the request URL + the
authenticated user + the timestamp is what makes the ticket
actionable.

This project emits `x-request-id` from the `TraceLayer` in
`crates/api/src/observability.rs`. Specialists should capture
that header on every HTTP response they observe and include it
in the evidence — it's the cheapest correlation key available.

### A finding is honest about uncertainty

A specialist that says "the bug is in the auth middleware" when it
has only observed a 401 is overclaiming. The finding should say:
"observed 401 on `GET /api/v1/documents/{id}` for a user that
should have View access; the response carries
`x-request-id=abc123`; possible causes include auth middleware
rejection, missing membership row, or stale session — the
specialist did not investigate further." The ticket-writer can
turn that into a useful ticket; an overclaim turns into a wrong
ticket.

### A finding may surface gaps

If a specialist can't complete a test — missing test account,
missing infra, environment doesn't allow the boundary — it
returns the finding with `result: inconclusive` and a
`gap_reason:` describing what's missing. Inconclusive findings
are *not* failures; they're calibration data for the next plan.

### A finding may suggest a new test

When a specialist observes something during execution that
suggests an additional test would be valuable, it does **not**
silently run that test. It includes
`scope_expansion_suggested:` as a sibling field on the finding,
naming the additional test. The user reads it during ticket
review and can put it on the next plan.

---

## What makes a ticket good

A ticket is the framework's only durable output (everything else
is ephemeral by design). Three properties matter most.

### A ticket is self-contained

A reader who finds the ticket months later, with no access to
the original conversation, no access to the plan, and no Claude
Code session, should still be able to act on it. That means:

- **Repro steps inline.** Not "see the plan"; the actual numbered
  steps.
- **Evidence inline or linked durably.** Screenshots committed to
  a known bucket and linked, not "see attached" without an
  attachment. Curl outputs pasted; log excerpts pasted with
  timestamps and request IDs.
- **Environment captured.** Which deploy (dev / staging / prod),
  which commit SHA if known, which browser / version for UI
  bugs, which user account or role.
- **Suspected area, not asserted cause.** "Suspected in
  `crates/api/src/routes/sharing.rs::add_doc_member`" is fine;
  "the bug is at `sharing.rs:142`" overclaims (see *honest about
  uncertainty* above).

### A ticket title names the bug, not the symptom

A symptom-named title — "share dialog returns 500" — describes
what was observed. A bug-named title — "share dialog accepts
external email when external-sharing is disabled, silently fails
to add member" — describes the actual defect. Bug-named titles
let the issue tracker organize itself by problem, not by
end-user observation.

If the bug isn't yet identified — the finding is
*repro_confirmed* but the cause is unclear — the title names the
*observation that proves the bug exists*, not the symptom. "Share
dialog silently drops external invites with no error feedback" is
not yet "the bug is X," but it is precise about what's wrong.

### One ticket per bug, not per finding

Multiple findings can roll up into one ticket if they share a
root cause — three findings all observing 500 errors after the
same auth middleware refactor are one ticket. The ticket-writer
makes this judgment and *names it explicitly* in the ticket body:
"This ticket consolidates findings F-2, F-5, F-7 which appear to
share a single root cause in the auth middleware."

Conversely, one finding can become two tickets if it surfaces
two distinct bugs — a category-B boundary exploration that
finds both a crash on empty input *and* a silent accept on
oversized input is two tickets. Two distinct fixes, two distinct
trackings.

### A ticket has a severity, not a guess

The framework's severity ladder:

- **S1 — Data loss, security, or auth bypass.** Anyone losing data
  or seeing data they shouldn't see. Files immediately, no batch.
- **S2 — User-facing broken flow.** Feature doesn't work; users
  hit it; no workaround.
- **S3 — User-facing degraded flow.** Feature works but
  inconsistently, or works with a workaround.
- **S4 — Latent / internal.** A bug that exists but no current
  user has hit it; cleanup-grade.

Severity is the *user impact*, not the *fix cost*. A trivial
typo bug that exposes another user's documents is S1, not S4.
The ticket-writer assigns severity from the finding's evidence;
the supervisor can override on review.

---

## Anti-patterns the framework asks reviewers to flag

These are the recurring failure modes the supervisor catches at
the two approval gates.

- **Vague plan entry.** "Test the editor." Cure: precondition +
  action + observable, or drop the entry.
- **Specialist overclaim.** A finding asserting a root cause from
  symptom-only evidence. Cure: rewrite the finding with explicit
  uncertainty, or run the test more deeply first.
- **Ticket as conversation.** Body says "as we discussed" or
  "from the plan above." Cure: inline everything, then check
  again that the ticket reads cold.
- **Roll-up theatre.** Multiple findings rolled into one ticket
  without naming the shared root cause. Cure: name it, or split
  the ticket.
- **Severity by fix cost.** "S4 because the fix is small." Cure:
  re-grade by user impact.
- **Evidence-free finding.** "Looks fine to me." Cure: either
  evidence or downgrade to `inconclusive` with a `gap_reason`.
- **Scope creep in the plan.** Entries that don't trace to the
  user's named risk. Cure: drop them, or surface as
  `scope-expansion-suggested` in the next plan.

---

## When in doubt, do less

The framework's bias is toward fewer, more specific tests rather
than more comprehensive coverage. A plan with three sharp tests
that catch real bugs is better than a plan with twelve vague
tests that mostly say "looks fine." This mirrors the Rust
framework's *additive bias* warning: when reviewing a plan,
prefer removing weak entries over adding more.
