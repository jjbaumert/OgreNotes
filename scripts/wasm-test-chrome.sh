#!/usr/bin/env bash
#
# Run the frontend's wasm-bindgen browser tests under headless Chrome with a
# ChromeDriver that actually matches the locally installed Chrome.
#
# WHY THIS SCRIPT EXISTS (issue #221)
# -----------------------------------
# `wasm-pack test --headless --chrome` downloads ChromeDriver from the
# Chrome-for-Testing *Stable* channel and never looks at which Chrome you
# actually have installed. If your Chrome is even one major version behind
# stable, ChromeDriver refuses the session:
#
#     session not created: This version of ChromeDriver only supports Chrome
#     version 151. Current browser version is 131.0.6778.85
#
# wasm-bindgen-test-runner swallows that message and reports only:
#
#     Error: http status: 404
#
# ...for every single test, which reads exactly like "the whole test suite is
# broken." It is not. No test code runs at all. See CONTRIBUTING.md.
#
# WHAT THIS SCRIPT DOES
# ---------------------
# Resolves the installed Chrome's major version, finds or fetches the matching
# ChromeDriver, and hands it to wasm-pack via `--chromedriver`. Nothing is
# installed system-wide: the driver is cached under `frontend/target/`, which
# is gitignored.
#
# NETWORK NOTE: if no matching driver is cached or on $PATH, this script
# downloads one from https://storage.googleapis.com/chrome-for-testing-public/
# into frontend/target/chromedriver/. Run with --check to diagnose only and
# never download anything.
#
# USAGE
#   scripts/wasm-test-chrome.sh --test browser
#   scripts/wasm-test-chrome.sh --lib
#   scripts/wasm-test-chrome.sh --check          # diagnose versions, run nothing
#
#   CHROME_BIN=/usr/bin/chromium-browser scripts/wasm-test-chrome.sh --lib
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$REPO_ROOT/frontend"
CACHE_DIR="$FRONTEND_DIR/target/chromedriver"
CFT_BASE="https://storage.googleapis.com/chrome-for-testing-public"
CFT_CATALOG="https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json"

CHECK_ONLY=0
if [[ "${1:-}" == "--check" ]]; then
  CHECK_ONLY=1
  shift
fi

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
note() { printf '  %s\n' "$*"; }

# ── Locate Chrome ────────────────────────────────────────────────────────
chrome_bin="${CHROME_BIN:-}"
if [[ -z "$chrome_bin" ]]; then
  for candidate in google-chrome google-chrome-stable chromium-browser chromium; do
    if command -v "$candidate" >/dev/null 2>&1; then
      chrome_bin="$(command -v "$candidate")"
      break
    fi
  done
fi
[[ -n "$chrome_bin" ]] || die "no Chrome/Chromium found on \$PATH; set CHROME_BIN=/path/to/chrome"

chrome_version="$("$chrome_bin" --version 2>/dev/null | grep -oE '[0-9]+(\.[0-9]+){3}' | head -1)"
[[ -n "$chrome_version" ]] || die "could not parse a version out of \`$chrome_bin --version\`"
chrome_major="${chrome_version%%.*}"

echo "Chrome:"
note "binary  $chrome_bin"
note "version $chrome_version (major $chrome_major)"

# ── Find a matching ChromeDriver ─────────────────────────────────────────
driver_major_of() {
  "$1" --version 2>/dev/null | grep -oE '[0-9]+(\.[0-9]+){3}' | head -1 | cut -d. -f1
}

driver=""

# 1. A chromedriver already on $PATH, if its major matches. wasm-pack prefers a
#    $PATH driver over its own download, so this is also the zero-config fix.
if command -v chromedriver >/dev/null 2>&1; then
  path_driver="$(command -v chromedriver)"
  path_major="$(driver_major_of "$path_driver")"
  if [[ "$path_major" == "$chrome_major" ]]; then
    driver="$path_driver"
    echo "ChromeDriver: using \$PATH copy (major $path_major) — $driver"
  else
    echo "ChromeDriver on \$PATH is major $path_major, need $chrome_major — ignoring $path_driver"
  fi
fi

# 2. A previously cached download for this major version.
if [[ -z "$driver" ]]; then
  cached="$CACHE_DIR/$chrome_major/chromedriver"
  if [[ -x "$cached" && "$(driver_major_of "$cached")" == "$chrome_major" ]]; then
    driver="$cached"
    echo "ChromeDriver: using cached copy — $driver"
  fi
fi

if [[ -z "$driver" && "$CHECK_ONLY" == "1" ]]; then
  cat <<EOF

No ChromeDriver for Chrome major $chrome_major is on \$PATH or cached.
Re-run without --check to download one into $CACHE_DIR/$chrome_major/,
or fix it at the machine level (see CONTRIBUTING.md, "Chrome").
EOF
  exit 1
fi

# 3. Download the matching driver into the gitignored cache.
if [[ -z "$driver" ]]; then
  echo "ChromeDriver: none matching major $chrome_major; downloading..."
  for tool in curl unzip python3; do
    command -v "$tool" >/dev/null 2>&1 || die "\`$tool\` is required to fetch ChromeDriver"
  done

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  # Try the exact installed version first — that URL exists for every real
  # Chrome stable release. Distro-built Chromium may carry a patch level that
  # Chrome-for-Testing never published, so fall back to the newest published
  # build sharing the same major.
  url="$CFT_BASE/$chrome_version/linux64/chromedriver-linux64.zip"
  if ! curl -fsSL --max-time 120 -o "$tmp/cd.zip" "$url" 2>/dev/null; then
    note "no exact build for $chrome_version; querying the Chrome-for-Testing catalog"
    curl -fsSL --max-time 60 -o "$tmp/catalog.json" "$CFT_CATALOG" \
      || die "could not reach the Chrome-for-Testing catalog"
    url="$(python3 - "$tmp/catalog.json" "$chrome_major" <<'PY'
import json, sys
catalog, major = sys.argv[1], sys.argv[2]
best = ""
for v in json.load(open(catalog))["versions"]:
    if v["version"].split(".")[0] != major:
        continue
    for d in v["downloads"].get("chromedriver", []):
        if d["platform"] == "linux64":
            best = d["url"]
print(best)
PY
)"
    [[ -n "$url" ]] || die "Chrome-for-Testing publishes no linux64 ChromeDriver for major $chrome_major"
    note "using $url"
    curl -fsSL --max-time 120 -o "$tmp/cd.zip" "$url" || die "download failed: $url"
  fi

  unzip -oq "$tmp/cd.zip" -d "$tmp/x"
  src="$(find "$tmp/x" -type f -name chromedriver | head -1)"
  [[ -n "$src" ]] || die "no chromedriver binary inside the downloaded archive"

  mkdir -p "$CACHE_DIR/$chrome_major"
  install -m 0755 "$src" "$CACHE_DIR/$chrome_major/chromedriver"
  driver="$CACHE_DIR/$chrome_major/chromedriver"
  note "cached at $driver"
fi

echo "ChromeDriver: $("$driver" --version 2>&1 | head -1)"

if [[ "$CHECK_ONLY" == "1" ]]; then
  echo
  echo "OK — Chrome $chrome_major and ChromeDriver $chrome_major agree."
  exit 0
fi

# ── Pin the browser binary too ───────────────────────────────────────────
# ChromeDriver launches whatever Chrome it finds on its own, which is not
# necessarily the binary we just version-matched against (it ignores
# CHROME_BIN and prefers google-chrome). wasm-bindgen-test-runner merges the
# JSON at WASM_BINDGEN_TEST_WEBDRIVER_JSON into the session capabilities, so
# `goog:chromeOptions.binary` makes the pairing explicit.
#
# If the crate ever grows its own frontend/webdriver.json, defer to it rather
# than silently overriding — the env var replaces the file, it does not merge.
if [[ -f "$FRONTEND_DIR/webdriver.json" ]]; then
  echo "note: frontend/webdriver.json exists; leaving capabilities to it"
else
  mkdir -p "$CACHE_DIR"
  cat > "$CACHE_DIR/webdriver.json" <<EOF
{"goog:chromeOptions":{"binary":"$chrome_bin"}}
EOF
  export WASM_BINDGEN_TEST_WEBDRIVER_JSON="$CACHE_DIR/webdriver.json"
fi

# ── Run ──────────────────────────────────────────────────────────────────
# Everything after the bare `--` is forwarded to `cargo test` by wasm-pack.
echo
echo "+ wasm-pack test --headless --chrome --chromedriver $driver -- $*"
cd "$FRONTEND_DIR"
exec wasm-pack test --headless --chrome --chromedriver "$driver" -- "$@"
