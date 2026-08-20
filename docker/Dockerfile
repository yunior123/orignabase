# Single-stage build to avoid GLIBC mismatch between builder and runtime.
# Keep this aligned with the minimum toolchain required by the dependency graph.
FROM rust:1.91-slim

RUN apt-get update && apt-get install -y pkg-config libssl-dev clang ca-certificates curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release --bin orignabase

# Move binary and config to runtime locations
RUN cp target/release/orignabase /usr/local/bin/orignabase && \
    mkdir -p /etc/orignabase /app/data/storage /app/public && \
    cp orignabase.toml /etc/orignabase/orignabase.toml && \
    cp rules.ob /etc/orignabase/rules.ob && \
    cp rules.test.ob /etc/orignabase/rules.test.ob && \
    rm -rf target .git

# Create non-root user for security
RUN useradd -m -u 1000 orignabase && \
    chown -R orignabase:orignabase /etc/orignabase /app/data /app/public

USER orignabase
EXPOSE 8080

# Health check to ensure container is responsive
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

ENTRYPOINT ["orignabase"]
CMD ["serve", "--config", "/etc/orignabase/orignabase.toml"]
