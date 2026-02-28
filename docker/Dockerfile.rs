# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM rust:slim AS builder

ARG FEATURES=""

WORKDIR /app

# Cache dependency compilation by building with stub sources first.
# Only the Cargo manifests and lockfile are copied in this layer.
COPY Cargo.toml Cargo.lock ./
COPY crates/signalk-types/Cargo.toml       crates/signalk-types/
COPY crates/signalk-store/Cargo.toml       crates/signalk-store/
COPY crates/signalk-internal/Cargo.toml    crates/signalk-internal/
COPY crates/signalk-plugin-api/Cargo.toml  crates/signalk-plugin-api/
COPY crates/signalk-plugin-client/Cargo.toml crates/signalk-plugin-client/
COPY crates/signalk-server/Cargo.toml      crates/signalk-server/
COPY crates/plugins/nmea0183-receive/Cargo.toml      crates/plugins/nmea0183-receive/
COPY crates/plugins/anchor-alarm/Cargo.toml           crates/plugins/anchor-alarm/
COPY crates/plugins/sensor-data-simulator/Cargo.toml  crates/plugins/sensor-data-simulator/
COPY crates/plugins/derived-data/Cargo.toml           crates/plugins/derived-data/
COPY crates/plugins/ais-status/Cargo.toml             crates/plugins/ais-status/
COPY crates/plugins/nmea2000-receive/Cargo.toml       crates/plugins/nmea2000-receive/
COPY crates/plugins/nmea0183-send/Cargo.toml          crates/plugins/nmea0183-send/
COPY crates/plugins/nmea2000-send/Cargo.toml          crates/plugins/nmea2000-send/

RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev libudev-dev && \
    rm -rf /var/lib/apt/lists/*

# Stub source files so `cargo build` can resolve and cache all deps.
RUN mkdir -p crates/signalk-types/src \
             crates/signalk-store/src \
             crates/signalk-internal/src \
             crates/signalk-plugin-api/src \
             crates/signalk-plugin-client/src \
             crates/signalk-server/src \
             crates/plugins/nmea0183-receive/src \
             crates/plugins/anchor-alarm/src \
             crates/plugins/sensor-data-simulator/src \
             crates/plugins/derived-data/src \
             crates/plugins/ais-status/src \
             crates/plugins/nmea2000-receive/src \
             crates/plugins/nmea0183-send/src \
             crates/plugins/nmea2000-send/src && \
    for d in signalk-types signalk-store signalk-internal signalk-plugin-api signalk-plugin-client; do \
        echo "// stub" > crates/$d/src/lib.rs; \
    done && \
    for d in nmea0183-receive anchor-alarm sensor-data-simulator derived-data ais-status nmea2000-receive nmea0183-send nmea2000-send; do \
        echo "// stub" > crates/plugins/$d/src/lib.rs; \
    done && \
    echo "// stub" > crates/signalk-server/src/lib.rs && \
    echo "fn main() {}" > crates/signalk-server/src/main.rs && \
    cargo build --release -p signalk-server --features "${FEATURES}" 2>/dev/null || true

# Now build with the real source.
# Docker invalidates the layer cache here when crates/ changes.
# Cargo recompiles only our project crates (external deps are already cached above).
COPY crates/ crates/
RUN cargo build --release -p signalk-server --features "${FEATURES}"

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl iproute2 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/signalk-server /usr/local/bin/signalk-server
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 3000

ENV RUST_LOG=signalk_server=info,signalk_store=info

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["signalk-server"]
