# syntax=docker/dockerfile:1.7
# Multi-stage build for OgreNotes API server + frontend.
# Stage 1: Build backend (Rust) — uses BuildKit cache mounts so the cargo
#          registry and target/ persist across image builds.
# Stage 2: Build frontend (Trunk/WASM) — same cache-mount strategy.
# Stage 3: Minimal runtime image
#
# IMPORTANT: callers MUST enable BuildKit (DOCKER_BUILDKIT=1 or `docker
# buildx build`) for the cache mounts and `# syntax=` directive above to
# take effect. Without BuildKit the cache mounts silently degrade to no-op
# bind dirs and every build re-fetches all crates.

# ─── Stage 1: Backend Build ────────────────────────────────────
FROM rust:slim-bookworm AS backend-builder

# Build target arch (from BuildKit --platform); used to namespace the cache
# mount so amd64 and arm64 builds on the same host don't share one target dir.
ARG TARGETARCH

# Phase 4 M-E4: samael's `xmlsec` feature wraps libxml2 + xmlsec1 for
# SAML response signature verification. Build-time needs the -dev
# packages; runtime needs only the shared-library packages (added
# in stage 3). libclang is needed because samael's xmlsec binding
# uses bindgen to wrap the xmlsec C headers at build time.
RUN apt-get update && apt-get install -y \
        pkg-config libssl-dev \
        libxml2-dev libxmlsec1-dev libxmlsec1-openssl \
        clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace manifests + sources together — the BuildKit cache mounts
# below make the old "stub lib + pre-build deps" trick redundant: cargo's
# registry and target dir survive between builds, so only changed crates
# recompile.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# `release-fast` (defined in workspace Cargo.toml) trades runtime perf for
# faster compilation — appropriate for the 1-task ECS test stack. Output
# binary is copied out of the cache mount before it tears down, since the
# mount is not persisted in the final image layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target,id=ogre-backend-target-${TARGETARCH} \
    cargo build --profile release-fast --bin ogrenotes-api \
 && mkdir -p /out \
 && cp target/release-fast/ogrenotes-api /out/ogrenotes-api

# ─── Stage 2: Frontend Build ───────────────────────────────────
FROM rust:slim-bookworm AS frontend-builder

ARG TRUNK_VERSION=0.21.14
# Build-time stamp baked into the WASM by frontend/build.rs. Deploy
# scripts pass this as `--build-arg GIT_HASH=<short-sha[-dirty]>` so the
# sidebar version row reflects the actual source state — `.dockerignore`
# excludes `.git/`, so build.rs can't compute it itself inside the image.
ARG GIT_HASH=unknown
ENV GIT_HASH=${GIT_HASH}

RUN apt-get update && apt-get install -y pkg-config libssl-dev curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install trunk from its prebuilt release binary (seconds, vs. minutes to
# compile from source via `cargo install trunk`), then add the wasm target.
# TARGETARCH is supplied by BuildKit from the build --platform; map it to
# trunk's release triple so an arm64 (Graviton) image gets the aarch64 binary
# instead of a non-runnable x86_64 one.
ARG TARGETARCH
RUN case "${TARGETARCH}" in \
        arm64) TRUNK_ARCH=aarch64-unknown-linux-gnu ;; \
        amd64|"") TRUNK_ARCH=x86_64-unknown-linux-gnu ;; \
        *) echo "unsupported TARGETARCH=${TARGETARCH}" >&2; exit 1 ;; \
    esac && \
    curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${TRUNK_ARCH}.tar.gz" \
    | tar -xz -C /usr/local/bin && \
    rustup target add wasm32-unknown-unknown

WORKDIR /app/frontend

COPY frontend/ .

# Trunk has no --profile flag, so frontend/Cargo.toml's [profile.release]
# stays build-speed-tuned (codegen-units=256, lto=false) for local dev/e2e.
# The deployed bundle, however, wants size: we override the release profile
# here via CARGO_PROFILE_RELEASE_* env (trunk shells out to cargo, which
# reads them). These MUST stay in sync with .github/workflows/bundle-size.yml
# so the CI gate measures what actually ships. wasm-opt=z still runs
# post-link via index.html's data-wasm-opt directive.
# `dist/` is what stage 3 copies; leave it outside the cache mount so it
# survives into the final image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/frontend/target,id=ogre-frontend-target-${TARGETARCH} \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
    CARGO_PROFILE_RELEASE_LTO=fat \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    CARGO_PROFILE_RELEASE_STRIP=true \
    CARGO_PROFILE_RELEASE_PANIC=abort \
    trunk build --release

# ─── Stage 3: Runtime ──────────────────────────────────────────
FROM debian:bookworm-slim

# Runtime libraries — `libxmlsec1-openssl` carries the openssl
# crypto backend that samael's xmlsec binding linked against at
# build time. Without it the ogrenotes-api binary fails to dlopen
# at startup with "libxmlsec1-openssl.so.1: cannot open shared
# object file".
RUN apt-get update && apt-get install -y \
        ca-certificates \
        libxml2 libxmlsec1 libxmlsec1-openssl \
    && rm -rf /var/lib/apt/lists/*

# Backend binary
COPY --from=backend-builder /out/ogrenotes-api /usr/local/bin/ogrenotes-api

# Frontend static files
COPY --from=frontend-builder /app/frontend/dist /app/frontend/dist

# Search index directory
RUN mkdir -p /data/search-index
ENV SEARCH_INDEX_PATH=/data/search-index

EXPOSE 3000

CMD ["ogrenotes-api"]
