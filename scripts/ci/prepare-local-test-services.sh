#!/usr/bin/env bash
set -euo pipefail

provision=false
release=false
no_default_features=false
features=""

while (($#)); do
    case "$1" in
        --provision)
            provision=true
            shift
            ;;
        --release)
            release=true
            shift
            ;;
        --no-default-features)
            no_default_features=true
            shift
            ;;
        --features)
            if (($# < 2)); then
                echo "--features requires a value" >&2
                exit 2
            fi
            features="$2"
            shift 2
            ;;
        -h|--help)
            cat <<'USAGE'
Usage: prepare-local-test-services.sh [--provision] [--release]
       [--no-default-features] [--features LIST]

Starts the local DynamoDB, MinIO, and Redis services and waits for their
ports. With --provision, runs the setup_dev binary using the selected Cargo
profile and feature options.
USAGE
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 2
            ;;
    esac
done

if [[ "$release" == true || "$no_default_features" == true || -n "$features" ]] && [[ "$provision" != true ]]; then
    echo "Cargo provisioning options require --provision" >&2
    exit 2
fi

docker compose up -d

timeout_seconds="${LOCAL_TEST_SERVICES_TIMEOUT_SECONDS:-30}"
if ! [[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]]; then
    echo "LOCAL_TEST_SERVICES_TIMEOUT_SECONDS must be a positive integer" >&2
    exit 2
fi
for service in "DynamoDB:8000" "MinIO:9000" "Redis:6379"; do
    name="${service%%:*}"
    port="${service##*:}"
    printf 'Waiting for %s on port %s...\n' "$name" "$port"
    ready=false
    for ((second = 0; second < timeout_seconds; second++)); do
        if nc -z 127.0.0.1 "$port"; then
            ready=true
            break
        fi
        sleep 1
    done
    if [[ "$ready" != true ]]; then
        echo "$name did not become ready on port $port within ${timeout_seconds}s" >&2
        docker compose ps
        exit 1
    fi
done

if [[ "$provision" == true ]]; then
    cargo_args=(run --locked)
    if [[ "$release" == true ]]; then
        cargo_args+=(--release)
    fi
    cargo_args+=(--bin setup_dev -p ogrenotes-api)
    if [[ "$no_default_features" == true ]]; then
        cargo_args+=(--no-default-features)
    fi
    if [[ -n "$features" ]]; then
        cargo_args+=(--features "$features")
    fi
    cargo "${cargo_args[@]}"
fi
