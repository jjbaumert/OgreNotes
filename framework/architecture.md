# Architecture — {{ project_name }}

This document is the layering contract for {{ project_name }}. It is the
reference both review agents (authoring, refactoring) consult before
flagging a layer violation, and it is the single place a human goes to
understand how the codebase is meant to fit together.

It is **descriptive of intent, not of current state**. Where the code
diverges from this document, the divergence is a finding for review,
not a license to update the document. Doc updates are a separate,
deliberate change with their own rationale.

## How to read this template

The framework defines five layers. Each layer has:

- **A one-sentence purpose.** If the purpose needs a paragraph, the
  layer is doing too much; split it before merging.
- **Forbidden knowledge.** A short list of things code in this layer
  must not import, name, or reason about. This is what review agents
  grep for first.
- **What it does know.** The intentional inputs and outputs.
- **Trust posture.** Whether values entering the layer are assumed
  validated, tainted, or unknown.

Dependency direction is strictly downward, enforced by the Cargo
dependency graph. A crate at layer N may depend on layers `< N`; a
crate at layer N may not depend on a crate at layer `≥ N+1`.

If a fact about your project doesn't fit one of these slots, that's
the interesting thing — write down where it landed and why, in the
project section at the bottom.

---

## Layer taxonomy (universal across projects)

### L1 — Foundation

**Purpose.** Pure types and helpers that any other crate in the
workspace can lean on without inheriting an opinion about persistence,
networking, or domain.

**Forbidden knowledge.** Database SDKs, HTTP frameworks, the domain
vocabulary (no `Document`, `User`, `Order` types here — those belong
upstairs). No `tokio::spawn` for fire-and-forget background work; this
layer hands back values, it does not run tasks.

**What it does know.** ID generation, timestamps, the workspace error
taxonomy that crates above will compose, configuration loaded from
the environment, basic metrics counters and histograms. Anything that
would otherwise be copy-pasted into three crates lives here.

**Observability ownership.** L1 owns observability *types* — counter
handles, histogram handles, span constructors — so every layer can
emit consistently. *Setup* of the subscriber and exporter is L4's
job (see L4 description). Observability is a cross-cutting concern,
not a layered one: every layer uses it, and the layer rules say
nothing about which layer "owns" a metric.

**Trust posture.** Inputs are assumed already validated by the caller;
this layer's job is to produce well-formed values, not to gatekeep.

### L2 — Persistence

**Purpose.** Translate domain values to and from the durable store,
and nothing else.

**Forbidden knowledge.** HTTP types, request handlers, the protocol
the edge happens to speak. No reaching up to ask "is the user
authenticated" — that's an edge concern, decided before a repo call
is made. The persistence layer must not know the difference between
a write that originated from a user, a background worker, or a test.

**What it does know.** The schema of the durable store (table names,
key encodings, blob layouts), the SDK that talks to it, retries and
conditional writes, and a single error type that the layer above maps
to its own.

**Trust posture.** Inputs are assumed already authorized; the layer
validates *shape* (required fields, encodings) and surfaces a typed
error if violated, but it does not re-check permissions.

### L3 — Domain capabilities

**Purpose.** Self-contained verticals that own a business capability
end-to-end below the protocol — auth, collaboration, notifications,
search. One capability per crate above some complexity threshold;
otherwise nested as a module under a single domain crate.

**Forbidden knowledge.** The HTTP framework, the WebSocket protocol,
each other's internals (capabilities are peers, not a stack —
`notify` does not import from `collab`; if they need to talk, they
talk through the edge or through L1 types). The shape of the client
that will eventually consume the capability.

**What it does know.** Its own domain rules and invariants, the
persistence repos it needs, the L1 types it composes, and one
domain-specific error type that the edge maps. Background tasks
*are* allowed at this layer when they belong to the capability —
the email cap counter, the idle compaction loop, the search indexer
— but they are owned and named by the capability that spawns them.

**Trust posture.** Mixed. Capabilities receive both already-validated
inputs from the edge and externally-sourced values (e.g. CRDT update
bytes, OAuth callbacks). Each capability documents which is which at
its public-API boundary.

### L4 — Edge

**Purpose.** Speak a wire protocol (HTTP, WebSocket, gRPC, queue
consumer) and compose the domain capabilities to serve it.

**Forbidden knowledge.** Storage internals (no direct DynamoDB, S3,
or SQL calls — go through a repo), CRDT internals (go through the
collab capability), client-side rendering or layout. The edge is the
only layer that knows the protocol; it is also the only layer that
knows about *all* the capabilities.

**What it does know.** Routing, middleware, request parsing,
response shaping, the boundary error type that maps every domain
error to a status code, the application state struct that holds the
repos and capability handles, the input validation that turns
untrusted bytes into typed values the domain can accept.

**Observability setup.** L4 wires up the tracing subscriber, the
metrics exporter, and the log format at process startup, then gets
out of the way. The *types* every layer uses come from L1; only L4
configures where they end up (stdout, OTLP, CloudWatch, etc.). The
project conventions slot below records the file that holds the
setup (e.g. `crates/api/src/observability.rs` in this project).

**Trust posture.** The edge receives **untrusted** input. It is
responsible for parse-don't-validate at the entry, for authentication
and authorization checks before a domain call, and for sanitizing
output. Below this layer, values are typed and assumed sound.

### L5 — Client

**Purpose.** Render the user-facing experience and call the edge.

**Forbidden knowledge.** Backend internals (it must not import
backend crate types directly), database SDKs, the protocol of *other*
edges it doesn't speak to, secret material that should only live
server-side.

**What it does know.** Its own UI framework, an HTTP/WS client
mirroring the edge's public surface, presentation logic, and a small
amount of duplicated wire-format types (the shape symmetry is
deliberate; sharing types via a third crate is a configurable choice
in projects where the client and edge are both Rust).

**Internal client layering.** A non-trivial client crate often grows
its own L1-equivalent (pure types) and L3-equivalent (domain compute)
modules — a formula evaluator, an editor model, a layout engine.
This is fine, and the framework's layer rules apply *recursively*
inside the client: the editor's pure model has no business importing
DOM types; the formula engine has no business reaching for the HTTP
client. When the same compute is needed server-side as well (e.g.
to validate a spreadsheet import without WASM), promote the
L3-equivalent module to a workspace L3 crate compiled to both WASM
and native. Frontend-specific guidance for the recursive case lives
in `framework/hints-frontend.md`.

**Trust posture.** The client itself is **untrusted** from the
backend's view. Anything the client checks for UX reasons must also
be checked at the edge.

---

## Trust boundaries

There are exactly three trust boundaries the framework asks every
project to draw, because every change near them deserves attention:

1. **Client ↔ Edge.** Untrusted input crosses here. Both authentication
   (who is this) and authorization (what may they do) live at the
   edge. Validation is parse-don't-validate: convert bytes into typed
   values once at entry, and pass typed values inward.
2. **Edge ↔ External services** (third-party APIs, OAuth providers,
   LLM endpoints). Outgoing requests carry secrets that must never
   reach the client; incoming responses are tainted and must be
   validated as if they were user input.
3. **Edge ↔ Persistence.** Crossed once per request hot path.
   Authorization is decided *before* the crossing, not after — a
   denied request must never reach a repo.

The internal `Foundation ↔ Persistence ↔ Domain ↔ Edge` boundaries
are layering boundaries, not trust boundaries. They exist to keep the
codebase legible, not to defend against attackers.

---

## Forbidden-knowledge cheat sheet

The agents grep for these. If a match is found in the wrong layer,
that's a layer violation finding before any taste-based review.

| Layer | Must not import / mention |
|-------|---------------------------|
| L1 Foundation | the storage SDK, the HTTP framework, domain types |
| L2 Persistence | the HTTP framework, the WebSocket protocol, the client |
| L3 Domain | the HTTP framework, the storage SDK directly (only via repos), other domain crates |
| L4 Edge | the storage SDK directly, client-side rendering |
| L5 Client | backend crate types, the storage SDK, server-only secrets |

Per-project specializations of this table go in the project section
below.

---

## Cross-target schema agreement

Whenever two pieces of code in different layers — or in the same
layer compiled to different targets — must agree on a wire format,
encode the agreement as a CI test in the crate that owns the
canonical schema. Drift between hand-written parallel implementations
is silent until it isn't, and "silent until it isn't" means a
production bug.

Concretely: if a Rust→WASM client mirrors a Rust server's tag names,
field encodings, or formula function signatures, the canonical home
is the server crate, and the server crate has a test that asserts
the client's parallel implementation agrees. The test reads the
client source (or a generated table from it) and compares against
its own table. The agents flag any new place where two schemas need
to agree without such a test as `re-implementation drift`.

This is not the same as "share a crate." A shared crate is the right
answer when the same code can run in both targets. When it can't —
because the targets are incompatible (server and WASM have different
build profiles, different available APIs) or because the
implementations are deliberately different (a client-side editor and
a server-side validator are different things that happen to agree on
schema) — the test is the safety net.

---

## Worked example: OgreNotes (this project)

OgreNotes is the calibration model. Its concrete realization of the
layer taxonomy:

### L1 Foundation — `crates/common`

`id::new_id` (21-char nanoid), `time::now_usec` (microsecond epoch),
`config::AppConfig` with redacting `Debug`, `error::CommonError`,
`metrics` (counters + histograms + a rolling-users gauge).

**Concrete forbidden knowledge.** No `aws_sdk_*`, no `axum`, no `yrs`,
no `Document` or `User` type. Verified: `crates/common/Cargo.toml`
lists no AWS or HTTP deps.

**Calibration note.** `AppConfig` is in L1 because every layer reads
from it; this is correct *because* the config is environment-loaded
plain values. Configs that grow live handles (an HTTP client, an
SDK client) belong in L4's application state, not here.

### L2 Persistence — `crates/storage`

Internally split into three concerns:

- `models/` — wire shapes (`DocumentMeta`, `DocMember`, `DocUpdate`),
  each with a `pk()` / `sk()` method that encodes the DynamoDB
  single-table layout.
- `repo/` — access patterns (`DocRepo`, `UserRepo`, `FolderRepo`,
  one per resource family). A repo holds a `DynamoClient` and an
  `S3Client` and exposes coarse-grained methods (`create`, `get`,
  `update_metadata`).
- `dynamo.rs` / `s3.rs` — thin wrappers over the AWS SDK clients.

Errors converge in `repo::RepoError` (`Dynamo`, `MissingField`, `S3`).
The edge maps these in `api::error::ApiError::From<RepoError>` —
notably, `Dynamo("ConditionalCheckFailed")` becomes
`ApiError::Conflict`, which is the only place that mapping lives.

**Concrete forbidden knowledge.** No `axum`, no `yrs`, no auth checks.
The repo never asks "is this user allowed to write" — that's decided
upstream.

### L3 Domain capabilities — `crates/auth`, `crates/collab`, `crates/notify`, `crates/search`, `crates/embeddings`

Each capability is a peer crate with a single bounded responsibility.
Cross-capability calls are absent: `notify` doesn't import from
`collab`; `auth` doesn't import from `notify`. The only allowed
upward dependency is on L1 (`common`) and, for capabilities that
need durable state, L2 (`storage`).

`collab` is a noteworthy case study: it has a `replay` binary
gated behind a feature flag (`features = ["replay"]`) which pulls in
storage and AWS SDKs. The feature gate keeps the capability's
default build hermetic — without it, the layer rule "domain doesn't
talk to AWS directly" would be silently broken.

**Concrete forbidden knowledge.** No `axum`. No
`use ogrenotes_api::*`. No `use ogrenotes_collab::*` from
`ogrenotes_notify`.

### L4 Edge — `crates/api`

`routes/` (one file per resource family), `middleware/` (auth, rate
limit, activity tracking, metrics), `state::AppState` (the composed
god-struct holding every repo and capability handle), `error::ApiError`
(the single boundary error type, with `IntoResponse` and `From<*>`
impls for every domain error).

Plus per-domain helper modules at the same level — `compaction`,
`digest`, `edit_activity`, `claude` — that orchestrate domain
capabilities for the edge's needs (idle compaction, daily digest
firing) but live in `api` because they're glue, not capability.

**Concrete forbidden knowledge.** No direct `aws_sdk_dynamodb` calls
in route handlers (always through a repo). No `yrs::Doc` constructed
in a handler (through `OgreDoc` from the `collab` capability).

### L5 Client — `frontend/`

Separate workspace member (excluded from the root workspace via
`exclude = ["frontend"]` because Leptos/WASM and the backend have
incompatible build profiles). Mirrors the edge's API surface in
`frontend/src/api/<resource>.rs` — one module per backend route
group, intentional shape symmetry.

`frontend/src/components/` (Leptos components), `frontend/src/editor/`
(the rich-text editor, paralleling the backend's `collab` schema),
`frontend/src/spreadsheet/` (the formula engine, also paralleling
backend logic where applicable).

**Concrete forbidden knowledge.** No `use ogrenotes_storage::*`, no
`use ogrenotes_collab::document::*`. The editor's schema is
deliberately a parallel implementation of the backend's CRDT schema,
not an import — they compile to different targets and a cross-schema
consistency test on the backend side asserts they agree on tag names.

---

## Project conventions slot

Things that are this-project-specific and the agents should know,
but other projects on the framework should *not* inherit:

- **Identifier strategy:** raw `String` for all IDs (`doc_id`,
  `user_id`, `folder_id`, …). Legacy choice that predates the
  framework. New code may newtype if the local benefit clears the
  cost-benefit gate, but the agents do *not* generate
  newtype-on-existing-IDs findings — the strategy is grandfathered
  here. New projects on the framework default to newtype-on-
  identifiers and record their own choice in this slot.
- **Observability ownership:** L1 (`crates/common/src/metrics`)
  holds counter and histogram types; L4 (`crates/api/src/observability.rs`)
  holds tracing-subscriber setup. Per the universal rule.
- **Cross-target schema agreement:** the editor schema in
  `crates/collab/src/schema.rs` (server, canonical) and
  `frontend/src/editor/schema.rs` (client, parallel) are kept in
  sync by a CI test in the `collab` crate. New schemas with
  parallel implementations across the build-target boundary follow
  the same pattern.
- DynamoDB single-table design with `<TYPE>#<id>` PK and structured
  SK encoding the row kind. Other projects on different stores would
  use different key conventions.
- `yrs` CRDT for collaborative editing, with a parallel WASM editor
  schema in the frontend. Project-specific to OgreNotes.
- AWS-centric infrastructure: DynamoDB + S3 + Bedrock + SES SMTP +
  CloudWatch metrics. Replaceable by other persistence/observability
  stacks without changing the layer taxonomy.
- `dev-login` feature flag for tests and local dev. A pattern other
  projects could adopt, but the specific endpoint shape is local.
- Single-binary deployment (`crates/api/src/main.rs` is the only
  long-running process). Other projects might run a separate
  worker; the framework supports that by adding a sibling L4 crate
  (e.g. `crates/worker`) with its own composed state.

---

## Blank template (copy for new projects)

```markdown
# Architecture — <project name>

## L1 Foundation — crate(s):
Purpose:
Forbidden knowledge:
Trust posture:

## L2 Persistence — crate(s):
Purpose:
Forbidden knowledge:
Trust posture:

## L3 Domain capabilities — crate(s):
Purpose:
Forbidden knowledge:
Trust posture:

## L4 Edge — crate(s):
Purpose:
Forbidden knowledge:
Trust posture:
Observability setup lives at:

## L5 Client — crate(s):
Purpose:
Forbidden knowledge:
Trust posture:
Internal layering inside the client (if applicable):

## Trust boundaries
1.
2.
3.

## Project conventions
- **Identifier strategy:** newtype | string-grandfathered | mixed (explain)
- **Observability ownership:** L1 holds types; L4 holds setup (or describe deviation)
- **Cross-target schema agreement:** list each cross-target mirror and the test that asserts it (or "none")
-
```
