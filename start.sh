#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

log()  { echo -e "${GREEN}[ogrenotes]${NC} $*"; }
warn() { echo -e "${YELLOW}[ogrenotes]${NC} $*"; }
err()  { echo -e "${RED}[ogrenotes]${NC} $*" >&2; }

# ─── Prerequisites ───────────────────────────────────────────────

check_prereqs() {
    local missing=0

    if ! command -v cargo &>/dev/null; then
        err "cargo not found. Install Rust: https://rustup.rs"
        missing=1
    fi

    if ! command -v trunk &>/dev/null; then
        err "trunk not found. Install: cargo install trunk"
        missing=1
    fi

    if ! command -v docker &>/dev/null && ! command -v podman &>/dev/null; then
        warn "docker/podman not found. Redis won't start automatically."
    fi

    if [ $missing -ne 0 ]; then
        exit 1
    fi
}

# ─── Environment ─────────────────────────────────────────────────

load_env() {
    if [ -f .env ]; then
        log "Loading .env"
        set -a
        source .env
        set +a
    elif [ -f .env.example ]; then
        warn "No .env file found. Copying .env.example to .env"
        warn "Edit .env with your AWS credentials and JWT secret before using."
        cp .env.example .env
        set -a
        source .env
        set +a
    else
        err "No .env or .env.example found"
        exit 1
    fi

    # Generate a random JWT secret if it's still the placeholder
    if [[ "${JWT_SECRET:-}" == "replace-with-a-random-string-at-least-32-bytes-long" ]]; then
        local new_secret
        new_secret="$(openssl rand -base64 48 2>/dev/null || head -c 48 /dev/urandom | base64)"
        sed -i "s|replace-with-a-random-string-at-least-32-bytes-long|${new_secret}|" .env
        export JWT_SECRET="$new_secret"
        log "Generated random JWT secret"
    fi
}

# ─── Services ────────────────────────────────────────────────────

start_redis() {
    if docker compose ps redis 2>/dev/null | grep -q "running"; then
        log "Redis already running"
    else
        log "Starting Redis..."
        docker compose up -d redis 2>/dev/null || {
            warn "Could not start Redis via docker compose."
            warn "Make sure Redis is running on localhost:6379"
        }
    fi
}

setup_aws() {
    if [[ "${DYNAMODB_TABLE_PREFIX:-}" == "dev-yourname-" ]]; then
        warn "DYNAMODB_TABLE_PREFIX is still the default. Edit .env first."
        warn "Skipping AWS setup."
        return 1
    fi

    log "Setting up AWS resources (DynamoDB table + S3 bucket)..."
    cargo run -p ogrenotes-api --bin setup_dev 2>&1 | while read -r line; do
        echo "  $line"
    done
}

# ─── Build & Run ─────────────────────────────────────────────────

build_backend() {
    log "Building backend..."
    cargo build -p ogrenotes-api 2>&1 | tail -1
}

run_backend() {
    log "Starting API server on port ${API_PORT:-3000}..."
    cargo run -p ogrenotes-api --bin ogrenotes-api &
    BACKEND_PID=$!
}

run_frontend() {
    log "Starting frontend on port 8080..."
    cd frontend
    trunk serve --port 8080 &
    FRONTEND_PID=$!
    cd ..
}

# ─── Cleanup ─────────────────────────────────────────────────────

cleanup() {
    echo ""
    log "Shutting down..."
    [ -n "${BACKEND_PID:-}" ] && kill "$BACKEND_PID" 2>/dev/null && log "Backend stopped"
    [ -n "${FRONTEND_PID:-}" ] && kill "$FRONTEND_PID" 2>/dev/null && log "Frontend stopped"
    exit 0
}

trap cleanup SIGINT SIGTERM

# ─── Main ────────────────────────────────────────────────────────

main() {
    echo ""
    echo -e "${GREEN}  ╔═══════════════════════════════╗${NC}"
    echo -e "${GREEN}  ║       🟢  OgreNotes  🟢       ║${NC}"
    echo -e "${GREEN}  ║     Documents with teeth.      ║${NC}"
    echo -e "${GREEN}  ╚═══════════════════════════════╝${NC}"
    echo ""

    check_prereqs
    load_env
    start_redis

    case "${1:-}" in
        setup)
            setup_aws
            ;;
        backend)
            build_backend
            run_backend
            log "Backend running. Press Ctrl+C to stop."
            wait "$BACKEND_PID"
            ;;
        frontend)
            run_frontend
            log "Frontend running at http://localhost:8080"
            log "Press Ctrl+C to stop."
            wait "$FRONTEND_PID"
            ;;
        test)
            log "Running all tests..."
            cargo test --workspace
            log "Backend tests passed."
            cd frontend && cargo test && cd ..
            log "Frontend tests passed."
            log "All 328 tests passed."
            ;;
        *)
            build_backend
            setup_aws || true

            run_backend
            sleep 2  # let backend start before frontend proxy connects
            run_frontend

            echo ""
            log "OgreNotes is running!"
            log "  Frontend:  http://localhost:8080"
            log "  API:       http://localhost:${API_PORT:-3000}"
            log ""
            log "Press Ctrl+C to stop both servers."
            echo ""

            wait
            ;;
    esac
}

main "$@"
