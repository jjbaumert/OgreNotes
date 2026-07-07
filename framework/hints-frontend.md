# Frontend Hints — client-crate-specific guidance

This document is a **conditional supplement** to `framework/hints.md`.
The agents read it *only* when the diff under review touches a
client crate. Findings on backend code do not cite this file.

## When this document applies

This file's content is calibrated for projects where the client is
written in Rust and compiles to WASM via Leptos (or a similar
fine-grained reactive framework: Yew, Dioxus, Sycamore — adapt
accordingly). It does **not** apply to:

- Projects with a JS/TS frontend (React, Svelte, Vue, etc.). Those
  have their own ecosystem of best practices; the framework offers
  no opinion.
- Projects with no client at all (CLIs, server-only services).
- Native-rendering Rust UIs (egui, iced, GPUI). The reactive-
  primitives section below is irrelevant; the rest may carry over
  case by case.

If your project's client doesn't match the conditions above, omit
this file from agent reads. The architecture doc's L5 description
still applies regardless.

## How this relates to the universal hints

The universal hints in `framework/hints.md` apply to every Rust
crate, including the client. The patterns to prefer (newtype,
typestate, parse-don't-validate, RAII guards, sealed traits, error
type design) and the named anti-patterns (boolean blindness,
primitive obsession, stringly-typed) are not relaxed for client
code. This file adds *additional* concerns specific to building a
non-trivial reactive WASM client; it does not subtract.

When a client-crate finding cites only universal hints, do not
reach for this file. When a finding is genuinely specific to the
client environment (signal discipline, contenteditable, bundle
size), use this file.

---

## Internal layering inside the client

The architecture doc's L5 paragraph names this directly: a non-
trivial client crate grows its own L1-equivalent (pure types) and
L3-equivalent (domain compute) modules, and the framework's layer
rules apply *recursively*.

In practice, agents reviewing client code should treat:

- **`api/<resource>.rs`** as the client's L4 (its edge — it
  speaks the wire protocol with the backend).
- **`components/`, `pages/`** as the client's L5 (presentation).
- **`editor/`, `spreadsheet/`** as the client's L3 (domain compute
  — formulas, document models, rich-text transforms).
- **Pure types and constants used across the above** as the
  client's L1.

The forbidden-knowledge cheat sheet applies recursively: the
editor's pure model has no business importing DOM types from
`web-sys`; the formula engine has no business reaching for the
HTTP client; the API client has no business performing a
spreadsheet calculation.

When the same compute is needed server-side as well — e.g. to
validate a spreadsheet import without WASM, or to render a
preview from the server — promote the L3-equivalent module to a
proper workspace L3 crate (`crates/formula`, `crates/editor-model`)
that compiles to both targets. The architecture doc L5 paragraph
is the canonical recommendation; this file gives the practical
trigger: "the same logic now lives in two places that must agree"
is the trigger; the cure is promotion plus a cross-target schema
test.

---

## Reactive primitives discipline

In a fine-grained reactive framework, the model assumes *signals
notify only the consumers that read them*. Findings cluster around
two failure modes.

### Coarse-grained re-renders

A component that reads `state` and rebuilds its entire subtree on
every change defeats the framework's incremental rendering. The
fix is usually to push the read into the leaf component that
actually needs the value, not to wrap a coarse component in a
memoization layer.

A finding here: "Component `Foo` reads `app_state.docs` and
re-renders the entire document list on any change to any document.
The list-item subcomponent should read its own document signal
directly; `Foo` should iterate over IDs only."

### Effects with non-explicit dependencies

A `create_effect` (or framework equivalent) that pulls from
multiple signals via conditional logic ("if x, read y, else read
z") creates an opaque dependency graph that's hard to reason about
and easy to leave stale. Cure: split into two effects with
explicit dependencies, or refactor the conditional to read both
and branch on the values.

### Holding signals across `await` points

Async code that reads a signal *before* an `await` and uses the
value *after* it has captured a stale snapshot: the signal may
have changed during the await. This is a correctness bug, not a
style issue. Cure: re-read after the await, or pass the captured
value forward explicitly so the staleness is visible.

---

## `web-sys` surface gating

Each `web-sys` feature flag pulls JS bindings into the WASM bundle.
A new flag is a finding the agent gates on:

- **Is the API needed at runtime, or is it a compile-time helper?**
  If it's a compile-time helper (e.g. for testing setup), it should
  live behind `#[cfg(test)]`, not in production deps.
- **Is there a smaller surface that suffices?** `HtmlElement`
  covers many cases that an over-specific `HtmlAnchorElement`
  would; aggregating to the smaller set keeps the bundle leaner.
- **Was a similar feature already enabled?** Audit existing flags
  before adding a new one — duplicates accumulate quickly.

Default suspicion: any PR that adds three or more `web-sys`
features in one diff is probably enabling more than the change
strictly needs. Flag and ask.

---

## Contenteditable and direct DOM mutation

When the client owns a rich-text or canvas-style editor, the
canonical mutation path is the editor's own transform layer. Any
DOM mutation that bypasses it — `set_inner_html`, direct
`element.append_child` for editable content, manual `Selection`
range manipulation outside the editor's selection module — is a
finding by default.

The cost of bypassing the transform layer is that CRDT updates,
undo/redo, and collaboration awareness all assume the transform
layer is the single source of truth. A direct mutation that
"happens to work" on the local screen produces silent divergence
when a remote update arrives.

The schema-duality test (architecture doc, *Cross-target schema
agreement*) catches drift between the editor's schema and the
server's. It does not catch direct DOM mutations that violate the
schema at runtime. The agent's job is to flag those at review
time.

---

## WASM bundle budget

A compiled WASM bundle has a per-page download cost the user pays
on every cold load. The framework recommends:

- A CI check (`cargo bloat` against a target size, or
  `wasm-opt --print-stack-ir` differential against main) on every
  PR. Project sets the threshold; +50KB on a 200KB bundle is a
  larger relative regression than +50KB on a 2MB bundle.
- A finding when a PR adds a heavyweight transitive dependency
  (`statrs`, `nalgebra`, `regex` with full Unicode tables) that
  wasn't already in the dep graph. The fix is usually a smaller
  alternative (`libm` instead of `statrs`, hand-rolled scanner
  instead of `regex`); this project does both.
- A finding when a PR adds a `wee_alloc`-style global allocator
  swap without a benchmark showing the swap is needed.

Bundle size is a place where the *additive bias* warning in the
universal hints applies hard. New features default to "we can
afford it"; over time the bundle grows past the budget. The agent
should err on the subtractive side here.

---

## Cross-target schema mirror

When the client mirrors a backend module — schema, formula
function set, validation rules, anything where the wire format
or computed result must agree — the canonical implementation
lives **on the backend**. The client's parallel implementation
is verified by a backend-side test (see architecture doc,
*Cross-target schema agreement*).

Findings here:

- **A new mirror without a test.** Flag as
  `re-implementation drift` and propose the test shape.
- **A test that's gone stale.** Either someone updated only one
  side (the test should fail; if it doesn't, the test is wrong),
  or the test has been disabled. Either is a finding.
- **A "shared crate" that compiles to both targets** is a *better*
  answer when both targets can use the same code. The agent
  considers this and recommends it when the cost is low (a few
  hundred lines of `#[cfg]`-free pure Rust). When the cost is
  high (target-incompatible deps, divergent build profiles), the
  parallel-implementation-plus-test pattern is preferred.

---

## What this doc is NOT

- **Not a Leptos tutorial.** It assumes the developer knows the
  framework; it offers review guidance, not introductory material.
- **Not a list of "Leptos best practices."** Frameworks evolve
  and best practices follow. This doc names *failure modes* the
  agent should look for, which are more durable.
- **Not a specification of any UI library.** The patterns
  generalize across fine-grained reactive frameworks; the names
  (`signal`, `create_effect`) are Leptos-flavored but the
  concepts apply to Yew (`use_state`), Dioxus (`use_signal`),
  Sycamore (`Signal`). Adapt the names; the failure modes are
  the same.
