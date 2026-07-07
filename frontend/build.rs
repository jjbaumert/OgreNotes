// Capture the current git short SHA at compile time and expose it as
// `GIT_HASH` so `option_env!("GIT_HASH")` resolves in the WASM bundle.
// Used by the sidebar version stamp to confirm at-a-glance which build
// is live in a deployed environment.
//
// Source order (first hit wins):
//   1. `GIT_HASH` already in the build env. Set by the deploy scripts
//      before invoking `docker build --build-arg GIT_HASH=…`, because
//      Docker's frontend builder stage has no `git` and no `.git/` dir
//      (`.dockerignore` excludes it). Without this path, every Docker
//      build would stamp "unknown".
//   2. `git rev-parse --short HEAD` on the host. The local
//      developer-machine path: `cargo build` / `trunk build` outside
//      Docker resolves this fine.
//   3. Literal "unknown" if neither works.

use std::env;
use std::process::Command;

fn main() {
    let stamp = env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(stamp_from_git);

    println!("cargo:rustc-env=GIT_HASH={stamp}");
    // Re-run when HEAD moves or the index changes so the hash stays
    // current without requiring a manual `cargo clean`. No-op when the
    // paths don't exist (Docker build).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    println!("cargo:rerun-if-env-changed=GIT_HASH");
}

fn stamp_from_git() -> String {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let dirty = Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    if dirty { format!("{hash}-dirty") } else { hash }
}
