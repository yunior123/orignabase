# Stage 1: Build
FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release --bin orignabase

# Stage 2: Runtime (distroless)
FROM gcr.io/distroless/cc-debian12

COPY --from=builder /app/target/release/orignabase /usr/local/bin/orignabase

EXPOSE 8080
ENTRYPOINT ["orignabase"]
CMD ["serve"]
