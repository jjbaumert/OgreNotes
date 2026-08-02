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

### Frontend WASM browser tests

The `#[wasm_bindgen_test]` suites need a real browser driven over WebDriver.
**Firefox is the supported path** and the one CI uses
(`.github/workflows/wasm-tests.yml`):

```bash
cd frontend
wasm-pack test --headless --firefox -- --test browser   # DOM suite
wasm-pack test --headless --firefox -- --lib            # inline lib tests
wasm-pack test --headless --firefox -- --test collab_e2e  # needs the local stack
```

#### Chrome: `Error: http status: 404` is a tooling failure, not a test failure

If `wasm-pack test --headless --chrome` fails like this — for *every* test,
before any test code runs:

```
Running headless tests in Chrome on `http://127.0.0.1:45947/`
driver status: signal: 9 (SIGKILL)
driver stdout:
    Starting ChromeDriver 151.0.7922.71 ... on port 45947
Error: http status: 404
```

**Nothing is wrong with the code.** `Error: http status: 404` is
wasm-bindgen-test-runner's generic report for *any* failed WebDriver session
creation; it discards the driver's actual explanation. That explanation is
almost always a version mismatch:

```
session not created: This version of ChromeDriver only supports Chrome
version 151. Current browser version is 131.0.6778.85
```

The cause is that `wasm-pack` downloads ChromeDriver from the
Chrome-for-Testing **Stable** channel and never inspects the Chrome you
actually have installed. Any Chrome that is not current stable fails, and
ChromeDriver requires an exact **major** version match. Firefox is unaffected
because geckodriver supports a wide range of Firefox versions.

To confirm the diagnosis in ten seconds, compare the two majors:

```bash
google-chrome --version          # e.g. 131.0.6778.85
scripts/wasm-test-chrome.sh --check
```

Fix it in whichever way suits you:

- **Use the helper** — `scripts/wasm-test-chrome.sh --test browser` resolves
  your Chrome's version, fetches the matching ChromeDriver into the
  (gitignored) `frontend/target/chromedriver/`, and passes it to `wasm-pack`.
  Nothing is installed system-wide. Use `CHROME_BIN=/usr/bin/chromium-browser`
  to test against a different Chromium build.
- **Update Chrome** so it matches current stable; `wasm-pack`'s automatic
  download then lines up on its own. On Fedora note that
  `/etc/yum.repos.d/google-chrome.repo` ships with `enabled=0` in some
  installs, which silently freezes Chrome at whatever version was first
  installed.
- **Put a matching `chromedriver` on `$PATH`** — `wasm-pack` prefers a `$PATH`
  driver over its own download, so this needs no flags.

Chrome coverage is a local-development nicety, not a gate: CI runs these
suites under Firefox only, so a broken local Chrome never blocks a PR.

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
