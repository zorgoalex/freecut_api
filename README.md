# Freecut

Freecut is a Rust service for 2D rectangular cut optimization with an HTTP API and SVG output.
It uses Axum and the `cut-optimizer-2d` engine and always returns an SVG artifact for successful optimizations.

## Features
- 2D rectangle nesting with kerf/spacing/trim support
- Rotation constraints and pattern direction flags
- Multi-start optimization with deterministic seeds
- JSON API with OpenAPI + Swagger UI
- Docker-ready single-binary service

## Tech Stack
- Rust (edition 2021)
- Axum (HTTP)
- cut-optimizer-2d (layout engine)
- utoipa + Swagger UI (OpenAPI docs)

## Quick Start (Local)
```bash
# Ensure Rust toolchain is available
. "$HOME/.cargo/env"

# Run the service
cargo run
```

Service listens on `0.0.0.0:8080` by default.

## Quick Start (Docker)
```bash
docker build -t freecut-mvp .
docker run --rm -p 8080:8080 freecut-mvp
```

## Health & Docs
- `GET /health/live`
- `GET /health/ready`
- `GET /version`
- `GET /openapi.json`
- `GET /docs`

## Main Endpoint
`POST /v1/optimize`

- Request/response are JSON.
- All dimensions are in millimeters (`mm`).
- Successful responses include SVG in `artifacts.svg`.

## Environment Variables
- `PORT` (default `8080`)
- `RUST_LOG` (default `info`)
- `MAX_BODY_BYTES` (default `5242880`)
- `MAX_INSTANCES` (default `5000`)
- `DEFAULT_TIME_LIMIT_MS` (default `1200`)
- `DEFAULT_RESTARTS` (default `7`)

## Testing
```bash
cargo test
```

Note: Swagger UI assets are downloaded during build; tests/builds may require network access.

## Docker Smoke Tests
These tests validate the running container via a host-network curl image.

```bash
# Start the container first
docker run --rm -p 8080:8080 freecut-mvp

# In another terminal
./scripts/docker_smoke.sh
```

Optional overrides:
```bash
BASE_URL=http://127.0.0.1:8080 CURL_IMAGE=curlimages/curl:8.6.0 ./scripts/docker_smoke.sh
```

## Notes
- Pattern direction flags are validated for rotation constraints, but only `none` currently affects optimization.
