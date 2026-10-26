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

# Bindgen otherwise discovers the highest preinstalled libclang, which can
# differ from the distro compiler we just installed. In particular, Clang 22
# generates incomplete structs with samael's bindgen 0.71 dependency.
libclang_package=$(dpkg-query -W -f='${Depends}\n' libclang-dev |
  sed -n 's/.*\(libclang-[0-9][0-9]*-dev\).*/\1/p')
if [[ -z "$libclang_package" ]]; then
  echo 'Could not resolve the installed libclang development package.' >&2
  exit 1
fi
libclang_path=$(dpkg-query -L "$libclang_package" | sed -n 's|/libclang\.so$||p')
if [[ ! -f "$libclang_path/libclang.so" ]]; then
  echo "Could not find libclang.so from $libclang_package." >&2
  exit 1
fi

# Native library upgrades must invalidate cached generated bindings too.
native_abi=$(dpkg-query -W -f='${Package}=${Version}\n' \
  libxml2-dev libxmlsec1-dev "$libclang_package" | sha256sum | cut -d ' ' -f 1)
printf 'Using %s at %s (native ABI %s)\n' "$libclang_package" "$libclang_path" "$native_abi"
if [[ -n ${GITHUB_ENV:-} ]]; then
  printf 'LIBCLANG_PATH=%s\nOGRE_NATIVE_ABI=%s\n' \
    "$libclang_path" "$native_abi" >> "$GITHUB_ENV"
fi
