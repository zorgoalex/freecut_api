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
- cut-optimizer-2d (layout engine, source: https://github.com/jasonrhansen/cut-optimizer-2d)
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

### Example Request
Example file: `examples/optimize_request.json`

```json
{
  "units": "mm",
  "params": {
    "kerf_mm": 2.0,
    "spacing_mm": 1.0,
    "trim_mm": {
      "left": 10.0,
      "right": 10.0,
      "top": 10.0,
      "bottom": 10.0
    },
    "time_limit_ms": 240,
    "restarts": 3,
    "objective": "min_waste",
    "seed": 12345
  },
  "stock": [
    { "id": "sheet-1000", "width_mm": 1000.0, "height_mm": 1000.0, "qty": 2 }
  ],
  "items": [
    { "id": "A", "width_mm": 200.0, "height_mm": 300.0, "qty": 2, "rotation": "allow_90", "pattern_direction": "none" },
    { "id": "B", "width_mm": 400.0, "height_mm": 400.0, "qty": 1, "rotation": "allow_90", "pattern_direction": "none" }
  ]
}
```

### Field-by-Field Explanation
- `units`: Measurement units; must be `"mm"`.
- `params`: Optimization parameters.
  - `kerf_mm`: Blade thickness (cut width) in mm.
  - `spacing_mm`: Minimum gap between parts in mm.
  - `trim_mm`: Unusable margins around the sheet in mm.
    - `left`, `right`, `top`, `bottom`: Margin sizes in mm.
  - `time_limit_ms`: Total time budget for optimization in milliseconds.
  - `restarts`: Number of optimization restarts (multi-start).
  - `objective`: Optimization goal: `"min_waste"` or `"min_sheets"`.
  - `seed`: Deterministic seed for reproducible results.
- `stock`: Available sheet materials.
  - `id`: Stock identifier.
  - `width_mm`, `height_mm`: Sheet dimensions in mm.
  - `qty`: Quantity of sheets of this size.
- `items`: Parts to be cut.
  - `id`: Part identifier.
  - `width_mm`, `height_mm`: Part dimensions in mm.
  - `qty`: Quantity of this part.
  - `rotation`: Rotation rule: `"forbid"` or `"allow_90"`.
  - `pattern_direction`: Grain/pattern direction: `"none"`, `"along_width"`, `"along_height"`.

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
