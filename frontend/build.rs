// Capture the current git short SHA at compile time and expose it as
// `GIT_HASH` so `option_env!("GIT_HASH")` resolves in the WASM bundle.
// Used by the sidebar version stamp to confirm at-a-glance which build
// is live in a deployed environment.
//
// Source order (first hit wins):
//   1. `GIT_HASH` already in the build env (other than "unknown", the
//      Dockerfile's default). Set by the AWS deploy scripts via
//      `docker build --build-arg GIT_HASH=…`.
//   2. `git rev-parse --short HEAD`. The developer-machine path:
//      `cargo build` / `trunk build` outside Docker.
//   3. `../.git` read directly, for builds with no `git` binary: the
//      self-host compose file mounts the repo's `.git/` into the
//      frontend build stage (`.dockerignore` keeps it out of the normal
//      context), so its images stamp the commit with no extra step.
//   4. Literal "unknown" if none works.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let stamp = env::var("GIT_HASH")
        .ok()
        .filter(|s| !s.is_empty() && s != "unknown")
        .or_else(stamp_from_git)
        .or_else(|| stamp_from_git_dir(Path::new("../.git")))
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=GIT_HASH={stamp}");
    // Re-run when HEAD moves or the index changes so the hash stays
    // current without requiring a manual `cargo clean`. No-op when the
    // paths don't exist (Docker build).
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    println!("cargo:rerun-if-env-changed=GIT_HASH");
}

fn stamp_from_git() -> Option<String> {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())?;

    let dirty = Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    Some(if dirty { format!("{hash}-dirty") } else { hash })
}

/// The short commit id from a `.git` directory's files: `HEAD` is either
/// a commit id (detached) or `ref: refs/heads/<branch>`, whose id lives in
/// `refs/heads/<branch>` or, once packed, in `packed-refs`. No dirty flag:
/// that needs the index, i.e. real git.
fn stamp_from_git_dir(git: &Path) -> Option<String> {
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let id = match head.strip_prefix("ref: ") {
        None => head.to_string(),
        Some(name) => fs::read_to_string(git.join(name))
            .ok()
            .map(|s| s.trim().to_string())
            .or_else(|| {
                fs::read_to_string(git.join("packed-refs")).ok()?.lines().find_map(|l| {
                    let (id, r) = l.split_once(' ')?;
                    (r == name).then(|| id.to_string())
                })
            })?,
    };
    (id.len() >= 7 && id.bytes().all(|b| b.is_ascii_hexdigit())).then(|| id[..7].to_string())
}
