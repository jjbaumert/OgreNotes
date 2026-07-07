# Shared Hints — Rust patterns, anti-patterns, and the agents' shared vocabulary

This is the cookbook both the authoring agent and the refactoring
agent consult before flagging code. It exists because rules without
rationale don't survive contact with novel situations: when the agent
has to choose between two readings of the code, it should be able
to look up the *why* of a guideline and apply judgment, not pattern
match.

The agents may quote anchors from this document by section heading
(e.g. *boolean blindness*, *parse-don't-validate*) when explaining a
finding. Names matter because they make the agent's output
auditable: a reviewer can trace any finding back to a concrete
guideline here.

---

## Pattern names are means, not ends

Before any pattern from this document is named in a finding, the
finding must explain the concrete improvement to the actual code:
which line is harder to read now, which kind of change just got
easier or harder, which silent failure mode just opened or closed.

A finding that says "this is primitive obsession; introduce a
newtype" without explaining what bug or readability win the newtype
buys is rejected as ceremony, not improvement. Names compress
shared understanding; they don't replace argument.

This rule is the framework's most important guardrail against the
common LLM failure mode of suggesting a refactor for the aesthetic
of having suggested one.

---

## The reason-to-change test

This is the framework's mechanical applicability test for "single
purpose per unit." Apply it to a function, struct, module, or crate.

Ask: **what category of change request would touch this code?**
List the categories in plain language. If the list has more than one
item, the unit is doing more than one thing.

Worked examples:

- `id::new_id` → "we changed the ID encoding" — one category. ✓
- `AppState` → "we added a new repo," "we added a new background
  service," "we added a new feature flag." Three categories on a
  god-struct. **Acceptable here**: AppState's job is composition;
  every new capability hits it by definition. The reason-to-change
  test isn't a verdict, it's a question — the answer can be "yes,
  on purpose, and here's why."
- A route handler that authenticates, authorizes, parses input,
  calls a repo, reshapes the response, *and* fires a notification
  → five categories. The notification fanout is the easiest to
  extract; a domain change to "what we notify on" should not touch
  a route handler.

The agents apply this test before suggesting an extraction or a
split. If the answer is one category, leave the unit alone.

---

## The audience checklist

Code in this framework is read by at least five distinct audiences,
each at a natural zoom level. The agents check each finding against
all five before it ships.

- **Designer** asks: does this match what the design document says
  the system does? If a finding contradicts a design doc, the
  finding includes that observation as a separate item.
- **Developer** asks: if I ship a feature next week that touches
  this area, does this code make my job easier or harder?
- **Reviewer** asks: can I tell from a 200-line diff whether this
  change is correct, or do I need to read 800 lines of context?
- **Security engineer** asks: where does untrusted input enter and
  where does it become trusted? Are those boundaries explicit?
- **Agent** (a future review agent or an autonomous coding agent)
  asks: can I locate the thing I need to change without rebuilding
  the whole architecture in my head from scratch?

A change that helps one audience at the expense of another is a
finding worth flagging, but it's not automatically a regression —
the agents present the tradeoff and let the supervisor decide.

---

## Rust patterns to prefer

Each entry below is one or two sentences of when-and-why. They are
*defaults*, not laws — but a deviation from a default is a place to
explain why in code or a finding.

### Newtype on identifiers and units that have meaning

When a string ID always belongs to a specific kind of resource, wrap
it: `pub struct DocId(pub String)`. The compiler will then refuse to
pass a `UserId` where a `DocId` is expected. The cost is one line of
declaration plus a `Display`/`From` impl; the win is that the cost
of confusing two kinds of identifier in a parameter list drops to
zero.

**Greenfield default.** This is the right default for new projects.
Existing codebases that use raw `String` IDs may *grandfather* the
choice in their architecture doc's identifier-strategy slot; once
recorded, agents do not generate newtype-on-existing-IDs findings
against the project. New IDs introduced after the strategy is
recorded may newtype if the local benefit clears the cost-benefit
gate — at that point the change touches one or two new types, not
the whole codebase.

### Typestate for objects that progress through phases

When an object is meaningfully different at different points in its
lifecycle (`Connection<Disconnected>` vs `Connection<Authenticated>`
vs `Connection<Subscribed>`), encode the phase in a phantom type
parameter and only define the methods valid in that phase on the
typed impl block. The compiler then refuses calls in the wrong order.

### Parse, don't validate

At every trust boundary, convert untrusted bytes into typed values
exactly once, in one place. Downstream code receives the typed value
and is free to assume soundness. The opposite is a function that
takes `&str` and "validates" it inline at every call site —
inevitably one site forgets, and the bug is silent.

### RAII guards for paired begin/end operations

Anything with a setup and a teardown — a held lock, an in-flight
metric counter, a temporary file, a tracing span — should be a
struct with `Drop`. The compiler then guarantees teardown on every
exit path including panics. The opposite is a `defer`-style block
the developer has to remember.

### Sealed traits for stable extension points

When a trait is implementable inside the crate but should not be
extensible by external callers, seal it with a private supertrait.
This lets you add methods to the trait later without breaking
downstream code, because there can be no downstream impl.

### Extension traits for cross-crate ergonomics

When you want to add a method to a foreign type for ergonomics
(`my_repo.with_retry()`), use an extension trait imported at the
call site. This is the orphan-rule-respecting alternative to
wrapping the foreign type — it preserves interop with code that
expects the original.

### Interior mutability with discipline

`RefCell` and `Cell` are appropriate when you have a logically
single-threaded value with a borrow pattern Rust's borrow checker
can't prove is sound. They are inappropriate as a general escape
hatch. `Mutex`/`RwLock` are appropriate when contention is real;
they are inappropriate when shared state could be passed by
ownership instead. Per-resource locks (a `DashMap<DocId, Mutex>`)
are a useful pattern; one global lock around a `HashMap` rarely is.

### Builders, gated

A builder is justified when a struct has many optional fields with
non-obvious interactions. It is not justified to set three required
fields. The threshold question is: does a caller ever want to
configure a partial value and pass it around, or do they always
finalize at construction? If the latter, plain field-by-field
construction (or a `new(...)` with required args plus
`with_*` chaining methods on the struct itself) is enough.

### Phantom types, gated

A phantom type adds an invariant the compiler enforces at zero
runtime cost. It is appropriate when there is a real bug class to
prevent (mixing currencies, mixing sanitized vs raw HTML, mixing
authenticated vs anonymous user IDs). It is inappropriate as
ornamentation on a value that's already typed adequately.

### Error type design: typed at the boundary, opaque inside

Each crate exposes one `thiserror`-derived enum at its public API.
Variants are typed (`#[error("not found: {0}")] NotFound(String)`)
not opaque. The crate above maps incoming variants to its own enum
with a `From` impl concentrated in one file. `anyhow` belongs only
in the topmost binary's `main`, never in a library API. This pattern
is in active use in this project; see *worthwhile-refactor example*
below for what it looks like when the rule is followed.

### Constructors that fail return `Result`, those that can't return `Self`

A `new()` that performs I/O or validation should be `try_new() ->
Result<Self, _>`. A constructor that pure-builds a value returns
`Self`. The rule is that reading `let x = T::new(...)` should never
be ambiguous about whether it can fail.

---

## Patterns to scrutinize (gate conditions)

These are not anti-patterns — they have legitimate uses — but each
one carries a default suspicion that an agent should articulate
before letting it pass.

### Trait objects (`Box<dyn Trait>`, `Arc<dyn Trait>`)

Gate: are there at least two implementations *that exist today*, or
is there a concrete plan in this PR to add the second one? If the
answer is "we might want to swap it later," that's not a gate pass
— concrete generics are cheaper, faster, and more debuggable. This
project uses `Arc<dyn ClaudeMessages>` because tests pass a fake;
that's a gate pass.

### `async-trait`

Gate: is the trait genuinely public-API, used across crate
boundaries? Inside one crate, prefer impl-trait-in-trait or concrete
async fns. The runtime cost (`Box::pin` per call) is real enough
that adding it casually is a regression.

### `tokio::spawn` from inside a domain capability

Gate: is the spawned future a known background concern of this
capability (e.g. the email cap counter, the search indexer), with a
named owner? Or is it an inline fire-and-forget that escaped a
review? Inline `tokio::spawn` is a structural smell — it makes the
control flow hard to reason about, and panics in the spawned task
become silent.

### `Arc<Mutex<T>>` in application state

Gate: is contention measured, or assumed? In an HTTP server, most
"shared mutable state" is actually per-request and could be passed
by ownership. When `Arc<Mutex<T>>` is the answer, prefer
`DashMap<K, Arc<Mutex<V>>>` or per-key locking, because global
contention scales to one core.

### Generic functions with five+ type parameters

Gate: is each parameter actually different at different call sites?
A function taking `<T, U, V, W, X>` where every real call uses the
same five concrete types is theatrical genericity — collapse to
the concrete signature.

### Tracing spans inside hot loops

Gate: have you measured the per-call cost? L1 owns observability
*types*, every layer is free to use them — but a `tracing::info_span!`
inside a per-edit, per-message, per-cell, or per-frame loop turns
a "free" abstraction into measurable overhead, and the cost only
shows up under load. Default suspicion: any span construction
inside a loop body is a regression unless a benchmark says otherwise.
The cure is usually to move the span outward (one span around the
batch, not one per item) or to drop down to a counter increment.

---

## Named anti-patterns (the agents' shared vocabulary)

The agents use these names in findings. Each name is a one-line
diagnosis; the finding includes a sentence of code-specific
rationale alongside.

### Boolean blindness

A function takes one or more `bool` parameters whose meaning isn't
visible at the call site. `apply_security_headers(router, true, csp)`
— what is `true`? Cure: replace with an enum (`enum DeployMode {
Dev, Prod }`) or split the function. Always cheaper at author time.

### Primitive obsession

A domain concept is encoded as a raw `String`, `i64`, or `u32`
everywhere, and the resulting confusion is paid for at every call
site. Cure: a newtype. The newtype's cost is one declaration; the
benefit is type-checked correctness for the rest of the codebase's
life.

### Stringly-typed

A closed set of values is encoded as strings (`"admin"`, `"member"`,
`"guest"`) instead of an enum. Inevitably someone writes `"admins"`
in one place. Cure: enum. The wire format remains a string via
`#[serde(rename_all = "lowercase")]`.

### Shotgun parameter

A function takes a long list of unrelated parameters of similar
types; the call site looks like
`do_thing(a, b, c, d, e, f, g, h)`. Reorder by accident and the
program compiles but means something else. Cure: introduce a struct
that names each role, or split the function into smaller ones whose
parameter lists are unambiguous.

### Leaky abstraction

A type that claims to hide a substrate exposes the substrate's
shape — `Repo::get_raw_dynamo_item(...)` defeats `Repo`'s purpose.
Cure: either remove the leaking method or rename the type so it no
longer claims to hide anything.

### God-struct (suspicion, not always a bug)

One struct holds every dependency in the application. `AppState` in
this project is a god-struct *on purpose* — composition is its
job. The gate is whether each field has a clear separate reason to
exist. When a god-struct is genuinely a bug, the cure is usually
splitting along trust boundaries, not breaking it up arbitrarily.

### Stamp coupling

A function takes a large struct only to read one or two of its
fields. The function ends up coupled to all the struct's other
fields by source-level dependency, even though it doesn't use them.
Cure: take the fields the function actually needs.

### Re-implementation drift

The same logic exists in two places, written separately, intended
to agree. The drift is silent until the day they don't. Cure:
either share the source (a common module, a shared crate) or
encode the agreement as a test that runs in CI. This project does
the latter for the cross-schema consistency between the backend
collab schema and the frontend editor schema; the test is the
authoritative place where drift surfaces. The architecture doc's
*Cross-target schema agreement* section is the canonical statement
of this pattern — invoke it when flagging a new place where two
schemas need to agree across a build-target boundary.

### Premature trait

A trait introduced for one implementation, justified by "we might
want to swap this later." A second implementation rarely arrives;
when it does, the trait shape almost always needs revision anyway.
Cure: delete the trait, use the concrete type. Add the trait when
the second implementation is concrete.

### Mock-shaped-hole

A piece of code's only purpose is to be replaced by a mock in
tests. The production code has structure (a trait, a level of
indirection) it would not have without the test. Cure: prefer
test seams that don't require production-code accommodation —
fakes that share a real trait already public-API; injecting clock
or RNG only when their behavior is the unit under test.

---

## Bias to scrutinize: additive over subtractive

LLM reviewers — and humans — bias toward adding code: a comment to
explain a confusing block, a wrapper to encapsulate a leak, a
helper to remove a duplicate. This bias is wrong more often than
not. Before suggesting an addition, the agents prefer:

- **Splitting the file** that contains the confusing block, so the
  block sits next to its actual neighbors.
- **Removing the leaky method** rather than wrapping it.
- **Inlining** the duplicated code if it's two short places that
  rarely change, instead of extracting a third.

Three short similar lines is fine. Three medium-length similar
lines that drift independently is a problem. The agents articulate
which case they think they're in.

---

## Framework-conventions slot

This section is filled in per project. For OgreNotes (this calibration
model):

- **Per-project knobs live in `framework/config.toml`**, read by
  both agents and the supervisor before they act. The schema is
  documented in `framework/config.toml.example`; current values
  for this project (supervisor kind, identifier strategy, etc.)
  are committed in `framework/config.toml`.
- **Error type at the HTTP boundary is `ApiError`**, defined in
  `crates/api/src/error.rs`. Every domain crate's error has a
  `From<DomainError> for ApiError` impl in that file. Domain crates
  do not depend on `ApiError`.
- **Storage models live in `crates/storage/src/models/<resource>.rs`**;
  access patterns in `crates/storage/src/repo/<resource>_repo.rs`.
  A new resource adds two files; a new method on a resource adds
  one method to its repo.
- **Route handlers live in `crates/api/src/routes/<resource>.rs`**.
  Each file exports one `pub fn router() -> Router<AppState>`.
  Routes nest in `crates/api/src/routes/mod.rs::api_router`.
- **DynamoDB single-table design**: PK encodes the primary entity
  family (`DOC#<id>`, `USER#<id>`), SK encodes the row kind within
  it (`METADATA`, `MEMBER#<user_id>`, `UPDATE#<clock>`). The PK/SK
  formatters live as `pk()`/`sk()` methods on each model struct.
- **CRDT updates are bytes** at every layer except inside the
  `collab` crate. The edge passes them through opaquely; the repo
  stores them as `Vec<u8>`; only `collab::document::OgreDoc` ever
  decodes one. This is the parse-don't-validate boundary for CRDT
  state.
- **Frontend mirror convention**: each backend route group
  `routes/<resource>.rs` has a counterpart
  `frontend/src/api/<resource>.rs` exposing client functions of
  the same names. This is intentional; refactoring a backend route
  surface should propagate to the frontend mirror as a separate
  finding.

For other projects on this framework, this section is replaced with
the project's own conventions; the section above (universal hints)
is unchanged.

---

## Before / after examples (calibration)

These three concrete examples calibrate the agents' severity ladder
against this project. They are intentionally drawn from realistic
review situations, including one that the agents should *decline* to
suggest. Examples are illustrative — the goal is the level of
specificity the agents should match in their findings.

### Author-time fix (cheap to apply, big legibility win)

**Situation.** A new route handler is added in
`crates/api/src/routes/admin.rs` to list users with an optional
"include deactivated" toggle:

```rust
pub async fn list_users(
    State(state): State<AppState>,
    Query(query): Query<ListUsersQuery>,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    let users = state.user_repo.list_all(query.include_deactivated).await?;
    // ...
}
```

**Finding (author-time).** The boolean parameter on
`UserRepo::list_all` is *boolean blindness* (see anti-pattern). At
every call site, `list_all(true)` requires the reader to remember
which boolean state means what. Cure at author time:

```rust
pub enum DeactivatedFilter { Include, Exclude }

impl UserRepo {
    pub async fn list_all(&self, filter: DeactivatedFilter) -> ... { ... }
}
```

**Why this is a fix and not a refactor.** It costs three lines and
one rename at the only existing call site. The same fix six months
from now means tracking down every call, recompiling, and re-testing.
Caught at author time, it's free.

### Refactor-worthwhile (multi-call-site win, behavior-preserving)

**Situation.** Several route handlers in
`crates/api/src/routes/documents.rs` repeat a pattern: fetch a
document, check the caller's access, return 404 or 403, then act.
The pattern was copied across six handlers and has drifted —
some treat trashed-doc-for-non-owner as 404 (correct), one as 403
(leaks existence).

**Finding (refactor-worthwhile).** Extract the access-check pattern
into one helper that returns a typed `AccessDecision` enum, separate
from the HTTP error type. The HTTP layer maps the decision to an
`ApiError` in one place; the decision itself is unit-testable
without a request context.

```rust
pub(crate) enum AccessDecision {
    Allowed,
    Trashed,    // owner-on-trash: 200 read-only
    NotFound,   // hide existence
    Forbidden,  // exists, you can't have it
}

async fn decide_access(...) -> Result<(DocumentMeta, AccessDecision), RepoError> { ... }
```

**Why this is worthwhile.** The fix removes a real bug (the existence
leak) by making the decision a closed set the compiler exhausts.
Tests can drive each variant directly. Behavior under correct
inputs is preserved; the bug under incorrect inputs is fixed.
**Patch-with-flag**: this is a behavior change at one call site, so
the agent flags it as a separate "behavior-changing" finding from
the rest of the refactor.

(This refactor was actually performed in the project; the
`AccessDecision` enum lives at
`crates/api/src/routes/documents.rs:204`. The agents can cite it
as a positive precedent.)

### Wouldn't bother (the agent should *decline* to suggest this)

**Situation.** Every storage model in `crates/storage/src/models/`
has a hand-rolled `pk()` method:

```rust
impl DocumentMeta { pub fn pk(&self) -> String { format!("DOC#{}", self.doc_id) } }
impl DocMember   { pub fn pk(&self) -> String { format!("DOC#{}", self.doc_id) } }
impl Folder      { pub fn pk(&self) -> String { format!("FOLDER#{}", self.folder_id) } }
// ...ten more
```

A naive refactor would propose a `Pk` trait or a generic
`format_pk(prefix: &str, id: &str)` helper to consolidate.

**Finding (wouldn't bother — the agent declines to suggest the
refactor).**

The duplication is *intentional and load-bearing*. Each `pk()` is
co-located with the model it serves; a reader looking at
`DocumentMeta` sees its key shape on the next line. A trait or
helper would scatter the key encoding across an abstraction that the
reader has to chase. The cost-benefit gate fails: there is no real
defect (no two models disagree about format), the lines are short
enough that drift is visually obvious in code review, and the
proposed abstraction would *worsen* legibility for the
storage-engineer audience by hiding the key shape.

**The agent's output here is a non-suggestion.** The refactoring
agent's report explicitly notes it considered and rejected the
extraction, with the above reasoning. Saying nothing is worse: a
future agent will reach for the same idea and either suggest it or
also reject it without recording the reasoning.

This example is the framework's most important calibration:
**not all duplication is a bug.** Three short similar lines is
fine.
