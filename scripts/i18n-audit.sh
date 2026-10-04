#!/usr/bin/env bash
# Fail on untranslated UI literals or catalog drift. Use --inventory to report only.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$REPO_ROOT/scripts/i18n-audit.py" "$@"
