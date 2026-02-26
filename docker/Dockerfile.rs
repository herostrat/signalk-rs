# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM rust:1.84-slim AS builder

WORKDIR /app

# Cache dependency compilation by building with stub sources first.
# Only the Cargo manifests and lockfile are copied in this layer.
COPY Cargo.toml Cargo.lock ./
COPY crates/signalk-types/Cargo.toml  crates/signalk-types/
COPY crates/signalk-store/Cargo.toml  crates/signalk-store/
COPY crates/signalk-internal/Cargo.toml crates/signalk-internal/
COPY crates/signalk-server/Cargo.toml  crates/signalk-server/

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Stub source files so `cargo build` can resolve and cache all deps.
RUN mkdir -p crates/signalk-types/src \
             crates/signalk-store/src \
             crates/signalk-internal/src \
             crates/signalk-server/src && \
    for d in signalk-types signalk-store signalk-internal; do \
        echo "// stub" > crates/$d/src/lib.rs; \
    done && \
    echo "// stub" > crates/signalk-server/src/lib.rs && \
    echo "fn main() {}" > crates/signalk-server/src/main.rs && \
    cargo build --release -p signalk-server 2>/dev/null || true

# Now build with the real source.
# Docker invalidates the layer cache here when crates/ changes.
# Cargo recompiles only our project crates (external deps are already cached above).
COPY crates/ crates/
RUN cargo build --release -p signalk-server

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/signalk-server /usr/local/bin/signalk-server

EXPOSE 3000

ENV RUST_LOG=signalk_server=info,signalk_store=info

CMD ["signalk-server"]
