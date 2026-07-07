# Supervisor review — two approval gates

The verification framework runs under explicit oversight. There
are exactly **two** approval gates the user (or a future
supervising agent applying this policy) acts on:

1. **The plan.** After the planner emits a high-level plan and
   before any specialist runs.
2. **The tickets.** After the ticket-writer drafts tickets and
   before any are filed in the issue tracker.

Specialists in between elaborate detail at runtime without
intervening approval. Findings are not individually approved —
they flow through to the ticket-writer. The supervisor sees them
as evidence inside ticket drafts at gate 2.

This document is the policy the supervisor applies at each gate.
It is calibrated to be **light**: the high-level-plan-approval
shape (the user's chosen design) keeps the gates from becoming
choke points. If a gate's review is taking more than a few
minutes, the gate is too detailed — either the agents' output
got too fine-grained, or the supervisor is reviewing at the wrong
zoom level.

---

## Gate 1 — Plan approval

The planner emits a compact artifact: a table or short list of
test entries, each with category, precondition + action +
observable, specialist, risk addressed, and rough effort. The
supervisor checks the *shape* of the plan, not the details that
specialists will fill in.

### P1. The plan traces back to the user's named risk

Every entry traces to the risk the user named in their
free-form-plus-references input. An entry that doesn't trace —
"while we're at it, let's also check X" — is either dropped,
moved to a `scope-expansion-suggested` finding for a future plan,
or the user explicitly enlarges the risk scope.

The supervisor asks: "If this entry passes, is it because we
verified something the user was actually worried about?" If
no, drop it.

### P2. Each entry is specific enough to elaborate

The plan entry must name precondition + action + observable (see
`hints.md`). The specialist is expected to fill in the exact
selectors, curl headers, or test data — but the *meaning* of
the test must already be unambiguous. "Test the share dialog"
fails; "with `link_sharing_allow_external=false`, opening the
share dialog with an external email should produce a visible
error" passes.

The supervisor asks: "Could two different specialists run this
entry and produce findings that mean the same thing?" If no,
tighten the entry.

### P3. The category fits

Every entry has one category from the taxonomy
(`R | A | B | G | C | P | L`). If a single entry seems to fit
two, it's two entries. Categories matter because they shape
what the specialist's structured output looks like — a category
mismatch produces a finding the ticket-writer can't read
cleanly.

The supervisor pays extra attention to category-L entries:
perf entries that don't name a *kind* (latency, n_plus_one,
allocation, etc.) are too vague for the perf-investigator. Push
back if the entry just says "investigate perf" without naming
which kind of perf.

### P4. Effort estimate is honest

The plan estimates effort per entry (minutes / hours). If an
estimate looks low for the entry's scope — e.g. "2 minutes" for
a full auth-matrix exploration — the supervisor asks for a
revision. Optimistic estimates create plans the user under-scopes
and then over-runs at execution.

### P5. The specialist assignment is feasible

The plan names a specialist for each entry: `frontend-doctor`,
`api-tester`, `human-instructor`, or `perf-investigator`. The
supervisor verifies the specialist exists and has the tools for
the test. A category-P auth-matrix test that needs an admin
token but the `api-tester` has no admin token configured is
*infeasible* — either the plan acquires the token first as a
setup entry, or the test moves to `human-instructor` (where
the human can sign in as the admin and report back).

For category-L entries: a sustained-load or chaos test
assigned to `perf-investigator` is infeasible (the framework's
perf-investigator does single-endpoint sampling and code-level
investigation, not k6-style load). Either narrow the entry to
what the specialist actually does, or surface a gap.

### P6. The plan size is appropriate

The supervisor's last question: is this the right amount of
work? A 30-entry plan to verify one PR's worth of change is
probably over-scoped. A 1-entry plan for a feature with five
acceptance criteria is probably under-scoped. The user's
free-form input is the rough size signal; the plan should
match it.

### Dispositions at Gate 1

- **approve** — plan ships to specialists as-is.
- **approve-with-edits** — specific entries get tightened (the
  user names which); planner does not re-emit a whole plan.
- **request-revision** — the planner re-emits the whole plan
  addressing the named issues.
- **cancel** — the run doesn't happen. Used when the plan
  reveals the user's risk is mis-scoped and needs rethinking.

---

## Gate 2 — Ticket approval

The ticket-writer drafts tickets from the findings, one ticket
per bug (which may consolidate multiple findings sharing a root
cause). The supervisor reviews drafts before they're filed.
*Drafts are not auto-filed* — this is a hard rule, configured
in `config.toml::ticket_writer.auto_file = false` and enforced
by the ticket-writer's prompt.

### T1. Each ticket is self-contained

A reader who lands on the ticket months later, with no
conversation context, must be able to act on it. The
supervisor verifies:

- Repro steps are inline, numbered, complete.
- Expected vs observed is stated explicitly.
- Evidence is inline (curl output, log excerpts with x-request-id)
  or linked to durable storage (screenshots committed somewhere
  the issue tracker can render).
- Environment is captured: deploy stage, commit SHA if known,
  browser/version for UI bugs, user account or role for auth
  bugs.

A draft that says "see the plan above" or "from the conversation"
is rejected — the verification framework's plans and findings
are ephemeral by design (see `README.md`).

### T2. The title names the bug, not the symptom

"Share dialog returns 500" is a symptom; "Share dialog silently
drops external invites with no error feedback when external-
sharing is disabled" names the bug.

If the bug isn't yet identified — the finding is repro-confirmed
but the cause is unclear — the title names the precise
observation that proves the bug exists, not the surface symptom.

### T3. Severity matches user impact, not fix cost

The framework's severity ladder is in `hints.md`. The supervisor
re-checks severity against the *user impact*:

- Data loss / security / auth bypass → S1.
- User-facing broken flow → S2.
- User-facing degraded flow → S3.
- Latent / internal → S4.

A common failure mode is downgrading severity because the fix
looks small. The supervisor overrides — fix cost is the
developer's problem, not the bug's classification.

### T4. Roll-ups name the shared root cause

When a ticket consolidates multiple findings into one bug, the
ticket body must name the shared root cause it's claiming:
"This ticket consolidates findings F-2, F-5, F-7, which appear
to share a single root cause in the auth middleware's session
validation path." If the root cause is *guessed*, the ticket
says so honestly — overclaiming a root cause is worse than
filing one ticket per finding.

The supervisor asks: "If F-5 turns out to have a different
root cause than F-2 and F-7, will this ticket need to be
split?" If yes, either split now or weaken the root-cause
claim in the body.

### T5. No duplicates with existing open tickets

The ticket-writer is expected to search the issue tracker for
existing open issues that might already cover the bug
(`gh issue list --state open --search "..."`). The supervisor
verifies the search was done and the result is sensible.
Filing a duplicate is noise; missing the duplicate-check is
the bug.

### T6. No tickets for inconclusive findings

A finding with `result: inconclusive` does *not* produce a
ticket. It produces a follow-up: re-plan with whatever was
missing (admin token, browser access, infrastructure) so the
next run can complete the test. The supervisor verifies no
inconclusive findings have been promoted to tickets.

### T7. No tickets for noise

A finding that confirms behavior is correct is not a ticket.
A finding whose only surprise is incidental (a slow page load
during a CRDT convergence test) is probably not a ticket
either, unless the surprise crosses a severity threshold.
The supervisor asks for each draft: "Is there a real bug
here that someone should fix?" If no, the finding is closed
without a ticket.

### Dispositions at Gate 2

- **approve** — ticket is filed via `gh issue create`.
- **approve-with-edits** — the user names corrections; the
  ticket-writer applies them and re-presents.
- **reject** — the ticket doesn't ship. The finding is closed
  without a ticket; the user records the reason ("not a real
  bug," "duplicate of #N," "expected behavior per spec").
- **defer** — the ticket would be useful but is being held
  (e.g. the bug exists but is intentionally out of scope for
  the current release). The user names where the deferred
  list lives — typically a checklist comment on the parent
  feature ticket.

---

## What this gate-policy does NOT do

- **Approve specialists individually.** Specialists elaborate
  detail and run tests without per-step approval. That's
  the design of high-level-plan-approval.
- **Approve findings individually.** Findings flow from
  specialists to the ticket-writer without a supervisor pass.
  The supervisor sees findings inside ticket drafts at gate 2.
- **Approve scope expansions silently.** When a specialist
  surfaces a `scope-expansion-suggested` finding (something
  worth testing that wasn't in the approved plan), the
  supervisor decides at ticket-review time whether to add it
  to a future plan — *not* whether to expand the current run.
  The current run is over once tickets file.

---

## The escape hatch

If something arises that this checklist doesn't cover, the
supervisor records the decision in plain prose alongside the
disposition. Recurrent escapes are the input to the next
revision of this checklist — the framework gets better only
if the gaps it doesn't cover are written down.
