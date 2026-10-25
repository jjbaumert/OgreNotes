#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

cat > "$tmp/bin/docker" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$DOCKER_LOG"
STUB
cat > "$tmp/bin/nc" <<'STUB'
#!/usr/bin/env bash
[[ "$3" != "$FAIL_PORT" ]]
STUB
cat > "$tmp/bin/sleep" <<'STUB'
#!/usr/bin/env bash
:
STUB
cat > "$tmp/bin/cargo" <<'STUB'
#!/usr/bin/env bash
printf 'called\n' >> "$CARGO_LOG"
STUB
chmod +x "$tmp/bin/"*

for service in '8000:DynamoDB' '9000:MinIO' '6379:Redis'; do
    port="${service%%:*}"
    name="${service##*:}"
    output="$tmp/$name.log"
    docker_log="$tmp/$name.docker"
    cargo_log="$tmp/$name.cargo"
    status=0
    PATH="$tmp/bin:$PATH" \
        FAIL_PORT="$port" \
        LOCAL_TEST_SERVICES_TIMEOUT_SECONDS=1 \
        DOCKER_LOG="$docker_log" \
        CARGO_LOG="$cargo_log" \
        bash "$root/scripts/ci/prepare-local-test-services.sh" --provision \
        >"$output" 2>&1 || status=$?

    if ((status == 0)); then
        echo "$name failure did not return a nonzero status" >&2
        exit 1
    fi
    if ! grep -Fq "$name did not become ready on port $port" "$output"; then
        echo "$name failure did not identify the unavailable service" >&2
        cat "$output" >&2
        exit 1
    fi
    if ! grep -Fxq 'compose ps' "$docker_log"; then
        echo "$name failure did not print Compose status" >&2
        exit 1
    fi
    if [[ -e "$cargo_log" ]]; then
        echo "Cargo provisioning ran after $name failed readiness" >&2
        exit 1
    fi
    echo "$name readiness failure contract passed"
done
