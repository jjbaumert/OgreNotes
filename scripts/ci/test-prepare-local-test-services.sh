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
[[ -z "${FAIL_PORT:-}" || "$3" != "$FAIL_PORT" ]]
STUB
cat > "$tmp/bin/sleep" <<'STUB'
#!/usr/bin/env bash
:
STUB
cat > "$tmp/bin/cargo" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$CARGO_LOG"
exit "${CARGO_EXIT:-0}"
STUB
cat > "$tmp/bin/sudo" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$SUDO_LOG"
STUB
cat > "$tmp/bin/apt-cache" <<'STUB'
#!/usr/bin/env bash
if [[ "${APT_NONE:-false}" == true || "$2" != "${APT_PACKAGE:-}" ]]; then
    printf '%s\n' '  Candidate: (none)'
else
    printf '%s\n' '  Candidate: 1.2.3-1'
fi
STUB
chmod +x "$tmp/bin/"*

for package in libxmlsec1-openssl libxmlsec1t64-openssl libxmlsec1-openssl1; do
    output="$tmp/$package.log"
    sudo_log="$tmp/$package.sudo"
    PATH="$tmp/bin:$PATH" \
        APT_PACKAGE="$package" \
        SUDO_LOG="$sudo_log" \
        bash "$root/scripts/ci/install-native-build-deps.sh" extra-one extra-two \
        >"$output" 2>&1

    if ! grep -Fq "Using xmlsec runtime package: $package" "$output"; then
        echo "Did not select available xmlsec package $package" >&2
        cat "$output" >&2
        exit 1
    fi
    if ! grep -Fq "apt-get install -y --no-install-recommends" "$sudo_log" ||
        ! grep -Fq "$package" "$sudo_log" ||
        ! grep -Fq 'extra-one extra-two' "$sudo_log"; then
        echo "Did not pass $package and extra packages to apt-get install" >&2
        cat "$sudo_log" >&2
        exit 1
    fi
done

output="$tmp/no-xmlsec.log"
sudo_log="$tmp/no-xmlsec.sudo"
status=0
PATH="$tmp/bin:$PATH" APT_NONE=true SUDO_LOG="$sudo_log" \
    bash "$root/scripts/ci/install-native-build-deps.sh" >"$output" 2>&1 || status=$?
if ((status == 0)) || ! grep -Fq 'No supported xmlsec OpenSSL runtime package is available' "$output"; then
    echo "Missing xmlsec packages did not fail with a useful diagnostic" >&2
    cat "$output" >&2
    exit 1
fi
if grep -Fq 'apt-get install' "$sudo_log"; then
    echo 'Dependency installation continued without an xmlsec runtime package' >&2
    exit 1
fi
echo 'xmlsec package selection and extra package forwarding contracts passed'

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

check_profile() {
    name="$1"
    expected="$2"
    shift 2
    cargo_log="$tmp/$name.profile"
    PATH="$tmp/bin:$PATH" \
        LOCAL_TEST_SERVICES_TIMEOUT_SECONDS=1 \
        DOCKER_LOG="$tmp/$name.docker" \
        CARGO_LOG="$cargo_log" \
        bash "$root/scripts/ci/prepare-local-test-services.sh" "$@" \
        >/dev/null 2>&1
    if ! grep -Fxq "$expected" "$cargo_log"; then
        echo "$name provisioning profile changed" >&2
        cat "$cargo_log" >&2
        exit 1
    fi
}

check_profile debug-default \
    'run --locked --bin setup_dev -p ogrenotes-api' --provision
check_profile release-default \
    'run --locked --release --bin setup_dev -p ogrenotes-api' --provision --release
check_profile wasm-features \
    'run --locked --bin setup_dev -p ogrenotes-api --no-default-features --features dev-login,xlsx,docx' \
    --provision --no-default-features --features dev-login,xlsx,docx
echo 'Cargo provisioning profile contracts passed'

cargo_log="$tmp/cargo-failure.log"
status=0
PATH="$tmp/bin:$PATH" \
    LOCAL_TEST_SERVICES_TIMEOUT_SECONDS=1 \
    DOCKER_LOG="$tmp/cargo-failure.docker" \
    CARGO_LOG="$cargo_log" \
    CARGO_EXIT=37 \
    bash "$root/scripts/ci/prepare-local-test-services.sh" --provision \
    >/dev/null 2>&1 || status=$?
if ((status != 37)); then
    echo "Cargo failure was not propagated (status $status)" >&2
    exit 1
fi
echo 'Cargo provisioning failure propagation contract passed'
