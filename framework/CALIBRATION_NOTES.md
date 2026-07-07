# Calibration Notes — survey, classification, mechanical checks, open questions

This document is the framework's reasoning trace, separate from the
deliverables. The four other documents in `framework/` (architecture,
hints, the two agent prompts, supervisor review) are the artifacts
the agents and reviewers will use; this one is for the human
designing the framework to audit how those artifacts were produced
and where the open decisions are.

---

## Survey: how OgreNotes layers in practice

### The dependency graph

The Cargo workspace at `Cargo.toml` defines eight library crates
plus one binary crate (`api`):

```
common  ────────────────────────────────► (no deps on workspace crates)
storage ────► common
auth    ────► common, storage
collab  ────► common                   (+ storage, gated behind `replay` feature)
search  ────► (no workspace deps; uses tantivy directly)
embeddings ► (no workspace deps; uses qdrant + bedrock)
notify  ────► common, storage
api     ────► common, storage, auth, collab, search, embeddings, notify
frontend (excluded from workspace) ───► (no workspace deps; mirrors api over HTTP)
```

The graph is a strict tree rooted at `common`. No cycles. No
peer-to-peer edges between domain crates (`auth` does not depend on
`notify`; `collab` does not depend on `search`). This is the
structural property the framework's L3 layer rule encodes — it is
already true here.

The one nuance is `collab`'s `replay` feature, which is the
opt-in path that pulls in `storage`, `aws-config`, and AWS SDKs.
Without the feature, `collab` is hermetic. With it, `collab`
becomes a special-purpose binary (the replay tool). The feature
gate is doing the work of "this capability is normally L3 but
becomes L4 when used as a binary"; that's a useful pattern worth
naming.

### Internal crate layering

Inside `crates/storage`:

- `models/<resource>.rs` — wire shapes; one file per resource family
  (`document`, `folder`, `user`, `notification`, `session`,
  `snapshot`, `thread`, `workspace`, `activity`, `admin_audit`).
- `repo/<resource>_repo.rs` — access patterns; one file per
  resource family.
- `dynamo.rs` and `s3.rs` — thin wrappers over AWS SDK clients.
- `models/mod.rs` and `repo/mod.rs` — re-export plus the cross-cutting
  enums (`AccessLevel`, `DocType`, `ChildType`, `NotifLevel`,
  `LinkSharingMode`, `InheritMode`, `WorkspaceRole`).

This split is clean and the framework should generalize the
**pattern** (separate wire-shapes from access patterns) without
mandating the **names** (other projects might use `entities` +
`repositories`, `schema` + `dao`, `records` + `queries`).

Inside `crates/api`:

- `routes/<resource>.rs` — one file per resource family, each
  exporting a `pub fn router() -> Router<AppState>`.
- `middleware/<concern>.rs` — auth, rate limit, activity, metrics.
- `state.rs` — `AppState` god-struct holding all repos and
  capability handles.
- `error.rs` — boundary `ApiError` with `IntoResponse` and `From<*>`
  impls for every domain error.
- Top-level helper modules (`compaction`, `digest`, `edit_activity`,
  `claude`, `observability`) for orchestration that needs an HTTP
  context but isn't HTTP itself.

Inside `frontend/src`:

- `api/<resource>.rs` — HTTP client mirror of backend route groups.
- `pages/<page>.rs` — Leptos page components (home, document,
  login, auth_complete).
- `components/<component>.rs` — reusable Leptos components.
- `editor/` — rich-text editor (model, transform, view, schema,
  yrs_bridge).
- `spreadsheet/` — formula engine.

### Where the project exemplifies the principles

- **Strategic layering.** Cargo dependency graph is a tree, with
  domain crates mutually independent. The architecture template
  describes this as the universal default.
- **Single purpose per unit.** Each route file is one resource
  family; each repo is one resource family; each middleware is one
  concern. Reason-to-change test passes for nearly every crate.
- **Multi-audience legibility.** Storage models are co-located with
  their PK/SK encoding so a database engineer reading `models/
  document.rs` sees the table layout next to the struct (this is
  a positive — see hint-doc *Wouldn't bother* example).
- **Rust-native patterns.** Heavy use of `thiserror` enums per
  crate, `From` conversions concentrated in `api/error.rs`,
  redacting `Debug` for `AppConfig`, fail-fast env validation,
  closed-set enums (`AccessLevel`, `DocType`) instead of strings.
- **Bias toward separation.** `AccessDecision` enum at
  `crates/api/src/routes/documents.rs:204` decouples a permission
  decision from the HTTP error type so the decision is unit-
  testable in isolation. This is exactly the kind of subtractive-
  in-spirit refactor the framework rewards (the HTTP error type is
  smaller, the decision logic is smaller, even though a new enum
  was added — it removed implicit complexity from the call sites).

### Where the project diverges from the principles

These are the places I have to push back against either the
project's actual code or the principles as stated, and explain
which way I went.

- **Identifiers are `String`, not newtyped.** Every `doc_id`,
  `user_id`, `folder_id` in this codebase is a raw `String`. The
  hint doc lists newtypes on identifiers as a preferred pattern.
  The codebase has chosen not to apply it. Resolved (Q1) by
  introducing the grandfather clause: the architecture doc's
  identifier-strategy slot records `string-grandfathered`, the
  agents read it, and findings against existing usage are
  silenced. New IDs may newtype where locally beneficial.
- **`AppState` is a god-struct.** The hint doc's *god-struct*
  anti-pattern entry explicitly notes that `AppState` is god-struct
  *on purpose* — composition is its job. This is a place where
  the principles needed nuance: "single purpose per unit" can't
  be applied dogmatically to a composition root. I encoded the
  nuance in the hint doc's anti-pattern entry directly.
- **Inline `tokio::spawn` for fire-and-forget side effects.** The
  `documents.rs` create handler does
  `tokio::spawn(async move { … SnapshotRepo::create(&snap) … })`
  inline (with a comment justifying it as non-load-bearing). The
  hint-doc *Patterns to scrutinize* section gates inline
  `tokio::spawn` from inside a domain capability; here the spawn
  is in the route handler (L4), not a domain capability, but the
  same legibility problem applies — a panic in the spawned task is
  silent. I kept the gate as written and noted this in open
  questions.
- **Non-trivial logic in route handlers.** Some route handlers in
  `routes/documents.rs` are well over 200 lines and do real work
  (CSP construction, schema duality validation, complex access
  logic). The principles would suggest extracting the logic to a
  domain capability. The project's judgment is that this glue
  layer ("what L4 needs to do for the edge that isn't a domain
  capability") legitimately holds non-trivial code, and the
  separate top-level api modules (`compaction`, `digest`,
  `edit_activity`) are where that goes when it gets large enough.
  The architecture document encodes this view in the L4 description
  ("plus per-domain helper modules at the same level… that
  orchestrate domain capabilities for the edge's needs").

---

## Generalizability classification

### (a) Universal — bake into the framework

These are stable across any Rust full-stack project on a similar
stack and the framework asserts them as defaults, not opinions:

- Cargo workspace with multiple library crates plus a binary crate.
- Strict-tree dependency graph; no cycles; lower layers do not
  depend on higher.
- `thiserror` enum per crate at the public API; `anyhow` only at
  the topmost binary's `main`.
- `From` conversions for cross-crate error mapping concentrated in
  one file at the boundary that consumes them.
- A `common` (or `core`, or `foundation`) crate at the bottom
  holding ID generation, timestamps, errors, configuration, and
  cross-cutting metrics.
- Fail-fast configuration validation at startup, with a custom
  redacting `Debug` impl for any config struct that holds secrets.
- Closed-set enums for status, role, and level fields, serialized
  via `#[serde(rename_all = "...")]` to wire-format strings.
- Co-located `#[cfg(test)]` modules for unit tests; `tests/`
  directory for integration tests.
- One protocol crate at the top (`api`, `edge`, `gateway` — name
  varies) that owns request parsing, response shaping, error
  mapping, and the application-state composition.

### (b) Configurable defaults — defaults the framework offers, projects override

These are reasonable in most cases but a project might legitimately
choose otherwise. The framework's templates ship them as defaults
with explicit override knobs:

- A composition god-struct (`AppState` in this project) holding all
  repos and capability handles, vs. a trait-based DI container.
  Default god-struct.
- `models/` + `repo/` split inside the persistence crate, vs. per-
  resource feature crates. Default models + repo.
- `axum` + `tokio` for the HTTP edge. Other projects might use
  `actix-web`, `tonic` (gRPC), or `poem`. Default axum.
- Domain capabilities as separate crates (`auth`, `notify`, etc.)
  vs. one `domain` crate with submodules. Default to separate
  crates above some complexity threshold (~five domain modules
  or ~2k lines per module).
- Frontend in the same monorepo as the backend, sharing a
  workspace root. Default same-repo, configurable to separate.
- Single-binary deployment vs. multi-binary (worker + API).
  Default single-binary.
- `proptest` for invariant tests. Default present, optional.
- Identifier types: framework default is newtype-on-identifiers;
  projects with strong reasons to use raw `String` document the
  decision in their architecture doc's project conventions slot
  (this project does not yet — see Section IV).

### (c) Project-specific — must NOT bake into the framework

These are OgreNotes-particular and the framework actively keeps
them out of its templates:

- `yrs` CRDT for collaborative editing.
- DynamoDB single-table design with structured PK/SK encoding.
- AWS SDK ecosystem (DynamoDB, S3, Bedrock, SES SMTP, CloudWatch).
- Leptos / Trunk / WASM frontend.
- The "Phase 1 / Phase 2 / Phase 3" milestone structure.
- `dev-login` feature flag for tests.
- The schema duality between backend `collab::schema` and the
  frontend editor schema.
- "Workspace > Folder > Document > Thread" hierarchy.
- The specific AWS deployment ergonomics (ECS Fargate, ALB, ACM,
  Route 53) reflected in `scripts/aws-test-deploy.sh` and the
  `.claude/agents/aws-*.md` runbooks.

---

## Mechanical checks the agents should *not* do

The framework reserves agent attention for judgment calls. The
following are mechanical and belong in lint, build, or pre-commit
infrastructure.

### Lints (clippy / custom lints / cargo-deny)

- **Forbidden imports per crate.** A `cargo-deny` `[deps.bans]`
  policy can express "the `notify` crate must not import
  `collab`," "the `common` crate must not import `axum`."
  cargo-deny can do this today. The framework documents the
  expected policy file shape in a future addition.
- **Layer-direction enforcement.** Adding the same constraint at
  build time makes layer violations a compile error rather than
  a review-time finding. Strongly preferred; the agents should
  never see a layer violation that the build allowed.
- **No `unwrap` / `expect` outside `#[cfg(test)]` and `main`.**
  Standard clippy lints (`clippy::unwrap_used`,
  `clippy::expect_used`) configured at workspace level.
- **`pub` items used only within their crate** — `dead_code` and
  the `unreachable_pub` lint catch most cases.
- **Naming conventions.** Clippy's `module_name_repetitions`,
  `enum_variant_names`, etc. cover most naming-as-role checks.
- **Standard formatting.** `cargo fmt` is non-negotiable in CI.
- **`anyhow::Error` in public API.** A custom lint or grep-based
  pre-commit can catch `pub fn …() -> anyhow::Result<…>` outside
  the binary `main` files.

### Build scripts / pre-commit hooks

- **Schema-duality drift.** This project has a backend `collab`
  schema and a frontend editor schema that must agree on tag
  names. There's already a cross-schema consistency test on the
  backend side. That kind of drift detection is mechanical (a
  test) and the framework recommends it as a generalizable
  pattern: any place two schemas must agree, encode the agreement
  as a test that runs in CI.
- **Cargo workspace member listing.** A pre-commit hook that
  fails if `crates/<name>/` exists but isn't listed in
  `Cargo.toml::workspace.members` (or vice versa) catches a
  common copy-paste error.
- **Public API surface diff.** `cargo public-api` produces a
  human-readable diff of pub items. CI can fail on undocumented
  breaking changes; the supervisor checklist Section A3 already
  asks for this signal.

### What remains for agent attention

After the mechanical checks above run, the agent's domain is:

- Whether a layer violation that does compile (e.g. via a
  re-export) is a real intent.
- Whether a unit's purpose has drifted away from its name.
- Whether an abstraction is justified by real complexity or is
  premature / leaky.
- Whether trust-boundary slippage exists at points the type
  system can't see (a `String` of HTML treated as if it were
  sanitized).
- Whether an addition would be better as a removal or a split.
- Whether a finding is calibrated against this project's actual
  conventions or imported from elsewhere.

This is judgment work, and that's where the framework's hint doc
and architecture doc live.

---

## Open questions — resolved

The first pass of this document raised nine open questions that
needed user resolution before the framework could be applied to a
second project. Each was resolved on 2026-05-09. The original
question framing is preserved below as historical context; each
entry now ends with a **Resolved** paragraph stating the decision
and the docs it touched.

### 1. Newtype-on-identifiers as default vs. project reality

The hints doc lists newtype-on-identifiers as the first preferred
pattern; the project uses raw `String` everywhere. Three readings
were on the table: rule is wrong, project is in tech debt, or
introduce a grandfather clause.

**Resolved (Q1 — grandfather clause).** Architecture doc gets an
**Identifier strategy** slot in each project's conventions
section; OgreNotes records `raw String (legacy; new code may
newtype if locally beneficial)`. The hints doc's
newtype-on-identifiers entry now explicitly notes that grandfathered
projects do not generate findings against existing usage. The
configuration is project-readable via
`identifier_strategy = "string-grandfathered"` in
`framework/config.toml`. Touched: `architecture.md`, `hints.md`,
`config.toml.example`, `agents/authoring.md`, `agents/refactoring.md`.

### 2. How much of the spreadsheet engine is L3?

The frontend's `spreadsheet/` and `editor/` directories are
non-trivial L3-equivalent compute living inside the L5 client.
Two clean answers: internal layering inside the client, or promote
shared compute to a workspace L3 crate.

**Resolved (Q2 — internal layering with escape hatch).** The
architecture doc's L5 description now acknowledges that a
non-trivial client crate layers internally, that the framework's
layer rules apply recursively inside it, and that promotion to a
workspace L3 crate is the recommended cure when the same compute
is needed server-side. The new `framework/hints-frontend.md`
gives concrete guidance on the recursive case. Touched:
`architecture.md`, `hints-frontend.md` (new).

### 3. The schema-duality pattern — should it be a framework-level recommendation?

The cross-schema consistency test between backend collab and
frontend editor is the project's cleverest architectural move,
but the first pass left it as a passing remark in the
*Re-implementation drift* anti-pattern entry rather than promoting
it to a recommendation.

**Resolved (Q3 — promote).** Architecture doc has a new
**Cross-target schema agreement** section stating the pattern
explicitly: any place two schemas must agree across a build-target
boundary, encode the agreement as a CI test in the canonical-
schema crate. The hints doc's *Re-implementation drift* entry
cross-references it. Touched: `architecture.md`, `hints.md`.

### 4. What does the refactoring agent do about test coverage gaps?

The refactoring agent's silence on adding tests for currently-
untested behavior left a hole — coverage gaps surfaced as
findings without a clear path to resolution. Forbidding tests
entirely was safe but slow; allowing them freely risked silent
specification.

**Resolved (Q4 — allow under property-only gate).** The
refactoring agent may produce `test-coverage-gap` findings when
`agents.may_add_test_coverage_gap_tests = true` in
`framework/config.toml`. Each test must assert a property
(round-trip, idempotence, monotonicity, no-crash-on-input) — not
a specific output value — and must come with a one-paragraph
high-confidence claim that the project intends to preserve the
property. Specific-output tests are forbidden because they freeze
incidental behavior. Supervisor test A9 enforces the property-
only constraint at review time. Touched: `agents/refactoring.md`,
`supervisor-review.md`, `config.toml.example`.

### 5. Breaking changes to public APIs across crate boundaries

The first pass treated public-API changes as out-of-scope for
agent patches, which made routine refactors high-friction. The
mitigation candidate was a deprecation-shim pattern: add the new
signature alongside the old, mark the old `#[deprecated]`,
propose a removal date.

**Resolved (Q5 — allow deprecation-shim patches).** Both agent
prompts now allow a `breaking-api-deprecation-shim` finding kind,
gated per project by
`agents.may_propose_breaking_changes` in `framework/config.toml`.
Supervisor test A10 enforces that the shim patch keeps the old
signature available — removal is a separate change after the
documented date, under explicit human approval. Touched:
`agents/authoring.md`, `agents/refactoring.md`,
`supervisor-review.md`, `config.toml.example`.

### 6. Per-project agent configuration mechanism

The supervisor doc's Section D abstractly described per-project
config without picking a file format. Three candidates:
`framework/config.toml`, per-agent frontmatter, or reusing
`.claude/settings.json`.

**Resolved (Q6 — `framework/config.toml`).** Single TOML file at
the project root, read by both agents and the supervisor before
they act. The schema is documented in
`framework/config.toml.example`, which doubles as a copy-paste
seed for new projects. Missing knobs fall back to the most-
restrictive default and emit an `open-question` finding. Touched:
`supervisor-review.md` (Section D rewrite), `config.toml.example`
(new), all four agent/hint docs reference the file.

### 7. Spec-amendment workflow

The framework needed an answer for how its own docs evolve. The
current Section B handling (proposed-spec-update findings flow
through human review) is sufficient for one project; the question
was whether to codify versioned amendments / change log up front.

**Resolved (Q7 — defer).** YAGNI applies. The current Section B
handling stands. The actual shape of an amendment workflow will
be clearer once a second project has adopted the framework and
generated friction. No doc edits.

### 8. Where does observability fit in the layer taxonomy?

`tracing` calls and metrics are pervasive; the first pass
mentioned metrics in L1 but was silent on the broader pattern.
The clean answer: L1 owns observability *types*, L4 owns
*setup*, every layer uses it. The question was whether to make
this explicit.

**Resolved (Q8 — make explicit).** L1 description in the
architecture doc now includes an **Observability ownership**
paragraph stating the rule. The hints doc grew a new *Tracing
spans inside hot loops* entry under *Patterns to scrutinize*,
gating loop-internal span construction on a measurement.
Touched: `architecture.md`, `hints.md`.

### 9. The frontend's relationship to the framework

The first pass handled the frontend implicitly — the agent
adapts, the framework offers no frontend-specific guidance.
The question was whether to ship a separate frontend hint set.

**Resolved (Q9 — ship a frontend hint set).**
`framework/hints-frontend.md` now exists as a conditional
supplement: agents read it *only* when the diff touches a client
crate. It covers internal client layering, reactive primitives
discipline (coarse re-renders, opaque effects, signals across
`await`), `web-sys` surface gating, contenteditable discipline,
WASM bundle budget, and cross-target schema mirroring. Backend
findings do not cite it. Both agent prompts now include it in
their conditional read-first list. Touched: `hints-frontend.md`
(new), `agents/authoring.md`, `agents/refactoring.md`.

---

## Recommended sequence for adoption

If the user wants to apply this framework to a second project, I'd
recommend the order:

1. Fill in the architecture-doc template for that project. Stop
   when you can't fit a fact in a slot — that's the friction
   you'll have to resolve before agents can be useful.
2. Fill in the hint-doc framework-conventions slot.
3. Copy `framework/config.toml.example` to `framework/config.toml`
   and set the per-project knobs (supervisor kind, identifier
   strategy, breaking-change permission, test-coverage-gap
   permission).
4. Configure the mechanical lint/build checks (cargo-deny,
   clippy levels, public-API diff, formatting).
5. Re-read the resolutions in Section IV — the decisions encoded
   there are this-framework defaults, but a new project may need
   to revisit any that don't fit. Record any deviation in the
   project-conventions slot.
6. Run the authoring agent on the next PR. Read the findings as
   a calibration test: are they specific, gated, and grounded in
   the doc anchors? If not, refine the doc before iterating.
7. Run the refactoring agent on a chunk of legacy code. Watch
   especially for the `considered-and-declined` section: that's
   the framework's most useful artifact, and an empty one
   suggests the agent is biased toward suggesting.

The framework will only get better if the supervisor's recurrent
escalations are written down and folded back into the docs. Plan
for at least one revision pass after the second project's first
month.
