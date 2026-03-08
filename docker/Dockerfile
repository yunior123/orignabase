# Stage 1: Build
FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev clang && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release --bin orignabase

# Stage 2: Runtime (distroless)
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/orignabase /usr/local/bin/orignabase
COPY --from=builder /app/orignabase.toml /etc/orignabase/orignabase.toml
COPY --from=builder /app/rules.ob /etc/orignabase/rules.ob

EXPOSE 8080
ENTRYPOINT ["orignabase"]
CMD ["serve", "--config", "/etc/orignabase/orignabase.toml"]
