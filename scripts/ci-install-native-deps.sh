#!/usr/bin/env bash
# Native build dependencies shared by CI, browser tests, and load tests.
# Additional workflow-specific apt packages can be passed as arguments.
set -euo pipefail
export LC_ALL=C

sudo apt-get update

# Ubuntu 26 uses libxmlsec1-openssl1, Ubuntu 24 uses the t64 package,
# and Ubuntu 22 retains libxmlsec1-openssl.
# Check candidates rather than package metadata, which can include obsolete
# packages that apt can no longer install.
xmlsec_openssl_package=
for package in libxmlsec1-openssl1 libxmlsec1t64-openssl libxmlsec1-openssl; do
  candidate=$(apt-cache policy "$package" | sed -n 's/^[[:space:]]*Candidate: //p')
  if [[ -n "$candidate" && "$candidate" != '(none)' ]]; then
    xmlsec_openssl_package=$package
    break
  fi
done

if [[ -z "$xmlsec_openssl_package" ]]; then
  echo 'No installable xmlsec OpenSSL runtime found in the apt repositories.' >&2
  exit 1
fi

sudo apt-get install -y \
  pkg-config libssl-dev \
  libxml2-dev libxmlsec1-dev "$xmlsec_openssl_package" \
  clang libclang-dev "$@"

# A different crypto backend does not satisfy samael's OpenSSL bindings.
pkg-config --exists xmlsec1-openssl
