# Multi-stage build for ws-server (split realtime architecture)
# Stage 1: Build the Rust binary
FROM rust:1.99-bookworm AS builder

WORKDIR /app

# Copy workspace files for cargo
COPY core/ core/
COPY server/ server/

# Build the ws-server binary in release mode
RUN cargo build --release --manifest-path server/Cargo.toml --bin ws-server

# Stage 2: Runtime image
FROM debian:bookworm-slim

# Install runtime dependencies: ca-certificates for outbound TLS, curl for the
# container healthcheck defined in docker-compose.prod.yml (it probes /healthz).
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /app/server/target/release/ws-server /usr/local/bin/ws-server

# Create non-root user for security
RUN useradd -r -u 1000 -U -d /app -s /bin/sh appuser

# Persistent state directory (design D5). It must be writable by the non-root
# runtime user: a fresh named volume mounted at /data inherits ownership from
# this image path on first use, so create it owned by `appuser` here. Without
# this the server cannot create the SQLite file and exits at startup.
RUN mkdir -p /data && chown appuser:appuser /data

USER appuser

# Default environment variables (can be overridden). No REDIS_URL: the shipped
# production deployment is the TIER=all monolith, which never opens Redis
# (change deploy-server-docker, design D1). The local split stack sets
# REDIS_URL explicitly in docker-compose.yml, so nothing relies on it here.
ENV TIER=all \
    BIND=0.0.0.0 \
    PORT=3000 \
    DATABASE_URL=sqlite:/data/app.db \
    RECONNECT_GRACE_SECS=30 \
    RUST_LOG=info

# Expose the default port
EXPOSE 3000

# Run the server
CMD ["ws-server"]