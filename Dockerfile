# GrintaHub Clicker - Docker Image
# Multi-stage build for smaller final image

# ===== Stage 1: Build =====
FROM rust:latest AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy source
COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./
COPY src-tauri/src ./src
COPY src-tauri/evasions ./evasions

# Build release binary (server mode, no desktop features)
RUN cargo build --release --no-default-features --bin server

# ===== Stage 2: Runtime =====
FROM debian:bookworm-slim

# Install runtime dependencies + Chrome
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    wget \
    gnupg \
    && wget -q -O - https://dl.google.com/linux/linux_signing_key.pub | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg \
    && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" > /etc/apt/sources.list.d/google-chrome.list \
    && apt-get update \
    && apt-get install -y google-chrome-stable \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -s /bin/bash grintahub

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/server /app/grintahub-server

# Copy web assets (React dashboard build)
COPY dist /app/dist

# Set ownership
RUN chown -R grintahub:grintahub /app

# Switch to non-root user
USER grintahub

# Config directory for persistence
VOLUME ["/home/grintahub/.config/grintahub-clicker"]

# Expose web dashboard port
EXPOSE 8080

# Environment variables
ENV GRINTAHUB_WEB_PORT=8080
ENV RUST_LOG=info

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/ || exit 1

# Run the server
CMD ["/app/grintahub-server"]
