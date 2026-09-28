#!/usr/bin/env bash
# Copy one AWS profile's keys per service out of the host's AWS config, so
# each container sees only its own credentials, never the rest of ~/.aws.
#
#   ./aws-creds.sh
#
# Reads OGRENOTES_AWS_PROFILE and CADDY_AWS_PROFILE from .env and writes
# aws/app/credentials and aws/caddy/credentials (gitignored, mode 600), each
# holding that profile as [default]. A service whose variable is unset gets
# no file, so it falls back to nothing and must have keys in its env file.
# Re-run after rotating a key, then `docker compose up -d`.
set -euo pipefail
cd "$(dirname "$0")"

[[ -f .env ]] || { echo "missing .env, copy .env.example first" >&2; exit 1; }
eval "$(grep -E '^(OGRENOTES_AWS_PROFILE|CADDY_AWS_PROFILE)=' .env || true)"

write() { # <dir> <profile or empty>
  local dir=$1 profile=$2 out=$1/credentials
  mkdir -p "$dir"
  if [[ -z $profile ]]; then
    rm -f "$out"
    echo "$dir: no profile set, no host credentials"
    return
  fi
  local id secret token
  id=$(aws configure get aws_access_key_id --profile "$profile" || true)
  secret=$(aws configure get aws_secret_access_key --profile "$profile" || true)
  token=$(aws configure get aws_session_token --profile "$profile" || true)
  if [[ -z $id || -z $secret ]]; then
    echo "profile '$profile' has no static access keys (SSO or role profiles aren't supported)" >&2
    exit 1
  fi
  (
    umask 077
    {
      echo "[default]"
      echo "aws_access_key_id = $id"
      echo "aws_secret_access_key = $secret"
      if [[ -n $token ]]; then echo "aws_session_token = $token"; fi
    } > "$out"
  )
  if [[ -n $token ]]; then
    echo "warning: profile '$profile' has a session token; it will expire" >&2
  fi
  echo "$dir: profile '$profile'"
}

write aws/app "${OGRENOTES_AWS_PROFILE:-}"
write aws/caddy "${CADDY_AWS_PROFILE:-}"
