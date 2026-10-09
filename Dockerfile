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

# Install runtime dependencies (ca-certificates for HTTPS, etc.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /app/server/target/release/ws-server /usr/local/bin/ws-server

# Create non-root user for security
RUN useradd -r -u 1000 -U -d /app -s /bin/sh appuser
USER appuser

# Default environment variables (can be overridden)
ENV TIER=all \
    BIND=0.0.0.0 \
    PORT=3000 \
    REDIS_URL=redis://redis:6379 \
    DATABASE_URL=sqlite:/data/app.db \
    RECONNECT_GRACE_SECS=30 \
    RUST_LOG=info

# Expose the default port
EXPOSE 3000

# Run the server
CMD ["ws-server"]