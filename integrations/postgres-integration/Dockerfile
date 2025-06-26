# PostgreSQL Integration Test Dockerfile
FROM rust:1.75-slim-bookworm as builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy workspace configuration
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src/ ./src/
COPY integrations/ ./integrations/

# Build the integration application
RUN cargo build --release --bin postgres-integration

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create app user
RUN useradd -m -u 1000 appuser

# Copy the binary
COPY --from=builder /app/target/release/postgres-integration /usr/local/bin/postgres-integration

# Set ownership
RUN chown appuser:appuser /usr/local/bin/postgres-integration

# Switch to app user
USER appuser

# Set environment variables
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD postgres-integration --help || exit 1

# Run the integration tests
CMD ["postgres-integration"]