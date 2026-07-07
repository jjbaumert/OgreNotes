#!/usr/bin/env bash
# i18n-audit.sh — find user-visible string literals in the frontend
# that aren't yet routed through the t!() macro.
#
# Phase 5 M-P2 piece 6 lint. Runs as inventory tool during the
# extraction pass (reports counts but doesn't fail), and flips to
# fail-on-new mode once the migration is complete by setting
# I18N_AUDIT_MAX below the current baseline.
#
# Heuristics — high-precision over recall: we'd rather miss a
# string we could have translated than yell about a CSS class name
# being capitalized. Three patterns:
#   1. Text content inside view! tags:   >"Capital text..."
#   2. ARIA labels:                       aria-label="Capital text"
#   3. Placeholders / titles:             placeholder="..." title="..."
#
# Excludes:
#   - Strings already wrapped in t!()
#   - data-* / class / id / type attrs (machine-readable, not UI text)
#   - Single-character content
#   - Emoji-only content (\u{...}, no letters)
#   - Strings inside `//` line comments
#   - Test files (#[cfg(test)] modules, both standalone and inline)
#   - Match-arm patterns (`"Escape" => ...`, `"INPUT" | "TEXTAREA"`)
#   - Printf format strings (contain `{...}` placeholder)
#   - All-caps token arrays (formula names, enum variants)
#   - Macro continuation lines (`assert!(..., "msg");`, trailing `,`)
#
# Output format: one line per hit, `path:line:kind:snippet`.
# Summary at end: per-category counts + grand total.
#
# Usage:
#   scripts/i18n-audit.sh                  # inventory mode (always exit 0)
#   I18N_AUDIT_MAX=42 scripts/i18n-audit.sh  # gate mode (exit 1 if > MAX)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$REPO_ROOT/frontend/src"

if [[ ! -d "$SRC_DIR" ]]; then
    echo "i18n-audit: $SRC_DIR not found" >&2
    exit 2
fi

# Build the file list once — every *.rs under frontend/src/ except
# test files. Exclusions:
#   - `i18n.rs` — formatter bindings reference Intl string keys
#     (`"second"`, etc.) which are Web-API tokens, not UI text.
#   - `spreadsheet/{eval,parser,functions}.rs` — formula language
#     tokens (SUM/AVERAGE/TRUE/FALSE/etc.) and test fixture data.
#     These are part of the spreadsheet formula grammar, not chrome,
#     and must not be localized (formulas are portable across locales).
mapfile -t FILES < <(find "$SRC_DIR" -type f -name '*.rs' \
    ! -path '*/tests/*' \
    ! -name 'i18n.rs' \
    ! -path '*/spreadsheet/eval.rs' \
    ! -path '*/spreadsheet/parser.rs' \
    ! -path '*/spreadsheet/functions.rs')

# ─── Noise filters ───────────────────────────────────────────────
#
# `drop_test_block` strips hits whose line number falls at or after
# the first `#[cfg(test)]` line in their file. Production files in
# this codebase put all inline tests at the end (the project's
# convention), so a single per-file boundary catches both
# `#[cfg(test)] mod tests { ... }` and `#[cfg(test)] fn helper()`
# without needing brace-tracking. Boundary is cached per file across
# the awk run via the `bdy[]` map.
drop_test_block() {
    awk -F: '
        function boundary(path,    cmd, first) {
            if (!(path in bdy)) {
                cmd = "grep -n \"^[[:space:]]*#\\[cfg(test)\\]\" \"" path "\" 2>/dev/null | head -1 | cut -d: -f1"
                cmd | getline first
                close(cmd)
                bdy[path] = (first == "") ? 99999999 : first + 0
            }
            return bdy[path]
        }
        { if ($2 + 0 < boundary($1)) print }
    '
}

# `drop_known_false_positives` filters tier-1 hits that match
# patterns the audit can recognize as non-chrome:
#
#   - lines containing ` => ` → Rust match-arm pattern
#   - lines containing ` | "` → multi-pattern match alternation
#   - lines containing `{...}` inside a string → printf/format
#     placeholder (`"A1:{}{}"`, `"v{}"`, `"{name}"`)
#   - lines starting with `<indent>"ALLCAPS",` → identifier list
#     (formula function names, enum tokens)
#   - lines ending with `);` after a string → assert!/panic!/format!
#     continuation
#   - lines ending in bare `,` after a string → multi-line macro arg
#
# Each filter is a simple grep -vE; chaining them keeps the script
# linear and skimmable. Order doesn't matter for correctness.
drop_known_false_positives() {
    grep -vE '=>' \
    | grep -vE ' \| "' \
    | grep -vE '"[^"]*\{[^"]*\}[^"]*"' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*"[A-Z][A-Z0-9.]*",' \
    | grep -vE '\);[[:space:]]*$' \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]+"[^"]+",[[:space:]]*$' \
    || true
}

# Tier 1: text content inside view! macros. Two grep patterns
# covering the two Leptos shapes we see in practice:
#
#   (a) Inline:  `>"Capital text..."</tag>` — text right after the
#       opening tag's `>`.
#   (b) On its own line:  `    "Capital text..."` — common when
#       a view! body wraps element children to a new line.
#
# Both gate on a leading capital letter so they catch sentence-y
# strings rather than CSS class fragments / data values. The
# emoji-prefixed strings in this codebase (e.g. `"\u{1F3E0} Home"`)
# don't start with a capital ASCII letter on the source line — we
# match those via a separate \u{...} prefix branch.
#
# Filters: line comments (`//`), lines already wrapping a t!() call,
# the noise filters above, and the per-file test-block boundary.
tier1_hits() {
    {
        # (a) inline `>"Capital..."`
        grep -rHnE '>"[A-Z][^"]{2,}"' "${FILES[@]}"
        # (b) bare line `    "Capital..."` not part of an attribute
        grep -rHnE '^[[:space:]]+"[A-Z][^"]{2,}"' "${FILES[@]}"
        # (c) bare line with emoji prefix `    "\u{...} Capital..."`
        grep -rHnE '^[[:space:]]+"\\u\{[0-9A-Fa-f]+\}[^"]*[A-Za-z]{3,}[^"]*"' "${FILES[@]}"
    } \
        | sort -u \
        | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
        | grep -vE '^[^:]+:[0-9]+:.*t!\(' \
        | drop_known_false_positives \
        | drop_test_block \
        || true
}

# Tier 2: ARIA labels. These are screen-reader-only text, every
# one of them needs translation.
tier2_hits() {
    grep -rHnE 'aria-label="[^"]{3,}"' "${FILES[@]}" \
        | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
        | grep -vE 'aria-label=move\|aria-label=\{' \
        | drop_test_block \
        || true
}

# Tier 3: placeholder / title attributes. Same logic.
tier3_hits() {
    grep -rHnE '(placeholder|title)="[A-Z][^"]{2,}"' "${FILES[@]}" \
        | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
        | drop_test_block \
        || true
}

T1=$(tier1_hits | wc -l)
T2=$(tier2_hits | wc -l)
T3=$(tier3_hits | wc -l)
TOTAL=$((T1 + T2 + T3))

# Detailed output goes to stderr so callers can pipe just the
# summary if they want (or redirect /dev/stderr to a file for the
# per-PR inventory).
{
    echo "── tier 1: text content (>\"...\") ───────────────────────"
    tier1_hits
    echo
    echo "── tier 2: aria-label ───────────────────────────────────"
    tier2_hits
    echo
    echo "── tier 3: placeholder / title ──────────────────────────"
    tier3_hits
    echo
} >&2

echo "i18n-audit: text=$T1 aria=$T2 placeholder/title=$T3 total=$TOTAL"

if [[ -n "${I18N_AUDIT_MAX:-}" ]]; then
    if (( TOTAL > I18N_AUDIT_MAX )); then
        echo "i18n-audit: FAIL (total $TOTAL > max $I18N_AUDIT_MAX)" >&2
        exit 1
    fi
    echo "i18n-audit: PASS (total $TOTAL ≤ max $I18N_AUDIT_MAX)" >&2
fi
