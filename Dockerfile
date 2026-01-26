# syntax=docker/dockerfile:1

FROM rust:1.93-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY rust-toolchain.toml ./

RUN cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENV PORT=8080 \
    RUST_LOG=info \
    MAX_BODY_BYTES=5242880 \
    MAX_INSTANCES=5000 \
    DEFAULT_TIME_LIMIT_MS=2000 \
    DEFAULT_RESTARTS=10

WORKDIR /app
COPY --from=builder /app/target/release/freecut /app/freecut

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl -fsS "http://localhost:${PORT}/health/live" || exit 1

CMD ["/app/freecut"]
