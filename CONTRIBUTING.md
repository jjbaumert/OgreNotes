# Contributing to OgreNotes

Thanks for your interest. OgreNotes is a personal project shared publicly so
the code can be read, learned from, and reused. Contributions are welcome, but
please read the expectations below first — see also the *Project status &
governance* section of the [README](README.md).

## Before you invest significant effort

- **Open an issue to discuss first.** For anything beyond a small fix, describe
  what you want to change and why before writing code. This avoids work that
  won't be merged.
- **Triage is best-effort.** Issues and pull requests may be closed without
  action. The maintainer retains final say over scope, design, and what gets
  merged. If you want to take OgreNotes in a different direction, the MIT
  license makes forking easy and explicitly permitted.

## Development setup

See [README.md](README.md#local-development) for prerequisites, local services
(DynamoDB Local, MinIO, Redis, Qdrant), and how to run the API server and
frontend.

Quick reference:

```bash
# Unit tests (no Docker needed)
cargo test --workspace --lib

# Integration tests (requires local Docker services)
cargo test --workspace

# Frontend lives outside the workspace — build/test from its own directory
cd frontend && trunk build
cargo test --bin ogrenotes-frontend --target x86_64-unknown-linux-gnu \
  --manifest-path frontend/Cargo.toml
```

## Code standards

- **Formatting & lints:** run `cargo fmt --all` and `cargo clippy --workspace
  --all-targets` before opening a PR. CI enforces both.
- **Dependency hygiene:** new dependencies are reviewed for necessity and
  license compatibility. `cargo deny check` runs in CI (config in `deny.toml`).
- **Architecture:** the backend is a layered Cargo workspace (Foundation →
  Persistence → Domain → Edge → Client). The `framework/` directory documents
  the layer taxonomy and preferred patterns; skim `framework/architecture.md`
  and `framework/hints.md` before large changes.
- **Tests encode behavior.** Adding tests for new code is expected. Changing an
  existing test means you are changing a behavioral contract — call that out
  explicitly in the PR description.
- **Frontend is separate.** `frontend/` is excluded from the root workspace and
  targets WASM; guidance lives in `framework/hints-frontend.md`.

## Pull request checklist

- [ ] Discussed in an issue first (for non-trivial changes)
- [ ] `cargo fmt --all` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace --lib` passing (plus integration tests if touched)
- [ ] No secrets, credentials, personal data, or internal infrastructure
      identifiers added
- [ ] PR description explains the *why*, not just the *what*

## Sign-off (DCO)

By contributing, you certify that you wrote the change or otherwise have the
right to submit it under the project's MIT license (the
[Developer Certificate of Origin](https://developercertificate.org/)). Add a
`Signed-off-by` line to each commit with `git commit -s`.

## Reporting security issues

Do **not** open a public issue for vulnerabilities. Follow
[SECURITY.md](SECURITY.md) instead.
