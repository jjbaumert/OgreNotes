#!/usr/bin/env bash
set -euo pipefail

sudo apt-get update

xmlsec_runtime_package=""
for candidate in libxmlsec1-openssl libxmlsec1t64-openssl libxmlsec1-openssl1; do
    version="$(apt-cache policy "$candidate" | awk '/Candidate:/ { print $2; exit }')"
    if [[ -n "$version" && "$version" != "(none)" ]]; then
        xmlsec_runtime_package="$candidate"
        break
    fi
done

if [[ -z "$xmlsec_runtime_package" ]]; then
    echo "No supported xmlsec OpenSSL runtime package is available" >&2
    exit 1
fi
echo "Using xmlsec runtime package: $xmlsec_runtime_package"

packages=(
    pkg-config
    libssl-dev
    libxml2-dev
    libxmlsec1-dev
    "$xmlsec_runtime_package"
    clang
    libclang-dev
)
packages+=("$@")

sudo apt-get install -y --no-install-recommends "${packages[@]}"
